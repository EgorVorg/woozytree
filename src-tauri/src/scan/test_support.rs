//! Общие помощники тестов сканера: синтетические фикстуры и временные папки.
//!
//! Реальные слепки `$MFT` с диска в репозиторий не попадают: в них настоящие
//! имена файлов пользователя. Поэтому тесты сами собирают учебный мини-MFT:
//! корень — запись №5, пара папок, файлы, запись с двумя именами для жёстких
//! ссылок и юникод-имена. Генерация детерминированная: одни и те же байты при
//! каждом запуске, поэтому тесты не зависят от состояния каталога `tests/fixtures`.
//!
//! Каталог `src-tauri/tests/fixtures/` в `.gitignore`; [`fixture`] пересоздаёт
//! файлы при первом обращении в процессе, так что свежий клон проходит
//! `cargo test --lib` без подготовки.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Размер синтетической записи MFT: 2 сектора по 512 байт, как у обычного тома.
const RECORD_LEN: usize = 1024;

/// Корень тома — запись `$MFT` №5 (закреплено форматом NTFS).
const ROOT_RECORD: u64 = 5;

/// Сколько записей в `sample.mft`. Хвост заполнен пустыми слотами: так у
/// сканера хватает записей на промежуточный отчёт прогресса, а тест отмены
/// действительно прерывает разбор на середине таблицы.
pub const SAMPLE_RECORD_COUNT: u64 = 2000;

/// Итоги сканирования `sample.mft`: их проверяют тесты сканера.
///
/// 9 имён: exe, readme, две у жёсткой ссылки, юникод-файл, ADS-файл,
/// сирота, `большой.bin` и `last.bin`.
pub const SAMPLE_FILE_COUNT: u64 = 9;
/// Папок шесть: `Program Files` и её DOS-алиас `PROGRA~1` сканер создаёт
/// отдельными узлами (дети при этом крепятся к длинному имени).
pub const SAMPLE_FOLDER_COUNT: u64 = 6;
/// Сумма нерезидентных `$DATA`: 8192 (exe) + 4096 (жёсткая ссылка) + 8192
/// (юникод) + 4096 (ADS) + 8192 (`большой.bin` из extension-записи) + 4096
/// (`last.bin`).
pub const SAMPLE_SIZE_ON_DISK: u64 = 36_864;
/// Сумма логических размеров: 5000 (exe) + 37 (readme) + 1000 (жёсткая
/// ссылка) + 5000 (юникод) + 10 (ADS-файл) + 15 (сирота) + 5000
/// (`большой.bin`) + 1000 (`last.bin`).
pub const SAMPLE_SIZE_LOGICAL: u64 = 17_062;
/// 3 события: имя-сирота, `$ATTRIBUTE_LIST` у `большого.bin` и имя из
/// extension-записи.
pub const SAMPLE_SKIPPED: u32 = 3;

/// Константы синтетического загрузочного сектора NTFS: 512-байтный сектор,
/// 8 секторов в кластере, том 1 ГиБ (2 097 152 сектора), `$MFT` с LCN 4096,
/// запись 1024 байта и произвольный серийный номер.
pub const BOOT_BYTES_PER_SECTOR: u16 = 512;
pub const BOOT_SECTORS_PER_CLUSTER: u8 = 8;
pub const BOOT_TOTAL_SECTORS: u64 = 2_097_152;
pub const BOOT_MFT_LCN: u64 = 4096;
pub const BOOT_RECORD_SIZE: usize = 1024;
pub const BOOT_VOLUME_SERIAL: u64 = 0x5A5A_1234_DEAD_BEEF;

/// Фиксированное время изменения файлов фикстуры: Unix-мс, чтобы байты не
/// зависели от часов машины.
const SAMPLE_MODIFIED_MS: i64 = 1_700_000_000_000;

// Номера и размеры заголовков атрибутов NTFS: зеркалят приватные константы
// `scan::mft`, чтобы не расширять их видимость ради тестового генератора.
const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;
const ATTR_HEADER_LEN: usize = 0x18;
const ATTR_NONRESIDENT_HEADER_LEN: usize = 0x40;

const IN_USE: u16 = 0x0001;
const DIRECTORY: u16 = 0x0002;

/// FILETIME считается в 100-нс интервалах от 1601-01-01, Unix-эпоха — на
/// 11 644 473 600 000 мс позже.
const FILETIME_UNIX_OFFSET_MS: i64 = 11_644_473_600_000;

