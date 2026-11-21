//! Разбор записей `$MFT` в данные о файлах и наполнение индекса сканирования.
//!
//! Задача 4 дала сырое чтение записей MFT ([`VolumeReader`]); здесь из сырых
//! байтов получаются имена, родители, размеры, время и флаги, а из них строится
//! дерево [`ScanIndex`]. Парсинг ручной: крейт `mft` 0.10 умеет то же самое, но
//! работает через `Read + Seek` по плоскому потоку `$MFT` (у нас записи приходят
//! по одной из несмежных экстентов) и тянет два десятка транзитивных зависимостей
//! ради четырёх полей времени. Ручной разбор — это один проход по цепочке
//! атрибутов, как в `volume.rs`, без лишних аллокаций на запись.
//!
//! Раскладка записи MFT (все смещения — от начала записи):
//! - `0x00` подпись `FILE`;
//! - `0x04` u16 — смещение Update Sequence Array (USA), `0x06` u16 — число слов USA;
//! - `0x10` u16 — номер последовательности записи;
//! - `0x14` u16 — смещение первого атрибута;
//! - `0x16` u16 — флаги: бит 0 — запись используется, бит 1 — это папка;
//! - `0x20` u64 — ссылка на базовую запись (не 0 у extension-записей);
//! - `0x2C` u32 — номер этой записи в `$MFT`.
//!
//! На диске последние 2 байта каждого 512-байтного сектора записи заменены
//! номером последовательности (USA), а настоящие байты лежат в массиве USA
//! заголовка. Без снятия fixup атрибут, пересекающий границу сектора, читается
//! с мусором в двух байтах — поэтому [`parse_record`] снимает fixup сам.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::model::{FileKey, FileMeta, FolderId, ScanError, ScanProgress, ScanTotals};
use crate::scan::index::{AddOutcome, ScanIndex};
use crate::scan::volume::VolumeReader;

/// Атрибут `$STANDARD_INFORMATION` — четыре времени и флаги файла.
const ATTR_STANDARD_INFORMATION: u32 = 0x10;
/// Атрибут `$ATTRIBUTE_LIST` — часть атрибутов живёт в extension-записях.
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
/// Атрибут `$FILE_NAME` — имя, родитель и (устаревшие) размеры.
const ATTR_FILE_NAME: u32 = 0x30;
/// Атрибут `$DATA` — данные файла; неименованный поток задаёт размеры файла.
const ATTR_DATA: u32 = 0x80;
/// Признак конца цепочки атрибутов.
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Минимальная длина заголовка атрибута (резидентного).
const ATTR_HEADER_LEN: usize = 0x18;
/// Заголовок нерезидентного атрибута: до полей runlist и размеров.
const ATTR_NONRESIDENT_HEADER_LEN: usize = 0x40;

/// Бит 0 флагов записи: запись используется (не удалена).
const RECORD_FLAG_IN_USE: u16 = 0x0001;
/// Бит 1 флагов записи: это папка.
const RECORD_FLAG_DIRECTORY: u16 = 0x0002;

/// Корень тома — запись `$MFT` №5.
const ROOT_RECORD: u64 = 5;

/// Запись `$BadClus` (номер 8 закреплён форматом NTFS). Её поток `$Bad` разрежен
/// и объявляет allocated, равный размеру всего тома, хотя реально занимает лишь
/// несколько кластеров с бэдами. В занятое место его учитывать нельзя.
const BAD_CLUSTER_RECORD: u64 = 8;

/// Младшие 48 бит ссылки MFT — номер записи, старшие 16 — номер последовательности.
const REF_RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Шаг fixup по умолчанию: классический сектор 512 байт. У живого тома шаг
/// берётся из загрузочного сектора (см. [`scan_ntfs_reader`]), у файла-слепка
/// загрузочного сектора нет.
const DEFAULT_FIXUP_STRIDE: usize = 512;

/// FILETIME считается в 100-нс интервалах от 1601-01-01, Unix-эпоха — на
/// 11 644 473 600 000 мс позже.
const FILETIME_UNIX_OFFSET_MS: i64 = 11_644_473_600_000;

/// Через сколько обработанных записей сообщать прогресс.
const PROGRESS_EVERY_RECORDS: u64 = 1000;

/// Одно имя файла из записи MFT.
///
/// Жёсткие ссылки дают несколько имён с одним `file_key`, поэтому на одну запись
/// приходится столько элементов, сколько у неё атрибутов `$FILE_NAME`.
#[derive(Debug)]
pub struct MftFileInfo {
    pub name: String,
    /// Номер записи-родителя в MFT (младшие 48 бит ссылки).
    pub parent_ref: u64,
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,
    /// `file_index` — номер записи MFT, `volume_serial` заполняет сканер.
    pub file_key: FileKey,
    pub is_directory: bool,
}

/// Разбивка [`ScanTotals::skipped`] по причинам.
///
/// `skipped` — это не «сколько файлов легло в корень», а число событий «запись
/// или имя разобрано неполностью». Причины считаются независимо, поэтому одно
/// имя или одна запись может дать несколько событий (например, базовая запись с
/// `$ATTRIBUTE_LIST` и потерянное имя её extension-записи). Сумма полей равна
/// `ScanTotals::skipped`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanDiag {
    /// Битые записи: чужая подпись, сломанный fixup, обрыв цепочки атрибутов.
    pub broken: u64,
    /// Базовые записи с `$ATTRIBUTE_LIST`: часть атрибутов вынесена в другие записи.
    pub partial: u64,
    /// Extension-записи, в которых были имена: имена не разбираются.
    pub extension_with_names: u64,
    /// Имена файлов, чей родитель не найден: файл положен в корень.
    pub orphan: u64,
    /// Папки, чей родитель так и не разрешился: папка положена в корень.
    pub unresolved_folder: u64,
}

impl ScanDiag {
    /// Сумма событий по всем причинам; равна `ScanTotals::skipped`.
    pub fn total(&self) -> u64 {
        self.broken
            + self.partial
            + self.extension_with_names
            + self.orphan
            + self.unresolved_folder
    }
}

