import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { formatCount, formatSeconds, formatSize } from "@/lib/format";
import type { ScanProgress } from "@/lib/types";

/** Звено пути от корня сканирования до открытой папки. */
export interface Crumb {
  id: number;
  name: string;
}

interface BreadcrumbsProps {
  /** Путь от корня: первый элемент — корень сканирования. */
  path: Crumb[];
  /** Перейти к звену пути (щелчок по крошке). */
  onNavigate: (id: number) => void;
  /** Подняться на уровень вверх; на корне кнопка отключена. */
  onUp: () => void;
}

/** Хлебные крошки «C:\ > Users > …» и кнопка «Вверх» (дублируется Alt+↑). */
export function Breadcrumbs({ path, onNavigate, onUp }: BreadcrumbsProps) {
  const atRoot = path.length <= 1;
  return (
    <nav
      aria-label="Путь по папкам"
      className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-4 py-2 text-sm"
    >
      <Button
        variant="outline"
        size="xs"
        onClick={onUp}
        disabled={atRoot}
        title="На уровень вверх (Alt+↑)"
      >
        Вверх
      </Button>

      <ol className="flex min-w-0 flex-wrap items-center gap-1">
        {path.map((crumb, index) => {
          const last = index === path.length - 1;
          return (
            <li key={crumb.id} className="flex min-w-0 items-center gap-1">
              {index > 0 ? (
                <span aria-hidden="true" className="text-muted-foreground/50">
                  &gt;
                </span>
              ) : null}
              <button
                type="button"
                onClick={() => onNavigate(crumb.id)}
                disabled={last}
                aria-current={last ? "location" : undefined}
                title={crumb.name}
                className={
                  last
                    ? "max-w-[24rem] truncate font-medium text-foreground"
                    : "max-w-[24rem] truncate text-muted-foreground hover:text-foreground"
                }
              >
                {crumb.name}
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

interface ScanStatusProps {
  progress: ScanProgress | null;
  /** Заполненность прогресс-бара, 0..100. */
  percent: number;
  scanning: boolean;
  /** Нажали «Остановить», но ядро ещё сворачивает работу. */
  stopping: boolean;
  /** Скан остановлен: цифры ниже — частичный срез, а не полный итог. */
  cancelled: boolean;
  /** Скан завершился ошибкой: прогресс и итоги очищены, показываем «Ошибка». */
  failed: boolean;
  /** Сколько записей не попало в индекс (0 — пропусков не было). */
  skipped: number;
}

/** Прогресс-бар и счётчики: файлы, папки, занятое место, время. */
export default function ScanStatus({
  progress,
  percent,
  scanning,
  stopping,
  cancelled,
  failed,
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
      : failed
        ? "Ошибка"
        : cancelled
          ? "Остановлено"
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

      {cancelled ? (
        <p
          role="status"
          className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          Данные частичные: сканирование остановлено до завершения.
        </p>
      ) : null}

      {failed ? (
        <p
          role="alert"
          className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          Сканирование завершилось ошибкой: данные не собраны.
        </p>
      ) : null}
    </section>
  );
}
