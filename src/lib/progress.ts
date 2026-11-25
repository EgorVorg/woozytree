// Процент заполнения прогресс-бара сканирования.
//
// База (`scannedUsed`) — занятое место сканируемого диска на момент старта.
// Она не зависит от текущего выбранного диска: смена диска после остановки не
// должна пересчитывать процент уже собранных данных.

export interface ScanPercentArgs {
  /** Сколько байт уже попало в индекс. */
  scannedBytes: number;
  /** Занятое место диска на старте скана, байт. */
  scannedUsed: number;
  /** Идёт сканирование. */
  scanning: boolean;
  /** Скан остановлен пользователем — данные частичные. */
  cancelled: boolean;
  /** Скан завершён успешно (есть итоги). */
  finished: boolean;
}

/** Заполненность бара, 0..100: живой скан — до 99, полный — 100. */
export function scanPercent({
  scannedBytes,
  scannedUsed,
  scanning,
  cancelled,
  finished,
}: ScanPercentArgs): number {
  const live =
    scannedUsed > 0
      ? Math.min(99, Math.round((scannedBytes / scannedUsed) * 100))
      : 0;
  if (cancelled) return live;
  if (!scanning) return finished ? 100 : 0;
  return live;
}
