//! scan_bench — временная утилита замера скорости сканирования NTFS через MFT.
//!
//! Пример: `scan_bench C:` (нужны права администратора) или
//! `scan_bench --fixture <путь к слепку MFT>` для проверки без тома.
//! Печатает итоги сканирования, верхние папки, время по фазам и разбивку
//! пропусков по причинам.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use woozytree_lib::model::{ScanError, ScanProgress};
use woozytree_lib::scan::index::ScanIndex;
use woozytree_lib::scan::mft::{scan_ntfs_reader_diag, ScanDiag};
use woozytree_lib::scan::volume::VolumeReader;

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

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
        reader
            .read_records(&mut |record| {
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
                    scan_ntfs_reader_diag(&reader, &mut index, &cancel, &mut |progress| {
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
        let scanned = scan_ntfs_reader_diag(&reader, &mut index, &cancel, &mut |progress| {
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
        root_totals.file_count, root_totals.folder_count, root_totals.size_on_disk,
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
}
