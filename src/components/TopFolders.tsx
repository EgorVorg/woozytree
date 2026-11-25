import { useMemo } from "react";

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { formatCount, formatPercent, formatSize } from "@/lib/format";
import type { FolderSummary, SizeMode } from "@/lib/types";

interface TopFoldersProps {
  rows: FolderSummary[];
  /** Настройка отображения: какой размер показывать и сортировать. */
  sizeMode: SizeMode;
  /** Итог корня в выбранном режиме — база процентов; 0 — ещё неизвестен. */
  rootSize: number;
}

/** Размер строки в выбранном режиме. */
function row_size(row: FolderSummary, sizeMode: SizeMode): number {
  return sizeMode === "on_disk" ? row.size_on_disk : row.size_logical;
}

/**
 * Таблица самых тяжёлых папок корня (обновляется по событию `scan://top`).
 *
 * Ядро присылает топ-20 по размеру на диске, а окно показывает и сортирует его
 * по выбранному в настройках размеру: переключение режима меняет цифры, порядок
 * строк и проценты согласованно с картой и таблицей.
 */
export default function TopFolders({
  rows,
  sizeMode,
  rootSize,
}: TopFoldersProps) {
  const sorted = useMemo(
    () =>
      [...rows]
        .sort((left, right) => row_size(right, sizeMode) - row_size(left, sizeMode))
        .slice(0, 20),
    [rows, sizeMode],
  );

  return (
    <section className="rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-4 pt-4 pb-2">
        <h2 className="text-sm font-medium">Самые тяжёлые папки</h2>
        <span className="text-xs text-muted-foreground">
          Топ-20 дочерних папок корня ·{" "}
          {sizeMode === "on_disk" ? "размер на диске" : "фактический размер"}
        </span>
      </header>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Имя</TableHead>
            <TableHead className="text-right">
              {sizeMode === "on_disk" ? "На диске" : "Фактический"}
            </TableHead>
            <TableHead className="text-right">Файлов</TableHead>
            <TableHead className="text-right">Папок</TableHead>
            <TableHead className="text-right">%</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sorted.length === 0 ? (
            <TableRow>
              <TableCell
                colSpan={5}
                className="py-8 text-center text-muted-foreground"
              >
                Пока нет данных — запустите сканирование.
              </TableCell>
            </TableRow>
          ) : (
            sorted.map((row) => {
              const size = row_size(row, sizeMode);
              // Пока итог скана неизвестен (живое сканирование), доля на диске
              // берётся из события, а фактическая честно показывается прочерком.
              const percent =
                rootSize > 0
                  ? formatPercent(size / rootSize)
                  : sizeMode === "on_disk"
                    ? formatPercent(row.percent_of_parent)
                    : "—";
              return (
                <TableRow key={row.id}>
                  <TableCell className="font-medium">{row.name}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatSize(size)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatCount(row.file_count)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatCount(row.folder_count)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {percent}
                  </TableCell>
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>
    </section>
  );
}
