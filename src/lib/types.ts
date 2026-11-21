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

/** Что сканировать: `{"drive": "C:"}` или `{"folder": "C:\\Temp"}`. */
export type ScanTarget = { drive: string } | { folder: string };
