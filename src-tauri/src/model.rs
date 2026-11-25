//! Модель данных сканера: идентификаторы папок, ключи файлов, DTO для фронтенда.

pub type FolderId = u32;

/// Уникальный ключ файла на диске (номер тома + файловая запись).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub volume_serial: u32,
    pub file_index: u64,
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub name: String,      // имя файла без пути
    pub size_on_disk: u64, // байт, занятых на диске
    pub size_logical: u64, // фактический размер
    pub modified: i64,     // время изменения, Unix-мс
    pub file_key: FileKey,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderTotals {
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub file_count: u32,
    pub folder_count: u32,
}

/// DTO для фронтенда: сводка по одной папке.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderSummary {
    pub id: FolderId,
    pub name: String,
    pub size_on_disk: u64,
    pub file_count: u32,
    pub folder_count: u32,
    pub percent_of_parent: f32, // доля от размера родителя, 0.0..=1.0
}

/// DTO для фронтенда: одна строка таблицы файлов папки.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileRow {
    pub name: String,
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,          // Unix мс
    pub percent_of_parent: f32, // доля от размера родителя, 0.0..=1.0
    pub hard_linked: bool,      // true у повторных ссылок (в итоги не входят)
}

/// Колонка, по которой сортируется таблица файлов.
#[derive(Debug, Clone, Copy)]
pub enum FileSort {
    Name,
    SizeOnDisk,
    SizeLogical,
    Modified,
}

/// DTO для фронтенда: страница файлов папки (`get_files`).
///
/// `total` — сколько всего файлов в папке: по нему таблица понимает, есть ли
/// ещё страницы, не догружая их заранее.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FilePage {
    pub rows: Vec<FileRow>,
    pub total: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanProgress {
    pub files: u64,
    pub folders: u64,
    pub size_on_disk: u64,
    pub elapsed_ms: u64,
}

/// Итоги сканирования.
///
/// `cancelled == true` означает, что пользователь остановил скан: цифры — срез на
/// момент остановки, а не полный результат. Такие итоги нельзя показывать как
/// завершённые 100 % и нельзя сохранять как финальные (см. `get_scan_totals`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanTotals {
    pub files: u64,
    pub folders: u64,
    pub size_on_disk: u64,
    pub size_logical: u64,
    /// Число событий «запись или имя разобрано неполностью / потеряло родителя».
    ///
    /// Это составной счётчик, а не число файлов, положенных в корень: сюда
    /// входят битые записи, базовые записи с `$ATTRIBUTE_LIST`, extension-записи
    /// с именами, имена-сироты и неразрешённые папки. Одно имя или одна запись
    /// может дать несколько событий, поэтому `skipped` не равен числу «файлов в
    /// корне». Разбивка по причинам — `crate::scan::mft::ScanDiag` (см.
    /// `scan_ntfs_reader_diag`).
    pub skipped: u32,
    /// Скан остановлен пользователем: данные частичные.
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DriveInfo {
    pub letter: String,
    pub label: String,
    pub fs: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Прямоугольник живой карты (treemap): координаты нормированы в 0..1.
///
/// Данные раскладки подключает Задача 3; в Задаче 1 событие `treemap` уже
/// приходит с нужным ритмом, а срез может быть пустым.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TreemapRect {
    /// Для папок — `FolderId`, для файлов — 0 (различать по `is_dir`).
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug)]
pub enum ScanError {
    VolumeAccess(String),
    Io(String),
    Parse(String),
}
