// Вкладка «Типы файлов»: карта расширений и таблица агрегатов по папке.
//
// Агрегацию, сортировку и раскладку считает ядро (`get_file_types`,
// `get_types_treemap`), поэтому карта и таблица показывают одни и те же группы,
// а окно не перебирает миллионы файлов. Canvas здесь отдельный, «плоский»:
// основная карта (Treemap.tsx) живёт на навигации, живых событиях и зуме,
// которых на вкладке типов нет; общими остаются раскладка в Rust и хит-тест
// `hit_test`. Так проще не сломать главную карту и не тащить в неё режимы.
import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  DEFAULT_TYPE_SORT,
  extension_label,
  next_type_sort,
  tile_extension,
  type TypeSortState,
} from "@/lib/filetypes";
import { formatCount, formatPercent, formatSize } from "@/lib/format";
import { getFileTypes, getTypesTreemap } from "@/lib/ipc";
import { hit_test } from "@/lib/treemap";
import type { FileTypeRow, TreemapRect, TypeSort } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Бюджет плиток карты типов: крупнейшие расширения; остальные — в таблице. */
const MAX_RECTS = 600;
/** Высота карты, px: компактнее основной карты — вкладка про таблицу. */
const MAP_HEIGHT = 320;
const LABEL_FONT = "'Space Grotesk Variable', 'Segoe UI', sans-serif";

/** Палитра тёмной темы: оттенки для расширений, нейтральный фон карты. */
const COLORS = {
  background: "#141014",
  tileBorder: "rgba(255, 255, 255, 0.14)",
  hoverFill: "rgba(255, 255, 255, 0.16)",
  hoverStroke: "#efd8ea",
  selectedStroke: "#f7f2f7",
  label: "rgba(255, 255, 255, 0.92)",
  labelMuted: "rgba(255, 255, 255, 0.55)",
};

/** Оттенки плиток: стабильный цвет типа по имени расширения. */
const TYPE_HUES = [322, 285, 262, 210, 176, 148, 96, 48, 24, 2];

const EMPTY_ROWS: FileTypeRow[] = [];
const EMPTY_RECTS: TreemapRect[] = [];

/** Оттенок расширения: одинаковое имя всегда одного цвета. */
function type_hue(name: string): number {
  let hash = 0;
  for (const symbol of name) {
    hash = (hash * 31 + symbol.charCodeAt(0)) >>> 0;
  }
  return TYPE_HUES[hash % TYPE_HUES.length];
}

/** Обрезать текст по ширине плитки, добавив многоточие. */
function fit_text(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number,
): string {
  if (maxWidth <= 0) return "";
  if (ctx.measureText(text).width <= maxWidth) return text;
  let end = text.length - 1;
  while (end > 0 && ctx.measureText(`${text.slice(0, end)}…`).width > maxWidth) {
    end -= 1;
  }
  return end > 0 ? `${text.slice(0, end)}…` : "";
}

