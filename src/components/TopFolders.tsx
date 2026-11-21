import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { formatCount, formatPercent, formatSize } from "@/lib/format";
import type { FolderSummary } from "@/lib/types";

interface TopFoldersProps {
  rows: FolderSummary[];
}

/** Таблица самых тяжёлых папок корня (обновляется по событию `scan://top`). */
export default function TopFolders({ rows }: TopFoldersProps) {
  return (
    <section className="rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-4 pt-4 pb-2">
        <h2 className="text-sm font-medium">Самые тяжёлые папки</h2>
        <span className="text-xs text-muted-foreground">
          Топ-20 дочерних папок корня
        </span>
      </header>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Имя</TableHead>
            <TableHead className="text-right">Размер</TableHead>
            <TableHead className="text-right">Файлов</TableHead>
            <TableHead className="text-right">Папок</TableHead>
            <TableHead className="text-right">%</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.length === 0 ? (
            <TableRow>
              <TableCell
                colSpan={5}
                className="py-8 text-center text-muted-foreground"
              >
                Пока нет данных — запустите сканирование.
              </TableCell>
            </TableRow>
          ) : (
            rows.map((row) => (
              <TableRow key={row.id}>
                <TableCell className="font-medium">{row.name}</TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatSize(row.size_on_disk)}
                </TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatCount(row.file_count)}
                </TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatCount(row.folder_count)}
                </TableCell>
                <TableCell className="text-right tabular-nums">
                  {formatPercent(row.percent_of_parent)}
                </TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </section>
  );
}
