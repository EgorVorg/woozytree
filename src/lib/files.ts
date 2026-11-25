// Чистая логика дерева папок и таблицы файлов: разворачивание ленивого дерева
// в плоский список строк, цепочка предков для навигации, допустимые колонки
// сортировки. Без React и IPC — поэтому покрыто unit-тестами.
import type { FileSort, FolderSummary } from "./types";

/** Корень сканирования: имя и итоги (из `ScanTotals`). */
export interface TreeRoot {
  name: string;
  size_on_disk: number;
  file_count: number;
  folder_count: number;
}

/** Одна видимая строка дерева (плоский список для виртуализации). */
export interface TreeRow {
  id: number;
  name: string;
  /** 0 — корень сканирования. */
  depth: number;
  size_on_disk: number;
  /** Файлов во всей папке (рекурсивно). */
  file_count: number;
  /** Папок внутри (рекурсивно); 0 — раскрывать нечего. */
  folder_count: number;
  has_children: boolean;
  expanded: boolean;
  /** Узел раскрыт, но `get_children` ещё не ответил. */
  loading: boolean;
}

/** Звено пути от корня до папки (совместимо с `Crumb` в App). */
export interface TreeCrumb {
  id: number;
  name: string;
}

/** Колонки, которые принимает `get_files`. */
const FILE_SORT_COLUMNS: readonly string[] = [
  "name",
  "size_on_disk",
  "size_logical",
  "modified",
];

/** Это колонка, по которой ядро умеет сортировать? */
export function is_file_sort(value: string): value is FileSort {
  return FILE_SORT_COLUMNS.includes(value);
}

/**
 * Развернуть ленивое дерево в плоский список видимых строк.
 *
 * `childrenByParent` содержит только уже загруженные ответы `get_children`;
 * узел попадает в список, если раскрыты все его предки. Порядок детей —
 * как отдал бэкенд (по убыванию размера).
 */
export function flatten_tree(
  root: TreeRoot,
  childrenByParent: ReadonlyMap<number, readonly FolderSummary[]>,
  expanded: ReadonlySet<number>,
  loading: ReadonlySet<number>,
): TreeRow[] {
  const rows: TreeRow[] = [];

  const push = (
    id: number,
    name: string,
    depth: number,
    size_on_disk: number,
    file_count: number,
    folder_count: number,
  ) => {
    const isExpanded = expanded.has(id);
    const hasChildren = folder_count > 0;
    const children = childrenByParent.get(id);
    rows.push({
      id,
      name,
      depth,
      size_on_disk,
      file_count,
      folder_count,
      has_children: hasChildren,
      expanded: isExpanded,
      loading: isExpanded && hasChildren && !children && loading.has(id),
    });
    if (!isExpanded || !children) return;
    for (const child of children) {
      push(
        child.id,
        child.name,
        depth + 1,
        child.size_on_disk,
        child.file_count,
        child.folder_count,
      );
    }
  };

  push(
    0,
    root.name,
    0,
    root.size_on_disk,
    root.file_count,
    root.folder_count,
  );
  return rows;
}

/**
 * Цепочка от корня до папки по плоскому списку видимых строк.
 *
 * Нужна для навигации из дерева: у клика по глубокому узлу нет «родителя» в
 * props, но он есть среди строк выше — предок всегда виден (иначе узел не был
 * бы в списке). Неизвестный id — `null`.
 */
export function tree_chain(
  rows: readonly TreeRow[],
  id: number,
): TreeCrumb[] | null {
  const index = rows.findIndex((row) => row.id === id);
  if (index < 0) return null;

  const chain: TreeRow[] = [rows[index]];
  let depth = rows[index].depth;
  for (let cursor = index - 1; cursor >= 0 && depth > 0; cursor -= 1) {
    const row = rows[cursor];
    if (row.depth === depth - 1) {
      chain.push(row);
      depth = row.depth;
    }
  }
  if (depth !== 0) return null;
  return chain.reverse().map((row) => ({ id: row.id, name: row.name }));
}
