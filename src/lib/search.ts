// Чистая логика панели поиска: форма фильтров → запрос к ядру.
//
// Здесь нет React и IPC — только преобразование того, что ввёл пользователь, в
// `SearchQuery`. Поэтому функции покрыты unit-тестами (как `lib/files.ts`).
import type { SearchQuery } from "./types";

/** Единицы размера в форме — как в WizTree: МиБ и ГиБ. */
export type SizeUnit = "mib" | "gib";

/** Пункты селектора единиц размера. */
export const SIZE_UNITS: ReadonlyArray<{ value: SizeUnit; label: string }> = [
  { value: "mib", label: "МиБ" },
  { value: "gib", label: "ГиБ" },
];

/** Сколько байт в единице размера. */
const BYTES_PER_UNIT: Record<SizeUnit, number> = {
  mib: 1024 ** 2,
  gib: 1024 ** 3,
};

/** Поля формы поиска: строки из полей ввода; пусто — границы нет. */
export interface SearchForm {
  name: string;
  /** Нижняя граница размера на диске, число в выбранной единице. */
  sizeFrom: string;
  /** Верхняя граница размера на диске; пусто — без границы (null в запросе). */
  sizeTo: string;
  sizeUnit: SizeUnit;
  /** Дата «YYYY-MM-DD» из `<input type="date">`; пусто — без границы. */
  dateFrom: string;
  dateTo: string;
}

/** Пустая форма = «всё подряд»: валидный поиск без фильтров. */
export function empty_search_form(): SearchForm {
  return {
    name: "",
    sizeFrom: "",
    sizeTo: "",
    sizeUnit: "mib",
    dateFrom: "",
    dateTo: "",
  };
}

/**
 * Размер из формы в байтах; `null` — границы нет.
 *
 * Пустая строка, мусор и отрицательное число границей не считаются. Запятая
 * принимается как десятичный разделитель: так удобнее вводить по-русски.
 */
export function size_bound(value: string, unit: SizeUnit): number | null {
  const text = value.trim().replace(",", ".");
  if (!text) return null;
  const number = Number(text);
  if (!Number.isFinite(number) || number < 0) return null;
  return Math.round(number * BYTES_PER_UNIT[unit]);
}

/** Разобрать «YYYY-MM-DD» без перекатывания даты (31 февраля — не 3 марта). */
function parse_date(value: string): { year: number; month: number; day: number } | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(year, month - 1, day);
  if (
    date.getFullYear() !== year ||
    date.getMonth() !== month - 1 ||
    date.getDate() !== day
  ) {
    return null;
  }
  return { year, month, day };
}

/** Начало местных суток даты (00:00:00.000) в мс; `null` — поля нет. */
export function date_start_ms(value: string): number | null {
  const parts = parse_date(value);
  if (!parts) return null;
  return new Date(parts.year, parts.month - 1, parts.day, 0, 0, 0, 0).getTime();
}

/** Конец местных суток (23:59:59.999) в мс; `null` — поля нет. */
export function date_end_ms(value: string): number | null {
  const parts = parse_date(value);
  if (!parts) return null;
  return new Date(
    parts.year,
    parts.month - 1,
    parts.day,
    23,
    59,
    59,
    999,
  ).getTime();
}

/**
 * Форма → запрос к `search_files`.
 *
 * Верхняя граница размера — `null`, а не `u64::MAX`: JS не может передать
 * максимальное u64 без потери точности (см. `SearchQuery` в model.rs).
 */
export function build_search_query(form: SearchForm): SearchQuery {
  return {
    name: form.name.trim(),
    min_size: size_bound(form.sizeFrom, form.sizeUnit) ?? 0,
    max_size: size_bound(form.sizeTo, form.sizeUnit),
    modified_after: date_start_ms(form.dateFrom),
    modified_before: date_end_ms(form.dateTo),
  };
}

/** Есть ли в форме хоть один фильтр; пустая форма ищет «всё подряд». */
export function has_filters(form: SearchForm): boolean {
  return (
    form.name.trim() !== "" ||
    form.sizeFrom.trim() !== "" ||
    form.sizeTo.trim() !== "" ||
    form.dateFrom !== "" ||
    form.dateTo !== ""
  );
}
