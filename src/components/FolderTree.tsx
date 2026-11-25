// Левая панель: дерево папок текущего корня сканирования.
//
// Дети грузятся лениво: `get_children` запрашивается только для раскрытых узлов
// (TanStack Query кэширует ответ по паре «скан + папка»). Плоский список
// видимых строк виртуализирован, поэтому в C:\Windows дерево не тормозит.
import { CaretRight, CircleNotch, FolderSimple } from "@phosphor-icons/react";
import { useQueries } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";

import { files_word, folders_in_label } from "@/lib/actions";
import {
  flatten_tree,
  tree_chain,
  type TreeCrumb,
  type TreeRow,
  type TreeRoot,
} from "@/lib/files";
import { formatCount, formatSize } from "@/lib/format";
import { getChildren } from "@/lib/ipc";
import { size_for } from "@/lib/settings";
import type { FolderSummary, SizeMode } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Высота строки дерева, px: по ней виртуализатор считает позиции. */
const ROW_HEIGHT = 26;

export interface FolderTreeProps {
  /** Путь от корня до открытой папки; последний элемент — выделенный узел. */
  path: TreeCrumb[];
  /** Имя и итоги корня сканирования. */
  root: TreeRoot;
  /** Идёт сканирование: индекс ядра ещё недоступен, запросы не отправляем. */
  scanning: boolean;
  /** Номер скана: входит в ключ кэша, чтобы новый скан не показывал старые дети. */
  scanRun: number;
  /** Настройка «на диске / фактический»: дерево показывает выбранный размер. */
  sizeMode: SizeMode;
  /** Открыть папку: цепочка от корня целиком заменяет путь. */
  onNavigate: (path: TreeCrumb[]) => void;
}

/** Склонение «файл/файла/файлов» для подсказки строки. */
function filesLabel(count: number): string {
  // Правила склонения живут в одном месте (`files_word`): та же функция
  // используется в сообщении об удалении.
  return files_word(count);
}

/** Склонение для фразы «в N папках»: «в 1 папке», «в 2 папках». */
function foldersLabel(count: number): string {
  return folders_in_label(count);
}

/** Строка дерева: треугольник раскрытия, имя, размер в выбранном режиме. */
function TreeRowView({
  row,
  sizeMode,
  selected,
  onToggle,
  onSelect,
}: {
  row: TreeRow;
  sizeMode: SizeMode;
  selected: boolean;
  onToggle: (id: number) => void;
  onSelect: (row: TreeRow) => void;
}) {
  // То же правило, что у таблицы, топа, карты и типов: режим размера меняет
  // все экраны одинаково, а не только те, что запрашивают данные у ядра.
  const size = size_for(sizeMode, row.size_on_disk, row.size_logical);
  const hint = `${row.name} · ${formatSize(size)} · ${formatCount(row.file_count)} ${filesLabel(row.file_count)} в ${formatCount(row.folder_count)} ${foldersLabel(row.folder_count)}`;
  return (
    <div
      className="flex h-full items-center gap-0.5"
      style={{ paddingLeft: 4 + row.depth * 12 }}
    >
      {row.has_children ? (
        <button
          type="button"
          aria-label={row.expanded ? "Свернуть папку" : "Раскрыть папку"}
          aria-expanded={row.expanded}
          onClick={() => onToggle(row.id)}
          className="flex size-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          {row.loading ? (
            <CircleNotch size={11} className="animate-spin" />
          ) : (
            <CaretRight
              size={11}
              weight="bold"
              className={cn("transition-transform", row.expanded && "rotate-90")}
            />
          )}
        </button>
      ) : (
        <span aria-hidden="true" className="size-4 shrink-0" />
      )}

      <button
        type="button"
        onClick={() => onSelect(row)}
        title={hint}
        aria-current={selected ? "true" : undefined}
        className={cn(
          "flex h-[22px] min-w-0 flex-1 items-center gap-1.5 rounded px-1 text-left text-xs",
          selected
            ? "bg-accent font-medium text-accent-foreground"
            : "hover:bg-muted/60",
        )}
      >
        <FolderSimple
          size={13}
          weight="fill"
          className={cn(
            "shrink-0",
            selected ? "text-accent-foreground" : "text-muted-foreground",
          )}
        />
        <span className="min-w-0 flex-1 truncate">{row.name}</span>
        <span className="shrink-0 tabular-nums text-[11px] text-muted-foreground">
          {formatSize(size)}
        </span>
      </button>
    </div>
  );
}

/**
 * Дерево папок: ленивое раскрытие, выделение = открытая папка, клик меняет
 * папку карты и таблицы.
 */
