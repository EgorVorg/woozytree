//! dump_mft — утилита снятия сырых записей `$MFT` с NTFS-тома.
//!
//! Пример: `dump_mft C: tests\fixtures\sample.mft --records 2000 --boot`
//!
//! Требует прав администратора: без них Windows не даёт открыть `\\.\C:`.
//!
//! Флаги:
//! - `--records N`  — записать только первые N записей (по умолчанию все);
//! - `--boot`       — сохранить первые 512 байт тома в `boot.bin` рядом с выходным файлом;
//! - `--corrupt`    — испортить байты последней записанной записи (фикстура `corrupt.mft`);
//! - `--spike`      — замерить альтернативный способ чтения через `FSCTL_GET_NTFS_FILE_RECORD`
//!   (код спайка, в рабочий путь не входит);
//! - `--spike-limit N` — ограничить замер спайка первыми N записями.

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_GET_NTFS_FILE_RECORD, NTFS_FILE_RECORD_INPUT_BUFFER};

use woozytree_lib::model::ScanError;
use woozytree_lib::scan::volume::{normalize_drive_letter, VolumeReader};

struct Args {
    drive: String,
    out_path: PathBuf,
    records: Option<u64>,
    corrupt: bool,
    boot: bool,
    spike: bool,
    spike_limit: Option<u64>,
}

fn usage() -> &'static str {
    "Использование: dump_mft <буква_тома> <выходной_файл> [--records N] [--corrupt] [--boot] \
     [--spike [--spike-limit N]]"
}

fn parse_args() -> Result<Args, String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut positional = Vec::new();
    let mut records = None;
    let mut corrupt = false;
    let mut boot = false;
    let mut spike = false;
    let mut spike_limit = None;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--records" => {
                let value = raw
                    .get(i + 1)
                    .ok_or_else(|| format!("--records требует значение\n{}", usage()))?;
                records = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("--records: ожидалось число, получено {value}"))?,
                );
                i += 1;
            }
            "--spike-limit" => {
                let value = raw
                    .get(i + 1)
                    .ok_or_else(|| format!("--spike-limit требует значение\n{}", usage()))?;
                spike_limit = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("--spike-limit: ожидалось число, получено {value}"))?,
                );
                i += 1;
            }
            "--corrupt" => corrupt = true,
            "--boot" => boot = true,
            "--spike" => spike = true,
            other if other.starts_with("--") => {
                return Err(format!("неизвестный флаг {other}\n{}", usage()));
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    if positional.len() < 2 {
        return Err(usage().to_string());
    }
    Ok(Args {
        drive: positional[0].clone(),
        out_path: PathBuf::from(&positional[1]),
        records,
        corrupt,
        boot,
        spike,
        spike_limit,
    })
}

fn describe(error: ScanError) -> String {
    match error {
        ScanError::VolumeAccess(message) => format!("доступ к тому: {message}"),
        ScanError::Io(message) => format!("ошибка ввода-вывода: {message}"),
        ScanError::Parse(message) => format!("ошибка разбора: {message}"),
    }
}

/// Портит байты последней записи в файле (фикстура `corrupt.mft`).
fn corrupt_last_record(path: &Path, record_size: usize) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("открытие {} для порчи: {e}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|e| format!("размер {}: {e}", path.display()))?
        .len();
    if len < record_size as u64 {
        return Err("файл короче одной записи — портить нечего".into());
    }

    let last_offset = len - record_size as u64;
    let mut record = vec![0u8; record_size];
    {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(last_offset))
            .map_err(|e| format!("переход к последней записи: {e}"))?;
        file.read_exact(&mut record)
            .map_err(|e| format!("чтение последней записи: {e}"))?;
    }

    // Ломаем подпись, указатель на первый атрибут и хвост записи: парсер должен
    // увидеть мусор, а не валидную FILE-запись.
    record[0..4].copy_from_slice(b"BAD!");
    record[0x14..0x16].copy_from_slice(&0xFFFFu16.to_le_bytes());
    for byte in record[record_size / 2..].iter_mut().step_by(3) {
        *byte ^= 0xA5;
    }

    {
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(last_offset))
            .map_err(|e| format!("переход к последней записи: {e}"))?;
        file.write_all(&record)
            .map_err(|e| format!("запись испорченной записи: {e}"))?;
        file.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Способ (б) из спайка: перебор записей через `FSCTL_GET_NTFS_FILE_RECORD`.
/// Возвращает (успешных записей, ошибок).
fn read_via_ioctl(volume: &File, record_count: u64, limit: Option<u64>) -> Result<(u64, u64), String> {
    let handle = HANDLE(volume.as_raw_handle());
    let mut out = vec![0u8; 8192];
    let mut ok = 0u64;
    let mut errors = 0u64;
    let total = limit.map_or(record_count, |n| n.min(record_count));

    for index in 0..total {
        let input = NTFS_FILE_RECORD_INPUT_BUFFER {
            FileReferenceNumber: index as i64,
        };
        let mut returned = 0u32;
        // SAFETY: буферы живут на время вызова, их размеры переданы корректно,
        // дескриптор тома открыт и валиден.
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_NTFS_FILE_RECORD,
                Some(&input as *const _ as *const c_void),
                std::mem::size_of::<NTFS_FILE_RECORD_INPUT_BUFFER>() as u32,
                Some(out.as_mut_ptr() as *mut c_void),
                out.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        match result {
            Ok(()) => ok += 1,
            Err(_) => errors += 1,
        }
    }
    Ok((ok, errors))
}

