// Чистые операции над путём навигации (крошки): их использует App, а тесты
// проверяют намерение «заменить путь», не поднимая React.
import type { Crumb } from "./types";

/** Копия звена пути без лишних полей источника. */
function copy_crumb(crumb: Crumb): Crumb {
  return { id: crumb.id, name: crumb.name };
}

/**
 * Путь после перехода из карты, дерева или поиска.
 *
 * Новый путь **заменяет** текущий, а не дописывается к нему: клик по Music
 * внутри Users приносит всю цепочку «C:\ > Users > Music», и добавление её к
 * прежнему пути дало бы дубли и «хвосты» соседних веток. Пустая цепочка
 * (файл или сводная плитка) оставляет путь как был.
 */
export function replace_path(
  current: readonly Crumb[],
  next: readonly Crumb[],
): Crumb[] {
  if (next.length === 0) return current.map(copy_crumb);
  return next.map(copy_crumb);
}

/** Путь после клика по крошке с id: всё, что глубже неё, отрезается. */
export function path_to_crumb(
  current: readonly Crumb[],
  id: number,
): Crumb[] {
  const index = current.findIndex((crumb) => crumb.id === id);
  return index >= 0
    ? current.slice(0, index + 1).map(copy_crumb)
    : current.map(copy_crumb);
}

/** Путь после «Вверх»: на корне (одно звено) не меняется. */
export function path_up(current: readonly Crumb[]): Crumb[] {
  return (current.length > 1 ? current.slice(0, -1) : current).map(copy_crumb);
}
