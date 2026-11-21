//! Сервис сканирования: выбор движка, события для интерфейса и общее состояние.
//!
//! Движок выбирается по типу цели: NTFS-том читается напрямую через `$MFT`,
//! всё остальное (не-NTFS, сетевые диски, отдельная папка) — обходом папок.

pub mod index;
pub mod mft;
pub mod volume;
pub mod walk;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetLogicalDriveStringsW, GetVolumeInformationW,
};
use windows::core::PCWSTR;

use crate::model::{DriveInfo, FolderSummary, ScanError, ScanProgress, ScanTotals};
use crate::scan::index::ScanIndex;
use crate::scan::mft::scan_ntfs;
use crate::scan::walk::walk;

/// Сколько самых тяжёлых папок корня показывать в топе.
const TOP_FOLDERS_LIMIT: usize = 20;

/// Что сканировать: целый диск или отдельную папку.
///
/// В JSON это `{"drive": "C"}` или `{"folder": "C:\\Temp"}`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanTarget {
    Drive(String),
    Folder(PathBuf),
}

/// Приёмник событий сканирования (в приложении — отправка в окно).
pub trait ScanSink {
    fn progress(&mut self, p: &ScanProgress);
    fn top_folders(&mut self, rows: &[FolderSummary]);
    fn done(&mut self, r: Result<&ScanTotals, &ScanError>);
}

/// Что делать с ошибкой: показать фатально или предложить обход папок.
#[derive(Debug, PartialEq, Eq)]
pub enum FallbackAction {
    ShowFatal,
    OfferWalkFallback,
}

/// Классифицировать ошибку сканирования.
///
/// Нет доступа к тому (нет прав администратора, не NTFS) — не приговор: тём же
/// охватом можно пройтись обходом папок. Остальные ошибки (ввод-вывод, разбор)
/// обходом не лечатся.
pub fn classify_error(err: &ScanError) -> FallbackAction {
    match err {
        ScanError::VolumeAccess(_) => FallbackAction::OfferWalkFallback,
        ScanError::Io(_) | ScanError::Parse(_) => FallbackAction::ShowFatal,
    }
}

/// Прогресс и итоги наружу в одном месте: движки только сообщают цифры.
///
/// Топ папок отправляется по завершении: движок держит `&mut ScanIndex` всё
/// время работы, а `ScanSink` не имеет доступа к индексу, поэтому снимать
/// промежуточные срезы дерева прямо из колбэка прогресса нельзя без изменения
/// зафиксированных интерфейсов `walk`/`scan_ntfs`.
pub fn run_scan(
    target: &ScanTarget,
    index: &mut ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
) -> Result<ScanTotals, ScanError> {
    let result = match target {
        // NTFS-том читаем напрямую; всё остальное — обходом.
        ScanTarget::Drive(letter) if is_ntfs(letter) => {
            let mut report = |p: &ScanProgress| sink.progress(p);
            scan_ntfs(letter, index, cancel, &mut report)
        }
        _ => {
            let mut report = |p: &ScanProgress| sink.progress(p);
            walk_target(target, index, cancel, &mut report)
        }
    };

    match result {
        Ok(totals) => {
            let rows: Vec<FolderSummary> = index
                .child_folders(index.root())
                .into_iter()
                .take(TOP_FOLDERS_LIMIT)
                .collect();
            sink.top_folders(&rows);
            sink.done(Ok(&totals));
            Ok(totals)
        }
        Err(err) => {
            sink.done(Err(&err));
            Err(err)
        }
    }
}