/** Перерисовать плоскую карту типов: заливки, подписи, подсветка. */
function draw_type_map(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  rects: TreemapRect[],
  selected: string | null,
  hovered: TreemapRect | null,
  folderSize: number,
): void {
  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = COLORS.background;
  ctx.fillRect(0, 0, width, height);

  for (const rect of rects) {
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (!(w > 0) || !(h > 0)) continue;
    const share = folderSize > 0 ? rect.size / folderSize : 0;
    const step = share >= 0.25 ? 8 : share >= 0.08 ? 4 : share >= 0.02 ? 0 : -4;
    ctx.fillStyle = `hsl(${type_hue(rect.name)} 38% ${32 + step}%)`;
    ctx.fillRect(x, y, w, h);
  }

  // Границы — отдельным проходом: 1 CSS px внутри плитки.
  ctx.strokeStyle = COLORS.tileBorder;
  ctx.lineWidth = 1;
  for (const rect of rects) {
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (w < 1 || h < 1) continue;
    ctx.strokeRect(x + 0.5, y + 0.5, Math.max(0.5, w - 1), Math.max(0.5, h - 1));
  }

  // Подписи: имя расширения, у крупных плиток — ещё и размер.
  ctx.textBaseline = "top";
  for (const rect of rects) {
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (w < 48 || h < 20) continue;
    const fontSize = w >= 120 && h >= 48 ? 13 : 11;
    ctx.font = `${fontSize}px ${LABEL_FONT}`;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x + 3, y + 2, Math.max(0, w - 6), Math.max(0, h - 4));
    ctx.clip();
    ctx.fillStyle = COLORS.label;
    ctx.fillText(fit_text(ctx, extension_label(tile_extension(rect.name)), w - 10), x + 5, y + 4);
    if (h >= 42 && w >= 90) {
      ctx.font = `11px ${LABEL_FONT}`;
      ctx.fillStyle = COLORS.labelMuted;
      ctx.fillText(
        fit_text(ctx, formatSize(rect.size), w - 10),
        x + 5,
        y + 4 + fontSize + 2,
      );
    }
    ctx.restore();
  }

  // Выбранный тип — пунктирная рамка, как выделение файла на основной карте.
  if (selected !== null) {
    const rect = rects.find((candidate) => tile_extension(candidate.name) === selected);
    if (rect) {
      const x = rect.x * width;
      const y = rect.y * height;
      const w = rect.w * width;
      const h = rect.h * height;
      if (w >= 1 && h >= 1) {
        ctx.save();
        ctx.setLineDash([5, 3]);
        ctx.strokeStyle = COLORS.selectedStroke;
        ctx.lineWidth = 2;
        ctx.strokeRect(x + 1, y + 1, Math.max(0.5, w - 2), Math.max(0.5, h - 2));
        ctx.restore();
      }
    }
  }

  // Плитка под курсором — поверх остальных.
  if (hovered) {
    const x = hovered.x * width;
    const y = hovered.y * height;
    const w = hovered.w * width;
    const h = hovered.h * height;
    if (w > 0 && h > 0) {
      ctx.fillStyle = COLORS.hoverFill;
      ctx.fillRect(x, y, w, h);
      ctx.strokeStyle = COLORS.hoverStroke;
      ctx.lineWidth = 2;
      ctx.strokeRect(x + 1, y + 1, Math.max(0.5, w - 2), Math.max(0.5, h - 2));
    }
  }
}

/** Колонки таблицы: расширение, файлы, размер, на диске, доля. */
interface ColumnDef {
  key: TypeSort | "percent";
  label: string;
  /** Ширина колонки в фиксированной разметке. */
  width: string;
  numeric: boolean;
  sortable: boolean;
}

const COLUMNS: readonly ColumnDef[] = [
  { key: "name", label: "Расширение", width: "auto", numeric: false, sortable: true },
  { key: "count", label: "Файлов", width: "92px", numeric: true, sortable: true },
  { key: "size_logical", label: "Размер", width: "112px", numeric: true, sortable: true },
  { key: "size_on_disk", label: "На диске", width: "112px", numeric: true, sortable: true },
  { key: "percent", label: "%", width: "76px", numeric: true, sortable: false },
];

export interface FileTypesPanelProps {
  /** Папка, чьи типы показываем: 0 — корень сканирования. */
  folderId: number;
  /** Идёт сканирование: индекс ядра ещё не готов, запросы не уходят. */
  scanning: boolean;
  /** Номер скана: входит в ключ кэша, чтобы новый скан не показывал старые типы. */
  scanRun: number;
}

/**
 * Карта расширений и таблица типов открытой папки.
 *
 * Клик по плитке выбирает тип: строка подсвечивается и подкручивается в
 * видимую область таблицы. Клик по строке делает то же; повторный клик снимает
 * выбор. Сортировка — клик по заголовку: повторный разворачивает порядок.
 */
