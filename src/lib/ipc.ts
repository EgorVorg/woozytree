// Обёртки над командами и событиями Tauri: фронтенд не знает про `invoke`/`listen`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import type { DisplaySettings } from "./settings";
import type {
  Crumb,
  DeleteReport,
  DriveInfo,
  FilePage,
  FileSort,
  FileTypeRow,
  FolderSummary,
  ScanProgress,
  ScanTarget,
  ScanTotals,
  SearchPage,
  SearchQuery,
  SizeMode,
  TreemapRect,
  TypeSort,
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

/**
 * Запустить сканирование в фоне; события придут по `scan://…`.
 *
 * Настройки отображения передаются снимком на старте: живую карту строит ядро
 * по событиям, и без снимка она рисовалась бы с дефолтами до конца скана.
 */
export function startScan(
  target: ScanTarget,
  settings: DisplaySettings,
): Promise<void> {
  return invoke("start_scan", {
    target,
    sizeMode: settings.sizeMode,
    includeFiles: settings.showFilesOnMap,
    minTileBytes: settings.minTileBytes,
  });
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
 *
 * `sizeMode` — настройка отображения: по ней ядро считает колонку `%` (доля от
 * выбранного размера родителя). Сортировка по-прежнему любая (`sort`).
 */
export function getFiles(
  folderId: number,
  sort: FileSort,
  desc: boolean,
  offset: number,
  limit: number,
  sizeMode: SizeMode,
): Promise<FilePage> {
  return invoke<FilePage>("get_files", {
    folderId,
    sort,
    desc,
    offset,
    limit,
    sizeMode,
  });
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
 *
 * Параметры отображения — настройки фронтенда: `sizeMode` (что считать
 * размером), `includeFiles` (показывать ли файлы) и `minTileBytes` (порог
 * мелочи: плитки мельче уходят в «прочее»; итоги не меняются).
 */
export function getTreemap(
  folderId: number,
  maxRects: number,
  sizeMode: SizeMode,
  includeFiles: boolean,
  minTileBytes: number,
): Promise<TreemapRect[]> {
  return invoke<TreemapRect[]>("get_treemap", {
    folderId,
    maxRects,
    sizeMode,
    includeFiles,
    minTileBytes,
  });
}

/**
 * Агрегаты по расширениям в поддереве папки — таблица вкладки «Типы файлов».
 *
 * `sort`/`desc` применяет ядро: таблица получает уже упорядоченные строки и не
 * сортирует миллионы файлов на стороне окна.
 */
export function getFileTypes(
  folderId: number,
  sort: TypeSort,
  desc: boolean,
): Promise<FileTypeRow[]> {
  return invoke<FileTypeRow[]>("get_file_types", { folderId, sort, desc });
}

/**
 * Плоские плитки карты типов: расширения в нормированном контейнере 0..1.
 * `maxRects` — бюджет раскладки, ядро клампит его 5000. `sizeMode` — настройка
 * отображения: порядок и площадь плиток считаются по выбранному размеру.
 */
export function getTypesTreemap(
  folderId: number,
  maxRects: number,
  sizeMode: SizeMode,
): Promise<TreemapRect[]> {
  return invoke<TreemapRect[]>("get_types_treemap", {
    folderId,
    maxRects,
    sizeMode,
  });
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

/**
 * Реальный абсолютный путь файла в папке индекса.
 *
 * Индекс хранит только имена, а действиям с файлами (удаление, открытие,
 * проводник, копирование пути) нужен путь на диске. Ядро строит его от
 * реального корня сканирования.
 */
export function getFilePath(
  folderId: number,
  fileName: string,
): Promise<string> {
  return invoke<string>("get_file_path", { folderId, fileName });
}

/**
 * Удалить пути: в корзину (`permanent === false`) или безвозвратно.
 *
 * Ошибка по одному пути не прерывает удаление остальных — она приходит в
 * `report.errors`. Корзина — системная, восстановить файл можно оттуда.
 */
export function deletePaths(
  paths: string[],
  permanent: boolean,
): Promise<DeleteReport> {
  return invoke<DeleteReport>("delete_paths", { paths, permanent });
}

/** Открыть файл приложением по умолчанию (`ShellExecuteW`, «open»). */
export function openPath(path: string): Promise<void> {
  return invoke("open_path", { path });
}

/** Показать файл в проводнике с выделением (`explorer.exe /select,<path>`). */
export function revealInExplorer(path: string): Promise<void> {
  return invoke("reveal_in_explorer", { path });
}