export default function FolderTree({
  path,
  root,
  scanning,
  scanRun,
  sizeMode,
  onNavigate,
}: FolderTreeProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Раскрытые узлы. Корень раскрыт всегда — иначе дерево пустое.
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(
    () => new Set([0]),
  );

  const selectedId = path.length > 0 ? path[path.length - 1].id : 0;

  // Новый скан — новый индекс: идентификаторы папок в нём другие. Раскрытие
  // прошлого индекса нельзя переносить в новый, иначе дерево раскрывается в
  // чужие ветки, а запросы уходят по id, которых в новом индексе может не быть
  // (get_children ответит ошибкой). Сбрасываем всё, кроме корня.
  useEffect(() => {
    setExpanded(new Set([0]));
  }, [scanRun]);

  // Путь задаёт предков выделенной папки: раскрываем их, чтобы выделение было
  // видно и при заходе в папку с карты или из крошек.
  useEffect(() => {
    setExpanded((current) => {
      let next: Set<number> | null = null;
      for (const crumb of path) {
        if (!current.has(crumb.id)) {
          next ??= new Set(current);
          next.add(crumb.id);
        }
      }
      return next ?? current;
    });
  }, [path]);

  // Запросы детей — по одному на раскрытый узел, кэш по «скан + папка».
  const expandedIds = useMemo(
    () => [...expanded].sort((left, right) => left - right),
    [expanded],
  );
  const childQueries = useQueries({
    queries: expandedIds.map((id) => ({
      queryKey: ["children", scanRun, id],
      queryFn: () => getChildren(id),
      enabled: !scanning,
      staleTime: Number.POSITIVE_INFINITY,
    })),
  });

  const childrenByParent = useMemo(() => {
    const map = new Map<number, FolderSummary[]>();
    expandedIds.forEach((id, index) => {
      const data = childQueries[index]?.data;
      if (data) map.set(id, data);
    });
    return map;
  }, [expandedIds, childQueries]);

  const loadingIds = useMemo(() => {
    const ids = new Set<number>();
    expandedIds.forEach((id, index) => {
      if (childQueries[index]?.isLoading) ids.add(id);
    });
    return ids;
  }, [expandedIds, childQueries]);

  const loadError = childQueries.find((query) => query.error)?.error;

  const rows = useMemo(
    () => flatten_tree(root, childrenByParent, expanded, loadingIds),
    [root, childrenByParent, expanded, loadingIds],
  );

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
    getItemKey: (index) => rows[index]?.id ?? index,
  });

  // Выделенная папка всегда должна быть на виду: после раскрытия предков и
  // загрузки их детей индекс строки меняется.
  const selectedIndex = useMemo(
    () => rows.findIndex((row) => row.id === selectedId),
    [rows, selectedId],
  );
  useEffect(() => {
    if (selectedIndex < 0) return;
    rowVirtualizer.scrollToIndex(selectedIndex, { align: "auto" });
  }, [selectedIndex, rowVirtualizer]);

  const toggle = (id: number) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const select = (row: TreeRow) => {
    const chain = tree_chain(rows, row.id);
    if (chain) onNavigate(chain);
  };

  // Глубоко раскрытое дерево иначе не вернуть к обзору: сбрасываем всё, кроме
  // корня — его дети остаются видны.
  const collapseAll = () => setExpanded(new Set([0]));

  const virtualItems = rowVirtualizer.getVirtualItems();

  return (
    <section className="flex min-w-0 flex-col rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-3 pt-3 pb-2">
        <h2 className="text-sm font-medium">Папки</h2>
        <button
          type="button"
          onClick={collapseAll}
          title="Свернуть все папки, кроме корня"
          className="rounded border border-border px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          Свернуть всё
        </button>
      </header>

      <div
        ref={scrollRef}
        className="relative mx-2 mb-2 h-[300px] overflow-auto"
        role="tree"
        aria-label="Дерево папок"
      >
        <div
          style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}
        >
          {virtualItems.map((item) => {
            const row = rows[item.index];
            if (!row) return null;
            return (
              <div
                key={row.id}
                role="treeitem"
                aria-selected={row.id === selectedId}
                aria-level={row.depth + 1}
                className="absolute top-0 left-0 w-full"
                style={{
                  height: item.size,
                  transform: `translateY(${item.start}px)`,
                }}
              >
                <TreeRowView
                  row={row}
                  sizeMode={sizeMode}
                  selected={row.id === selectedId}
                  onToggle={toggle}
                  onSelect={select}
                />
              </div>
            );
          })}
        </div>

        {scanning ? (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-card/85 p-4 text-center text-xs text-muted-foreground">
            Дерево папок появится после завершения сканирования.
          </div>
        ) : null}
      </div>

      {loadError ? (
        <p className="mx-2 mb-2 text-[11px] text-destructive">
          Не удалось загрузить папки: {String(loadError)}
        </p>
      ) : (
        <p className="mx-2 mb-2 text-[11px] text-muted-foreground">
          Размер —{" "}
          {sizeMode === "on_disk" ? "занято на диске" : "фактический"} ·
          треугольник — раскрыть · клик — открыть
        </p>
      )}
    </section>
  );
}
