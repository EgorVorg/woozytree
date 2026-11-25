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
import { size_for } from "@/lib/settings";
import { rank_top_folders } from "@/lib/top";
import type { FolderSummary, SizeMode } from "@/lib/types";

interface TopFoldersProps {
  rows: FolderSummary[];
  /** Настройка отображения: какой размер показывать и сортировать. */
  sizeMode: SizeMode;
  /** Итог корня в выбранном режиме — база процентов; 0 — ещё неизвестен. */
  rootSize: number;
}

/**
 * Таблица самых тяжёлых папок корня (обновляется по событию `scan://top`).
 *
 * Ядро присылает 100 кандидатов по занятому месту, а окно выбирает из них
 * топ-20 по выбранному в настройках размеру: иначе папка с большим фактическим
 * размером, но скромным занятым могла бы не попасть в список вовсе. Цифры,
 * порядок строк и проценты согласованы с картой и таблицей.
 */
export default function TopFolders({
  rows,
  sizeMode,
  rootSize,
}: TopFoldersProps) {
  const sorted = useMemo(
    () => rank_top_folders(rows, sizeMode),
    [rows, sizeMode],
  );

  return (
    <section className="rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-4 pt-4 pb-2">
        <h2 className="text-sm font-medium">Самые тяжёлые папки</h2>
        <span className="text-xs text-muted-foreground">
          Топ-20 дочерних папок корня · кандидаты: 100 крупнейших на диске ·
          показаны по{" "}
          {sizeMode === "on_disk" ? "размеру на диске" : "фактическому размеру"}
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
              const size = size_for(
                sizeMode,
                row.size_on_disk,
                row.size_logical,
              );
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
