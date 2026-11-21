// Единое форматирование чисел для интерфейса: размеры — в ГиБ/МиБ с одним знаком.

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

/** Размер на диске: «12.3 ГиБ» или «456.7 МиБ», всегда с одним знаком. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0.0 МиБ";
  }
  if (bytes >= GIB) {
    return `${(bytes / GIB).toFixed(1)} ГиБ`;
  }
  return `${(bytes / MIB).toFixed(1)} МиБ`;
}

/** Время сканирования: «1.4 с». */
export function formatSeconds(elapsedMs: number): string {
  return `${(elapsedMs / 1000).toFixed(1)} с`;
}

/** Доля от родителя (0..1) в проценты с одним знаком: «25.0 %». */
export function formatPercent(fraction: number): string {
  return `${(fraction * 100).toFixed(1)} %`;
}

/** Счётчики с разделителями разрядов: «1 234 567». */
export function formatCount(value: number): string {
  return value.toLocaleString("ru-RU");
}
