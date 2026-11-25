// TS-зеркало DTO из `src-tauri/src/model.rs` и `scan/mod.rs`.

/** Сводка по одной папке (событие `scan://top`). */
export interface FolderSummary {
  id: number;
  name: string;
  size_on_disk: number;
  file_count: number;
  folder_count: number;
  /** Доля от размера родителя: 0.0..=1.0. */
  percent_of_parent: number;
}

/** Промежуточный прогресс сканирования (событие `scan://progress`). */
export interface ScanProgress {
  files: number;
  folders: number;
  size_on_disk: number;
  elapsed_ms: number;
}

/** Итоги сканирования (событие `scan://done`, команда `get_scan_totals`). */
export interface ScanTotals {
  files: number;
  folders: number;
  size_on_disk: number;
  size_logical: number;
  skipped: number;
  /** Скан остановлен пользователем: цифры — частичный срез, а не полный итог. */
  cancelled: boolean;
  elapsed_ms: number;
}

/** Логический диск из команды `list_drives`. */
export interface DriveInfo {
  letter: string;
  label: string;
  fs: string;
  total_bytes: number;
  free_bytes: number;
}

/** Что сканировать: `{"drive": "C:"}`, `{"folder": "C:\\Temp"}` или `{"all_drives": ["C:", "D:"]}`. */
export type ScanTarget =
  | { drive: string }
  | { folder: string }
  | { all_drives: string[] };

/** Выбор в селекторе тулбара: все диски, конкретный диск или отдельная папка. */
export type ScanChoice =
  | { kind: "all" }
  | { kind: "drive"; letter: string }
  | { kind: "folder"; path: string };

/** Одна строка таблицы файлов (`FilePage.rows`). */
export interface FileRow {
  name: string;
  /** Занято на диске, байт. */
  size_on_disk: number;
  /** Фактический размер, байт. */
  size_logical: number;
  /** Время изменения, Unix-мс (0 — время неизвестно). */
  modified: number;
  /** Доля от размера родителя: 0.0..=1.0. */
  percent_of_parent: number;
  /** Повторная жёсткая ссылка: размер не входит в итоги папки. */
  hard_linked: boolean;
}

/** Страница файлов папки (`get_files`). */
export interface FilePage {
  rows: FileRow[];
  /** Всего файлов в папке — по нему таблица понимает, есть ли ещё страницы. */
  total: number;
}

/** Колонки, по которым ядро умеет сортировать таблицу файлов. */
export type FileSort = "name" | "size_on_disk" | "size_logical" | "modified";

/** Строка таблицы типов файлов (`get_file_types`). */
export interface FileTypeRow {
  /** Расширение без точки, в нижнем регистре; `""` — файл без расширения. */
  extension: string;
  /** Сколько имён файлов этого типа (включая повторные жёсткие ссылки). */
  count: number;
  /** Занято на диске, байт (без дубликатов жёстких ссылок). */
  size_on_disk: number;
  /** Фактический размер, байт (без дубликатов жёстких ссылок). */
  size_logical: number;
  /** Доля от занятого места папки: 0.0..=1.0. */
  percent_of_parent: number;
}

/**
 * Колонки, по которым ядро сортирует таблицу типов файлов.
 *
 * Времени изменения здесь нет: у группы расширений его не существует.
 */
export type TypeSort = "name" | "count" | "size_on_disk" | "size_logical";

/** Фильтры поиска файлов (команда `search_files`). */
export interface SearchQuery {
  /** Подстрока имени без учёта регистра; «» — без фильтра по имени. */
  name: string;
  /** Нижняя граница размера на диске, байт; 0 — без границы. */
  min_size: number;
  /** Верхняя граница размера на диске; `null` — без границы. */
  max_size: number | null;
  /** Время изменения не раньше (включительно), Unix-мс; `null` — без границы. */
  modified_after: number | null;
  /** Время изменения не позже (включительно), Unix-мс; `null` — без границы. */
  modified_before: number | null;
}

/** Строка результатов поиска (`SearchPage.rows`). */
export interface SearchRow {
  name: string;
  /** Путь папки файла от корня индекса. */
  folder_path: string;
  /** Папка файла — для перехода на карту. */
  folder_id: number;
  /** Занято на диске, байт. */
  size_on_disk: number;
  /** Фактический размер, байт. */
  size_logical: number;
  /** Время изменения, Unix-мс (0 — время неизвестно). */
  modified: number;
  /** Повторная жёсткая ссылка: размер не входит в итоги папки. */
  hard_linked: boolean;
}

/** Страница результатов поиска (`search_files`). */
export interface SearchPage {
  rows: SearchRow[];
  /** Всего файлов под фильтры — по нему таблица понимает, есть ли ещё страницы. */
  total: number;
}

/** Звено пути от корня сканирования до папки (`get_path_crumbs`). */
export interface Crumb {
  id: number;
  name: string;
}

/**
 * Прямоугольник карты (результат `get_treemap` и событие `scan://treemap`).
 *
 * Координаты нормированы в 0..1 относительно контейнера. Прямоугольники
 * вложенные: родитель идёт в списке раньше своих детей, дети рисуются поверх.
 */
export interface TreemapRect {
  /** У папок — FolderId; у файлов и сводной плитки «прочее» — 0. */
  id: number;
  name: string;
  /** Занято на диске, байт. */
  size: number;
  is_dir: boolean;
  x: number;
  y: number;
  w: number;
  h: number;
}