/// Скан папки (или не-NTFS диска) обходом: итоги собираются из корня индекса.
fn walk_target(
    target: &ScanTarget,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanTotals, ScanError> {
    let root = match target {
        ScanTarget::Folder(path) => path.clone(),
        ScanTarget::Drive(letter) => PathBuf::from(drive_root(letter)),
    };

    let started = Instant::now();
    let stats = walk(&root, index, cancel, progress)?;
    let totals = index.folder_totals(index.root());

    Ok(ScanTotals {
        files: u64::from(totals.file_count),
        folders: u64::from(totals.folder_count),
        size_on_disk: totals.size_on_disk,
        size_logical: totals.size_logical,
        // В skipped входит всё, что не попало в индекс: недоступные записи и
        // reparse-точки. Дубликаты жёстких ссылок — не пропуски, они учтены
        // отдельным счётчиком `WalkStats::hard_link_duplicates`.
        skipped: stats.skipped.saturating_add(stats.reparse_skipped),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Общее состояние сервиса: отмена текущего запуска, готовый индекс и итоги.
///
/// Запуск сканирования создаёт новый флаг отмены, поэтому `cancel_scan` не может
/// случайно остановить уже завершённое сканирование, а результат устаревшего
/// запуска не затирает свежий индекс.
pub struct ScanService {
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    index: Mutex<Option<Arc<ScanIndex>>>,
    totals: Mutex<Option<ScanTotals>>,
}

impl Default for ScanService {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanService {
    pub fn new() -> ScanService {
        ScanService {
            cancel: Mutex::new(None),
            index: Mutex::new(None),
            totals: Mutex::new(None),
        }
    }

    /// Начать новый запуск: старый останавливается, возвращается его флаг отмены.
    pub fn begin(&self) -> Arc<AtomicBool> {
        self.cancel();
        let flag = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = Some(Arc::clone(&flag));
        flag
    }

    /// Попросить текущий запуск остановиться (если он есть).
    pub fn cancel(&self) {
        if let Some(flag) = self.cancel.lock().unwrap().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Актуален ли запуск с этим флагом (не начат ли уже следующий).
    pub fn is_current(&self, flag: &Arc<AtomicBool>) -> bool {
        let active = self.cancel.lock().unwrap();
        active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, flag))
    }

    /// Сохранить результат, если этот запуск всё ещё актуален.
    pub fn finish(&self, flag: &Arc<AtomicBool>, index: Arc<ScanIndex>, totals: Option<ScanTotals>) {
        let active = self.cancel.lock().unwrap();
        let is_current = active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, flag));
        if !is_current {
            return;
        }
        *self.index.lock().unwrap() = Some(index);
        *self.totals.lock().unwrap() = totals;
    }

    /// Готовый индекс после сканирования (во время сканирования его ещё нет).
    pub fn index(&self) -> Option<Arc<ScanIndex>> {
        self.index.lock().unwrap().clone()
    }

    /// Итоги последнего успешного сканирования.
    pub fn totals(&self) -> Option<ScanTotals> {
        self.totals.lock().unwrap().clone()
    }
}

/// Корень тома для Win32-вызовов: `C`/`C:`/`C:\` → `C:\`.
pub(crate) fn drive_root(drive_letter: &str) -> String {
    let letter = drive_letter.trim_end_matches('\\').trim_end_matches(':');
    format!("{letter}:\\")
}

/// Лежит ли на диске файловая система NTFS.
///
/// Проверка именно через `GetVolumeInformationW`: буква диска может быть и
/// сетевой, и FAT32/exFAT — для них быстрый разбор `$MFT` неприменим.
pub fn is_ntfs(drive_letter: &str) -> bool {
    let root = to_wide(&drive_root(drive_letter));
    let mut fs_name = [0u16; 16];
    let read_ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_name[..]),
        )
        .is_ok()
    };
    read_ok && from_wide(&fs_name).eq_ignore_ascii_case("NTFS")
}

