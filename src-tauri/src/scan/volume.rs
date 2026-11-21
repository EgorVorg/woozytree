//! Прямое чтение сырых записей `$MFT` с NTFS-тома.
//!
//! Почему сырое чтение: разбор записей MFT напрямую с диска (загрузочный
//! сектор → размещение `$MFT` → последовательное чтение) даёт скорость на
//! порядок выше, чем обход дерева папок или перебор записей через
//! `FSCTL_GET_NTFS_FILE_RECORD`. Замеры — в `docs/superpowers/spikes/2026-11-21-mft-reading.md`.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::model::ScanError;

/// Первые 512 байт тома — классический загрузочный сектор NTFS.
pub const BOOT_SECTOR_LEN: usize = 512;
/// Читаем загрузочный сектор с запасом: на 4Kn-дисках сектор 4096 байт,
/// а сырое чтение тома требует, чтобы длина запроса была кратна сектору.
const BOOT_READ_BYTES: usize = 4096;
/// Одна порция последовательного чтения `$MFT`.
const READ_CHUNK_BYTES: u64 = 1 << 20;

const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Параметры NTFS-тома, нужные для чтения `$MFT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootSector {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    /// Размер одной записи MFT в байтах (обычно 1024).
    pub record_size: usize,
    /// LCN (номер кластера) начала `$MFT`.
    pub mft_lcn: u64,
    pub total_sectors: u64,
    pub volume_serial: u64,
}

impl BootSector {
    /// Размер кластера в байтах — единица адресации `$DATA`-runlist.
    pub fn bytes_per_cluster(&self) -> u64 {
        self.bytes_per_sector as u64 * self.sectors_per_cluster as u64
    }
}

/// Непрерывный участок `$MFT`: номер первого кластера и длина в кластерах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub lcn: u64,
    pub clusters: u64,
}

/// Откуда читаются записи: с живого тома или из файла-слепка (тесты).
enum Source {
    Volume {
        file: RefCell<File>,
        extents: Vec<Extent>,
        cluster_size: u64,
        /// Реальный (логический) размер `$MFT`, байт.
        mft_size: u64,
    },
    Fixture {
        file: RefCell<File>,
        record_count: u64,
    },
}

/// Читатель сырых записей MFT: либо с тома, либо из файла-слепка (для тестов).
pub struct VolumeReader {
    source: Source,
    record_size: usize,
    boot: Option<BootSector>,
    boot_raw: Vec<u8>,
}

impl VolumeReader {
    /// Открыть том `\\.\C:` и найти размещение `$MFT`.
    ///
    /// Нужны права администратора: иначе Windows не даёт открыть сырой том.
    pub fn open(drive_letter: &str) -> Result<VolumeReader, ScanError> {
        let letter = normalize_drive_letter(drive_letter)?;
        let volume_path = format!(r"\\.\{}:", letter);

        let mut file = File::open(&volume_path).map_err(|e| {
            ScanError::VolumeAccess(format!(
                "том {letter}: не удалось открыть {volume_path}: {e}. \
                 Нужны права администратора и NTFS-диск."
            ))
        })?;

        let mut raw = vec![0u8; BOOT_READ_BYTES];
        read_exact_at(&mut file, &mut raw, 0).map_err(|e| {
            ScanError::VolumeAccess(format!(
                "том {letter}: не удалось прочитать загрузочный сектор: {e}"
            ))
        })?;
        let boot = parse_boot_sector(&raw)?;

        let cluster_size = boot.bytes_per_cluster();
        // Запись №0 $MFT лежит в начале MFT. Читаем целый кластер: сырое
        // чтение тома требует кратной сектору длины, а запись может быть
        // меньше кластера.
        let read_len = cluster_size.max(boot.record_size as u64) as usize;
        let mut first = vec![0u8; read_len];
        read_exact_at(&mut file, &mut first, boot.mft_lcn * cluster_size).map_err(|e| {
            ScanError::Io(format!(
                "том {letter}: не удалось прочитать запись №0 $MFT (LCN {}): {e}",
                boot.mft_lcn
            ))
        })?;

        let record0 = &mut first[..boot.record_size];
        apply_fixup(record0, boot.bytes_per_sector as usize)?;
        let (extents, mft_size) = parse_mft_data(record0)?;
        let record_count = mft_size / boot.record_size as u64;
        if record_count == 0 {
            return Err(ScanError::Parse(format!(
                "том {letter}: $MFT пуст (реальный размер {mft_size} байт)"
            )));
        }

        let mut boot_raw = raw;
        boot_raw.truncate(BOOT_SECTOR_LEN);

        Ok(VolumeReader {
            source: Source::Volume {
                file: RefCell::new(file),
                extents,
                cluster_size,
                mft_size,
            },
            record_size: boot.record_size,
            boot: Some(boot),
            boot_raw,
        })
    }

    /// Открыть файл-слепок последовательности записей MFT (для тестов).
    ///
    /// `record_size` задаётся явно, потому что у слепка нет загрузочного сектора.
    pub fn from_fixture(path: &Path, record_size: usize) -> Result<VolumeReader, ScanError> {
        if record_size == 0 {
            return Err(ScanError::Parse("нулевой размер записи MFT".into()));
        }
        let file = File::open(path).map_err(|e| {
            ScanError::Io(format!("не удалось открыть слепок MFT {}: {e}", path.display()))
        })?;
        let len = file
            .metadata()
            .map_err(|e| ScanError::Io(format!("не удалось узнать размер {}: {e}", path.display())))?
            .len();
        if len == 0 {
            return Err(ScanError::Parse(format!("слепок MFT {} пуст", path.display())));
        }
        if len % record_size as u64 != 0 {
            return Err(ScanError::Parse(format!(
                "размер слепка {} ({len} байт) не кратен размеру записи {record_size}",
                path.display()
            )));
        }

        Ok(VolumeReader {
            source: Source::Fixture {
                file: RefCell::new(file),
                record_count: len / record_size as u64,
            },
            record_size,
            boot: None,
            boot_raw: Vec::new(),
        })
    }

    /// Размер одной записи MFT в байтах.
    pub fn record_size(&self) -> usize {
        self.record_size
    }

    /// Сколько записей ожидается прочитать.
    pub fn record_count(&self) -> u64 {
        match &self.source {
            Source::Volume { mft_size, .. } => mft_size / self.record_size as u64,
            Source::Fixture { record_count, .. } => *record_count,
        }
    }

    /// Отдать все записи в `sink` по одной; вернуть их количество.
    pub fn read_records(&self, sink: &mut dyn FnMut(&[u8])) -> Result<u64, ScanError> {
        match &self.source {
            Source::Fixture {
                file,
                record_count,
            } => read_fixture(file, *record_count, self.record_size, sink),
            Source::Volume {
                file,
                extents,
                cluster_size,
                mft_size,
            } => read_volume(
                file,
                extents,
                *cluster_size,
                *mft_size,
                self.record_size,
                sink,
            ),
        }
    }

    /// Параметры тома; у слепка их нет (`None`).
    pub fn boot_sector(&self) -> Option<&BootSector> {
        self.boot.as_ref()
    }

    /// Первые 512 байт тома (пусто у слепка).
    pub fn boot_bytes(&self) -> &[u8] {
        &self.boot_raw
    }

    /// Экстенты `$MFT` (у слепка пусто) — для диагностики и утилиты dump_mft.
    pub fn mft_extents(&self) -> &[Extent] {
        match &self.source {
            Source::Volume { extents, .. } => extents,
            Source::Fixture { .. } => &[],
        }
    }
}

/// Прочитать том последовательно по экстентам `$MFT` и нарезать на записи.
fn read_volume(
    file: &RefCell<File>,
    extents: &[Extent],
    cluster_size: u64,
    mft_size: u64,
    record_size: usize,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<u64, ScanError> {
    let mut file = file.borrow_mut();
    let chunk_clusters = (READ_CHUNK_BYTES / cluster_size).max(1);
    let chunk_bytes = (chunk_clusters * cluster_size) as usize;
    // Запас `record_size`: хвост неполной записи переносится в начало буфера.
    let mut buf = vec![0u8; chunk_bytes + record_size];
    let mut carry = 0usize;
    let mut remaining = mft_size;
    let mut records = 0u64;

    for extent in extents {
        if remaining == 0 {
            break;
        }
        let mut cluster_index = 0u64;
        while cluster_index < extent.clusters && remaining > 0 {
            let take_clusters = chunk_clusters.min(extent.clusters - cluster_index);
            let take_bytes = (take_clusters * cluster_size) as usize;
            let offset = extent.lcn * cluster_size + cluster_index * cluster_size;
            read_exact_at(&mut file, &mut buf[carry..carry + take_bytes], offset).map_err(|e| {
                ScanError::Io(format!(
                    "не удалось прочитать {} байт $MFT по смещению {offset}: {e}",
                    take_bytes
                ))
            })?;

            // Последний экстент может быть длиннее `$MFT`: больше реального
            // размера не читаем, иначе наружу уйдут записи вне таблицы.
            let usable = take_bytes.min(remaining as usize);
            let total = carry + usable;
            let ready = total / record_size;
            for i in 0..ready {
                let start = i * record_size;
                sink(&buf[start..start + record_size]);
            }
            records += ready as u64;

            // Неполная запись остаётся в начале буфера до следующей порции.
            let consumed = ready * record_size;
            buf.copy_within(consumed..total, 0);
            carry = total - consumed;

            remaining -= usable as u64;
            cluster_index += take_clusters;
        }
    }

    Ok(records)
}

/// Прочитать записи из файла-слепка (в нём запись идёт строго за записью).
fn read_fixture(
    file: &RefCell<File>,
    record_count: u64,
    record_size: usize,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<u64, ScanError> {
    const RECORDS_PER_CHUNK: u64 = 256;
    let mut file = file.borrow_mut();
    let mut buf = vec![0u8; record_size * RECORDS_PER_CHUNK as usize];
    let mut count = 0u64;

    while count < record_count {
        let take = RECORDS_PER_CHUNK.min(record_count - count) as usize;
        let bytes = take * record_size;
        let offset = count * record_size as u64;
        read_exact_at(&mut file, &mut buf[..bytes], offset)
            .map_err(|e| ScanError::Io(format!("чтение слепка MFT: {e}")))?;
        for i in 0..take {
            let start = i * record_size;
            sink(&buf[start..start + record_size]);
        }
        count += take as u64;
    }

    Ok(count)
}

fn read_exact_at(file: &mut File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

/// Привести аргумент пользователя к букве тома: "C", "c:", "C:\" -> "C".
pub fn normalize_drive_letter(arg: &str) -> Result<String, ScanError> {
    let trimmed = arg.trim().trim_end_matches([':', '\\', '/']);
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase().to_string()),
        _ => Err(ScanError::VolumeAccess(format!(
            "ожидалась буква тома (например C), получено: {arg}"
        ))),
    }
}

/// Разобрать загрузочный сектор NTFS (первые 512 байт тома).
pub fn parse_boot_sector(bytes: &[u8]) -> Result<BootSector, ScanError> {
    if bytes.len() < BOOT_SECTOR_LEN {
        return Err(ScanError::Parse(format!(
            "загрузочный сектор короче {BOOT_SECTOR_LEN} байт: {}",
            bytes.len()
        )));
    }
    if &bytes[3..11] != b"NTFS    " {
        return Err(ScanError::Parse(
            "не NTFS: нет подписи 'NTFS    ' по смещению 3".into(),
        ));
    }

    // Раскладка загрузочного сектора NTFS:
    //   0x0B u16 — байт в секторе, 0x0D u8 — секторов в кластере,
    //   0x28 u64 — всего секторов, 0x30 u64 — LCN начала $MFT,
    //   0x40 i8  — кластеров на запись MFT (знаковое!), 0x48 u64 — серийный номер.
    let bytes_per_sector = u16::from_le_bytes([bytes[0x0B], bytes[0x0C]]);
    let sectors_per_cluster = bytes[0x0D];
    if bytes_per_sector == 0 || sectors_per_cluster == 0 {
        return Err(ScanError::Parse(
            "нулевой размер сектора или кластера в загрузочном секторе".into(),
        ));
    }
    let bytes_per_cluster = bytes_per_sector as u64 * sectors_per_cluster as u64;

    let total_sectors = u64::from_le_bytes(bytes[0x28..0x30].try_into().unwrap());
    let mft_lcn = u64::from_le_bytes(bytes[0x30..0x38].try_into().unwrap());
    let volume_serial = u64::from_le_bytes(bytes[0x48..0x50].try_into().unwrap());

    // Положительное значение — кластеров на запись, отрицательное — логарифм
    // по основанию 2: например -10 означает 2^10 = 1024 байта.
    let clusters_per_record = bytes[0x40] as i8;
    let record_size = if clusters_per_record > 0 {
        clusters_per_record as usize * bytes_per_cluster as usize
    } else if clusters_per_record < 0 {
        let shift = (-(clusters_per_record as i16)) as u32;
        if shift >= 32 {
            return Err(ScanError::Parse(
                "неправдоподобный размер записи MFT в загрузочном секторе".into(),
            ));
        }
        1usize << shift
    } else {
        return Err(ScanError::Parse(
            "нулевой размер записи MFT в загрузочном секторе".into(),
        ));
    };
    if !(64..=65536).contains(&record_size) {
        return Err(ScanError::Parse(format!(
            "неправдоподобный размер записи MFT: {record_size}"
        )));
    }

    Ok(BootSector {
        bytes_per_sector,
        sectors_per_cluster,
        record_size,
        mft_lcn,
        total_sectors,
        volume_serial,
    })
}

/// Разобрать runlist (mapping pairs) в список экстентов.
///
/// Формат элемента: заголовочный байт, в котором младшие 4 бита — длина поля
/// размера, старшие 4 — длина поля смещения. Размер — число кластеров,
/// смещение — знаковое и отсчитывается от LCN предыдущего экстента.
pub fn parse_runlist(bytes: &[u8]) -> Result<Vec<Extent>, ScanError> {
    let mut extents = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;

    while pos < bytes.len() {
        let header = bytes[pos];
        pos += 1;
        if header == 0 {
            break;
        }
        let len_bytes = (header & 0x0F) as usize;
        let off_bytes = (header >> 4) as usize;
        if len_bytes == 0 || len_bytes > 8 || off_bytes > 8 {
            return Err(ScanError::Parse("битый заголовок элемента runlist".into()));
        }
        if pos + len_bytes + off_bytes > bytes.len() {
            return Err(ScanError::Parse("runlist обрывается раньше времени".into()));
        }

        let mut clusters = 0u64;
        for k in 0..len_bytes {
            clusters |= (bytes[pos + k] as u64) << (8 * k);
        }
        pos += len_bytes;
        if clusters == 0 {
            return Err(ScanError::Parse("нулевая длина экстента в runlist".into()));
        }

        if off_bytes > 0 {
            let mut delta = 0i64;
            for k in 0..off_bytes {
                delta |= (bytes[pos + k] as i64) << (8 * k);
            }
            // Знаковое расширение: старший бит поля смещения — знак.
            let shift = 64 - 8 * off_bytes as u32;
            delta = (delta << shift) >> shift;
            pos += off_bytes;
            lcn += delta;
        }
        if lcn < 0 {
            return Err(ScanError::Parse("отрицательный LCN в runlist".into()));
        }
        extents.push(Extent {
            lcn: lcn as u64,
            clusters,
        });
    }

    if extents.is_empty() {
        return Err(ScanError::Parse("runlist пуст".into()));
    }
    Ok(extents)
}

/// Снять fixup (Update Sequence Array) с записи MFT.
///
/// На диске последние 2 байта каждого сектора записи заменены номером
/// последовательности (USN), а настоящие байты лежат в USA-массиве заголовка.
/// Без этой замены разбор атрибутов может прочитать мусор на границах секторов.
fn apply_fixup(record: &mut [u8], bytes_per_sector: usize) -> Result<(), ScanError> {
    if record.len() < 0x30 || bytes_per_sector < 2 {
        return Err(ScanError::Parse("запись MFT короче заголовка".into()));
    }
    let usa_offset = u16::from_le_bytes([record[0x04], record[0x05]]) as usize;
    let usa_count = u16::from_le_bytes([record[0x06], record[0x07]]) as usize;
    if usa_count == 0 || usa_offset + usa_count * 2 > record.len() {
        return Err(ScanError::Parse(
            "некорректный Update Sequence Array в записи MFT".into(),
        ));
    }

    let usn = [record[usa_offset], record[usa_offset + 1]];
    for i in 1..usa_count {
        let tail = i * bytes_per_sector;
        if tail > record.len() {
            return Err(ScanError::Parse(
                "запись MFT короче, чем обещает Update Sequence Array".into(),
            ));
        }
        if record[tail - 2..tail] != usn {
            return Err(ScanError::Parse(format!(
                "fixup: не совпал номер последовательности в секторе {i}"
            )));
        }
        let from = usa_offset + i * 2;
        let saved = [record[from], record[from + 1]];
        record[tail - 2..tail].copy_from_slice(&saved);
    }
    Ok(())
}

/// Достать из записи №0 runlist и реальный размер неименованного `$DATA`.
fn parse_mft_data(record: &[u8]) -> Result<(Vec<Extent>, u64), ScanError> {
    if record.len() < 0x30 || &record[0..4] != b"FILE" {
        return Err(ScanError::Parse(
            "запись №0 $MFT не начинается с подписи FILE".into(),
        ));
    }
    let first_attr = u16::from_le_bytes([record[0x14], record[0x15]]) as usize;
    let mut off = first_attr;

    while off + 0x18 <= record.len() {
        let attr_type = u32::from_le_bytes(record[off..off + 4].try_into().unwrap());
        if attr_type == ATTR_END {
            break;
        }
        let attr_len = u32::from_le_bytes(record[off + 4..off + 8].try_into().unwrap()) as usize;
        if attr_len < 0x18 || off + attr_len > record.len() {
            return Err(ScanError::Parse(
                "некорректная длина атрибута в записи №0".into(),
            ));
        }

        let non_resident = record[off + 0x08] != 0;
        let name_len = record[off + 0x09];
        // $MFT — это неименованный нерезидентный поток $DATA (атрибут 0x80).
        if attr_type == ATTR_DATA && non_resident && name_len == 0 {
            let run_off = u16::from_le_bytes([record[off + 0x20], record[off + 0x21]]) as usize;
            let real_size =
                u64::from_le_bytes(record[off + 0x30..off + 0x38].try_into().unwrap());
            let runlist_start = off + run_off;
            if run_off < 0x40 || runlist_start >= off + attr_len {
                return Err(ScanError::Parse(
                    "runlist $DATA выходит за границы атрибута".into(),
                ));
            }
            let extents = parse_runlist(&record[runlist_start..off + attr_len])?;
            return Ok((extents, real_size));
        }

        off += attr_len;
    }

    Err(ScanError::Parse(
        "в записи №0 не найден неименованный нерезидентный $DATA".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn parse_boot_sector_reads_mft_params() {
        let raw = std::fs::read(fixture("boot.bin")).expect("нет фикстуры boot.bin");
        let boot = parse_boot_sector(&raw).expect("загрузочный сектор должен разбираться");

        assert_eq!(boot.bytes_per_sector, 512);
        assert_eq!(boot.sectors_per_cluster, 8);
        assert_eq!(boot.record_size, 1024);
        assert_eq!(boot.mft_lcn, 786_432);
        assert_eq!(boot.volume_serial, 0xe896_0b86_960b_548a);
    }

    #[test]
    fn parse_runlist_decodes_extents() {
        // Три прогона: (100, 4); смещение +6 → (106, 8); смещение +194 → (300, 2).
        // Заголовок 0x11 — оба поля по 1 байту; 0x21 — размер 2 байта, смещение 1.
        #[rustfmt::skip]
        let raw = [
            0x11, 0x04, 0x64,             // LCN 100, 4 кластера
            0x11, 0x08, 0x06,             // +6 → LCN 106, 8 кластеров
            0x21, 0x02, 0xC2, 0x00,       // +194 → LCN 300, 2 кластера
            0x00,                         // конец runlist
        ];

        let extents = parse_runlist(&raw).expect("runlist должен разбираться");
        assert_eq!(
            extents,
            vec![
                Extent {
                    lcn: 100,
                    clusters: 4
                },
                Extent {
                    lcn: 106,
                    clusters: 8
                },
                Extent {
                    lcn: 300,
                    clusters: 2
                },
            ]
        );
    }

    #[test]
    fn from_fixture_reads_dumped_records() {
        let reader = VolumeReader::from_fixture(&fixture("sample.mft"), 1024)
            .expect("фикстура sample.mft должна открываться");
        assert_eq!(reader.record_size(), 1024);
        assert_eq!(reader.record_count(), 2000);

        let mut records = 0u64;
        let mut wrong_size = 0u64;
        let mut first_signature_ok = false;
        let count = reader
            .read_records(&mut |record| {
                if record.len() != 1024 {
                    wrong_size += 1;
                }
                if records == 0 {
                    first_signature_ok = record.starts_with(b"FILE");
                }
                records += 1;
            })
            .expect("чтение фикстуры не должно падать");

        assert_eq!(count, 2000);
        assert_eq!(records, 2000);
        assert_eq!(wrong_size, 0);
        assert!(first_signature_ok, "первая запись должна начинаться с FILE");
    }
}
