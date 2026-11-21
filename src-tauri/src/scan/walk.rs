//! Запасной режим сканирования: обход папок.
//!
//! Используется для не-NTFS и сетевых дисков, а также для сканирования отдельной
//! папки. NTFS-диски читаются напрямую через MFT — этот обход резервный.

use std::ffi::c_void;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_STANDARD_INFO, FileStandardInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx,
};

use crate::model::{FileKey, FileMeta, FolderId, ScanError, ScanProgress};
use crate::scan::index::{AddOutcome, ScanIndex};

/// Прогресс сообщается примерно раз в столько файлов.
const PROGRESS_EVERY_FILES: u64 = 1000;

/// Итоги обхода, которые не входят в модель индекса.
#[derive(Debug, Default)]
pub struct WalkStats {
    /// Записи, которые не удалось прочитать (нет прав, висячая ссылка и т.п.).
    pub skipped: u32,
    /// Каталоги-reparse-точки (junction, симлинки папок), в которые обход не заходил.
    pub reparse_skipped: u32,
    /// Жёсткие ссылки, чьи байты не задвоены (второе и последующие имена файла).
    pub hard_link_duplicates: u32,
}

/// Нужно ли намеренно не заходить в запись обхода.
///
/// Пропускаем только **каталоги** с атрибутом reparse: junction и симлинк папки
/// могут вести на предка (бесконечный цикл) или на уже посчитанные данные
/// (двойной счёт). Файл с reparse-атрибутом — облачный плейсхолдер OneDrive
/// («Файлы по запросу») или WOF-сжатый файл (CompactOS): его байты лежат на
/// диске, поэтому он учитывается обычным путём.
///
/// `is_dir` — именно бит `FILE_ATTRIBUTE_DIRECTORY`, а не `Metadata::is_dir()`:
/// для junction и симлинка папки std считает `is_dir() == false`, потому что
/// относит их к симлинкам.
fn is_skippable_reparse(is_dir: bool, attributes: u32) -> bool {
    is_dir && attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

/// Обойти папку `root` и сложить найденное в `index`.
///
/// Обход итеративный (явный стек), поэтому глубокая вложенность не переполняет стек.
/// Ошибки отдельных записей не прерывают обход — они попадают в [`WalkStats::skipped`].
pub fn walk(
    root: &Path,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<WalkStats, ScanError> {
    let started = Instant::now();
    let mut stats = WalkStats::default();

    // Счётчики для прогресса: учитываются все встреченные файлы, включая повторы
    // жёстких ссылок, — прогресс показывает объём работы, а не бухгалтерию размеров.
    let mut files = 0u64;
    let mut folders = 0u64;
    let mut bytes = 0u64;
    let mut next_report_at = PROGRESS_EVERY_FILES;

    let root_id = index.root();
    let mut stack: Vec<(PathBuf, FolderId, bool)> = vec![(root.to_path_buf(), root_id, true)];

    'walk: while let Some((dir_path, dir_id, is_root)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let entries = match fs::read_dir(&dir_path) {
            Ok(entries) => entries,
            Err(err) => {
                if is_root {
                    return Err(ScanError::Io(format!(
                        "не удалось прочитать папку {}: {err}",
                        dir_path.display()
                    )));
                }
                // Одна недоступная папка не должна прерывать весь обход.
                stats.skipped += 1;
                continue;
            }
        };

        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                break 'walk;
            }

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    stats.skipped += 1;
                    continue;
                }
            };

            let path = entry.path();
            // symlink_metadata не идёт по ссылке — нужны атрибуты самой записи.
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => {
                    stats.skipped += 1;
                    continue;
                }
            };

            let attributes = meta.file_attributes();
            let is_reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
            let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
            if is_skippable_reparse(is_directory, attributes) {
                // Не заходим в reparse-каталоги: junction может вести на предка
                // (бесконечный цикл), а симлинк — на уже посчитанные данные
                // (двойной счёт). Висячая ссылка недоступна — это обычный пропуск.
                if fs::metadata(&path).is_ok() {
                    stats.reparse_skipped += 1;
                } else {
                    stats.skipped += 1;
                }
                continue;
            }

            let name = entry.file_name().to_string_lossy().into_owned();

            if is_directory {
                folders += 1;
                let child_id = index.folder(dir_id, &name);
                stack.push((path, child_id, false));
                continue;
            }

            // Файл с reparse-атрибутом std считает симлинком (`is_file()` == false),
            // но для нас это обычный файл: облачный плейсхолдер или WOF-сжатый файл.
            if !meta.is_file() && !is_reparse {
                continue; // не файл и не папка (устройства и т.п.) — пропускаем молча
            }

            // Ключ файла и реальный размер на диске берём по открытому хэндлу.
            let (file_key, size_on_disk) = match file_info(&path) {
                Ok(info) => info,
                Err(_) => {
                    stats.skipped += 1;
                    continue;
                }
            };

            let outcome = index.add_file(
                dir_id,
                FileMeta {
                    name,
                    size_on_disk,
                    size_logical: meta.len(),
                    modified: modified_ms(&meta),
                    file_key,
                },
            );
            // Повторный FileKey — жёсткая ссылка: индекс не задваивает байты,
            // а обход сообщает, сколько таких имён встретилось.
            if matches!(outcome, AddOutcome::HardLinkDuplicate) {
                stats.hard_link_duplicates += 1;
            }
            files += 1;
            bytes += size_on_disk;

            if files >= next_report_at {
                report(progress, files, folders, bytes, started);
                next_report_at += PROGRESS_EVERY_FILES;
                if cancel.load(Ordering::Relaxed) {
                    break 'walk;
                }
            }
        }
    }

    // Итоговый отчёт нужен и после отмены: UI должен увидеть, сколько успели.
    report(progress, files, folders, bytes, started);
    Ok(stats)
}

