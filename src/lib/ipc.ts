// Обёртки над командами и событиями Tauri: фронтенд не знает про `invoke`/`listen`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import type {
  Crumb,
  DriveInfo,
  FilePage,
  FileSort,
  FolderSummary,
  ScanProgress,
  ScanTarget,
  ScanTotals,
  SearchPage,
  SearchQuery,
  TreemapRect,
} from "./types";

/**
 * Системный диалог выбора папки для сканирования.
 *
 * `null` — пользователь закрыл диалог: выбор в тулбаре остаётся прежним.
 */
export async function pickFolder(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Выберите папку для сканирования",
  });
  return typeof selected === "string" ? selected : null;
}

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

/**
 * Страница файлов папки: сортировка и срез `offset..offset+limit` — на стороне
 * ядра, окно получает только видимый кусок.
 */
export function getFiles(
  folderId: number,
  sort: FileSort,
  desc: boolean,
  offset: number,
  limit: number,
): Promise<FilePage> {
  return invoke<FilePage>("get_files", { folderId, sort, desc, offset, limit });
}

/** Итоги последнего завершённого сканирования. */
export function getScanTotals(): Promise<ScanTotals> {
  return invoke<ScanTotals>("get_scan_totals");
}

/**
 * Поиск файлов по готовому индексу: фильтры и страница `offset..offset+limit`
 * считает ядро. Пустые фильтры — «всё подряд».
 */
export function searchFiles(
  q: SearchQuery,
  offset: number,
  limit: number,
): Promise<SearchPage> {
  return invoke<SearchPage>("search_files", { q, offset, limit });
}

/** Цепочка папок от корня до указанной — для перехода из поиска на карту. */
export function getPathCrumbs(folderId: number): Promise<Crumb[]> {
  return invoke<Crumb[]>("get_path_crumbs", { folderId });
}

/**
 * Прямоугольники карты для папки (нормированные 0..1). `maxRects` — бюджет
 * раскладки, ядро клампит его 20 000.
 */
export function getTreemap(
  folderId: number,
  maxRects: number,
): Promise<TreemapRect[]> {
  return invoke<TreemapRect[]>("get_treemap", { folderId, maxRects });
}

export function onProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan://progress", (event) => cb(event.payload));
}

export function onTop(cb: (rows: FolderSummary[]) => void): Promise<UnlistenFn> {
  return listen<FolderSummary[]>("scan://top", (event) => cb(event.payload));
}

/** Живая карта корня сканирования: срез `scan://treemap` раз в секунду. */
export function onTreemap(
  cb: (rects: TreemapRect[]) => void,
): Promise<UnlistenFn> {
  return listen<TreemapRect[]>("scan://treemap", (event) => cb(event.payload));
}

export function onDone(cb: (totals: ScanTotals) => void): Promise<UnlistenFn> {
  return listen<ScanTotals>("scan://done", (event) => cb(event.payload));
}

export function onError(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>("scan://error", (event) => cb(event.payload));
}