export default function FileTypesPanel({
  folderId,
  scanning,
  scanRun,
}: FileTypesPanelProps) {
  const [sortState, setSortState] = useState<TypeSortState>(DEFAULT_TYPE_SORT);
  const [selected, setSelected] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const selectedRowRef = useRef<HTMLTableRowElement | null>(null);
  const tableScrollRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [hover, setHover] = useState<{ x: number; y: number; nx: number; ny: number } | null>(null);

  // Таблица: строки уже отсортированы ядром. Сортировка в ключе кэша — смена
  // колонки берёт готовый ответ или пересчитывает его в ядре.
  const typesQuery = useQuery({
    queryKey: ["file_types", scanRun, folderId, sortState.sort, sortState.desc],
    queryFn: () => getFileTypes(folderId, sortState.sort, sortState.desc),
    enabled: !scanning,
    staleTime: Number.POSITIVE_INFINITY,
  });

  // Карта: бюджет плиток и раскладка — в ядре; сортировка таблицы на неё не
  // влияет, поэтому ключ без `sort`.
  const mapQuery = useQuery({
    queryKey: ["types_treemap", scanRun, folderId],
    queryFn: () => getTypesTreemap(folderId, MAX_RECTS),
    enabled: !scanning,
    staleTime: Number.POSITIVE_INFINITY,
  });

  const rows = typesQuery.data ?? EMPTY_ROWS;
  const rects = useMemo(() => mapQuery.data ?? EMPTY_RECTS, [mapQuery.data]);

  const totals = useMemo(
    () =>
      rows.reduce(
        (sum, row) => ({
          count: sum.count + row.count,
          logical: sum.logical + row.size_logical,
          disk: sum.disk + row.size_on_disk,
        }),
        { count: 0, logical: 0, disk: 0 },
      ),
    [rows],
  );

  // Размер canvas в CSS-пикселях: devicePixelRatio учитывается при отрисовке.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const measure = () => {
      const bounds = canvas.getBoundingClientRect();
      setSize({ width: bounds.width, height: bounds.height });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  const hoveredRect = useMemo(
    () => (hover ? hit_test(rects, hover.nx, hover.ny) : null),
    [hover, rects],
  );

  // Отрисовка: canvas масштабируется под devicePixelRatio.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || size.width <= 0 || size.height <= 0) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const ratio = window.devicePixelRatio || 1;
    const pixelWidth = Math.max(1, Math.round(size.width * ratio));
    const pixelHeight = Math.max(1, Math.round(size.height * ratio));
    if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
    if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    draw_type_map(
      ctx,
      size.width,
      size.height,
      rects,
      selected,
      hoveredRect,
      totals.disk,
    );
  }, [size, rects, selected, hoveredRect, totals.disk]);

  // Выбранная строка могла уехать за пределы прокрутки таблицы — показываем её,
  // прокручивая только контейнер таблицы. `scrollIntoView` здесь не годится:
  // он двигает всю страницу, и карта типов уезжает с экрана.
  useEffect(() => {
    const row = selectedRowRef.current;
    const container = tableScrollRef.current;
    if (!row || !container) return;
    const rowRect = row.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    if (rowRect.top < containerRect.top) {
      container.scrollTop -= containerRect.top - rowRect.top + 4;
    } else if (rowRect.bottom > containerRect.bottom) {
      container.scrollTop += rowRect.bottom - containerRect.bottom + 4;
    }
  }, [selected]);

  const toggleSort = (column: TypeSort) => {
    setSortState((current) => next_type_sort(current, column));
  };

  const toggleSelected = (extension: string) => {
    setSelected((current) => (current === extension ? null : extension));
  };

  const handlePointerMove = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const x = event.clientX - bounds.left;
    const y = event.clientY - bounds.top;
    setHover({ x, y, nx: x / bounds.width, ny: y / bounds.height });
  };

  const handleClick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const rect = hit_test(
      rects,
      (event.clientX - bounds.left) / bounds.width,
      (event.clientY - bounds.top) / bounds.height,
    );
    if (rect) toggleSelected(tile_extension(rect.name));
  };

  const tableMessage = scanning
    ? "Типы файлов появятся после завершения сканирования."
    : typesQuery.isPending
      ? "Считаем типы файлов…"
      : typesQuery.error
        ? `Не удалось получить типы файлов: ${String(typesQuery.error)}`
        : rows.length === 0
          ? "В этой папке нет файлов."
          : null;

  const mapMessage = scanning
    ? "Карта типов появится после завершения сканирования."
    : mapQuery.isPending
      ? "Строим карту типов…"
      : mapQuery.error
        ? `Не удалось построить карту типов: ${String(mapQuery.error)}`
        : rects.length === 0
          ? "Нет данных для карты."
          : null;

  const hoveredRow = hoveredRect
    ? rows.find((row) => row.extension === tile_extension(hoveredRect.name))
    : undefined;
  const hoverShare =
    hoveredRect && totals.disk > 0 ? hoveredRect.size / totals.disk : 0;
  const tooltip = hoveredRect
    ? (() => {
        const tooltipWidth = 220;
        const tooltipHeight = 92;
        const left =
          hover && hover.x + 16 + tooltipWidth > size.width
            ? Math.max(4, hover.x - tooltipWidth - 16)
            : (hover?.x ?? 0) + 16;
        const top =
          hover && hover.y + 16 + tooltipHeight > size.height
            ? Math.max(4, hover.y - tooltipHeight - 16)
            : (hover?.y ?? 0) + 16;
        return { left, top };
      })()
    : null;

  return (
    <section className="flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-card p-3">
      <header className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-sm font-medium">Типы файлов</h2>
        <span className="text-[11px] text-muted-foreground">
          агрегаты рекурсивно по поддереву · клик по плитке — выбрать тип ·
          сортировка по клику
        </span>
      </header>

      <div
        className="relative overflow-hidden border border-border bg-[#141014]"
        style={{ height: MAP_HEIGHT }}
      >
        <canvas
          ref={canvasRef}
          className="block h-full w-full"
          style={{ cursor: hoveredRect ? "pointer" : "default" }}
          aria-label="Карта типов файлов: плитки-расширения"
          onPointerMove={handlePointerMove}
          onPointerLeave={() => setHover(null)}
          onClick={handleClick}
        />

        {tooltip && hoveredRect ? (
          <div
            className="pointer-events-none absolute z-10 w-56 rounded-md border border-border bg-popover/95 px-3 py-2 text-xs text-popover-foreground shadow-lg"
            style={{ left: tooltip.left, top: tooltip.top }}
          >
            <div className="font-medium">
              {extension_label(tile_extension(hoveredRect.name))}
            </div>
            <div className="mt-1 tabular-nums text-muted-foreground">
              {formatSize(hoveredRect.size)} · {formatPercent(hoverShare)} от
              папки
            </div>
            <div className="mt-0.5 text-muted-foreground">
              {hoveredRow
                ? `${formatCount(hoveredRow.count)} файлов · клик — выбрать`
                : "клик — выбрать"}
            </div>
          </div>
        ) : null}

        {mapMessage ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-6 text-center text-sm text-muted-foreground">
            {mapMessage}
          </div>
        ) : null}
      </div>

      <div
        ref={tableScrollRef}
        className="max-h-[420px] min-h-[160px] overflow-auto rounded border border-border"
      >
        <table className="w-full table-fixed border-collapse text-xs">
          <thead className="sticky top-0 z-10 bg-card">
            <tr className="border-b border-border">
              {COLUMNS.map((column) => {
                const active =
                  column.sortable && sortState.sort === column.key;
                const ariaSort = active
                  ? sortState.desc
                    ? "descending"
                    : "ascending"
                  : undefined;
                return (
                  <th
                    key={column.key}
                    scope="col"
                    aria-sort={ariaSort}
                    className={cn(
                      "h-8 px-2 align-middle font-medium whitespace-nowrap text-muted-foreground",
                      column.numeric ? "text-right" : "text-left",
                    )}
                    style={{ width: column.width }}
                  >
                    {column.sortable ? (
                      <button
                        type="button"
                        onClick={() => toggleSort(column.key as TypeSort)}
                        title={`Сортировать по «${column.label}»`}
                        className={cn(
                          "inline-flex cursor-pointer items-center gap-0.5 rounded-none outline-none hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring/50",
                          column.numeric && "flex-row-reverse",
                          active && "text-foreground",
                        )}
                      >
                        <span>{column.label}</span>
                        <span aria-hidden="true" className="w-2 text-[10px]">
                          {active ? (sortState.desc ? "↓" : "↑") : ""}
                        </span>
                      </button>
                    ) : (
                      <span>{column.label}</span>
                    )}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 ? (
              <tr>
                <td
                  colSpan={COLUMNS.length}
                  className="py-12 text-center text-muted-foreground"
                >
                  {tableMessage ?? "Нет данных."}
                </td>
              </tr>
            ) : (
              rows.map((row) => {
                const isSelected = row.extension === selected;
                return (
                  <tr
                    key={row.extension === "" ? "(no-extension)" : row.extension}
                    ref={isSelected ? selectedRowRef : undefined}
                    onClick={() => toggleSelected(row.extension)}
                    title={`Выбрать тип «${extension_label(row.extension)}»`}
                    className={cn(
                      "cursor-pointer border-b border-border/50 hover:bg-muted/40",
                      isSelected && "bg-muted/70 text-foreground",
                    )}
                  >
                    <td className="truncate px-2 py-1">
                      {extension_label(row.extension)}
                    </td>
                    <td className="px-2 py-1 text-right tabular-nums">
                      {formatCount(row.count)}
                    </td>
                    <td className="px-2 py-1 text-right tabular-nums">
                      {formatSize(row.size_logical)}
                    </td>
                    <td className="px-2 py-1 text-right tabular-nums">
                      {formatSize(row.size_on_disk)}
                    </td>
                    <td className="px-2 py-1 text-right tabular-nums">
                      {formatPercent(row.percent_of_parent)}
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <span className="tabular-nums">
          {scanning || typesQuery.isPending
            ? "Итого по типам: —"
            : `Итого по типам: ${formatCount(totals.count)} файлов`}
        </span>
        <span className="tabular-nums">
          {scanning || typesQuery.isPending
            ? ""
            : `${formatSize(totals.logical)} · ${formatSize(totals.disk)} на диске`}
        </span>
      </div>
    </section>
  );
}