/// Чем закончился разбор записи. Нужно сканеру, чтобы отличить нормальную
/// запись от мусора и от фрагмента файла, но наружу отдать только данные.
enum RecordParse {
    /// Запись не используется: удалённый файл или пустой слот `$MFT`.
    Unused,
    /// Extension-запись: сюда NTFS выносит часть атрибутов файла, когда они не
    /// помещаются в базовую запись. Размеры отсюда нужны: неименованный `$DATA`
    /// крупного файла часто лежит именно здесь, а без него файл выглядел бы
    /// пустым. Имена из extension-записей теряются — `$ATTRIBUTE_LIST` не
    /// разбирается (известное ограничение, такие записи попадают в `skipped`).
    Extension {
        /// Номер базовой записи файла (младшие 48 бит ссылки).
        base_index: u64,
        /// Размеры неименованного `$DATA`, если он лежит в этой записи.
        data: Option<(u64, u64)>,
        /// Именованные потоки `$DATA` из этой записи.
        streams: Vec<(String, u64)>,
        /// Были ли в записи имена (тогда они потеряны).
        has_names: bool,
    },
    /// Мусор: чужая подпись, битый fixup или обрыв цепочки атрибутов.
    Broken,
    /// Имена записи (по элементу на каждый `$FILE_NAME`) и признак того, что
    /// запись разобрана неполностью ($ATTRIBUTE_LIST или обрыв цепочки атрибутов).
    Info {
        infos: Vec<MftFileInfo>,
        partial: bool,
        /// Именованные потоки `$DATA` (альтернативные данные): имя и занятые байты.
        streams: Vec<(String, u64)>,
    },
}

/// Значение резидентного атрибута `$FILE_NAME`.
struct FileNameValue {
    name: String,
    parent_ref: u64,
    modified: i64,
    /// Пространство имён: 0 — POSIX, 1 — Win32 (длинное имя), 2 — DOS (8.3),
    /// 3 — Win32 и DOS вместе.
    namespace: u8,
}

/// Разобрать одну запись `$MFT` в имена файлов.
///
/// Возвращает по элементу на каждый атрибут `$FILE_NAME` (жёсткие ссылки дают
/// несколько элементов с одним `file_key`). Пустой `Vec` — запись не используется,
/// это extension-запись или мусор.
pub fn parse_record(record: &[u8]) -> Vec<MftFileInfo> {
    let mut buf = Vec::with_capacity(record.len());
    match parse_record_with_stride(record, DEFAULT_FIXUP_STRIDE, &mut buf) {
        RecordParse::Info {
            mut infos, streams, ..
        } => {
            // Место под именованными потоками (альтернативными данными) входит
            // в занятое место файла, хотя логический размер задаёт неименованный
            // `$DATA`. У папки собственных данных нет — ей не прибавляем.
            // Сегменты одного потока повторяют (или обнуляют) размер, поэтому
            // сначала берём максимум по имени потока, а затем складываем потоки.
            if !streams.is_empty() && infos.iter().any(|info| !info.is_directory) {
                let mut by_name: HashMap<(u64, String), u64> = HashMap::new();
                collect_streams(&mut by_name, 0, streams);
                let total: u64 = by_name.values().sum();
                for info in &mut infos {
                    if !info.is_directory {
                        info.size_on_disk += total;
                    }
                }
            }
            infos
        }
        RecordParse::Unused | RecordParse::Extension { .. } | RecordParse::Broken => Vec::new(),
    }
}

