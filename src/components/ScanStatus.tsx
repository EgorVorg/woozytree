import { Progress } from "@/components/ui/progress";
import { formatCount, formatSeconds, formatSize } from "@/lib/format";
import type { ScanProgress } from "@/lib/types";

interface ScanStatusProps {
  progress: ScanProgress | null;
  /** Заполненность прогресс-бара, 0..100. */
  percent: number;
  scanning: boolean;
  /** Нажали «Остановить», но ядро ещё сворачивает работу. */
  stopping: boolean;
  /** Сколько записей не попало в индекс (0 — пропусков не было). */
  skipped: number;
}

/** Прогресс-бар и счётчики: файлы, папки, занятое место, время. */
export default function ScanStatus({
  progress,
  percent,
  scanning,
  stopping,
  skipped,
}: ScanStatusProps) {
  const files = progress?.files ?? 0;
  const folders = progress?.folders ?? 0;
  const size = progress?.size_on_disk ?? 0;
  const elapsed = progress?.elapsed_ms ?? 0;

  const state = stopping
    ? "Остановка…"
    : scanning
      ? "Сканирование…"
      : progress
        ? "Готово"
        : "Ожидание";

  return (
    <section className="space-y-3 rounded-lg border border-border bg-card p-4">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>{state}</span>
        <span className="tabular-nums">{Math.round(percent)} %</span>
      </div>

      <Progress value={percent} />

      <p className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
        <span>
          Файлов:{" "}
          <span className="tabular-nums text-foreground">
            {formatCount(files)}
          </span>
        </span>
        <span>
          Папок:{" "}
          <span className="tabular-nums text-foreground">
            {formatCount(folders)}
          </span>
        </span>
        <span>
          Занято:{" "}
          <span className="tabular-nums text-foreground">
            {formatSize(size)}
          </span>
        </span>
        <span>
          Время:{" "}
          <span className="tabular-nums text-foreground">
            {formatSeconds(elapsed)}
          </span>
        </span>
      </p>

      {skipped > 0 ? (
        <p className="text-xs text-muted-foreground">
          Пропущено записей: {formatCount(skipped)}
        </p>
      ) : null}
    </section>
  );
}