/// Каталог сгенерированных фикстур.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Путь к синтетической фикстуре; при первом обращении файлы создаются заново.
///
/// `name` — одно из имён, которые пишет [`write_fixtures`]: `sample.mft`,
/// `corrupt.mft`, `boot.bin`.
pub fn fixture(name: &str) -> PathBuf {
    static WRITTEN: OnceLock<()> = OnceLock::new();
    WRITTEN.get_or_init(write_fixtures);
    fixtures_dir().join(name)
}

fn write_fixtures() {
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|error| {
        panic!(
            "не удалось создать каталог фикстур {}: {error}",
            dir.display()
        )
    });
    write_file(&dir.join("sample.mft"), &sample_mft_bytes());
    write_file(&dir.join("corrupt.mft"), &corrupt_mft_bytes());
    write_file(&dir.join("boot.bin"), &boot_sector_bytes());
}

fn write_file(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|error| {
        panic!(
            "не удалось записать синтетическую фикстуру {}: {error}",
            path.display()
        )
    });
}

/// Синтетический мини-MFT: системная запись №0, корень №5, папки, файлы,
/// жёсткая ссылка, юникод и все виды «пропусков» из [`SAMPLE_SKIPPED`].
fn sample_records() -> Vec<Vec<u8>> {
    let mut records: Vec<Vec<u8>> = (0..SAMPLE_RECORD_COUNT)
        .map(|_| vec![0u8; RECORD_LEN])
        .collect();

    // №0 — системная запись `$MFT`: подпись FILE есть, имени нет, поэтому
    // сканер считает её служебной и пропускает. Так у слепка, как и у живого
    // тома, первая запись начинается с FILE.
    records[0] = make_record(RecordSpec::new(0, IN_USE));

    // №1..4 — пустые слоты, как у настоящей таблицы после системных записей.
    records[5] = directory(5, ".", ROOT_RECORD);
    records[6] = make_record(
        RecordSpec::new(6, IN_USE | DIRECTORY)
            // DOS-алиас идёт первым в записи, но в индексе должно оказаться
            // длинное имя — его и проверяет тест.
            .name_ns("PROGRA~1", ROOT_RECORD, 2)
            .name_ns("Program Files", ROOT_RECORD, 1)
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    records[7] = directory(7, "Users", ROOT_RECORD);
    records[8] = directory(8, "документы", ROOT_RECORD);
    records[9] = directory(9, "$Recycle.Bin", ROOT_RECORD);
    records[10] = directory(10, "WoozyTree", 6);
    records[11] = make_record(
        RecordSpec::new(11, IN_USE)
            .name("woozytree.exe", 10)
            .data(DataSpec::NonResident {
                allocated: 8192,
                real: 5000,
            })
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    records[12] = make_record(
        RecordSpec::new(12, IN_USE)
            .name("readme.txt", 10)
            .data(DataSpec::Resident(37))
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    // №13 — файл с двумя именами (жёсткая ссылка): размеры должны считаться
    // один раз, а имена — оба.
    records[13] = make_record(
        RecordSpec::new(13, IN_USE)
            .name("первая-ссылка.txt", 7)
            .name("second-link.txt", 7)
            .data(DataSpec::NonResident {
                allocated: 4096,
                real: 1000,
            })
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    records[14] = make_record(
        RecordSpec::new(14, IN_USE)
            .name("файл 🚀 тест.txt", 8)
            .data(DataSpec::NonResident {
                allocated: 8192,
                real: 5000,
            })
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    // №15 — файл с именованным потоком (ADS).
    records[15] = make_record(
        RecordSpec::new(15, IN_USE)
            .name("с-потоком.txt", 8)
            .data(DataSpec::Resident(10))
            .stream("Zone.Identifier", 4096, 3000)
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    // №16 — имя-сирота: родителя 4242 в таблице нет.
    records[16] = make_record(
        RecordSpec::new(16, IN_USE)
            .name("сирота.bin", 4242)
            .data(DataSpec::Resident(15)),
    );
    // №17 — базовая запись с $ATTRIBUTE_LIST, $DATA вынесен в №18.
    records[17] = make_record(
        RecordSpec::new(17, IN_USE)
            .name("большой.bin", ROOT_RECORD)
            .with_attribute_list()
            .modified_ms(SAMPLE_MODIFIED_MS),
    );
    records[18] = make_record(
        RecordSpec::new(18, IN_USE)
            .base_record(17)
            .data(DataSpec::NonResident {
                allocated: 8192,
                real: 5000,
            }),
    );
    // №19 — extension-запись с именем: имя теряется (нет разбора $ATTRIBUTE_LIST).
    records[19] = make_record(
        RecordSpec::new(19, IN_USE)
            .base_record(11)
            .name("потерянное-имя.bin", ROOT_RECORD),
    );
    // №20 — удалённая запись: флаг «в использовании» снят.
    records[20] = make_record(
        RecordSpec::new(20, 0)
            .name("удалённый.txt", ROOT_RECORD)
            .data(DataSpec::Resident(10)),
    );

    // Последняя запись — обычный файл: `corrupt.mft` ломает именно её, поэтому
    // «тот же набор, но с испорченным хвостом» действительно проверяется.
    records[SAMPLE_RECORD_COUNT as usize - 1] = make_record(
        RecordSpec::new(1999, IN_USE)
            .name("last.bin", ROOT_RECORD)
            .data(DataSpec::NonResident {
                allocated: 4096,
                real: 1000,
            })
            .modified_ms(SAMPLE_MODIFIED_MS),
    );

    records
}

fn directory(number: u32, name: &str, parent: u64) -> Vec<u8> {
    make_record(
        RecordSpec::new(number, IN_USE | DIRECTORY)
            .name(name, parent)
            .modified_ms(SAMPLE_MODIFIED_MS),
    )
}

fn sample_mft_bytes() -> Vec<u8> {
    flatten(&sample_records())
}

fn corrupt_mft_bytes() -> Vec<u8> {
    let mut bytes = sample_mft_bytes();
    let start = bytes.len() - RECORD_LEN;
    let last = &mut bytes[start..];
    // Детерминированный мусор вместо записи: парсер должен увидеть чужую
    // подпись и не пытаться разбирать атрибуты.
    for (index, byte) in last.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    last[..4].copy_from_slice(b"BAD!");
    last[0x14..0x16].copy_from_slice(&0xFFFFu16.to_le_bytes());
    bytes
}

fn flatten(records: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * RECORD_LEN);
    for record in records {
        bytes.extend_from_slice(record);
    }
    bytes
}

/// Синтетический загрузочный сектор NTFS с известными константами.
fn boot_sector_bytes() -> Vec<u8> {
    let mut boot = vec![0u8; 512];
    boot[3..11].copy_from_slice(b"NTFS    ");
    boot[0x0B..0x0D].copy_from_slice(&BOOT_BYTES_PER_SECTOR.to_le_bytes());
    boot[0x0D] = BOOT_SECTORS_PER_CLUSTER;
    boot[0x28..0x30].copy_from_slice(&BOOT_TOTAL_SECTORS.to_le_bytes());
    boot[0x30..0x38].copy_from_slice(&BOOT_MFT_LCN.to_le_bytes());
    // Отрицательное значение — логарифм по основанию 2: -10 = 1024 байта.
    boot[0x40] = 0xF6;
    boot[0x48..0x50].copy_from_slice(&BOOT_VOLUME_SERIAL.to_le_bytes());
    boot
}

/// Временная папка для тестовых файлов-слепков.
///
/// Обычно это системный `%TEMP%`, но в ограниченной среде запись туда может быть
/// запрещена — тогда берём папку внутри `target`, который и так принадлежит сборке.
pub fn test_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|_| {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        std::fs::create_dir_all(&base).expect("не удалось создать папку для тестовых tempdir");
        tempfile::tempdir_in(base).expect("не удалось создать временную папку теста")
    })
}

/// Значение `$FILE_NAME` для синтетической записи.
struct NameSpec<'a> {
    name: &'a str,
    /// Номер записи-родителя; в ссылку попадает с номером последовательности.
    parent_ref: u64,
    /// Пространство имён: 3 — Win32 и DOS (обычный случай), 2 — DOS-алиас.
    namespace: u8,
    /// Время создания в `$FILE_NAME` (смещение 0x08), Unix-мс.
    /// `None` — взять `RecordSpec::modified_ms` (как раньше).
    creation_ms: Option<i64>,
    /// Время изменения в `$FILE_NAME` (смещение 0x10), Unix-мс.
    /// `None` — взять `RecordSpec::modified_ms`.
    modification_ms: Option<i64>,
}

/// Какие данные записать в `$DATA` синтетической записи.
pub enum DataSpec {
    /// Атрибута `$DATA` нет (папка или файл без данных).
    None,
    /// Резидентные данные заданной длины — отдельных кластеров не занимают.
    Resident(u64),
    /// Нерезидентные данные: занято на диске и реальный размер.
    NonResident { allocated: u64, real: u64 },
}

/// Именованный поток `$DATA` (альтернативные данные, ADS).
struct StreamSpec<'a> {
    name: &'a str,
    allocated: u64,
    real: u64,
}

/// Описание синтетической записи MFT для [`make_record`].
pub struct RecordSpec<'a> {
    record_number: u32,
    flags: u16,
    names: Vec<NameSpec<'a>>,
    data: DataSpec,
    /// Именованные потоки `$DATA` (ADS), нерезидентные.
    streams: Vec<StreamSpec<'a>>,
    /// Время изменения в `$STANDARD_INFORMATION`, Unix-мс.
    modified_ms: Option<i64>,
    /// Ссылка на базовую запись (не 0 — запись является extension-записью).
    base_record: u64,
    /// Добавить `$ATTRIBUTE_LIST` (атрибуты могут лежать в других записях).
    attribute_list: bool,
}

