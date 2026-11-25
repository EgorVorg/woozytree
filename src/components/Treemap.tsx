// Карта занятости диска: canvas 2D, hover с подсказкой, клик-зум по папкам.
//
// Данные приходят двумя путями:
// 1) событие `scan://treemap` — живой срез корня сканирования (раз в секунду);
// 2) команда `get_treemap` — полная раскладка выбранной папки после скана.
//
// Правило живых обновлений: пока пользователь на корне, карту обновляет поток;
// внутри папки поток только запоминается и не перетирает открытый вид. При
// возврате на корень показывается последний срез, а открытая после завершения
// сканирования папка перезапрашивается командой.
import { useEffect, useMemo, useRef, useState } from "react";

import { formatCount, formatPercent, formatSize } from "@/lib/format";
import { getTreemap, onTreemap } from "@/lib/ipc";
import {
  analyze_rects,
  crumbs_for_tile,
  hit_test,
  is_other,
  type Crumb,
  type TreemapTile,
} from "@/lib/treemap";
import type { TreemapRect } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Бюджет раскладки: столько же, сколько у живой карты корня в Rust. */
const MAX_RECTS = 2000;

/** Палитра тёмной темы пресета: папки — оттенки акцента (mauve), файлы — нейтральные. */
const COLORS = {
  background: "#141014",
  tileBorder: "rgba(255, 255, 255, 0.14)",
  folderHue: 322,
  folderSaturation: 17,
  fileHue: 320,
  fileSaturation: 5,
  other: "#2a252a",
  hoverFill: "rgba(255, 255, 255, 0.16)",
  hoverStroke: "#efd8ea",
  selectedStroke: "#f7f2f7",
  label: "rgba(255, 255, 255, 0.92)",
  labelMuted: "rgba(255, 255, 255, 0.55)",
};

const LABEL_FONT = "'Space Grotesk Variable', 'Segoe UI', sans-serif";

export interface TreemapProps {
  /** Папка, чью карту показываем; 0 — корень сканирования. */
  folderId: number;
  /** Путь до открытой папки: карта дополняет его предками кликнутой плитки. */
  path: readonly Crumb[];
  /** Перейти по полному пути `crumbs` (как клик в дереве), а не на один уровень. */
  onNavigate: (crumbs: Crumb[]) => void;
  /** Идёт сканирование: индекс ядра ещё недоступен, ждём живые события. */
  scanning?: boolean;
  /** Скан остановлен: карта показывает частичный срез и помечается плашкой. */
  cancelled?: boolean;
}

interface CanvasSize {
  width: number;
  height: number;
}

interface HoverState {
  /** Координаты курсора в CSS-пикселях относительно canvas. */
  x: number;
  y: number;
  /** Нормированные координаты 0..1 для хит-теста. */
  nx: number;
  ny: number;
}

interface SelectedFile {
  name: string;
  size: number;
}

interface LabelChip {
  rect: TreemapRect;
  /** Положение и размер «ярлыка» в CSS-пикселях canvas. */
  x: number;
  y: number;
  width: number;
  height: number;
  /** Подпись, уже обрезанная по длине. */
  label: string;
}

/** Ключ выбранной плитки: переживает живые обновления координат. */
function tileKey(rect: TreemapRect): string {
  return `${rect.is_dir ? "d" : "f"}|${rect.name}|${rect.size}`;
}

/** Короткая подпись для ярлыка папки: не длиннее ~24 символов. */
function shortLabel(name: string): string {
  return name.length > 24 ? `${name.slice(0, 23)}…` : name;
}

/**
 * Ярлыки папок, внутри которых нарисованы дети.
 *
 * Без них вложенная карта нечитаема: имя крупной папки закрыто плитками детей,
 * и непонятно, куда нажимать, чтобы войти ровно на один уровень. Ярлык — это
 * кликабельный заголовок плитки, который всегда поверх содержимого.
 */
