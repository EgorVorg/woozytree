// Единое форматирование чисел для интерфейса: размеры — в ГиБ/МиБ/КиБ с одним
// знаком.

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;
const KIB = 1024;

/** Размер на диске: «12.3 ГиБ», «456.7 МиБ», «0.5 КиБ» или «0 Б», всегда с одним знаком. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    // Пустой файл — ровно ноль: «0 Б», а не «0.0 КиБ» (ноль не во что делить).
    return "0 Б";
  }
  if (bytes >= GIB) {
    return `${(bytes / GIB).toFixed(1)} ГиБ`;
  }
  if (bytes >= MIB) {
    return `${(bytes / MIB).toFixed(1)} МиБ`;
  }
  return `${(bytes / KIB).toFixed(1)} КиБ`;
}

/** Время сканирования: «1.4 с». */
export function formatSeconds(elapsedMs: number): string {
  return `${(elapsedMs / 1000).toFixed(1)} с`;
}

/** Доля от родителя (0..1) в проценты с одним знаком: «25.0 %». */
export function formatPercent(fraction: number): string {
  return `${(fraction * 100).toFixed(1)} %`;
}

/**
 * Дата изменения файла: «дд.мм.гггг чч:мм» в местном времени (как в проводнике).
 * Неизвестное время (0, отрицательное, NaN) — прочерк.
 */
export function formatDateTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) {
    return "—";
  }
  const date = new Date(ms);
  const time = date.getTime();
  if (Number.isNaN(time)) {
    return "—";
  }
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Счётчики с разделителями разрядов: «1 234 567». */
export function formatCount(value: number): string {
  return value.toLocaleString("ru-RU");
}
