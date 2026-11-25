// Топ папок: ядро присылает кандидатов по занятому месту, а окно выбирает
// лучшие по выбранному режиму размера. Чистая функция — проверяется без React.
import { size_for } from "./settings";
import type { FolderSummary, SizeMode } from "./types";

/** Сколько папок показываем в таблице «Самые тяжёлые папки». */
export const TOP_FOLDERS_LIMIT = 20;

/**
 * Топ папок по выбранному размеру.
 *
 * Кандидатов больше 20 (их отбирает ядро по занятому месту), поэтому при
 * режиме «фактический» в топ попадают папки, которые по занятому месту могли
 * не войти в первую двадцатку. Сортировка детерминирована: размер убыв., при
 * равенстве — имя по возрастанию.
 */
export function rank_top_folders(
  rows: readonly FolderSummary[],
  sizeMode: SizeMode,
  limit = TOP_FOLDERS_LIMIT,
): FolderSummary[] {
  if (limit <= 0) return [];
  return [...rows]
    .sort((left, right) => {
      const diff =
        size_for(sizeMode, right.size_on_disk, right.size_logical) -
        size_for(sizeMode, left.size_on_disk, left.size_logical);
      return diff !== 0 ? diff : left.name.localeCompare(right.name, "ru");
    })
    .slice(0, limit);
}