function chipLayout(
  tiles: TreemapTile[],
  width: number,
  height: number,
): LabelChip[] {
  if (width <= 0 || height <= 0) return [];
  // Ярлыки — только у плиток верхнего уровня текущего вида: так они не
  // наезжают друг на друга и открывают папку ровно на один уровень.
  const candidates = tiles
    .filter(
      (tile) =>
        tile.depth === 0 &&
        tile.has_children &&
        tile.rect.is_dir &&
        tile.rect.id !== 0 &&
        tile.rect.w * width >= 96 &&
        tile.rect.h * height >= 34,
    )
    .sort(
      (left, right) =>
        right.rect.w * right.rect.h - left.rect.w * left.rect.h,
    )
    .slice(0, 60);

  return candidates.map((tile) => {
    const label = shortLabel(tile.rect.name);
    const chipWidth = Math.min(
      tile.rect.w * width - 8,
      Math.max(44, label.length * 6.2 + 12),
    );
    return {
      rect: tile.rect,
      x: tile.rect.x * width + 4,
      y: tile.rect.y * height + 4,
      width: chipWidth,
      height: 17,
      label,
    };
  });
}

/** Ярлык под точкой в CSS-пикселях canvas (клик по нему — вход в папку). */
function chipAt(chips: LabelChip[], x: number, y: number): LabelChip | null {
  for (let index = chips.length - 1; index >= 0; index -= 1) {
    const chip = chips[index];
    if (
      x >= chip.x &&
      x <= chip.x + chip.width &&
      y >= chip.y &&
      y <= chip.y + chip.height
    ) {
      return chip;
    }
  }
  return null;
}

/** Ступень яркости по занимаемой площади: крупные плитки чуть светлее. */
function areaStep(share: number): number {
  if (share >= 0.25) return 8;
  if (share >= 0.08) return 4;
  if (share >= 0.02) return 0;
  return -4;
}

/** Заливка плитки: папки — оттенки акцента по глубине, файлы — нейтральные. */
function tileFill(tile: TreemapTile, totalArea: number): string {
  if (is_other(tile.rect)) return COLORS.other;
  const area = tile.rect.w * tile.rect.h;
  const share = totalArea > 0 ? Math.min(1, area / totalArea) : 0;
  if (tile.rect.is_dir) {
    const lightness = Math.min(
      52,
      30 + Math.min(12, tile.depth * 3) + areaStep(share),
    );
    return `hsl(${COLORS.folderHue} ${COLORS.folderSaturation}% ${lightness}%)`;
  }
  const lightness = Math.min(
    44,
    26 + Math.min(8, tile.depth * 2) + areaStep(share),
  );
  return `hsl(${COLORS.fileHue} ${COLORS.fileSaturation}% ${lightness}%)`;
}

