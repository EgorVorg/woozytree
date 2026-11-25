// Правая панель: таблица файлов открытой папки.
//
// TanStack Table — колонки, строки и сортировка (сортирует ядро: ручной режим),
// TanStack Virtual — виртуальный список (в C:\Windows десятки тысяч строк),
// TanStack Query — страницы `get_files` с кэшем по папке и сортировке.
import { LinkSimple } from "@phosphor-icons/react";
import {
  createColumnHelper,
  rowSortingFeature,
  tableFeatures,
  useTable,
  type SortingState,
} from "@tanstack/react-table";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";

import FileActionsMenu, {
  type FileActionTarget,
} from "@/components/FileActionsMenu";
import { is_file_sort } from "@/lib/files";
import {
  formatCount,
  formatDateTime,
  formatPercent,
  formatSize,
} from "@/lib/format";
import { getFiles } from "@/lib/ipc";
import type { DeleteReport, FileRow, FileSort } from "@/lib/types";
import { cn } from "@/lib/utils";

/**
 * Размер страницы: столько строк просим у ядра за один раз.
 *
 * 2000, а не 10 000: страница — это один JSON-ответ IPC (~0.36 МБ против
 * 1.8 МБ), поэтому смена сортировки и вход в папку заметно быстрее, а
 * «догрузить ещё» срабатывает раньше.
 */
const PAGE_SIZE = 2_000;
/** Высота строки, px: виртуализатор считает позиции по ней. */
const ROW_HEIGHT = 26;
/** За сколько строк до конца начинаем догрузку следующей страницы. */
const LOAD_AHEAD = 20;

const features = tableFeatures({ rowSortingFeature });
const columnHelper = createColumnHelper<typeof features, FileRow>();
const emptyRows: FileRow[] = [];

/** Числовая колонка: размер в байтах с одним знаком после запятой. */
function sizeCell(value: number) {
  return <span className="tabular-nums">{formatSize(value)}</span>;
}

const columns = columnHelper.columns([
  columnHelper.accessor("name", {
    header: "Имя",
    sortDescFirst: false,
    cell: (info) => {
      const row = info.row.original;
      return (
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="truncate" title={row.name}>
            {row.name}
          </span>
          {row.hard_linked ? (
            <span
              className="inline-flex shrink-0 items-center gap-0.5 rounded border border-border bg-muted/60 px-1 py-0 text-[10px] leading-4 font-normal text-muted-foreground"
              title="жёсткая ссылка: размер не входит в итоги"
            >
              <LinkSimple size={10} weight="bold" />
              жёсткая ссылка
            </span>
          ) : null}
        </span>
      );
    },
  }),
  columnHelper.accessor("size_logical", {
    header: "Размер",
    sortDescFirst: true,
    cell: (info) => sizeCell(info.getValue()),
  }),
  columnHelper.accessor("size_on_disk", {
    header: "На диске",
    sortDescFirst: true,
    cell: (info) => sizeCell(info.getValue()),
  }),
  columnHelper.display({
    id: "percent_of_parent",
    header: "%",
    enableSorting: false,
    cell: (info) => (
      <span className="tabular-nums">
        {formatPercent(info.row.original.percent_of_parent)}
      </span>
    ),
  }),
  columnHelper.accessor("modified", {
    header: "Изменён",
    sortDescFirst: true,
    cell: (info) => (
      <span className="tabular-nums">{formatDateTime(info.getValue())}</span>
    ),
  }),
]);

/** Ширины и выравнивание колонок: таблица фиксированной разметки (table-fixed). */
const COLUMN_CLASS: Record<string, string> = {
  name: "",
  size_logical: "w-[118px] text-right",
  size_on_disk: "w-[118px] text-right",
  percent_of_parent: "w-[76px] text-right",
  modified: "w-[144px] text-right",
};

/** Стрелка сортировки у заголовка: показывает направление текущей колонки. */
function SortMark({ direction }: { direction: false | "asc" | "desc" }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-block w-2.5 text-[10px] leading-none",
        direction ? "text-foreground" : "text-muted-foreground/40",
      )}
    >
      {direction === "asc" ? "▲" : direction === "desc" ? "▼" : "↕"}
    </span>
  );
}

