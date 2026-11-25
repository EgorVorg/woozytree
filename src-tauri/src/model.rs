//! Модель данных сканера: идентификаторы папок, ключи файлов, DTO для фронтенда.

use std::path::PathBuf;

pub type FolderId = u32;

/// Что сканировать: целый диск, отдельную папку или все диски разом.
///
/// serde externally-tagged (по умолчанию для enum, `snake_case` — для имён
/// вариантов): `{"drive": "C"}` | `{"folder": "C:\\Temp"}` |
/// `{"all_drives": ["C:", "D:"]}`. Все варианты — объекты, поэтому TS-тип
/// `ScanTarget` простой (см. `src/lib/types.ts`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanTarget {
    Drive(String),
    Folder(PathBuf),
    /// Буквы томов от фронтенда (`list_drives`); каждый том сканируется в свою
    /// подпапку корня индекса.
    AllDrives(Vec<String>),
}

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

/// Фильтры поиска файлов по готовому индексу.
///
/// Пустые фильтры валидны: `name: ""`, `min_size: 0` и `None` в границах
/// означают «всё подряд». Верхняя граница размера — `Option`, а не `u64::MAX`:
/// это значение нельзя передать из JS без потери точности.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SearchQuery {
    /// Подстрока имени без учёта регистра: правила те же, что у сортировки
    /// (ASCII, кириллица, «ё» ≈ «е»).
    pub name: String,
    /// Нижняя граница размера на диске в байтах; 0 — без границы.
    pub min_size: u64,
    /// Верхняя граница размера на диске в байтах; `None` — без границы.
    pub max_size: Option<u64>,
    /// Время изменения не раньше (включительно), Unix-мс; `None` — без границы.
    pub modified_after: Option<i64>,
    /// Время изменения не позже (включительно), Unix-мс; `None` — без границы.
    pub modified_before: Option<i64>,
}

/// DTO для фронтенда: строка результатов поиска.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchRow {
    pub name: String,
    /// Путь папки файла от корня индекса, разделитель `\`.
    pub folder_path: String,
    /// Папка файла — для перехода на карту (`get_path_crumbs`).
    pub folder_id: u32,
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,
    pub hard_linked: bool,
}

/// DTO для фронтенда: страница результатов поиска (`search_files`).
#[derive(Debug, serde::Serialize)]
pub struct SearchPage {
    pub rows: Vec<SearchRow>,
    /// Сколько всего файлов подходит под фильтры — для пагинации.
    pub total: u32,
}

/// Звено пути от корня индекса до папки (`get_path_crumbs`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrumbDto {
    pub id: u32,
    pub name: String,
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