fn main() {
    if let Err(message) = run() {
        eprintln!("dump_mft: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let started = Instant::now();

    let reader = VolumeReader::open(&args.drive).map_err(describe)?;
    let record_size = reader.record_size();
    let record_count = reader.record_count();

    if let Some(boot) = reader.boot_sector() {
        println!(
            "[info] том {}: сектор={} кластер={} запись MFT={} MFT_LCN={} серийный={:#x}",
            args.drive,
            boot.bytes_per_sector,
            boot.bytes_per_cluster(),
            boot.record_size,
            boot.mft_lcn,
            boot.volume_serial
        );
        let volume_bytes = boot.total_sectors * boot.bytes_per_sector as u64;
        println!(
            "[info] размер тома: {:.1} ГиБ",
            volume_bytes as f64 / (1u64 << 30) as f64
        );
    }
    println!(
        "[info] $MFT: {} экстент(ов), записей {}, байт {}",
        reader.mft_extents().len(),
        record_count,
        record_count * record_size as u64
    );
    println!("[info] экстенты: {:?}", reader.mft_extents());

    if let Some(dir) = args.out_path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| format!("создание {}: {e}", dir.display()))?;
        }
    }

    let limit = args.records.unwrap_or(u64::MAX);
    let mut out = BufWriter::new(
        File::create(&args.out_path)
            .map_err(|e| format!("создание {}: {e}", args.out_path.display()))?,
    );
    let mut written = 0u64;
    let mut write_error: Option<String> = None;

    let read_started = Instant::now();
    let read = reader
        .read_records(&mut |record: &[u8]| {
            if written >= limit || write_error.is_some() {
                return;
            }
            if let Err(e) = out.write_all(record) {
                write_error = Some(format!("запись в {}: {e}", args.out_path.display()));
                return;
            }
            written += 1;
        })
        .map_err(describe)?;
    let read_elapsed = read_started.elapsed();

    if let Some(e) = write_error {
        return Err(e);
    }
    out.flush().map_err(|e| format!("сброс буфера: {e}"))?;
    drop(out);

    let bytes_read = read * record_size as u64;
    let mib_per_second = bytes_read as f64 / (1u64 << 20) as f64 / read_elapsed.as_secs_f64().max(1e-9);
    println!(
        "[dump] прочитано {} записей ({} байт) за {:.3} с ({:.1} МиБ/с)",
        read,
        bytes_read,
        read_elapsed.as_secs_f64(),
        mib_per_second
    );
    println!(
        "[dump] записано {} записей ({} байт) -> {}",
        written,
        written * record_size as u64,
        args.out_path.display()
    );

    let expected = written * record_size as u64;
    let actual = fs::metadata(&args.out_path)
        .map_err(|e| format!("размер {}: {e}", args.out_path.display()))?
        .len();
    if actual != expected {
        return Err(format!(
            "размер {} не совпал: ожидалось {expected} байт, получено {actual}",
            args.out_path.display()
        ));
    }

    if args.boot {
        let boot_path = args
            .out_path
            .parent()
            .map(|dir| dir.join("boot.bin"))
            .unwrap_or_else(|| PathBuf::from("boot.bin"));
        fs::write(&boot_path, reader.boot_bytes())
            .map_err(|e| format!("запись {}: {e}", boot_path.display()))?;
        println!(
            "[dump] загрузочный сектор ({} байт) -> {}",
            reader.boot_bytes().len(),
            boot_path.display()
        );
    }

    if args.corrupt {
        corrupt_last_record(&args.out_path, record_size)?;
        println!(
            "[dump] последняя запись испорчена -> {}",
            args.out_path.display()
        );
    }

    if args.spike {
        let letter = normalize_drive_letter(&args.drive).map_err(describe)?;
        let volume_path = format!(r"\\.\{}:", letter);
        let volume = File::open(&volume_path)
            .map_err(|e| format!("открытие {volume_path} для замера (нужен администратор): {e}"))?;

        println!("[spike] замер способа (б) FSCTL_GET_NTFS_FILE_RECORD...");
        let io_started = Instant::now();
        let (ok, errors) = read_via_ioctl(&volume, record_count, args.spike_limit)?;
        let elapsed = io_started.elapsed();
        let measured = args.spike_limit.unwrap_or(record_count).min(record_count);
        let per_record_us = elapsed.as_secs_f64() * 1e6 / measured.max(1) as f64;
        println!(
            "[spike] способ (б): {} записей (ok={ok}, err={errors}) за {:.3} с, {:.1} мкс/запись",
            measured,
            elapsed.as_secs_f64(),
            per_record_us
        );
        if args.spike_limit.is_some() {
            println!(
                "[spike] экстраполяция на все {} записей: {:.1} с",
                record_count,
                per_record_us * record_count as f64 / 1e6
            );
        }
    }

    println!("[done] всего {:.3} с", started.elapsed().as_secs_f64());
    Ok(())
}