/// Логические диски с меткой, файловой системой и занятым местом.
pub fn list_drives() -> Vec<DriveInfo> {
    // Буфера хватает на 26 букв с разделителями: 26 * 4 u16.
    let mut buffer = vec![0u16; 512];
    let len = unsafe { GetLogicalDriveStringsW(Some(buffer.as_mut_slice())) } as usize;
    if len == 0 {
        return Vec::new();
    }

    // Строки идут через нули и завершаются двойным нулём; пустая строка — конец.
    let mut drives = Vec::new();
    let mut start = 0usize;
    while start < len {
        let Some(offset) = buffer[start..len].iter().position(|&c| c == 0) else {
            break;
        };
        let end = start + offset;
        if end == start {
            break;
        }
        let root = from_wide(&buffer[start..=end]);
        start = end + 1;

        let root_wide = to_wide(&root);
        let mut label = [0u16; 64];
        let mut fs_name = [0u16; 16];
        let info_ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(root_wide.as_ptr()),
                Some(&mut label[..]),
                None,
                None,
                None,
                Some(&mut fs_name[..]),
            )
            .is_ok()
        };

        // Свободное место есть не у всех дисков (например, у пустого привода),
        // поэтому нули в этом случае — допустимый ответ, а не ошибка.
        let mut total_bytes = 0u64;
        let mut free_bytes = 0u64;
        let size_ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root_wide.as_ptr()),
                None,
                Some(&mut total_bytes as *mut u64),
                Some(&mut free_bytes as *mut u64),
            )
            .is_ok()
        };

        drives.push(DriveInfo {
            letter: root.trim_end_matches('\\').to_string(),
            label: if info_ok { from_wide(&label) } else { String::new() },
            fs: if info_ok {
                from_wide(&fs_name)
            } else {
                String::new()
            },
            total_bytes: if size_ok { total_bytes } else { 0 },
            free_bytes: if size_ok { free_bytes } else { 0 },
        });
    }

    drives
}