/** Обрезать текст по ширине плитки, добавив многоточие. */
function fitText(
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

/** Перерисовать карту: заливки, границы 1px, подписи, подсветка, ярлыки папок. */
function drawTreemap(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  tiles: TreemapTile[],
  chips: LabelChip[],
  hovered: TreemapRect | null,
  hoveredChip: TreemapRect | null,
  selectedKey: string | null,
): void {
  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = COLORS.background;
  ctx.fillRect(0, 0, width, height);

  const totalArea = width * height;
  for (const tile of tiles) {
    const { rect } = tile;
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (!(w > 0) || !(h > 0)) continue;
    ctx.fillStyle = tileFill(tile, totalArea);
    ctx.fillRect(x, y, w, h);
  }

  // Границы рисуем отдельным проходом: 1 CSS px внутри плитки.
  ctx.strokeStyle = COLORS.tileBorder;
  ctx.lineWidth = 1;
  for (const tile of tiles) {
    const { rect } = tile;
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (w < 1 || h < 1) continue;
    ctx.strokeRect(x + 0.5, y + 0.5, Math.max(0.5, w - 1), Math.max(0.5, h - 1));
  }

  // Подписи — только на «листовых» плитках, поверх которых ничего не лежит.
  ctx.textBaseline = "top";
  for (const tile of tiles) {
    const { rect } = tile;
    const x = rect.x * width;
    const y = rect.y * height;
    const w = rect.w * width;
    const h = rect.h * height;
    if (tile.has_children || w < 64 || h < 22) continue;
    const fontSize = w >= 150 && h >= 48 ? 13 : 11;
    ctx.font = `${fontSize}px ${LABEL_FONT}`;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x + 3, y + 2, Math.max(0, w - 6), Math.max(0, h - 4));
    ctx.clip();
    ctx.fillStyle = COLORS.label;
    ctx.fillText(fitText(ctx, rect.name, w - 10), x + 5, y + 4);
    if (h >= 42 && w >= 90) {
      ctx.font = `11px ${LABEL_FONT}`;
      ctx.fillStyle = COLORS.labelMuted;
      ctx.fillText(
        fitText(ctx, formatSize(rect.size), w - 10),
        x + 5,
        y + 4 + fontSize + 2,
      );
    }
    ctx.restore();
  }

  // Подсветка выбранного файла (действия — План 3).
  if (selectedKey) {
    const selected = tiles.find((tile) => tileKey(tile.rect) === selectedKey);
    if (selected) {
      const { rect } = selected;
      const x = rect.x * width;
      const y = rect.y * height;
      const w = rect.w * width;
      const h = rect.h * height;
      if (w >= 1 && h >= 1) {
        ctx.save();
        ctx.setLineDash([5, 3]);
        ctx.strokeStyle = COLORS.selectedStroke;
        ctx.lineWidth = 2;
        ctx.strokeRect(
          x + 1,
          y + 1,
          Math.max(0.5, w - 2),
          Math.max(0.5, h - 2),
        );
        ctx.restore();
      }
    }
  }

  // Плитка под курсором — поверх всего.
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
      ctx.strokeRect(
        x + 1,
        y + 1,
        Math.max(0.5, w - 2),
        Math.max(0.5, h - 2),
      );
    }
  }

  // Ярлыки папок с детьми — последним слоем, чтобы имя всегда читалось.
  ctx.textBaseline = "middle";
  for (const chip of chips) {
    const active = hoveredChip !== null && chip.rect === hoveredChip;
    ctx.font = `11px ${LABEL_FONT}`;
    ctx.fillStyle = active ? "rgba(64, 48, 62, 0.95)" : "rgba(20, 16, 20, 0.82)";
    ctx.fillRect(chip.x, chip.y, chip.width, chip.height);
    ctx.strokeStyle = active
      ? COLORS.hoverStroke
      : "rgba(255, 255, 255, 0.22)";
    ctx.lineWidth = 1;
    ctx.strokeRect(
      chip.x + 0.5,
      chip.y + 0.5,
      Math.max(0.5, chip.width - 1),
      Math.max(0.5, chip.height - 1),
    );
    ctx.save();
    ctx.beginPath();
    ctx.rect(chip.x + 2, chip.y, Math.max(0, chip.width - 4), chip.height);
    ctx.clip();
    ctx.fillStyle = COLORS.label;
    ctx.fillText(chip.label, chip.x + 5, chip.y + chip.height / 2 + 0.5);
    ctx.restore();
  }
}

/** Подпись типа плитки для всплывающей подсказки. */
function tileKind(rect: TreemapRect): string {
  if (is_other(rect)) return "прочее — мелкие файлы сводной плиткой";
  return rect.is_dir ? "папка" : "файл";
}

/**
 * Canvas-карта папки: плитки в порядке списка (родители раньше детей),
 * hover-подсказка, зум кликом по папке, подсветка выбранного файла.
 */
