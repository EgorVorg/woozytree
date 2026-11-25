// Область кэша запроса: какие части ключа должны совпасть, чтобы прошлые
// данные можно было показывать как placeholder.
//
// Танстак-кэш живёт на всё окно, и при смене папки/скана старые строки — это
// чужие данные. Placeholder («Сортируем…») допустим только когда сменилась
// сортировка в том же контексте: папка и номер скана те же.

/** Совпадает ли начало прошлого ключа с ожидаемой областью (тег, скан, папка). */
export function same_query_scope(
  previousKey: readonly unknown[] | undefined,
  expected: readonly unknown[],
): boolean {
  if (!previousKey) return false;
  if (previousKey.length < expected.length) return false;
  return expected.every((part, index) => previousKey[index] === part);
}