impl<'a> RecordSpec<'a> {
    pub fn new(record_number: u32, flags: u16) -> RecordSpec<'a> {
        RecordSpec {
            record_number,
            flags,
            names: Vec::new(),
            data: DataSpec::None,
            streams: Vec::new(),
            modified_ms: None,
            base_record: 0,
            attribute_list: false,
        }
    }

    pub fn name(mut self, name: &'a str, parent_ref: u64) -> RecordSpec<'a> {
        self.names.push(NameSpec {
            name,
            parent_ref,
            namespace: 3,
            creation_ms: None,
            modification_ms: None,
        });
        self
    }

    /// Имя с явным пространством имён (2 — короткий DOS-алиас «8.3»).
    pub fn name_ns(mut self, name: &'a str, parent_ref: u64, namespace: u8) -> RecordSpec<'a> {
        self.names.push(NameSpec {
            name,
            parent_ref,
            namespace,
            creation_ms: None,
            modification_ms: None,
        });
        self
    }

    /// Задать последнему добавленному `$FILE_NAME` отдельные времена создания
    /// и изменения. В `$FILE_NAME` создание лежит по 0x08, изменение — по
    /// 0x10; разные значения позволяют поймать чтение не того поля.
    pub fn name_times(mut self, creation_ms: i64, modification_ms: i64) -> RecordSpec<'a> {
        let last = self
            .names
            .last_mut()
            .expect("name_times вызывается после name");
        last.creation_ms = Some(creation_ms);
        last.modification_ms = Some(modification_ms);
        self
    }

    pub fn data(mut self, data: DataSpec) -> RecordSpec<'a> {
        self.data = data;
        self
    }

    /// Добавить именованный поток `$DATA` (альтернативные данные).
    pub fn stream(mut self, name: &'a str, allocated: u64, real: u64) -> RecordSpec<'a> {
        self.streams.push(StreamSpec {
            name,
            allocated,
            real,
        });
        self
    }

    pub fn modified_ms(mut self, ms: i64) -> RecordSpec<'a> {
        self.modified_ms = Some(ms);
        self
    }

    /// Сделать запись extension-записью указанной базовой записи.
    pub fn base_record(mut self, base: u64) -> RecordSpec<'a> {
        self.base_record = base;
        self
    }

    /// Добавить `$ATTRIBUTE_LIST`.
    pub fn with_attribute_list(mut self) -> RecordSpec<'a> {
        self.attribute_list = true;
        self
    }
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}