fn report(
    progress: &mut dyn FnMut(&ScanProgress),
    files: u64,
    folders: u64,
    size_on_disk: u64,
    started: Instant,
) {
    progress(&ScanProgress {
        files,
        folders,
        size_on_disk,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
}

/// Уникальный ключ файла (номер тома + file index) и занятый на диске размер.
///
/// Ключ берётся из `GetFileInformationByHandle` — по нему в индексе распознаются
/// жёсткие ссылки. Занятое место — `AllocationSize` из `GetFileInformationByHandleEx`
/// (учитывает сжатие и разреженность); если файловая система такой запрос не
/// поддерживает, остаётся логический размер.
///
/// Хэндл открывается только с правом `FILE_READ_ATTRIBUTES`: данных не читаем, поэтому
/// учитываются и файлы, закрытые на чтение (занятые системные, с запрещающей DACL).
fn file_info(path: &Path) -> io::Result<(FileKey, u64)> {
    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        // Открываем саму запись, а не цель reparse-точки: облачный плейсхолдер не
        // гидратируется, а файл с незнакомым системе тегом всё равно отдаёт размеры.
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let handle = HANDLE(file.as_raw_handle());

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut info) }.is_err() {
        return Err(io::Error::last_os_error());
    }

    let mut standard = FILE_STANDARD_INFO::default();
    let mut size_on_disk = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut FILE_STANDARD_INFO as *mut c_void,
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok.is_ok() {
        // 0 — законный размер для полностью разреженного/сжатого файла,
        // поэтому успешный запрос используем даже с нулём.
        size_on_disk = standard.AllocationSize as u64;
    }

    Ok((
        FileKey {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        },
        size_on_disk,
    ))
}