/// Прочитать целое число из записи; `None`, если поля нет (запись короче).
fn u16_at(bytes: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let slice = bytes.get(off..end)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    let slice = bytes.get(off..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_at(bytes: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    let slice = bytes.get(off..end)?;
    let mut value = [0u8; 8];
    value.copy_from_slice(slice);
    Some(u64::from_le_bytes(value))
}

/// FILETIME → Unix-мс. Значение 0 в NTFS означает «время не задано», его
/// оставляем нулём, а не уводим в 1601 год.
fn filetime_to_unix_ms(filetime: u64) -> i64 {
    if filetime == 0 {
        return 0;
    }
    (filetime / 10_000) as i64 - FILETIME_UNIX_OFFSET_MS
}

/// Снять fixup (Update Sequence Array) на месте.
///
/// Последние 2 байта каждого сектора записи заменены номером последовательности,
/// а настоящие байты лежат в массиве USA заголовка. Возвращает `false`, если
/// массив USA или номера в секторах не сходятся — такая запись испорчена.
fn apply_fixup(record: &mut [u8], stride: usize) -> bool {
    // Меньше 512 байт сектор NTFS не бывает; при крошечном шаге хвост сектора
    // попал бы внутрь самого массива USA и разбор испортил бы его.
    if stride < 512 {
        return false;
    }
    let (Some(usa_offset), Some(usa_count)) = (u16_at(record, 0x04), u16_at(record, 0x06)) else {
        return false;
    };
    let (usa_offset, usa_count) = (usize::from(usa_offset), usize::from(usa_count));
    if usa_count < 2 {
        return false;
    }
    let Some(usa_end) = usa_offset.checked_add(usa_count * 2) else {
        return false;
    };
    if usa_end > record.len() {
        return false;
    }

    let usn = [record[usa_offset], record[usa_offset + 1]];
    for sector in 1..usa_count {
        let Some(tail) = sector.checked_mul(stride) else {
            return false;
        };
        if tail > record.len() {
            return false;
        }
        if record[tail - 2..tail] != usn {
            return false;
        }
        let saved_at = usa_offset + sector * 2;
        // Копируем настоящие байты сектора из массива USA. Массив идёт в первом
        // секторе, поэтому его чтение не задевает уже восстановленные хвосты.
        let saved = [record[saved_at], record[saved_at + 1]];
        record[tail - 2..tail].copy_from_slice(&saved);
    }
    true
}

/// Разобрать запись с заданным шагом fixup; `buf` — многоразовый буфер под копию
/// записи, чтобы не выделять память на каждую запись в горячем цикле.
fn parse_record_with_stride(record: &[u8], stride: usize, buf: &mut Vec<u8>) -> RecordParse {
    if record.len() < ATTR_HEADER_LEN {
        return RecordParse::Broken;
    }
    if record.iter().all(|byte| *byte == 0) {
        // Пустой слот $MFT: подписи нет, но и порчи нет — это не пропуск данных.
        return RecordParse::Unused;
    }
    if &record[0..4] != b"FILE" {
        return RecordParse::Broken;
    }

    // Fixup правит байты записи, поэтому работаем с копией исходного среза.
    buf.clear();
    buf.extend_from_slice(record);
    if !apply_fixup(buf, stride) {
        return RecordParse::Broken;
    }
    let record: &[u8] = buf;

    let Some(flags) = u16_at(record, 0x16) else {
        return RecordParse::Broken;
    };
    if flags & RECORD_FLAG_IN_USE == 0 {
        return RecordParse::Unused;
    }
    // Ненулевая ссылка на базовую запись означает extension-запись: в ней лежит
    // часть атрибутов файла. Сама по себе она файлом не является (номер её записи
    // не ключ файла), но неименованный `$DATA` отсюда нужен: у крупных файлов он
    // часто вынесен именно в extension-запись.
    let base_index = u64_at(record, 0x20).map_or(0, |base| base & REF_RECORD_MASK);
    let is_extension = base_index != 0;

    let record_number = u64::from(u32_at(record, 0x2C).unwrap_or(0));
    let is_directory = flags & RECORD_FLAG_DIRECTORY != 0;
    let Some(first_attr) = u16_at(record, 0x14).map(usize::from) else {
        return RecordParse::Broken;
    };

    let mut names: Vec<FileNameValue> = Vec::new();
    let mut data_sizes = (0u64, 0u64);
    let mut has_data = false;
    // Именованные потоки $DATA (альтернативные данные): они тоже занимают
    // кластеры. Сегменты одного потока повторяют (или обнуляют) размер, поэтому
    // сканер берёт максимум по имени потока — см. `scan_ntfs_reader`.
    let mut streams: Vec<(String, u64)> = Vec::new();
    let mut standard_modified: Option<i64> = None;
    let mut partial = false;
    let mut off = first_attr;

    while off + 8 <= record.len() {
        let Some(attr_type) = u32_at(record, off) else {
            partial = true;
            break;
        };
        if attr_type == ATTR_END {
            break;
        }
        let Some(attr_len) = u32_at(record, off + 0x04).map(|len| len as usize) else {
            partial = true;
            break;
        };
        // Атрибут обязан занимать хотя бы свой заголовок и не выходить за запись.
        let fits = attr_len >= ATTR_HEADER_LEN
            && off
                .checked_add(attr_len)
                .is_some_and(|end| end <= record.len());
        if !fits {
            partial = true;
            break;
        }

        let non_resident = record[off + 0x08] != 0;
        let name_len = record[off + 0x09];

        match attr_type {
            ATTR_STANDARD_INFORMATION if !non_resident => {
                if let (Some(value_len), Some(value_off)) = (
                    u32_at(record, off + 0x10),
                    u16_at(record, off + 0x14).map(usize::from),
                ) {
                    let value = off + value_off;
                    // Время изменения — второе из четырёх времён (0x08).
                    if value_len >= 0x10
                        && off
                            .checked_add(attr_len)
                            .is_some_and(|end| value + 0x10 <= end)
                    {
                        standard_modified =
                            u64_at(record, value + 0x08).map(filetime_to_unix_ms);
                    }
                }
            }
            ATTR_FILE_NAME if !non_resident => {
                if let Some(value) = parse_file_name_value(record, off, attr_len) {
                    names.push(value);
                }
            }
            ATTR_DATA => {
                // Неименованный поток задаёт размеры файла, именованные (ADS) —
                // только занимаемое место. Потоки $BadClus не считаем: «$Bad» —
                // разреженный поток на весь том, его allocated равен размеру диска.
                if non_resident {
                    if attr_len < ATTR_NONRESIDENT_HEADER_LEN {
                        partial = true;
                    } else if let (Some(allocated), Some(real)) =
                        (u64_at(record, off + 0x28), u64_at(record, off + 0x30))
                    {
                        if name_len == 0 {
                            data_sizes = (allocated, real);
                            has_data = true;
                        } else if record_number != BAD_CLUSTER_RECORD && allocated > 0 {
                            if let Some(name) = attribute_name(record, off, name_len) {
                                streams.push((name, allocated));
                            }
                        }
                    }
                } else if name_len == 0 {
                    if let Some(value_len) = u32_at(record, off + 0x10) {
                        // Резидентные данные лежат внутри записи: отдельных кластеров
                        // они не занимают, поэтому size_on_disk = 0.
                        data_sizes = (0, u64::from(value_len));
                        has_data = true;
                    }
                }
            }
            ATTR_ATTRIBUTE_LIST => {
                // Часть атрибутов файла может лежать в extension-записях; без
                // разбора $ATTRIBUTE_LIST картина заведомо неполная — помечаем.
                partial = true;
            }
            _ => {}
        }

        off += attr_len;
    }

    if is_extension {
        return RecordParse::Extension {
            base_index,
            data: has_data.then_some(data_sizes),
            streams,
            has_names: !names.is_empty(),
        };
    }

    if names.is_empty() {
        return if partial {
            RecordParse::Broken
        } else {
            RecordParse::Unused
        };
    }

    // У папки собственных данных нет: её итоги складываются из файлов индексом.
    let (size_on_disk, size_logical) = if is_directory { (0, 0) } else { data_sizes };

    // Длинное имя (Win32) ставим впереди короткого DOS-алиаса «8.3»: сканер
    // крепит детей папки к первому имени записи, и дерево должно показывать
    // «Program Files», а не «PROGRA~1». Оба имени при этом остаются в выдаче.
    names.sort_by_key(|value| value.namespace == 2);

    let infos = names
        .into_iter()
        .map(|name| MftFileInfo {
            name: name.name,
            parent_ref: name.parent_ref,
            size_on_disk,
            size_logical,
            // Время берём из $STANDARD_INFORMATION, если его нет — из $FILE_NAME.
            modified: standard_modified.unwrap_or(name.modified),
            file_key: FileKey {
                volume_serial: 0, // заполняет сканер: у слепка загрузочного сектора нет
                file_index: record_number,
            },
            is_directory,
        })
        .collect();

    RecordParse::Info {
        infos,
        partial,
        streams,
    }
}

/// Прочитать имя атрибута (UTF-16LE, длина в символах).
fn attribute_name(record: &[u8], attr_off: usize, name_len: u8) -> Option<String> {
    let name_off = usize::from(u16_at(record, attr_off + 0x0A)?);
    let start = attr_off.checked_add(name_off)?;
    let end = start.checked_add(usize::from(name_len) * 2)?;
    let units: Vec<u16> = record
        .get(start..end)?
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Разобрать значение резидентного `$FILE_NAME`.
///
/// Раскладка значения: `0x00` ссылка на родителя (младшие 48 бит — номер записи),
/// `0x08` время создания, `0x10` время изменения, `0x18` изменение MFT,
/// `0x20` время доступа, `0x28`/`0x30` — устаревшие размеры, `0x40` длина имени
/// в символах UTF-16, `0x42` — само имя.
fn parse_file_name_value(record: &[u8], attr_off: usize, attr_len: usize) -> Option<FileNameValue> {
    let value_len = u32_at(record, attr_off + 0x10)? as usize;
    let value_off = usize::from(u16_at(record, attr_off + 0x14)?);
    let value_start = attr_off.checked_add(value_off)?;
    let value_end = value_start.checked_add(value_len)?;
    let attr_end = attr_off.checked_add(attr_len)?;

    // Минимальное значение $FILE_NAME — 0x42 байта до начала имени.
    if value_len < 0x42 || value_end > record.len() || value_end > attr_end {
        return None;
    }

    let parent_ref = u64_at(record, value_start)? & REF_RECORD_MASK;
    // 0x10 — время изменения; 0x08 — создание, его брать нельзя.
    let modified = filetime_to_unix_ms(u64_at(record, value_start + 0x10)?);
    let units_len = usize::from(*record.get(value_start + 0x40)?);
    let namespace = *record.get(value_start + 0x41)?;
    let name_start = value_start + 0x42;
    let name_end = name_start + units_len * 2;
    if name_end > value_end {
        return None;
    }

    let units: Vec<u16> = record[name_start..name_end]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let name = String::from_utf16_lossy(&units);
    if name.is_empty() {
        return None;
    }

    Some(FileNameValue {
        name,
        parent_ref,
        modified,
        namespace,
    })
}

/// Просканировать NTFS-том, читая записи `$MFT` через `reader`.
///
/// Построение двухпроходное: первый проход разбирает все записи и складывает
/// имена в память, второй — создаёт папки и файлы в индексе. Иначе нельзя:
/// родитель по номеру записи может встретиться в `$MFT` и позже ребёнка, а
/// `FolderId` выдаётся только при создании папки.
///
/// Тонкая обёртка над [`scan_ntfs_reader_diag`]: отдаёт только итоги.
pub fn scan_ntfs_reader(
    reader: &VolumeReader,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanTotals, ScanError> {
    scan_ntfs_reader_diag(reader, index, cancel, progress).map(|(totals, _)| totals)
}

/// То же, что [`scan_ntfs_reader`], но вместе с разбивкой `skipped` по причинам.
///
/// `ScanTotals::skipped` — составной счётчик: без разбивки нельзя сказать,
/// сколько пропусков дали битые записи, сколько — `$ATTRIBUTE_LIST`, а сколько —
/// потерянные родители. Разбивка нужна бенчмарку и отчётам; на логику
/// сканирования она не влияет.
pub fn scan_ntfs_reader_diag(
    reader: &VolumeReader,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<(ScanTotals, ScanDiag), ScanError> {
    let started = Instant::now();
    // Шаг fixup у живого тома равен размеру сектора из загрузочного сектора
    // (4Kn-диски), у файла-слепка берём классические 512 байт.
    let stride = reader
        .boot_sector()
        .map_or(DEFAULT_FIXUP_STRIDE, |boot| {
            usize::from(boot.bytes_per_sector)
        });
    let volume_serial = reader
        .boot_sector()
        .map_or(0, |boot| boot.volume_serial as u32);

    // Первый проход: разобрать записи. Имена всех записей держим в памяти —
    // иначе не разрешить родителей, которые лежат в $MFT позже детей.
    let mut entries: Vec<MftFileInfo> = Vec::with_capacity(64 * 1024);
    // Размеры из extension-записей: номер базовой записи -> ($DATA allocated, real).
    let mut extension_data: HashMap<u64, (u64, u64)> = HashMap::new();
    // Именованные потоки: (номер файла, имя потока) -> максимум занятых байт.
    let mut stream_bytes: HashMap<(u64, String), u64> = HashMap::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut record_index: u64 = 0;
    // Пропуски считаем по причинам: `ScanTotals::skipped` — их сумма (см. `ScanDiag`).
    let mut diag = ScanDiag::default();
    let mut parsed_on_disk: u64 = 0;

    reader.read_records(&mut |record| {
        // Отмена проверяется на каждой записи: порции чтения $MFT крупнее, а
        // остановить разбор между записями дешевле, чем после всей таблицы.
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match parse_record_with_stride(record, stride, &mut buffer) {
            RecordParse::Info {
                infos,
                partial,
                streams,
            } => {
                if partial {
                    // $ATTRIBUTE_LIST не разбирается — запись неполная.
                    diag.partial += 1;
                }
                for mut info in infos {
                    info.file_key.volume_serial = volume_serial;
                    // Номер записи — её позиция в $MFT: это надёжнее поля 0x2C,
                    // которого может не быть в старых версиях NTFS.
                    info.file_key.file_index = record_index;
                    parsed_on_disk += info.size_on_disk;
                    entries.push(info);
                }
                collect_streams(&mut stream_bytes, record_index, streams);
            }
            RecordParse::Extension {
                base_index,
                data,
                streams,
                has_names,
            } => {
                if let Some(sizes) = data {
                    extension_data.entry(base_index).or_insert(sizes);
                }
                collect_streams(&mut stream_bytes, base_index, streams);
                if has_names {
                    // Имена из extension-записей не разбираются: $ATTRIBUTE_LIST
                    // не поддерживается — значит, часть имён файла потеряна.
                    diag.extension_with_names += 1;
                }
            }
            RecordParse::Broken => diag.broken += 1,
            RecordParse::Unused => {}
        }

        record_index += 1;
        if record_index % PROGRESS_EVERY_RECORDS == 0 {
            // Пока идёт чтение, точных итогов нет: размер показан без учёта
            // дедупликации жёстких ссылок и данных из extension-записей,
            // папки ещё не созданы.
            progress(&ScanProgress {
                files: entries.len() as u64,
                folders: 0,
                size_on_disk: parsed_on_disk,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
    })?;

    // Файлам, чей $DATA вынесен в extension-запись, подставляем размеры оттуда:
    // иначе каждый такой файл выглядел бы пустым (на C: это ~136 ГиБ).
    if !extension_data.is_empty() {
        for entry in &mut entries {
            if entry.is_directory || entry.size_on_disk != 0 || entry.size_logical != 0 {
                continue;
            }
            if let Some(&(allocated, real)) = extension_data.get(&entry.file_key.file_index) {
                entry.size_on_disk = allocated;
                entry.size_logical = real;
            }
        }
    }

    // Место под именованными потоками (альтернативными данными) тоже занято.
    // Сегменты одного потока повторяют полный размер или обнуляют его, поэтому
    // по каждому имени берётся максимум и только потом всё складывается.
    if !stream_bytes.is_empty() {
        let mut streams_total: HashMap<u64, u64> = HashMap::new();
        for ((file_index, _name), allocated) in &stream_bytes {
            *streams_total.entry(*file_index).or_insert(0) += *allocated;
        }
        for entry in &mut entries {
            if entry.is_directory {
                continue;
            }
            if let Some(extra) = streams_total.get(&entry.file_key.file_index) {
                entry.size_on_disk += *extra;
            }
        }
    }

    // Второй проход: дерево. Корень тома — запись №5, отдельной папки ему не нужно.
    let mut folders: u64 = 0;
    let mut files: u64 = 0;
    let mut size_on_disk: u64 = 0;
    let mut size_logical: u64 = 0;
    let mut dirs: HashMap<u64, FolderId> = HashMap::new();
    dirs.insert(ROOT_RECORD, index.root());

    let mut pending: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_directory && entry.file_key.file_index != ROOT_RECORD)
        .map(|(position, _)| position)
        .collect();

    // Папки создаём волнами: за проход разрешается тот, чей родитель уже известен.
    // Пока волна приносит новые папки — повторяем; оставшиеся неразрешимы
    // (родитель вне таблицы или цикл ссылок).
    loop {
        let unresolved_before = pending.len();
        pending.retain(|&position| {
            let entry = &entries[position];
            let Some(parent) = dirs.get(&entry.parent_ref).copied() else {
                return true;
            };
            let (id, created) = create_folder(index, parent, &entry.name);
            if created {
                folders += 1;
            }
            // У папки с несколькими именами (DOS-алиас) дети крепятся к первому.
            dirs.entry(entry.file_key.file_index).or_insert(id);
            false
        });
        if pending.len() == unresolved_before {
            break;
        }
    }

    for &position in &pending {
        let entry = &entries[position];
        let (id, created) = create_folder(index, index.root(), &entry.name);
        if created {
            folders += 1;
        }
        dirs.entry(entry.file_key.file_index).or_insert(id);
        // Родитель папки так и не нашёлся — она положена в корень.
        diag.unresolved_folder += 1;
    }

    // Файлы: каждое имя — отдельная запись индекса, но байты жёсткой ссылки
    // засчитываются только один раз (это делает index.add_file по file_key).
    for entry in entries {
        if entry.is_directory {
            continue;
        }
        let parent = match dirs.get(&entry.parent_ref).copied() {
            Some(parent) => parent,
            None => {
                // Родителя нет в таблице — файл не теряем, кладём в корень.
                diag.orphan += 1;
                index.root()
            }
        };
        let meta = FileMeta {
            name: entry.name,
            size_on_disk: entry.size_on_disk,
            size_logical: entry.size_logical,
            modified: entry.modified,
            file_key: entry.file_key,
        };
        if matches!(index.add_file(parent, meta), AddOutcome::Counted) {
            size_on_disk += entry.size_on_disk;
            size_logical += entry.size_logical;
        }
        files += 1;
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    progress(&ScanProgress {
        files,
        folders,
        size_on_disk,
        elapsed_ms,
    });

    let skipped = u32::try_from(diag.total()).unwrap_or(u32::MAX);
    Ok((
        ScanTotals {
            files,
            folders,
            size_on_disk,
            size_logical,
            skipped,
            cancelled: cancel.load(Ordering::Relaxed),
            elapsed_ms,
        },
        diag,
    ))
}

/// Просканировать NTFS-том: открыть его и делегировать в [`scan_ntfs_reader`].
pub fn scan_ntfs(
    drive_letter: &str,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanTotals, ScanError> {
    let reader = VolumeReader::open(drive_letter)?;
    scan_ntfs_reader(&reader, index, cancel, progress)
}

/// Создать папку в индексе и сказать, появился ли новый узел.
///
/// `index.folder` возвращает существующую папку с тем же именем у того же
/// родителя, поэтому счётчик папок нельзя увеличивать на каждый вызов: имена
/// нескольких неразрешённых папок могут совпасть, когда все они попадают в корень.
fn create_folder(index: &mut ScanIndex, parent: FolderId, name: &str) -> (FolderId, bool) {
    let root = index.root();
    let before = index.folder_totals(root).folder_count;
    let id = index.folder(parent, name);
    let created = index.folder_totals(root).folder_count != before;
    (id, created)
}

/// Запомнить именованные потоки записи, оставив по каждому имени максимум.
///
/// Нерезидентный атрибут может быть разбит на несколько сегментов в разных
/// записях: сегменты либо повторяют полный размер, либо содержат 0, поэтому
/// складывать их нельзя — берём максимум по (файл, имя потока).
fn collect_streams(
    stream_bytes: &mut HashMap<(u64, String), u64>,
    file_index: u64,
    streams: Vec<(String, u64)>,
) {
    for (name, allocated) in streams {
        let slot = stream_bytes.entry((file_index, name)).or_insert(0);
        if allocated > *slot {
            *slot = allocated;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::test_support::{
        DataSpec, RecordSpec, SAMPLE_FILE_COUNT, SAMPLE_FOLDER_COUNT, SAMPLE_RECORD_COUNT,
        SAMPLE_SIZE_LOGICAL, SAMPLE_SIZE_ON_DISK, SAMPLE_SKIPPED, fixture, make_record, test_dir,
        write_u64,
    };

    #[test]
    fn test_parse_record_reads_unicode_name() {
        let record = make_record(
            RecordSpec::new(42, RECORD_FLAG_IN_USE)
                .name("файл 🚀 тест.txt", ROOT_RECORD)
                .data(DataSpec::NonResident {
                    allocated: 8192,
                    real: 5000,
                })
                .modified_ms(1_700_000_000_000),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1, "у записи одно $FILE_NAME");
        let info = &infos[0];
        assert_eq!(info.name, "файл 🚀 тест.txt");
        assert_eq!(info.parent_ref, ROOT_RECORD);
        assert_eq!(info.size_on_disk, 8192);
        assert_eq!(info.size_logical, 5000);
        assert_eq!(info.modified, 1_700_000_000_000);
        assert_eq!(info.file_key.file_index, 42);
        assert!(!info.is_directory);
    }

    #[test]
    fn test_parse_record_falls_back_to_file_name_modification_time() {
        // $STANDARD_INFORMATION нет — время берётся из $FILE_NAME. Там 0x08 —
        // это создание, а 0x10 — изменение; ждём именно изменение.
        const CREATED_MS: i64 = 1_600_000_000_000;
        const MODIFIED_MS: i64 = 1_650_000_000_000;

        let record = make_record(
            RecordSpec::new(11, RECORD_FLAG_IN_USE)
                .name("без-si.txt", ROOT_RECORD)
                .name_times(CREATED_MS, MODIFIED_MS)
                .data(DataSpec::Resident(10)),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].modified, MODIFIED_MS,
            "из $FILE_NAME берётся время изменения (0x10), а не создания (0x08)"
        );
    }

    #[test]
    fn test_parse_record_returns_all_hard_link_names() {
        let record = make_record(
            RecordSpec::new(77, RECORD_FLAG_IN_USE)
                .name("первая-ссылка.txt", ROOT_RECORD)
                .name("вторая-ссылка.txt", 6)
                .data(DataSpec::NonResident {
                    allocated: 4096,
                    real: 1000,
                })
                .modified_ms(1_600_000_000_000),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 2, "по элементу на каждое $FILE_NAME");
        assert_eq!(infos[0].name, "первая-ссылка.txt");
        assert_eq!(infos[1].name, "вторая-ссылка.txt");
        assert_eq!(infos[0].parent_ref, ROOT_RECORD);
        assert_eq!(infos[1].parent_ref, 6);
        // Жёсткие ссылки — это один файл: ключ и размеры общие.
        assert_eq!(infos[0].file_key, infos[1].file_key);
        assert_eq!(infos[0].size_on_disk, infos[1].size_on_disk);
        assert_eq!(infos[0].file_key.file_index, 77);
    }

    #[test]
    fn test_parse_record_reads_directory_flags_and_zero_sizes() {
        let record = make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name("папка", ROOT_RECORD)
                .modified_ms(1_500_000_000_000),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1);
        assert!(infos[0].is_directory, "бит 1 флагов записи — это папка");
        assert_eq!(infos[0].size_on_disk, 0, "у папки нет собственных данных");
        assert_eq!(infos[0].size_logical, 0);
    }

    #[test]
    fn test_parse_record_reads_resident_data_size() {
        let record = make_record(
            RecordSpec::new(8, RECORD_FLAG_IN_USE)
                .name("маленький.txt", ROOT_RECORD)
                .data(DataSpec::Resident(37)),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].size_logical, 37, "резидентные данные — их длина");
        assert_eq!(
            infos[0].size_on_disk, 0,
            "резидентные данные не занимают отдельных кластеров"
        );
    }

    #[test]
    fn test_parse_record_puts_long_name_before_dos_alias() {
        // У папки два имени: короткий DOS-алиас «8.3» и настоящее длинное имя.
        // Первым должен идти Win32-вариант — к нему сканер крепит детей.
        let record = make_record(
            RecordSpec::new(1101, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name_ns("PROGRA~1", ROOT_RECORD, 2)
                .name_ns("Program Files", ROOT_RECORD, 1),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 2, "оба имени остаются в выдаче");
        assert_eq!(infos[0].name, "Program Files");
        assert_eq!(infos[1].name, "PROGRA~1");
    }

    #[test]
    fn test_parse_record_counts_named_stream_space() {
        // Альтернативный поток занимает кластеры, хотя логический размер файла
        // определяется неименованным $DATA.
        let record = make_record(
            RecordSpec::new(9, RECORD_FLAG_IN_USE)
                .name("с-потоком.txt", ROOT_RECORD)
                .data(DataSpec::Resident(10))
                .stream("Zone.Identifier", 4096, 3000),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].size_logical, 10, "логический размер — из $DATA");
        assert_eq!(
            infos[0].size_on_disk, 4096,
            "место под именованным потоком входит в занятое"
        );
    }

    #[test]
    fn test_parse_record_ignores_bad_cluster_stream() {
        // $BadClus:$Bad — разреженный поток на весь том: его allocated равен
        // размеру диска и не должен попадать в занятое место.
        let record = make_record(
            RecordSpec::new(8, RECORD_FLAG_IN_USE)
                .name("$BadClus", ROOT_RECORD)
                .stream("$Bad", 1u64 << 40, 1u64 << 40),
        );

        let infos = parse_record(&record);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].size_on_disk, 0, "поток $Bad не занимает места");
    }

    #[test]
    fn test_parse_record_ignores_deleted_record() {
        // Флаг «в использовании» снят — запись удалена и в индекс не попадает.
        let record = make_record(
            RecordSpec::new(9, 0)
                .name("удалённый.txt", ROOT_RECORD)
                .data(DataSpec::Resident(10)),
        );
        assert!(parse_record(&record).is_empty());
    }

    #[test]
    fn test_parse_record_ignores_extension_record() {
        // Базовая запись у файла другая: это обломок атрибутов, а не файл.
        let mut record = make_record(
            RecordSpec::new(10, RECORD_FLAG_IN_USE).name("обломок.txt", ROOT_RECORD),
        );
        write_u64(&mut record, 0x20, 1234);
        assert!(parse_record(&record).is_empty());
    }

    #[test]
    fn parse_record_skips_corrupt_record_without_panic() {
        let reader = VolumeReader::from_fixture(&fixture("corrupt.mft"), 1024)
            .expect("фикстура corrupt.mft должна открываться");

        let mut last: Vec<u8> = Vec::new();
        let count = reader
            .read_records(&mut |record| last = record.to_vec())
            .expect("чтение фикстуры не должно падать");
        assert_eq!(count, SAMPLE_RECORD_COUNT);

        assert!(
            parse_record(&last).is_empty(),
            "испорченная запись (подпись BAD!) не должна давать имена"
        );

        // Остальные записи парсятся без паники и дают имена.
        let mut records = 0u64;
        let mut parsed = 0usize;
        VolumeReader::from_fixture(&fixture("corrupt.mft"), 1024)
            .expect("фикстура corrupt.mft должна открываться")
            .read_records(&mut |record| {
                records += 1;
                if records < count {
                    parsed += parse_record(record).len();
                }
            })
            .expect("чтение фикстуры не должно падать");
        assert_eq!(records, SAMPLE_RECORD_COUNT);
        assert!(parsed > 0, "испорчена только последняя запись");
    }

    #[test]
    fn test_scan_ntfs_builds_index_with_valid_parents() {
        let reader = VolumeReader::from_fixture(&fixture("sample.mft"), 1024)
            .expect("фикстура sample.mft должна открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let mut reports: Vec<ScanProgress> = Vec::new();

        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |progress| {
            reports.push(progress.clone())
        })
        .expect("сканирование фикстуры не должно падать");

        assert!(!reports.is_empty(), "прогресс должен сообщать итоги");
        // Точные итоги фиксирует генератор `test_support`: любая ошибка в
        // разборе имён, размеров, жёстких ссылок или ADS сразу видна.
        assert_eq!(totals.files, SAMPLE_FILE_COUNT, "файлы фикстуры");
        assert_eq!(totals.folders, SAMPLE_FOLDER_COUNT, "папки фикстуры");
        assert_eq!(totals.size_on_disk, SAMPLE_SIZE_ON_DISK, "занятое место");
        assert_eq!(totals.size_logical, SAMPLE_SIZE_LOGICAL, "логический размер");
        assert_eq!(totals.skipped, SAMPLE_SKIPPED, "пропуски по всем причинам");

        // Все разобранные имена дошли до индекса: итоги сканера совпадают с
        // итогами корня индекса, то есть у каждой записи нашёлся родитель-папка
        // (иначе она легла бы в корень с пометкой в skipped).
        let root_totals = index.folder_totals(index.root());
        assert_eq!(totals.files, root_totals.file_count as u64);
        assert_eq!(totals.folders, root_totals.folder_count as u64);
        assert_eq!(totals.size_on_disk, root_totals.size_on_disk);
        assert_eq!(totals.size_logical, root_totals.size_logical);

        // Папки-дети корня из фикстуры видны в индексе с корректными именами.
        let children = index.child_folders(index.root());
        let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
        assert!(names.contains(&"Program Files"), "дети корня: {names:?}");
        assert!(names.contains(&"$Recycle.Bin"), "дети корня: {names:?}");
        // Дети папки крепятся к длинному имени, а не к DOS-алиасу «PROGRA~1».
        let program_files = children
            .iter()
            .find(|child| child.name == "Program Files")
            .expect("папка Program Files должна быть в индексе");
        assert!(
            program_files.file_count > 0 || program_files.folder_count > 0,
            "дети Program Files должны быть под длинным именем: {program_files:?}"
        );
        for child in &children {
            assert!(
                child.percent_of_parent <= 1.0,
                "доля ребёнка не может превышать родителя: {:?}",
                child
            );
        }

        // В фикстуре есть родитель вне таблицы (сирота), запись с
        // $ATTRIBUTE_LIST и extension-запись с именем — это и есть пропуски.
        assert_eq!(
            totals.skipped, SAMPLE_SKIPPED,
            "неразрешимые родители и неполные записи честно попадают в skipped"
        );
    }

    #[test]
    fn test_scan_ntfs_counts_hard_links_once() {
        let dir = test_dir();
        let fixture_path = dir.path().join("hard-links.mft");

        // Слепок: записи 0..4 пусты, запись 5 — корень, 6 — папка data,
        // 7 — файл с двумя именами внутри неё.
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name("data", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(7, RECORD_FLAG_IN_USE)
                .name("first.bin", 6)
                .name("second.bin", 6)
                .data(DataSpec::NonResident {
                    allocated: 4096,
                    real: 1000,
                })
                .modified_ms(1_700_000_000_000),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок с жёсткими ссылками должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        let children = index.child_folders(index.root());
        let data_id: FolderId = children
            .iter()
            .find(|child| child.name == "data")
            .expect("папка data должна быть в индексе")
            .id;
        let data_totals = index.folder_totals(data_id);

        // Оба имени видны, но байты файла засчитаны ровно один раз.
        assert_eq!(data_totals.file_count, 2, "обе жёсткие ссылки видны");
        assert_eq!(data_totals.size_on_disk, 4096, "размер учтён один раз");
        assert_eq!(data_totals.size_logical, 1000);
        assert_eq!(totals.size_on_disk, 4096);
        assert_eq!(totals.files, 2);
        assert_eq!(totals.skipped, 0, "родители разрешимы");
    }

    #[test]
    fn test_scan_ntfs_counts_named_stream_space_once() {
        // Альтернативный поток занимает кластеры: занятое место больше
        // логического размера, который задаёт неименованный резидентный $DATA.
        // Сканер складывает ADS отдельно от разбора записи — проверяем и его.
        let dir = test_dir();
        let fixture_path = dir.path().join("named-stream.mft");
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE)
                .name("с-потоком.txt", ROOT_RECORD)
                .data(DataSpec::Resident(10))
                .stream("Zone.Identifier", 4096, 3000)
                .modified_ms(1_700_000_000_000),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок с именованным потоком должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        let root_totals = index.folder_totals(index.root());
        assert_eq!(totals.files, 1);
        assert_eq!(
            root_totals.size_on_disk, 4096,
            "место под именованным потоком входит в занятое"
        );
        assert_eq!(
            root_totals.size_logical, 10,
            "логический размер задаёт неименованный $DATA"
        );
        assert_eq!(totals.skipped, 0, "родитель разрешим");
    }

    #[test]
    fn test_scan_ntfs_skips_unresolved_parents_to_root() {
        let dir = test_dir();
        let fixture_path = dir.path().join("orphan.mft");
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        // Родитель 4242 в слепке отсутствует — файл обязан попасть в корень.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE)
                .name("сирота.bin", 4242)
                .data(DataSpec::Resident(15)),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок с сиротой должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        assert_eq!(totals.files, 1);
        assert_eq!(totals.skipped, 1, "неразрешимый родитель — это пропуск");
        let root_totals = index.folder_totals(index.root());
        assert_eq!(root_totals.file_count, 1);
        assert_eq!(root_totals.size_logical, 15);
    }

    #[test]
    fn test_scan_counts_skip_causes_separately() {
        // В одном слепке — все пять причин пропуска: битая запись, базовая
        // запись с $ATTRIBUTE_LIST, extension-запись с именем, имя-сирота и
        // неразрешённая папка. Проверяем, что счётчики не смешаны, а
        // ScanTotals::skipped — их сумма.
        let dir = test_dir();
        let fixture_path = dir.path().join("skip-causes.mft");
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        // 6: имя-сирота (родитель 4242 в слепке отсутствует).
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE)
                .name("сирота.bin", 4242)
                .data(DataSpec::Resident(15)),
        ));
        // 7: битая запись — чужая подпись вместо FILE.
        let mut broken = vec![0u8; 1024];
        broken[0..4].copy_from_slice(b"BAD!");
        bytes.extend_from_slice(&broken);
        // 8: неполная базовая запись с $ATTRIBUTE_LIST.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(8, RECORD_FLAG_IN_USE)
                .name("неполный.bin", ROOT_RECORD)
                .data(DataSpec::Resident(10))
                .with_attribute_list(),
        ));
        // 9: extension-запись с именем — имя потеряно.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(9, RECORD_FLAG_IN_USE)
                .base_record(6)
                .name("потерянное-имя.bin", ROOT_RECORD),
        ));
        // 10: папка с несуществующим родителем.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(10, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY).name("висячая", 4242),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок с пропусками должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let (totals, diag) = scan_ntfs_reader_diag(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        assert_eq!(diag.broken, 1, "битая запись");
        assert_eq!(diag.partial, 1, "базовая запись с $ATTRIBUTE_LIST");
        assert_eq!(diag.extension_with_names, 1, "имя из extension-записи");
        assert_eq!(diag.orphan, 1, "имя-сирота");
        assert_eq!(diag.unresolved_folder, 1, "неразрешённая папка");
        assert_eq!(diag.total(), 5, "сумма причин");
        assert_eq!(
            totals.skipped,
            diag.total() as u32,
            "skipped — сумма причин, а не отдельная величина"
        );
        assert_eq!(totals.files, 2, "сирота и неполный файл");
        assert_eq!(totals.folders, 1, "неразрешённая папка легла в корень");
    }

    #[test]
    fn test_scan_ntfs_takes_sizes_from_extension_record() {
        // У крупного файла неименованный $DATA вынесен в extension-запись:
        // базовая запись без $DATA не должна выглядеть пустой.
        let dir = test_dir();
        let fixture_path = dir.path().join("extension.mft");
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        // Базовая запись: имя есть, $DATA нет (он в extension-записи).
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE)
                .name("большой.bin", ROOT_RECORD)
                .with_attribute_list()
                .modified_ms(1_700_000_000_000),
        ));
        // Extension-запись: без имён, с нерезидентным $DATA файла 6.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(7, RECORD_FLAG_IN_USE)
                .base_record(6)
                .data(DataSpec::NonResident {
                    allocated: 8192,
                    real: 5000,
                }),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок с extension-записью должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        let root_totals = index.folder_totals(index.root());
        assert_eq!(totals.files, 1);
        assert_eq!(root_totals.file_count, 1);
        assert_eq!(
            root_totals.size_on_disk, 8192,
            "размеры берутся из $DATA extension-записи"
        );
        assert_eq!(root_totals.size_logical, 5000);
        // Базовая запись с $ATTRIBUTE_LIST — неполный разбор, это пропуск.
        assert_eq!(totals.skipped, 1, "запись с $ATTRIBUTE_LIST попадает в skipped");
    }

    #[test]
    fn test_scan_ntfs_counts_extension_record_names_as_skipped() {
        // Имена из extension-записей теряются: $ATTRIBUTE_LIST не разбирается.
        let dir = test_dir();
        let fixture_path = dir.path().join("extension-names.mft");
        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..ROOT_RECORD {
            bytes.extend_from_slice(&[0u8; 1024]);
        }
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(5, RECORD_FLAG_IN_USE | RECORD_FLAG_DIRECTORY)
                .name(".", ROOT_RECORD)
                .modified_ms(1_700_000_000_000),
        ));
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(6, RECORD_FLAG_IN_USE)
                .name("видимый.bin", ROOT_RECORD)
                .data(DataSpec::Resident(10))
                .modified_ms(1_700_000_000_000),
        ));
        // Extension-запись с именем: это имя файла мы не увидим.
        bytes.extend_from_slice(&make_record(
            RecordSpec::new(7, RECORD_FLAG_IN_USE)
                .base_record(6)
                .name("потерянное-имя.bin", ROOT_RECORD),
        ));
        std::fs::write(&fixture_path, &bytes).expect("не удалось записать слепок");

        let reader = VolumeReader::from_fixture(&fixture_path, 1024)
            .expect("слепок должен открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {})
            .expect("сканирование слепка не должно падать");

        assert_eq!(totals.files, 1, "видно только имя базовой записи");
        assert_eq!(
            totals.skipped, 1,
            "потерянное имя extension-записи обязано попасть в skipped"
        );
    }

    #[test]
    fn test_scan_ntfs_stops_on_cancel() {
        let reader = VolumeReader::from_fixture(&fixture("sample.mft"), 1024)
            .expect("фикстура sample.mft должна открываться");
        let mut index = ScanIndex::new("C:");
        let cancel = AtomicBool::new(false);
        let totals = scan_ntfs_reader(&reader, &mut index, &cancel, &mut |_| {
            cancel.store(true, Ordering::Relaxed);
        })
        .expect("отмена — не ошибка");

        // После отмены разбор прекращается: в индекс попадает меньше имён,
        // чем есть в фикстуре, но функция возвращает Ok.
        assert!(
            totals.files < SAMPLE_FILE_COUNT,
            "после отмены должно быть обработано меньше имён: {totals:?}"
        );
    }
}
