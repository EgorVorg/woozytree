// Обёртки над командами и событиями Tauri: фронтенд не знает про `invoke`/`listen`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  DriveInfo,
  FolderSummary,
  ScanProgress,
  ScanTarget,
  ScanTotals,
} from "./types";

/** Логические диски с меткой, ФС и свободным местом. */
export function listDrives(): Promise<DriveInfo[]> {
  return invoke<DriveInfo[]>("list_drives");
}

/** Запустить сканирование в фоне; события придут по `scan://…`. */
export function startScan(target: ScanTarget): Promise<void> {
  return invoke("start_scan", { target });
}

/** Попросить текущее сканирование остановиться. */
export function cancelScan(): Promise<void> {
  return invoke("cancel_scan");
}

/** Дочерние папки готового индекса (по завершении сканирования). */
export function getChildren(folderId: number): Promise<FolderSummary[]> {
  return invoke<FolderSummary[]>("get_children", { folderId });
}

/** Итоги последнего завершённого сканирования. */
export function getScanTotals(): Promise<ScanTotals> {
  return invoke<ScanTotals>("get_scan_totals");
}

export function onProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan://progress", (event) => cb(event.payload));
}

export function onTop(cb: (rows: FolderSummary[]) => void): Promise<UnlistenFn> {
  return listen<FolderSummary[]>("scan://top", (event) => cb(event.payload));
}

export function onDone(cb: (totals: ScanTotals) => void): Promise<UnlistenFn> {
  return listen<ScanTotals>("scan://done", (event) => cb(event.payload));
}

export function onError(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>("scan://error", (event) => cb(event.payload));
}