/// Строка в UTF-16 с завершающим нулём — так Win32 ждёт пути.
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 с завершающим нулём → `String` (обрезка по первому нулю).
fn from_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FolderSummary, ScanError, ScanProgress, ScanTotals};
    use crate::scan::index::ScanIndex;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Своя временная папка на каждый тест (в ограниченной среде `%TEMP%`
    /// может быть недоступен, поэтому есть запасной вариант внутри `target`).
    fn test_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap_or_else(|_| {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-tmp");
            std::fs::create_dir_all(&base).expect("не удалось создать папку для тестовых tempdir");
            tempfile::tempdir_in(base).expect("не удалось создать временную папку теста")
        })
    }

    /// Приёмник событий сканирования: запоминает всё, что прислал `run_scan`.
    #[derive(Default)]
    struct RecordingSink {
        progress: Vec<ScanProgress>,
        tops: Vec<Vec<FolderSummary>>,
        done: Option<Result<ScanTotals, String>>,
    }

    impl ScanSink for RecordingSink {
        fn progress(&mut self, p: &ScanProgress) {
            self.progress.push(p.clone());
        }

        fn top_folders(&mut self, rows: &[FolderSummary]) {
            self.tops.push(rows.to_vec());
        }

        fn done(&mut self, r: Result<&ScanTotals, &ScanError>) {
            self.done = Some(match r {
                Ok(totals) => Ok(totals.clone()),
                Err(err) => Err(format!("{err:?}")),
            });
        }
    }

    #[test]
    fn scan_service_keeps_only_current_run_results() {
        let service = ScanService::new();
        let old = service.begin();
        let current = service.begin();

        // Новый запуск обязан остановить прошлый, а его результат — не попасть
        // в состояние: иначе запоздавший поток затрёт свежий индекс.
        assert!(
            old.load(Ordering::Relaxed),
            "begin должен остановить предыдущий запуск"
        );
        assert!(service.is_current(&current));
        assert!(!service.is_current(&old));

        let mut stale = ScanIndex::new("root");
        stale.folder(stale.root(), "stale");
        service.finish(&old, Arc::new(stale), None);
        assert!(
            service.index().is_none(),
            "результат устаревшего запуска не должен сохраняться"
        );

        let mut fresh = ScanIndex::new("root");
        fresh.folder(fresh.root(), "fresh");
        service.finish(&current, Arc::new(fresh), None);
        let stored = service.index().expect("результат текущего запуска должен сохраниться");
        assert_eq!(stored.child_folders(stored.root()).len(), 1);
    }

    #[test]
    fn run_scan_on_folder_walks_and_reports_progress() {
        let dir = test_dir();
        for name in ["a.bin", "b.bin", "c.bin"] {
            std::fs::write(dir.path().join(name), [0u8; 16]).unwrap();
        }

        let mut index = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();

        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("сканирование папки должно завершиться без ошибки");

        assert_eq!(totals.files, 3);
        assert_eq!(index.folder_totals(index.root()).file_count, 3);
        assert!(
            !sink.progress.is_empty(),
            "прогресс должен сообщаться хотя бы итоговым отчётом"
        );
        assert_eq!(
            sink.progress.last().map(|p| p.files),
            Some(3),
            "итоговый прогресс должен содержать число файлов"
        );
        assert!(
            sink.done.as_ref().is_some_and(|done| done.is_ok()),
            "по завершении sink должен получить итоги: {:?}",
            sink.done
        );
    }

    #[test]
    fn run_scan_drive_ntfs_vs_folder() {
        let dir = test_dir();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("nested.bin"), [1u8; 32]).unwrap();
        std::fs::write(dir.path().join("top.bin"), [2u8; 64]).unwrap();

        // Эталон: прямой обход той же папки.
        let mut direct = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let stats = walk::walk(dir.path(), &mut direct, &cancel, &mut |_| {})
            .expect("прямой обход должен пройти без ошибки");
        let direct_totals = direct.folder_totals(direct.root());

        let mut index = ScanIndex::new("folder");
        let mut sink = RecordingSink::default();
        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("ScanTarget::Folder всегда идёт через walk и не должен падать");

        assert_eq!(totals.files, u64::from(direct_totals.file_count));
        assert_eq!(totals.folders, u64::from(direct_totals.folder_count));
        assert_eq!(totals.size_on_disk, direct_totals.size_on_disk);
        assert_eq!(totals.size_logical, direct_totals.size_logical);
        // В skipped для обхода входит всё, что не попало в индекс, кроме
        // дубликатов жёстких ссылок (они — не пропуски).
        assert_eq!(totals.skipped, stats.skipped + stats.reparse_skipped);

        // Топ папок — дочерние папки корня, их присылают по завершении.
        let top = sink.tops.last().expect("топ папок должен быть отправлен");
        assert_eq!(top.len(), 1, "у корня ровно одна дочерняя папка: {top:?}");
        assert_eq!(top[0].name, "sub");
    }

    #[test]
    fn classify_error_maps_volume_access_to_fallback() {
        assert_eq!(
            classify_error(&ScanError::VolumeAccess("нет прав на том".to_string())),
            FallbackAction::OfferWalkFallback
        );
        assert_eq!(
            classify_error(&ScanError::Io("диск отвалился".to_string())),
            FallbackAction::ShowFatal
        );
        assert_eq!(
            classify_error(&ScanError::Parse("битая запись".to_string())),
            FallbackAction::ShowFatal
        );
    }

    #[test]
    fn cancel_scan_stops_run() {
        let dir = test_dir();
        for i in 0..32 {
            std::fs::write(dir.path().join(format!("file-{i:02}.bin")), b"x").unwrap();
        }

        let mut index = ScanIndex::new("folder");
        // Отмена до старта: обход не должен ничего прочитать и не должен падать.
        let cancel = AtomicBool::new(true);
        let mut sink = RecordingSink::default();

        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("отмена — не ошибка, run_scan обязан вернуть Ok");

        assert_eq!(totals.files, 0, "до отмены файлы не должны считаться");
        assert_eq!(totals.skipped, 0, "отмена не создаёт пропусков");
    }
}
