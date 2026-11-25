// Чистые функции карты: хит-тест «точка → плитка» и разметка вложенности.
//
// Контракт вывода Rust (`treemap_for_folder`): прямоугольники вложенные,
// родитель идёт в списке раньше своих детей, координаты нормированы 0..1.
// Поэтому «верхняя» плитка под точкой — последняя подходящая в списке.
import type { TreemapRect } from "./types";

/** Допуск для проверок «плитка лежит внутри родителя» при погрешности f64. */
const EPS = 1e-9;

/** Звено пути от корня сканирования до открытой папки. */
export interface Crumb {
  id: number;
  name: string;
}

/** Плитка с посчитанными связями: глубина, родитель, доля от родителя. */
export interface TreemapTile {
  rect: TreemapRect;
  /** 0 — плитка верхнего уровня (ребёнок текущей папки). */
  depth: number;
  /** Имя родительской плитки; null у верхнего уровня. */
  parent_name: string | null;
  /** Размер родителя; у верхнего уровня — сумма размеров всех его плиток. */
  parent_size: number;
  /** Доля от родителя, 0..1. */
  percent_of_parent: number;
  /** Лежат ли поверх этой плитки плитки детей (тогда подпись не рисуем). */
  has_children: boolean;
}

/** Лежит ли точка внутри прямоугольника; границы включены. */
function contains(
  rect: TreemapRect,
  x: number,
  y: number,
  epsilon: number,
): boolean {
  return (
    x >= rect.x - epsilon &&
    x <= rect.x + rect.w + epsilon &&
    y >= rect.y - epsilon &&
    y <= rect.y + rect.h + epsilon
  );
}

/**
 * Плитка под точкой в нормированных координатах 0..1.
 *
 * Перебор идёт с конца списка: вложенные дети нарисованы поверх родителей,
 * поэтому побеждает последняя (самая верхняя) подходящая плитка. Нулевые
 * плитки (вырожденная раскладка) попаданий не дают.
 */
export function hit_test(
  rects: TreemapRect[],
  x: number,
  y: number,
): TreemapRect | null {
  for (let index = rects.length - 1; index >= 0; index -= 1) {
    const rect = rects[index];
    if (!(rect.w > 0) || !(rect.h > 0)) continue;
    if (contains(rect, x, y, 0)) return rect;
  }
  return null;
}

/**
 * Индекс ближайшего родителя плитки: последняя предыдущая плитка, внутрь
 * которой попадает её центр. Для плитки верхнего уровня — `null`.
 */
export function parent_index(
  rects: TreemapRect[],
  index: number,
): number | null {
  const rect = rects[index];
  if (!rect) return null;
  const centerX = rect.x + rect.w / 2;
  const centerY = rect.y + rect.h / 2;
  for (let parent = index - 1; parent >= 0; parent -= 1) {
    const candidate = rects[parent];
    if (!(candidate.w > 0) || !(candidate.h > 0)) continue;
    if (contains(candidate, centerX, centerY, EPS)) return parent;
  }
  return null;
}

/**
 * Разметка плиток для отрисовки и подсказок: глубина, размер родителя и доля
 * от него. Один проход, потому что родитель всегда раньше ребёнка.
 */
export function analyze_rects(rects: TreemapRect[]): TreemapTile[] {
  const parents = rects.map((_, index) => parent_index(rects, index));

  let top_size = 0;
  for (let index = 0; index < rects.length; index += 1) {
    if (parents[index] === null) top_size += rects[index].size;
  }

  const has_children = rects.map(() => false);
  for (const parent of parents) {
    if (parent !== null) has_children[parent] = true;
  }

  const depths: number[] = [];
  return rects.map((rect, index) => {
    const parent = parents[index];
    const depth = parent === null ? 0 : depths[parent] + 1;
    depths[index] = depth;
    const parent_size = parent === null ? top_size : rects[parent].size;
    const percent_of_parent =
      parent_size > 0 ? Math.min(1, rect.size / parent_size) : 0;
    return {
      rect,
      depth,
      parent_name: parent === null ? null : rects[parent].name,
      parent_size,
      percent_of_parent,
      has_children: has_children[index],
    };
  });
}

/** Сводная плитка «прочее»: id = 0, не папка, особое имя из Rust. */
export function is_other(rect: TreemapRect): boolean {
  return !rect.is_dir && rect.id === 0 && rect.name === "прочее";
}

/**
 * Полный путь до кликнутой плитки: префикс `base` (путь до открытой папки),
 * цепочка её предков внутри текущей выдачи и сама плитка.
 *
 * Карта рисует вложенные плитки, поэтому клик по Music внутри Users должен
 * давать «C:\ > Users > Music», а не «C:\ > Music». Контракт выдачи Rust
 * (`treemap_for_folder`) гарантирует, что родитель идёт в списке раньше
 * ребёнка, поэтому цепочку собираем через `parent_index` вверх до плитки
 * верхнего уровня.
 *
 * Для не-навигации возвращает `[]`: файл только подсвечивается, сводная
 * плитка «прочее» (id = 0) действий не даёт. Повтор последнего звена
 * отбрасывается — повторный клик по той же папке не плодит крошки и
 * React-ключи.
 */
export function crumbs_for_tile(
  rects: TreemapRect[],
  tile: TreemapRect,
  base: readonly Crumb[] = [],
): Crumb[] {
  if (!tile.is_dir || tile.id === 0) return [];
  const index = rects.indexOf(tile);
  if (index < 0) return [];

  const ancestors: TreemapRect[] = [];
  let parent = parent_index(rects, index);
  while (parent !== null) {
    ancestors.unshift(rects[parent]);
    parent = parent_index(rects, parent);
  }

  const crumbs: Crumb[] = base.map((crumb) => ({ ...crumb }));
  for (const node of [...ancestors, tile]) {
    if (crumbs.length > 0 && crumbs[crumbs.length - 1].id === node.id) continue;
    crumbs.push({ id: node.id, name: node.name });
  }
  return crumbs;
}

/**
 * Папка индекса, в которой лежит плитка-файл.
 *
 * Действия с файлом (удаление, открытие, проводник, копирование пути) требуют
 * `folder_id` + имя, а карта рисует вложенные плитки: файл может лежать не в
 * открытой папке, а в одной из вложенных. Ближайший предок-папка с настоящим id
 * и есть его папка; файл верхнего уровня — ребёнок открытой папки.
 */
export function folder_id_for_tile(
  rects: TreemapRect[],
  tile: TreemapRect,
  currentFolderId: number,
): number {
  let index = rects.indexOf(tile);
  if (index < 0) return currentFolderId;
  let parent = parent_index(rects, index);
  while (parent !== null) {
    const candidate = rects[parent];
    if (candidate.is_dir && candidate.id !== 0) return candidate.id;
    parent = parent_index(rects, parent);
  }
  return currentFolderId;
}

/**
 * Имя корневой хлебной крошки по букве диска из `DriveInfo.letter`.
 *
 * `letter` приходит из Rust уже без обратного слэша, но с двоеточием («C:»),
 * поэтому шаблон `${letter}:\\` давал бы «C::\». Принимаем все три формы:
 * «C:», «C:\» и «C» → «C:\».
 */
export function root_crumb_name(letter: string): string {
  const trimmed = letter.trim();
  if (trimmed === "") return "";
  const base = trimmed.replace(/[\\:]+$/, "");
  return base === "" ? "" : `${base}:\\`;
}