fn write_u16(record: &mut [u8], off: usize, value: u16) {
    record[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(record: &mut [u8], off: usize, value: u32) {
    record[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn write_u64(record: &mut [u8], off: usize, value: u64) {
    record[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

/// Unix-мс → FILETIME (100-нс интервалы от 1601-01-01).
fn unix_ms_to_filetime(ms: i64) -> u64 {
    ((ms + FILETIME_UNIX_OFFSET_MS) as u64) * 10_000
}

/// Собрать синтетическую запись MFT на 1024 байта (2 сектора по 512).
///
/// Повторяет то, что пишет NTFS: заголовок, массив USA и fixup в хвостах
/// секторов, затем резидентные `$STANDARD_INFORMATION`, `$FILE_NAME` и
/// `$DATA` (резидентный или нерезидентный).
pub fn make_record(spec: RecordSpec<'_>) -> Vec<u8> {
    const USA_OFFSET: usize = 0x30;
    const USA_COUNT: usize = 3; // номер последовательности + 2 сектора
    const FIRST_ATTR: usize = 0x38;

    let mut record = vec![0u8; RECORD_LEN];
    record[0..4].copy_from_slice(b"FILE");
    write_u16(&mut record, 0x04, USA_OFFSET as u16);
    write_u16(&mut record, 0x06, USA_COUNT as u16);
    write_u16(&mut record, 0x10, 1); // номер последовательности записи
    write_u16(&mut record, 0x12, spec.names.len().max(1) as u16);
    write_u16(&mut record, 0x14, FIRST_ATTR as u16);
    write_u16(&mut record, 0x16, spec.flags);
    write_u64(&mut record, 0x20, spec.base_record);
    write_u32(&mut record, 0x2C, spec.record_number);

    let mut off = FIRST_ATTR;

    // $STANDARD_INFORMATION: 4 времени по 8 байт, далее флаги и версии.
    if let Some(modified_ms) = spec.modified_ms {
        let time = unix_ms_to_filetime(modified_ms);
        let value_len = 0x48;
        write_u32(&mut record, off, ATTR_STANDARD_INFORMATION);
        write_u32(&mut record, off + 0x04, align8(ATTR_HEADER_LEN + value_len) as u32);
        record[off + 0x08] = 0; // резидентный
        record[off + 0x0A..off + 0x0C].copy_from_slice(&(ATTR_HEADER_LEN as u16).to_le_bytes());
        write_u16(&mut record, off + 0x0E, 1);
        write_u32(&mut record, off + 0x10, value_len as u32);
        write_u16(&mut record, off + 0x14, ATTR_HEADER_LEN as u16);
        let value = off + ATTR_HEADER_LEN;
        write_u64(&mut record, value, time); // created
        write_u64(&mut record, value + 0x08, time); // modified
        write_u64(&mut record, value + 0x10, time); // MFT changed
        write_u64(&mut record, value + 0x18, time); // accessed
        off += align8(ATTR_HEADER_LEN + value_len);
    }

    if spec.attribute_list {
        let value_len = 0x20;
        write_u32(&mut record, off, ATTR_ATTRIBUTE_LIST);
        write_u32(&mut record, off + 0x04, align8(ATTR_HEADER_LEN + value_len) as u32);
        record[off + 0x08] = 0;
        write_u16(&mut record, off + 0x0E, 2);
        write_u32(&mut record, off + 0x10, value_len as u32);
        write_u16(&mut record, off + 0x14, ATTR_HEADER_LEN as u16);
        off += align8(ATTR_HEADER_LEN + value_len);
    }

    // $FILE_NAME: 0x42 байта фиксированных полей + имя в UTF-16LE.
    for name_spec in &spec.names {
        let units: Vec<u16> = name_spec.name.encode_utf16().collect();
        let value_len = 0x42 + units.len() * 2;
        write_u32(&mut record, off, ATTR_FILE_NAME);
        write_u32(&mut record, off + 0x04, align8(ATTR_HEADER_LEN + value_len) as u32);
        record[off + 0x08] = 0; // резидентный
        write_u16(&mut record, off + 0x0E, 3);
        write_u32(&mut record, off + 0x10, value_len as u32);
        write_u16(&mut record, off + 0x14, ATTR_HEADER_LEN as u16);
        let value = off + ATTR_HEADER_LEN;
        // Старшие 16 бит ссылки — номер последовательности записи родителя.
        write_u64(&mut record, value, name_spec.parent_ref | (1u64 << 48));
        // $FILE_NAME: 0x08 — время создания, 0x10 — время изменения. По
        // умолчанию оба равны `RecordSpec::modified_ms`, но тест может
        // задать их разными, чтобы поймать чтение не того поля.
        if let Some(creation_ms) = name_spec.creation_ms.or(spec.modified_ms) {
            write_u64(&mut record, value + 0x08, unix_ms_to_filetime(creation_ms));
        }
        if let Some(modification_ms) = name_spec.modification_ms.or(spec.modified_ms) {
            write_u64(
                &mut record,
                value + 0x10,
                unix_ms_to_filetime(modification_ms),
            );
        }
        write_u64(&mut record, value + 0x28, 0); // allocated (устаревшее)
        write_u64(&mut record, value + 0x30, 0); // real (устаревшее)
        write_u32(&mut record, value + 0x38, 0x20); // FILE_ATTRIBUTE_ARCHIVE
        record[value + 0x40] = units.len() as u8;
        record[value + 0x41] = name_spec.namespace;
        for (i, unit) in units.iter().enumerate() {
            write_u16(&mut record, value + 0x42 + i * 2, *unit);
        }
        off += align8(ATTR_HEADER_LEN + value_len);
    }

    match spec.data {
        DataSpec::None => {}
        DataSpec::Resident(len) => {
            let value_len = len as usize;
            write_u32(&mut record, off, ATTR_DATA);
            write_u32(&mut record, off + 0x04, align8(ATTR_HEADER_LEN + value_len) as u32);
            record[off + 0x08] = 0; // резидентный
            write_u16(&mut record, off + 0x0E, 4);
            write_u32(&mut record, off + 0x10, value_len as u32);
            write_u16(&mut record, off + 0x14, ATTR_HEADER_LEN as u16);
            off += align8(ATTR_HEADER_LEN + value_len);
        }
        DataSpec::NonResident { allocated, real } => {
            // Runlist: один прогон на 1 кластер и конец списка.
            let runlist = [0x11u8, 0x01, 0x00, 0x00];
            let attr_len = ATTR_NONRESIDENT_HEADER_LEN + runlist.len();
            write_u32(&mut record, off, ATTR_DATA);
            write_u32(&mut record, off + 0x04, align8(attr_len) as u32);
            record[off + 0x08] = 1; // нерезидентный
            write_u16(&mut record, off + 0x0E, 4);
            write_u64(&mut record, off + 0x10, 0); // starting VCN
            write_u64(&mut record, off + 0x18, 0); // last VCN
            write_u16(&mut record, off + 0x20, ATTR_NONRESIDENT_HEADER_LEN as u16);
            write_u64(&mut record, off + 0x28, allocated);
            write_u64(&mut record, off + 0x30, real);
            write_u64(&mut record, off + 0x38, real);
            record[off + ATTR_NONRESIDENT_HEADER_LEN..off + attr_len]
                .copy_from_slice(&runlist);
            off += align8(attr_len);
        }
    }

    // Именованные нерезидентные потоки $DATA: заголовок, имя UTF-16, runlist.
    for stream in &spec.streams {
        let units: Vec<u16> = stream.name.encode_utf16().collect();
        let name_off = ATTR_NONRESIDENT_HEADER_LEN;
        let run_off = name_off + align8(units.len() * 2);
        let runlist = [0x11u8, 0x01, 0x00, 0x00];
        let attr_len = run_off + runlist.len();
        write_u32(&mut record, off, ATTR_DATA);
        write_u32(&mut record, off + 0x04, align8(attr_len) as u32);
        record[off + 0x08] = 1; // нерезидентный
        record[off + 0x09] = units.len() as u8;
        write_u16(&mut record, off + 0x0A, name_off as u16);
        write_u16(&mut record, off + 0x0E, 5);
        write_u16(&mut record, off + 0x20, run_off as u16);
        write_u64(&mut record, off + 0x28, stream.allocated);
        write_u64(&mut record, off + 0x30, stream.real);
        write_u64(&mut record, off + 0x38, stream.real);
        for (i, unit) in units.iter().enumerate() {
            write_u16(&mut record, off + name_off + i * 2, *unit);
        }
        record[off + run_off..off + run_off + runlist.len()].copy_from_slice(&runlist);
        off += align8(attr_len);
    }

    write_u32(&mut record, off, ATTR_END);
    write_u32(&mut record, 0x18, (off + 8) as u32); // used size
    write_u32(&mut record, 0x1C, RECORD_LEN as u32); // allocated size

    // Fixup: в хвостах секторов лежит номер последовательности, настоящие
    // байты — в массиве USA.
    let usn = 0x0001u16;
    write_u16(&mut record, USA_OFFSET, usn);
    for sector in 1..USA_COUNT {
        let tail = sector * 512;
        let saved = [record[tail - 2], record[tail - 1]];
        write_u16(
            &mut record,
            USA_OFFSET + sector * 2,
            u16::from_le_bytes(saved),
        );
        write_u16(&mut record, tail - 2, usn);
    }

    record
}
