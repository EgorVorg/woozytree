//! scan_bench — временная утилита замера скорости сканирования NTFS через MFT.
//!
//! Пример: `scan_bench C:` (нужны права администратора) или
//! `scan_bench --fixture <путь к слепку MFT>` для проверки без тома.
//! Печатает итоги сканирования, верхние папки, время по фазам и разбивку
//! пропусков по причинам. `scan_bench --files 100000` мерит время выдачи
//! страницы таблицы файлов на синтетической папке.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use woozytree_lib::model::{
    FileKey, FileMeta, FileSort, FolderId, FolderSummary, ScanError, ScanProgress, ScanTotals,
    TreemapRect,
};
use woozytree_lib::scan::index::ScanIndex;
use woozytree_lib::scan::mft::{ScanDiag, scan_ntfs_reader_diag};
use woozytree_lib::scan::volume::VolumeReader;
use woozytree_lib::scan::{ScanSink, ScanTarget, run_scan};

fn describe(error: ScanError) -> String {
    match error {
        ScanError::VolumeAccess(message) => format!("доступ к тому: {message}"),
        ScanError::Io(message) => format!("ошибка ввода-вывода: {message}"),
        ScanError::Parse(message) => format!("ошибка разбора: {message}"),
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

/// Папка с наибольшим числом файлов непосредственно в ней — худший случай для
/// таблицы файлов. Обходим всё дерево: нужно реальное место, где файлов больше
/// всего, а не первая крупная папка.
fn biggest_files_folder(index: &ScanIndex, root_name: &str) -> (u32, String, u32) {
    let mut best = (
        index.root(),
        root_name.to_string(),
        index.file_count(index.root()),
    );
    let mut stack = vec![(index.root(), root_name.to_string())];
    while let Some((id, name)) = stack.pop() {
        let count = index.file_count(id);
        if count > best.2 {
            best = (id, name, count);
        }
        for child in index.child_folders(id) {
            stack.push((child.id, child.name));
        }
    }
    best
}

/// Время выдачи страницы (100 строк) по каждой колонке — лучшее из трёх замеров,
/// плюс самая невыгодная последняя страница.
fn time_pages(index: &ScanIndex, folder: FolderId, count: u32) {
    for (sort, label) in [
        (FileSort::Name, "имя"),
        (FileSort::SizeOnDisk, "размер на диске"),
        (FileSort::SizeLogical, "логический размер"),
        (FileSort::Modified, "изменён"),
    ] {
        let mut best = Duration::MAX;
        let mut rows = 0;
        for _ in 0..3 {
            let started = Instant::now();
            rows = index.child_files(folder, sort, true, 0, 100).len();
            best = best.min(started.elapsed());
        }
        println!(
            "[files] child_files({label}, desc, 0..100): {best:?} (лучшее из 3), строк {rows}"
        );
    }

    // Последняя страница — самый невыгодный offset: до неё всё равно сортируется
    // весь список папки.
    let offset = (count as usize).saturating_sub(100);
    let started = Instant::now();
    let rows = index.child_files(folder, FileSort::SizeOnDisk, true, offset, 100);
    let elapsed = started.elapsed();
    let sum: u64 = rows.iter().map(|row| row.size_on_disk).sum();
    println!(
        "[files] последняя страница offset {offset}: {elapsed:?}, строк {}, сумма {} байт",
        rows.len(),
        sum
    );
}

/// Замер выдачи файлов на реальной папке — самой большой по числу файлов.
///
/// Сортировка идёт при выдаче, поэтому каждый вызов — это копия списка папки и
/// полная сортировка; замер показывает цену одной страницы таблицы.
fn bench_files(index: &ScanIndex, root_name: &str) {
    let started = Instant::now();
    let (folder, name, count) = biggest_files_folder(index, root_name);
    println!(
        "[files] поиск папки с максимумом файлов: {:?}",
        started.elapsed()
    );
    println!("[files] папка {name}: файлов в ней {count}");
    if count > 0 {
        time_pages(index, folder, count);
    }
}

/// Синтетическая папка на N файлов: цена выдачи не зависит от того, есть ли на
/// диске папка нужного размера. `scan_bench --files 100000`.
fn run_files_bench(count: usize) {
    let started = Instant::now();
    let mut index = ScanIndex::new("synthetic");
    let root = index.root();
    let folder = index.folder(root, "many");
    for position in 0..count {
        let number = position as u64;
        // Порядок «перемешан» умножением на нечётную константу (взаимно простую с
        // 1e6): имена, размеры и время не монотонны по вставке, как и на диске.
        // Длинный общий префикс — как у имён WinSxS: сравнение имён не вырождается
        // в сравнение первых байт.
        let scrambled = number.wrapping_mul(2_654_435_761) % 1_000_000;
        let size_on_disk = scrambled + 1;
        index.add_file(
            folder,
            FileMeta {
                name: format!("amd64_microsoft_windows_component_{scrambled:07}.bin"),
                size_on_disk,
                size_logical: size_on_disk / 2,
                modified: scrambled as i64 * 1000,
                file_key: FileKey {
                    volume_serial: 1,
                    file_index: number,
                },
            },
        );
    }
    println!(
        "[files] синтетическая папка: файлов загружено {}, загрузка индекса {:?}",
        index.file_count(folder),
        started.elapsed()
    );
    time_pages(&index, folder, index.file_count(folder));
}

/// Приёмник живого сканирования: печатает прогресс, топ и карту с отметками
/// времени — так видно, что события идут во время чтения, а не после него.
struct LiveSink {
    started: Instant,
    progress_events: u64,
    top_events: u64,
    treemap_events: u64,
    first_top: Option<Duration>,
    last_top: Option<Duration>,
    max_top_gap: Duration,
    last_progress: Option<ScanProgress>,
    live_folders_seen: bool,
}

impl LiveSink {
    fn new() -> LiveSink {
        LiveSink {
            started: Instant::now(),
            progress_events: 0,
            top_events: 0,
            treemap_events: 0,
            first_top: None,
            last_top: None,
            max_top_gap: Duration::ZERO,
            last_progress: None,
            live_folders_seen: false,
        }
    }
}

impl ScanSink for LiveSink {
    fn progress(&mut self, p: &ScanProgress) {
        self.progress_events += 1;
        self.last_progress = Some(p.clone());
        if p.folders > 0 {
            self.live_folders_seen = true;
        }
        // Не засоряем лог: первое событие и каждое сотое.
        if self.progress_events == 1 || self.progress_events % 100 == 0 {
            println!(
                "[live] прогресс #{:>4} в {:.2} с: файлов {}, папок {}, занято {:.2} ГиБ",
                self.progress_events,
                self.started.elapsed().as_secs_f64(),
                p.files,
                p.folders,
                gib(p.size_on_disk)
            );
        }
    }

    fn top_folders(&mut self, rows: &[FolderSummary]) {
        let at = self.started.elapsed();
        self.top_events += 1;
        if let Some(previous) = self.last_top {
            self.max_top_gap = self.max_top_gap.max(at - previous);
        }
        self.first_top.get_or_insert(at);
        self.last_top = Some(at);
        let head = rows
            .first()
            .map(|row| format!("{}, {:.2} ГиБ", row.name, gib(row.size_on_disk)))
            .unwrap_or_else(|| "пусто".to_string());
        println!(
            "[live] топ #{:>3} в {:.2} с: папок {}, крупнейшая {}",
            self.top_events,
            at.as_secs_f64(),
            rows.len(),
            head
        );
    }

    fn treemap(&mut self, rects: &[TreemapRect]) {
        self.treemap_events += 1;
        println!(
            "[live] карта #{:>3} в {:.2} с: прямоугольников {}",
            self.treemap_events,
            self.started.elapsed().as_secs_f64(),
            rects.len()
        );
    }

    fn done(&mut self, r: Result<&ScanTotals, &ScanError>) {
        let text = match r {
            Ok(totals) => format!(
                "файлов {}, папок {}, занято {:.3} ГиБ",
                totals.files,
                totals.folders,
                gib(totals.size_on_disk)
            ),
            Err(err) => format!("ошибка: {err:?}"),
        };
        println!(
            "[live] done в {:.2} с: {text}",
            self.started.elapsed().as_secs_f64()
        );
    }
}

/// Живой режим: скан через `run_scan` — тот же путь, что и в приложении,
/// с таймером живых событий (топ и карта раз в секунду).
fn run_live(drive: &str) {
    let letter = drive.trim_end_matches([':', '\\', '/']).to_string();
    let target = ScanTarget::Drive(letter.clone());
    let mut index = ScanIndex::new(drive);
    let cancel = AtomicBool::new(false);
    let mut sink = LiveSink::new();
    let started = Instant::now();

    let totals = match run_scan(&target, &mut index, &mut sink, &cancel) {
        Ok(totals) => totals,
        Err(error) => {
            eprintln!("scan_bench: {}", describe(error));
            std::process::exit(1);
        }
    };
    let wall = started.elapsed();

    println!("[bench] цель: {drive} (живой режим run_scan)");
    println!(
        "[bench] всего: файлов {}, папок {}, занято {:.3} ГиБ ({} байт), логически {:.3} ГиБ, \
         пропущено {}, elapsed_ms {}",
        totals.files,
        totals.folders,
        gib(totals.size_on_disk),
        totals.size_on_disk,
        gib(totals.size_logical),
        totals.skipped,
        totals.elapsed_ms
    );
    println!(
        "[bench] по стенным часам: {:.3} с; прогресс {}, топ {}, карта {}",
        wall.as_secs_f64(),
        sink.progress_events,
        sink.top_events,
        sink.treemap_events
    );
    println!(
        "[bench] живые события: первый топ в {:?}, максимальная пауза между топами {:?}, \
         папки в прогрессе живыми: {}",
        sink.first_top,
        sink.max_top_gap,
        if sink.live_folders_seen {
            "да"
        } else {
            "нет"
        }
    );
    if let Some(progress) = &sink.last_progress {
        println!(
            "[bench] последний прогресс: файлов {}, папок {}, занято {:.3} ГиБ",
            progress.files,
            progress.folders,
            gib(progress.size_on_disk)
        );
    }
    let root_totals = index.folder_totals(index.root());
    println!(
        "[bench] итоги корня индекса: файлов {}, папок {}, занято {} байт, логически {}",
        root_totals.file_count,
        root_totals.folder_count,
        root_totals.size_on_disk,
        root_totals.size_logical
    );

    bench_files(&index, drive);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Живой режим: скан через run_scan с таймером событий (топ и карта).
    if args.first().map(String::as_str) == Some("--live") {
        run_live(args.get(1).map(String::as_str).unwrap_or("C"));
        return;
    }

    // Синтетический замер выдачи страницы: `scan_bench --files 100000`.
    if args.first().map(String::as_str) == Some("--files") {
        let count = args
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(100_000);
        run_files_bench(count);
        return;
    }

    // Диагностический режим: печать числа разобранных имён по каждой записи
    // слепка (для сверки с независимым разбором).
    if args.first().map(String::as_str) == Some("--diag") {
        let path = args.get(1).cloned().unwrap_or_default();
        let reader = match VolumeReader::from_fixture(Path::new(&path), 1024) {
            Ok(reader) => reader,
            Err(message) => {
                eprintln!("scan_bench: {}", describe(message));
                std::process::exit(2);
            }
        };
        let mut index = 0u64;
        let cancel = AtomicBool::new(false);
        reader
            .read_records(&cancel, &mut |record| {
                let count = woozytree_lib::scan::mft::parse_record(record).len();
                if count > 0 {
                    println!("{index} {count}");
                }
                index += 1;
            })
            .expect("чтение слепка");
        return;
    }

    let target = args.first().cloned().unwrap_or_else(|| "C:".to_string());

    let mut index = ScanIndex::new(&target);
    let cancel = AtomicBool::new(false);
    let mut progress_calls = 0u64;
    let mut last_progress = ScanProgress {
        files: 0,
        folders: 0,
        size_on_disk: 0,
        elapsed_ms: 0,
    };

    let started = Instant::now();
    let mut diag: Option<ScanDiag> = None;
    let result = if target == "--fixture" {
        let path = match args.get(1) {
            Some(path) => path,
            None => {
                eprintln!("scan_bench: --fixture требует путь к файлу-слепку");
                std::process::exit(2);
            }
        };
        VolumeReader::from_fixture(Path::new(path), 1024)
            .map_err(describe)
            .and_then(|reader| {
                let scanned =
                    scan_ntfs_reader_diag(&reader, &mut index, &cancel, &mut |progress, _index| {
                        progress_calls += 1;
                        last_progress = progress.clone();
                    })
                    .map_err(describe);
                if let Ok((_, breakdown)) = &scanned {
                    diag = Some(*breakdown);
                }
                scanned.map(|(totals, _)| totals)
            })
    } else {
        // Диагностика тома до сканирования: сколько записей и как разложен $MFT.
        let reader = match VolumeReader::open(&target) {
            Ok(reader) => reader,
            Err(message) => {
                eprintln!("scan_bench: {}", describe(message));
                std::process::exit(1);
            }
        };
        println!(
            "[bench] $MFT: записей {}, запись {} байт, экстентов {}, серийный номер {:08x}",
            reader.record_count(),
            reader.record_size(),
            reader.mft_extents().len(),
            reader
                .boot_sector()
                .map_or(0, |boot| boot.volume_serial as u32)
        );
        let scanned =
            scan_ntfs_reader_diag(&reader, &mut index, &cancel, &mut |progress, _index| {
                progress_calls += 1;
                last_progress = progress.clone();
            })
            .map_err(describe);
        if let Ok((_, breakdown)) = &scanned {
            diag = Some(*breakdown);
        }
        scanned.map(|(totals, _)| totals)
    };
    let wall = started.elapsed();

    let totals = match result {
        Ok(totals) => totals,
        Err(message) => {
            eprintln!("scan_bench: {message}");
            std::process::exit(1);
        }
    };

    println!("[bench] цель: {target}");
    println!(
        "[bench] всего: файлов {}, папок {}, занято {:.3} ГиБ ({} байт), логически {:.3} ГиБ, \
         пропущено {}, elapsed_ms {}",
        totals.files,
        totals.folders,
        gib(totals.size_on_disk),
        totals.size_on_disk,
        gib(totals.size_logical),
        totals.skipped,
        totals.elapsed_ms
    );
    println!(
        "[bench] по стенным часам: {:.3} с, сообщений прогресса {}, последнее: {:?}",
        wall.as_secs_f64(),
        progress_calls,
        last_progress
    );

    // Пропуски — составной счётчик: печатаем разбивку по причинам, чтобы число
    // из totals.skipped можно было интерпретировать.
    if let Some(diag) = diag {
        println!(
            "[bench] пропуски по причинам: битых записей {}, неполных базовых \
             ($ATTRIBUTE_LIST) {}, extension-имён {}, имён-сирот {}, неразрешённых \
             папок {}; сумма {}",
            diag.broken,
            diag.partial,
            diag.extension_with_names,
            diag.orphan,
            diag.unresolved_folder,
            diag.total()
        );
    }

    let root_totals = index.folder_totals(index.root());
    println!(
        "[bench] итоги корня индекса: файлов {}, папок {}, занято {} байт, логически {}",
        root_totals.file_count,
        root_totals.folder_count,
        root_totals.size_on_disk,
        root_totals.size_logical
    );

    let children = index.child_folders(index.root());
    println!("[bench] папок верхнего уровня: {}", children.len());
    for child in children.iter().take(8) {
        println!(
            "   {:<40} {:>12.3} ГиБ  файлов {}, папок {}",
            child.name,
            gib(child.size_on_disk),
            child.file_count,
            child.folder_count
        );
    }

    bench_files(&index, &target);
}