export interface FileTableProps {
  /** Папка, чьи файлы показываем; 0 — корень сканирования. */
  folderId: number;
  /** Идёт сканирование: индекс ядра ещё недоступен. */
  scanning: boolean;
  /** Номер скана: входит в ключ кэша, чтобы новый скан не показывал старые файлы. */
  scanRun: number;
  /** Файл удалён: родитель показывает плашку «данные устарели». */
  onFileDeleted: (report: DeleteReport) => void;
  /** Ошибка действия с файлом — текст для красной плашки. */
  onActionError: (message: string) => void;
  /** Нейтральное сообщение (путь скопирован). */
  onNotice: (message: string) => void;
}

/**
 * Таблица файлов: сортировка по клику на заголовок (перезапрос `get_files`),
 * догрузка страниц при прокрутке к концу, пометка жёстких ссылок.
 */
export default function FileTable({
  folderId,
  scanning,
  scanRun,
  onFileDeleted,
  onActionError,
  onNotice,
}: FileTableProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [sorting, setSorting] = useState<SortingState>([
    { id: "size_on_disk", desc: true },
  ]);
  // Файл под правым кликом: меню действий открывается им же.
  const [menuTarget, setMenuTarget] = useState<FileActionTarget | null>(null);

  const first = sorting[0];
  const sort: FileSort = first && is_file_sort(first.id) ? first.id : "size_on_disk";
  const desc = first ? first.desc : true;

  const query = useInfiniteQuery({
    queryKey: ["files", scanRun, folderId, sort, desc],
    queryFn: ({ pageParam }) =>
      getFiles(folderId, sort, desc, pageParam, PAGE_SIZE),
    initialPageParam: 0,
    getNextPageParam: (lastPage, allPages) => {
      const loaded = allPages.reduce((sum, page) => sum + page.rows.length, 0);
      if (lastPage.rows.length === 0) return undefined;
      return loaded < lastPage.total ? loaded : undefined;
    },
    enabled: !scanning,
    staleTime: Number.POSITIVE_INFINITY,
    // Строки прошлого запроса показываем только при смене сортировки в той же
    // папке: при переходе в другую папку или на новый скан это были бы чужие
    // файлы, выданные за текущие.
    placeholderData: (previousData, previousQuery) => {
      if (!previousQuery) return undefined;
      const [tag, run, folder] = previousQuery.queryKey as [
        string,
        number,
        number,
      ];
      if (tag !== "files" || run !== scanRun || folder !== folderId) {
        return undefined;
      }
      return previousData;
    },
  });

  // Все загруженные страницы одним списком: таблица и виртуализатор работают
  // с ним, а не с отдельными страницами.
  const rows = useMemo(
    () => query.data?.pages.flatMap((page) => page.rows) ?? emptyRows,
    [query.data],
  );
  const total = query.data?.pages[0]?.total ?? 0;

  const table = useTable({
    features,
    columns,
    data: rows,
    state: { sorting },
    onSortingChange: setSorting,
    manualSorting: true,
    enableSortingRemoval: false,
  });

  // «Сортировать пришлось ядру»: строки уже пришли в нужном порядке.
  const tableRows = table.getRowModel().rows;
  const rowVirtualizer = useVirtualizer({
    count: tableRows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    getItemKey: (index) => tableRows[index]?.id ?? index,
  });

  // Догрузка следующей страницы: подходим к концу загруженного списка.
  const virtualItems = rowVirtualizer.getVirtualItems();
  const lastVirtual =
    virtualItems.length > 0 ? virtualItems[virtualItems.length - 1] : undefined;
  const lastVirtualIndex = lastVirtual?.index ?? 0;
  const { fetchNextPage, hasNextPage, isFetchingNextPage } = query;
  useEffect(() => {
    if (!hasNextPage || isFetchingNextPage) return;
    if (lastVirtualIndex >= rows.length - LOAD_AHEAD) {
      void fetchNextPage();
    }
  }, [
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
    lastVirtualIndex,
    rows.length,
  ]);

  // Новая папка или сортировка — читаем список сверху.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [scanRun, folderId, sort, desc]);

  const padTop = virtualItems[0]?.start ?? 0;
  const padBottom = Math.max(
    0,
    rowVirtualizer.getTotalSize() - (lastVirtual?.end ?? 0),
  );

  const firstPending = query.isPending;
  const isEmpty = !firstPending && total === 0;
  // Строки на экране — прошлый порядок: идёт перезапрос после смены колонки.
  // Приглушаем их и честно пишем «Сортируем…», чтобы старый порядок не
  // выдавался за новый.
  const reordering = query.isPlaceholderData;
  const message = scanning
    ? "Таблица файлов появится после завершения сканирования."
    : firstPending
      ? "Загружаем файлы…"
      : query.error
        ? `Не удалось загрузить файлы: ${String(query.error)}`
        : isEmpty
          ? "Папка пуста"
          : null;

  return (
    <section className="flex min-w-0 flex-col rounded-lg border border-border bg-card">
      <header className="flex flex-wrap items-baseline justify-between gap-2 px-3 pt-3 pb-2">
        <h2 className="text-sm font-medium">Файлы</h2>
        <span className="text-[11px] text-muted-foreground">
          клик по заголовку — сортировка · прокрутка вниз — догрузка · правый
          клик — действия
        </span>
      </header>

      <div
        ref={scrollRef}
        className="mx-2 max-h-[300px] overflow-auto rounded border border-border"
      >
        <table className="w-full table-fixed border-collapse text-xs">
          <thead className="sticky top-0 z-10 bg-card">
            {table.getHeaderGroups().map((headerGroup) => (
              <tr key={headerGroup.id} className="border-b border-border">
                {headerGroup.headers.map((header) => {
                  const canSort = header.column.getCanSort();
                  const sorted = header.column.getIsSorted();
                  return (
                    <th
                      key={header.id}
                      className={cn(
                        "h-8 px-2 text-left align-middle font-medium whitespace-nowrap text-muted-foreground",
                        COLUMN_CLASS[header.column.id] ?? "",
                      )}
                    >
                      {header.isPlaceholder ? null : canSort ? (
                        <button
                          type="button"
                          onClick={header.column.getToggleSortingHandler()}
                          className="inline-flex items-center gap-1 hover:text-foreground"
                          title="Сортировать по этой колонке"
                        >
                          <table.FlexRender header={header} />
                          <SortMark direction={sorted} />
                        </button>
                      ) : (
                        <table.FlexRender header={header} />
                      )}
                    </th>
                  );
                })}
              </tr>
            ))}
          </thead>
          <tbody className={cn(reordering && "opacity-60")}>
            {padTop > 0 ? (
              <tr aria-hidden="true">
                <td colSpan={columns.length} style={{ height: padTop, padding: 0 }} />
              </tr>
            ) : null}

            {tableRows.length === 0 ? (
              <tr>
                <td
                  colSpan={columns.length}
                  className="py-10 text-center text-muted-foreground"
                >
                  {message ?? "Папка пуста"}
                </td>
              </tr>
            ) : (
              virtualItems.map((item) => {
                const row = tableRows[item.index];
                if (!row) return null;
                return (
                  <tr
                    key={row.id}
                    style={{ height: item.size }}
                    className="border-b border-border/50 hover:bg-muted/40"
                    onContextMenu={(event) => {
                      // Правый клик по строке: меню действий у курсора.
                      event.preventDefault();
                      setMenuTarget({
                        folderId,
                        fileName: row.original.name,
                        x: event.clientX,
                        y: event.clientY,
                      });
                    }}
                  >
                    {row.getAllCells().map((cell) => (
                      <td
                        key={cell.id}
                        className={cn(
                          "px-2 align-middle",
                          COLUMN_CLASS[cell.column.id] ?? "",
                        )}
                      >
                        <table.FlexRender cell={cell} />
                      </td>
                    ))}
                  </tr>
                );
              })
            )}

            {padBottom > 0 ? (
              <tr aria-hidden="true">
                <td
                  colSpan={columns.length}
                  style={{ height: padBottom, padding: 0 }}
                />
              </tr>
            ) : null}
          </tbody>
        </table>

        {message && tableRows.length > 0 ? (
          <p className="p-3 text-xs text-muted-foreground">{message}</p>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-2 text-[11px] text-muted-foreground">
        <span className="tabular-nums">
          {scanning || firstPending
            ? "Файлов: —"
            : `Показано ${formatCount(rows.length)} из ${formatCount(total)}`}
        </span>
        <span>
          {scanning
            ? "Идёт сканирование"
            : query.error
              ? "Ошибка загрузки"
              : firstPending
                ? "Загружаем файлы…"
                : reordering
                  ? "Сортируем…"
                  : isFetchingNextPage
                    ? "Загружаем следующую страницу…"
                    : hasNextPage
                      ? "Прокрутите вниз — догрузим ещё"
                      : isEmpty
                        ? "Папка пуста"
                        : "Это все файлы папки"}
        </span>
      </div>

      <FileActionsMenu
        target={menuTarget}
        onOpenChange={(open) => {
          if (!open) setMenuTarget(null);
        }}
        onDeleted={onFileDeleted}
        onError={onActionError}
        onNotice={onNotice}
      />
    </section>
  );
}
