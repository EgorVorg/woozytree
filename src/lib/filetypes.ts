// Чистые функции вкладки «Типы файлов»: подписи расширений и переключение
// сортировки таблицы. Вынесены из компонента, чтобы проверять без DOM.
import type { SizeMode, TypeSort } from "./types";

/** Подпись группы без расширения — та же, что рисует карта типов в ядре. */
export const NO_EXTENSION_LABEL = "(нет)";

/** Состояние сортировки таблицы типов. */
export interface TypeSortState {
  sort: TypeSort;
  desc: boolean;
}

/** По умолчанию — крупные типы сверху, как таблица файлов. */
export const DEFAULT_TYPE_SORT: TypeSortState = {
  sort: "size_on_disk",
  desc: true,
};

/** Колонка размера, которой соответствует настройка «на диске / фактический». */
export function size_sort_column(sizeMode: SizeMode): TypeSort {
  return sizeMode === "logical" ? "size_logical" : "size_on_disk";
}

/**
 * Сортировка по умолчанию для текущего режима размера.
 *
 * Таблица типов, как и таблица файлов, при смене режима переключает колонку
 * размера: иначе в «фактическом» режиме по умолчанию сортировала бы колонка
 * «на диске», которой пользователь не просил.
 */
export function default_type_sort(sizeMode: SizeMode): TypeSortState {
  return { sort: size_sort_column(sizeMode), desc: true };
}

/** Подпись расширения в таблице: пустая строка — «(нет)». */
export function extension_label(extension: string): string {
  return extension === "" ? NO_EXTENSION_LABEL : extension;
}

/** Расширение по имени плитки карты типов («(нет)» — пустое расширение). */
export function tile_extension(name: string): string {
  return name === NO_EXTENSION_LABEL ? "" : name;
}

/**
 * Новое состояние сортировки после клика по колонке.
 *
 * Повторный клик по той же колонке разворачивает порядок; переход на другую
 * колонку начинает с «крупного сверху», а имя — с возрастания (как в
 * проводнике и таблице файлов).
 */
export function next_type_sort(
  current: TypeSortState,
  column: TypeSort,
): TypeSortState {
  if (current.sort === column) {
    return { sort: column, desc: !current.desc };
  }
  return { sort: column, desc: column !== "name" };
}