/// Время изменения в Unix-мс; любая ошибка — 0 (время не влияет на размеры).
fn modified_ms(meta: &fs::Metadata) -> i64 {
    match meta.modified() {
        Ok(time) => time
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::test_support::test_dir;
    use std::path::{Path, PathBuf};

    fn walk_index(root: &Path) -> (ScanIndex, WalkStats) {
        let mut index = ScanIndex::new("root");
        let cancel = AtomicBool::new(false);
        let mut reports = Vec::new();
        let stats = walk(root, &mut index, &cancel, &mut |progress| {
            reports.push(progress.clone())
        })
        .expect("walk должен завершиться без ошибки");

        // Итоговый отчёт обязателен: по нему UI узнаёт, что обход закончен.
        assert!(!reports.is_empty(), "прогресс должен сообщать итоги");
        (index, stats)
    }

    fn verbatim(path: &Path) -> PathBuf {
        PathBuf::from(format!(r"\\?\{}", path.display()))
    }

    fn wide_len(path: &Path) -> usize {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count()
    }

    fn make_junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            // mklink печатает «Junction created…» — не засоряем вывод тестов.
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("не удалось запустить cmd /c mklink /J");
        assert!(
            status.success(),
            "mklink /J не создал junction {:?} -> {:?}",
            link,
            target
        );
    }

    #[test]
    fn test_walk_sums_file_sizes() {
        let dir = test_dir();
        std::fs::write(dir.path().join("a.bin"), [0u8; 10]).unwrap();
        std::fs::write(dir.path().join("b.bin"), [1u8; 32]).unwrap();

        let (index, stats) = walk_index(dir.path());
        let totals = index.folder_totals(index.root());

        assert_eq!(totals.file_count, 2);
        assert_eq!(totals.size_logical, 42);
        assert!(
            totals.size_on_disk >= 42,
            "занято на диске должно быть не меньше логического размера: {}",
            totals.size_on_disk
        );
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn test_walk_handles_unicode_and_long_paths() {
        let dir = test_dir();
        let root = dir.path();

        let unicode_name = "файл 🚀 тест.txt";
        std::fs::write(root.join(unicode_name), b"unicode").unwrap(); // 7 байт

        // Создаём через префикс `\\?\`: он работает всегда, а обход проверяется
        // и с префиксом, и без него.
        let long_root = verbatim(root);
        let mut deep = long_root.clone();
        let mut names: Vec<String> = Vec::new();
        while wide_len(&deep) <= 280 {
            let name = format!("длинная-папка-{:02}", names.len());
            deep.push(&name);
            std::fs::create_dir(&deep).unwrap();
            names.push(name);
        }
        std::fs::write(deep.join("длинный файл.txt"), b"hello-world").unwrap(); // 11 байт

        // С префиксом `\\?\` оба файла обязаны быть в индексе с верными размерами.
        let (mut prefixed_index, prefixed_stats) = walk_index(&long_root);
        let prefixed_totals = prefixed_index.folder_totals(prefixed_index.root());
        assert_eq!(
            prefixed_totals.file_count, 2,
            "с префиксом \\\\?\\ должны найтись оба файла, пропущено: {}",
            prefixed_stats.skipped
        );
        assert_eq!(prefixed_totals.size_logical, 7 + 11);
        assert!(prefixed_totals.size_on_disk >= 7 + 11);

        let mut deep_id = prefixed_index.root();
        for name in &names {
            deep_id = prefixed_index.folder(deep_id, name);
        }
        let deep_totals = prefixed_index.folder_totals(deep_id);
        assert_eq!(deep_totals.file_count, 1);
        assert_eq!(deep_totals.size_logical, 11);
        assert!(deep_totals.size_on_disk >= 11);

        // Без префикса Windows может отклонить длинный путь — это допустимо, но
        // walk обязан вернуть Ok, а недоступное — учесть в skipped.
        let mut plain_index = ScanIndex::new("root");
        let cancel = AtomicBool::new(false);
        let plain_stats = walk(root, &mut plain_index, &cancel, &mut |_| {})
            .expect("walk без префикса обязан вернуть Ok даже при недоступных путях");
        let plain_totals = plain_index.folder_totals(plain_index.root());
        if plain_totals.file_count != 2 || plain_totals.size_logical != 7 + 11 {
            assert!(
                plain_stats.skipped >= 1,
                "недоступный длинный путь должен попасть в skipped: {:?}",
                plain_stats
            );
        }
    }

    #[test]
    fn test_walk_counts_hard_link_once() {
        let dir = test_dir();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, [7u8; 100]).unwrap();
        std::fs::hard_link(&first, &second).unwrap();

        let (index, stats) = walk_index(dir.path());
        let totals = index.folder_totals(index.root());

        // Вторая ссылка — отдельное имя на диске, её байты не задваиваются,
        // а сам факт дубликата виден в статистике обхода.
        let single_disk_size = file_info(&first).unwrap().1;
        assert!(single_disk_size > 0);
        assert_eq!(totals.file_count, 2);
        assert_eq!(totals.size_logical, 100);
        assert_eq!(
            totals.size_on_disk, single_disk_size,
            "размер файла должен быть учтён ровно один раз"
        );
        assert_eq!(stats.hard_link_duplicates, 1);
    }

    #[test]
    fn test_walk_counts_data_locked_file() {
        let dir = test_dir();
        let locked = dir.path().join("locked.bin");
        std::fs::write(&locked, [3u8; 64]).unwrap();

        // Запрещаем чтение данных (RD): метаданные и размеры остаются доступны,
        // как у занятых системных файлов и файлов с запрещающей DACL.
        // Группа задаётся SID `*S-1-1-0`, а не локализованным именем `Everyone`:
        // алиас не резолвится на неанглийской Windows.
        let output = std::process::Command::new("cmd")
            .arg("/c")
            .arg("icacls")
            .arg(&locked)
            .arg("/deny")
            .arg("*S-1-1-0:(RD)")
            .output()
            .expect("не удалось запустить icacls");
        assert!(
            output.status.success(),
            "icacls не сработал: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let (index, stats) = walk_index(dir.path());

        // Снимаем запрет, чтобы временная папка штатно удалилась.
        let _ = std::process::Command::new("cmd")
            .arg("/c")
            .arg("icacls")
            .arg(&locked)
            .arg("/remove:d")
            .arg("*S-1-1-0")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        let totals = index.folder_totals(index.root());
        assert_eq!(totals.file_count, 1, "файл с закрытыми данными должен быть учтён");
        assert_eq!(totals.size_logical, 64);
        assert!(totals.size_on_disk >= 64);
        assert_eq!(stats.skipped, 0, "пропусков быть не должно: {:?}", stats);
    }

    #[test]
    fn is_skippable_reparse_only_for_directories() {
        let reparse = FILE_ATTRIBUTE_REPARSE_POINT.0;
        let dir_attrs = FILE_ATTRIBUTE_DIRECTORY.0;
        // Пропускаем только каталоги-точки (junction/симлинк папки): в них нельзя
        // заходить из-за циклов и двойного счёта. Именно так выглядит junction:
        // DIRECTORY | REPARSE.
        assert!(is_skippable_reparse(true, dir_attrs | reparse));
        // Файл с reparse-атрибутом (облачный плейсхолдер, WOF) — обычный файл.
        assert!(!is_skippable_reparse(false, reparse));
        // Каталог без reparse-атрибута — обычная папка.
        assert!(!is_skippable_reparse(true, dir_attrs));
        assert!(!is_skippable_reparse(false, 0));
    }

    /// Пометить обычный файл reparse-точкой пользовательского (ISV) тега.
    ///
    /// Такой тег Windows принимает на пользовательском файле без повышения прав и
    /// без данных: файл остаётся на месте, но получает `FILE_ATTRIBUTE_REPARSE_POINT`.
    /// Это модель облачных плейсхолдеров и WOF-сжатых файлов, которые обход обязан
    /// учитывать, а не выбрасывать.
    fn make_reparse_file(path: &Path, len: usize) {
        use windows::Win32::System::IO::DeviceIoControl;
        use windows::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

        /// Диапазон тегов, зарезервированный для ISV/фильтров: не требует прав.
        const ISV_REPARSE_TAG: u32 = 0x8000_1000;
        /// `REPARSE_DATA_BUFFER`: тег (u32), длина данных (u16), резерв (u16).
        const HEADER_LEN: usize = 8;

        std::fs::write(path, vec![0u8; len]).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let handle = HANDLE(file.as_raw_handle());

        let mut buffer = [0u8; HEADER_LEN];
        buffer[..4].copy_from_slice(&ISV_REPARSE_TAG.to_le_bytes());
        // ReparseDataLength = 0, Reserved = 0: пустая полезная нагрузка.
        unsafe {
            DeviceIoControl(
                handle,
                FSCTL_SET_REPARSE_POINT,
                Some(buffer.as_ptr() as *const c_void),
                buffer.len() as u32,
                None,
                0,
                None,
                None,
            )
        }
        .expect("FSCTL_SET_REPARSE_POINT не сработал");
    }

    #[test]
    fn test_walk_counts_file_with_reparse_point() {
        let dir = test_dir();
        let file = dir.path().join("cloud-placeholder.bin");
        let len = 64;
        make_reparse_file(&file, len);

        // Убедимся, что атрибут действительно выставлен: иначе тест ничего не проверяет.
        let meta = fs::symlink_metadata(&file).unwrap();
        assert_ne!(
            meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0,
            0,
            "файл должен получить атрибут reparse-точки"
        );

        let (index, stats) = walk_index(dir.path());
        fs::remove_file(&file).ok(); // иначе tempdir не удалится: тег неизвестен системе
        let totals = index.folder_totals(index.root());

        // Файлы с reparse-атрибутом (облачные плейсхолдеры, WOF) — обычные файлы:
        // их байты лежат на диске, поэтому они обязаны попасть в индекс и в размер.
        assert_eq!(
            totals.file_count, 1,
            "файл с reparse-атрибутом потерян: {:?}",
            stats
        );
        assert_eq!(totals.size_logical, len as u64);
        assert!(
            totals.size_on_disk > 0,
            "занятое место обязано учитываться по AllocationSize"
        );
        assert_eq!(
            stats.reparse_skipped, 0,
            "файл — не пропущенная reparse-точка"
        );
        assert_eq!(
            stats.skipped, 0,
            "файл не должен попасть в skipped: {:?}",
            stats
        );
    }

    fn make_sparse_file(path: &Path, len: u64) {
        use windows::Win32::System::IO::DeviceIoControl;
        use windows::Win32::System::Ioctl::FSCTL_SET_SPARSE;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        let handle = HANDLE(file.as_raw_handle());
        unsafe { DeviceIoControl(handle, FSCTL_SET_SPARSE, None, 0, None, 0, None, None) }
            .expect("FSCTL_SET_SPARSE не сработал");
        // Разреженный файл: увеличение длины не выделяет кластеры на диске.
        file.set_len(len).unwrap();
    }

    #[test]
    fn test_walk_reports_zero_allocation_sparse_file() {
        let dir = test_dir();
        let sparse = dir.path().join("sparse.bin");
        let logical = 1024 * 1024;
        make_sparse_file(&sparse, logical);

        let (index, stats) = walk_index(dir.path());
        let totals = index.folder_totals(index.root());

        assert_eq!(totals.file_count, 1);
        assert_eq!(totals.size_logical, logical);
        assert_eq!(
            totals.size_on_disk, 0,
            "полностью разреженный файл занимает 0 байт (AllocationSize == 0)"
        );
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn test_walk_does_not_follow_junctions() {
        let dir = test_dir();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("f.txt"), [0u8; 100]).unwrap();

        // Цикл: подпапка ссылается на своего предка. Заход в неё не завершился бы.
        let junction = sub.join("loop");
        make_junction(&junction, dir.path());

        let (index, stats) = walk_index(dir.path());
        std::fs::remove_dir(&junction).ok(); // убираем саму ссылку, цель не трогаем

        let totals = index.folder_totals(index.root());
        assert!(
            stats.reparse_skipped >= 1,
            "reparse-точка должна быть учтена: {:?}",
            stats
        );
        assert_eq!(totals.file_count, 1, "файлы не должны задвоиться");
        assert_eq!(totals.size_logical, 100);
    }

    #[test]
    fn test_walk_tolerates_dangling_junction() {
        let dir = test_dir();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = dir.path().join("dangling");
        make_junction(&junction, &target);
        std::fs::remove_dir_all(&target).unwrap(); // цель удалена — ссылка «в никуда»

        let mut index = ScanIndex::new("root");
        let cancel = AtomicBool::new(false);
        let result = walk(dir.path(), &mut index, &cancel, &mut |_| {});
        std::fs::remove_dir(&junction).ok();

        let stats = result.expect("висячий junction не должен ронять обход");
        assert!(
            stats.skipped >= 1,
            "недоступная запись обязана попасть в skipped: {:?}",
            stats
        );
    }

    #[test]
    fn test_walk_stops_on_cancel() {
        let dir = test_dir();
        let total = 1200;
        for i in 0..total {
            std::fs::write(dir.path().join(format!("file-{i:04}.bin")), b"x").unwrap();
        }

        let mut index = ScanIndex::new("root");
        let cancel = AtomicBool::new(false);
        walk(dir.path(), &mut index, &cancel, &mut |_progress| {
            cancel.store(true, Ordering::Relaxed);
        })
        .expect("отмена — не ошибка");

        let totals = index.folder_totals(index.root());
        assert!(
            totals.file_count < total,
            "обход обязан остановиться досрочно, обработано {} из {}",
            totals.file_count,
            total
        );
    }
}