export default function Treemap({
  folderId,
  path,
  onNavigate,
  scanning = false,
  cancelled = false,
}: TreemapProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [size, setSize] = useState<CanvasSize>({ width: 0, height: 0 });
  const [rects, setRects] = useState<TreemapRect[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [hover, setHover] = useState<HoverState | null>(null);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [selectedFile, setSelectedFile] = useState<SelectedFile | null>(null);

  // Последний живой срез корня и текущая папка — для подписки без переподписок.
  const liveRef = useRef<TreemapRect[] | null>(null);
  const folderIdRef = useRef(folderId);
  folderIdRef.current = folderId;
  // Данные на момент клика: клик всегда применяется к тому, что нарисовано.
  const rectsRef = useRef(rects);
  rectsRef.current = rects;

  const tiles = useMemo(() => analyze_rects(rects), [rects]);
  const chips = useMemo(
    () => chipLayout(tiles, size.width, size.height),
    [tiles, size],
  );
  const hoveredRect = useMemo(
    () => (hover ? hit_test(rects, hover.nx, hover.ny) : null),
    [hover, rects],
  );
  const hoveredChip = useMemo(
    () => (hover ? chipAt(chips, hover.x, hover.y) : null),
    [hover, chips],
  );
  const hoveredTile = useMemo(
    () =>
      hoveredRect
        ? (tiles.find((tile) => tile.rect === hoveredRect) ?? null)
        : null,
    [hoveredRect, tiles],
  );
  const hoveredChipTile = useMemo(
    () =>
      hoveredChip
        ? (tiles.find((tile) => tile.rect === hoveredChip.rect) ?? null)
        : null,
    [hoveredChip, tiles],
  );
  // Ярлык папки важнее плитки под ним: он нарисован поверх и кликабелен.
  const tooltipTile = hoveredChipTile ?? hoveredTile;

  // Размер canvas в CSS-пикселях: devicePixelRatio учитывается при отрисовке.
  // Меряем сам canvas, а не рамку: координаты клика и ярлыков должны совпадать.
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

  // Подписка на живой поток корня: внутри папки только запоминаем срез.
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void onTreemap((next) => {
      liveRef.current = next;
      if (folderIdRef.current === 0) {
        setRects(next);
        setError(null);
        setNotice(null);
        setLoading(false);
      }
    }).then((stop) => {
      if (alive) {
        unlisten = stop;
      } else {
        stop();
      }
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // Данные для текущей папки.
  useEffect(() => {
    let alive = true;
    setSelectedKey(null);
    setSelectedFile(null);

    // Корень во время скана (и после него) показывает последний живой срез.
    if (folderId === 0 && liveRef.current) {
      setRects(liveRef.current);
      setLoading(false);
      setError(null);
      setNotice(null);
      return () => {
        alive = false;
      };
    }

    if (scanning) {
      // Индекс ядра доступен только после завершения скана: живую папку
      // раскрыть нечем, честно говорим об этом и ждём поток корня.
      setRects([]);
      setLoading(false);
      setError(null);
      setNotice(
        folderId === 0
          ? "Ждём первые данные сканирования…"
          : "Карта папки появится после завершения сканирования.",
      );
      return () => {
        alive = false;
      };
    }

    setRects([]);
    setLoading(true);
    setError(null);
    setNotice(null);
    void getTreemap(folderId, MAX_RECTS)
      .then((next) => {
        if (!alive) return;
        setRects(next);
        setLoading(false);
      })
      .catch((err: unknown) => {
        if (!alive) return;
        setError(`Не удалось построить карту: ${String(err)}`);
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [folderId, scanning]);

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
    drawTreemap(
      ctx,
      size.width,
      size.height,
      tiles,
      chips,
      hoveredChip ? null : hoveredRect,
      hoveredChip?.rect ?? null,
      selectedKey,
    );
  }, [size, tiles, chips, hoveredRect, hoveredChip, selectedKey]);

  const handlePointerMove = (
    event: React.PointerEvent<HTMLCanvasElement>,
  ) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const x = event.clientX - bounds.left;
    const y = event.clientY - bounds.top;
    setHover({ x, y, nx: x / bounds.width, ny: y / bounds.height });
  };

  const handleClick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const x = event.clientX - bounds.left;
    const y = event.clientY - bounds.top;
    // Ярлык папки нарисован поверх детей: он и открывает папку на один уровень.
    const chip = chipAt(chips, x, y);
    if (chip) {
      const crumbs = crumbs_for_tile(rectsRef.current, chip.rect, path);
      if (crumbs.length > 0) {
        setSelectedKey(null);
        setSelectedFile(null);
        onNavigate(crumbs);
      }
      return;
    }
    const nx = x / bounds.width;
    const ny = y / bounds.height;
    // Хит-тест по данным на момент клика (rectsRef обновляется на каждый рендер).
    const rect = hit_test(rectsRef.current, nx, ny);
    if (!rect) {
      setSelectedKey(null);
      setSelectedFile(null);
      return;
    }
    if (rect.is_dir && rect.id !== 0) {
      const crumbs = crumbs_for_tile(rectsRef.current, rect, path);
      if (crumbs.length > 0) {
        setSelectedKey(null);
        setSelectedFile(null);
        onNavigate(crumbs);
      }
      return;
    }
    if (is_other(rect)) {
      // Сводная плитка «прочее» — не папка и не файл: действий не выполняем.
      return;
    }
    setSelectedKey(tileKey(rect));
    setSelectedFile({ name: rect.name, size: rect.size });
  };

  const cursor =
    hoveredChip ||
    (hoveredRect && hoveredRect.is_dir && hoveredRect.id !== 0)
      ? "pointer"
      : "default";

  const message = loading
    ? "Строим карту…"
    : error
      ? error
      : notice
        ? notice
        : rects.length === 0
          ? "Нет данных для этой папки."
          : null;

  // Подсказка прижимается к краям canvas, чтобы не уезжать за карту.
  const tooltip = tooltipTile
    ? (() => {
        const tooltipWidth = 248;
        const tooltipHeight = 96;
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
    <section className="rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-4 pt-4 pb-2">
        <h2 className="text-sm font-medium">Карта занятости</h2>
        <span className="text-xs text-muted-foreground">
          {rects.length > 0 ? `${formatCount(tiles.length)} плиток · ` : ""}
          клик по папке или ярлыку — войти · Alt+↑ — вверх
        </span>
      </header>

      <div
        className={cn(
          "relative mx-4 mb-3 h-[440px] overflow-hidden border bg-[#141014]",
          cancelled ? "border-dashed border-destructive/60" : "border-border",
        )}
      >
        <canvas
          ref={canvasRef}
          className="block h-full w-full"
          style={{ cursor }}
          aria-label="Карта занятости: плитки папок и файлов"
          onPointerMove={handlePointerMove}
          onPointerLeave={() => setHover(null)}
          onClick={handleClick}
        />

        {cancelled ? (
          <div
            role="status"
            className="pointer-events-none absolute top-2 right-2 z-10 rounded-md border border-destructive/50 bg-[#141014]/90 px-2 py-1 text-[11px] text-destructive"
          >
            Данные частичные
          </div>
        ) : null}

        {tooltip && tooltipTile ? (
          <div
            className="pointer-events-none absolute z-10 w-60 rounded-md border border-border bg-popover/95 px-3 py-2 text-xs text-popover-foreground shadow-lg"
            style={{ left: tooltip.left, top: tooltip.top }}
          >
            <div className="truncate font-medium" title={tooltipTile.rect.name}>
              {tooltipTile.rect.name}
            </div>
            <div className="mt-1 tabular-nums text-muted-foreground">
              {formatSize(tooltipTile.rect.size)} ·{" "}
              {formatPercent(tooltipTile.percent_of_parent)} от родителя
            </div>
            <div className="mt-0.5 text-muted-foreground">
              {hoveredChip
                ? "папка · клик по ярлыку — открыть"
                : tileKind(tooltipTile.rect)}
              {tooltipTile.parent_name
                ? ` · в папке ${tooltipTile.parent_name}`
                : ""}
            </div>
          </div>
        ) : null}

        {message ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-6 text-center text-sm text-muted-foreground">
            {message}
          </div>
        ) : null}
      </div>

      {selectedFile ? (
        <p
          role="status"
          className="mx-4 mb-4 flex flex-wrap items-baseline gap-2 text-xs text-muted-foreground"
        >
          <span>Выбран файл:</span>
          <span className="font-medium text-foreground">
            {selectedFile.name}
          </span>
          <span className="tabular-nums">{formatSize(selectedFile.size)}</span>
          <span className="text-muted-foreground/70">
            (действия с файлами — в следующем плане)
          </span>
        </p>
      ) : (
        <p className="mx-4 mb-4 text-xs text-muted-foreground">
          Наведите курсор на плитку — имя, размер и доля от родителя. Клик по
          файлу подсвечивает его, клик по папке или её ярлыку открывает карту
          папки.
        </p>
      )}
    </section>
  );
}
