// Вкладка «Поиск»: фильтры и таблица результатов по готовому индексу.
//
// Фильтры и страницы считает ядро (`search_files`), окно получает только
// видимый кусок. Строки виртуализированы (страница — 2000 строк, как в таблице
// файлов): результат на 1.26 млн файлов не должен раздувать DOM.
import { LinkSimple, TextAa } from "@phosphor-icons/react";
import {
  createColumnHelper,
  tableFeatures,
  useTable,
} from "@tanstack/react-table";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { formatCount, formatDateTime, formatSize } from "@/lib/format";
import { getPathCrumbs, searchFiles } from "@/lib/ipc";
import {
  build_search_query,
  empty_search_form,
  has_filters,
  SIZE_UNITS,
  type SearchForm,
  type SizeUnit,
} from "@/lib/search";
import type { Crumb, SearchQuery, SearchRow } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Размер страницы результатов: столько строк просим у ядра за один раз. */
const PAGE_SIZE = 2_000;
/** Высота строки, px: виртуализатор считает позиции по ней. */
const ROW_HEIGHT = 26;
/** За сколько строк до конца начинаем догрузку следующей страницы. */
const LOAD_AHEAD = 20;

const features = tableFeatures({});
const columnHelper = createColumnHelper<typeof features, SearchRow>();
const emptyRows: SearchRow[] = [];

/** Общий стиль полей формы: тёмная тема, высота как у кнопок. */
const INPUT_CLASS =
  "h-8 min-w-0 rounded-none border border-input bg-transparent px-2 text-xs outline-none placeholder:text-muted-foreground/60 focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring/50 [color-scheme:dark]";

const columns = columnHelper.columns([
  columnHelper.accessor("name", {
    header: "Имя",
    cell: (info) => {
      const row = info.row.original;
      return (
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="truncate" title={row.name}>
            {row.name}
          </span>
          {row.is_alias ? (
            <span
              className="inline-flex shrink-0 items-center gap-0.5 rounded border border-border bg-muted/60 px-1 py-0 text-[10px] leading-4 font-normal text-muted-foreground"
              title="короткое имя 8.3: второе имя того же файла, размер не входит в итоги"
            >
              <TextAa size={10} weight="bold" />
              короткое имя
            </span>
          ) : row.hard_linked ? (
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
  columnHelper.accessor("folder_path", {
    header: "Папка",
    cell: (info) => (
      <span className="block truncate" title={info.getValue()}>
        {info.getValue()}
      </span>
    ),
  }),
  columnHelper.accessor("size_logical", {
    header: "Размер",
    cell: (info) => (
      <span className="tabular-nums">{formatSize(info.getValue())}</span>
    ),
  }),
  columnHelper.accessor("size_on_disk", {
    header: "На диске",
    cell: (info) => (
      <span className="tabular-nums">{formatSize(info.getValue())}</span>
    ),
  }),
  columnHelper.accessor("modified", {
    header: "Изменён",
    cell: (info) => (
      <span className="tabular-nums">{formatDateTime(info.getValue())}</span>
    ),
  }),
]);

/** Ширины и выравнивание колонок: таблица фиксированной разметки. */
const COLUMN_CLASS: Record<string, string> = {
  name: "w-[30%]",
  folder_path: "",
  size_logical: "w-[104px] text-right",
  size_on_disk: "w-[104px] text-right",
  modified: "w-[144px] text-right",
};

export interface SearchPanelProps {
  /** Идёт сканирование: индекс ядра ещё не готов, поиск недоступен. */
  scanning: boolean;
  /** Номер скана: входит в ключ кэша, чтобы новый скан не показывал старые строки. */
  scanRun: number;
  /** Открыть папку файла на вкладке «Карта» (путь от корня целиком). */
  onOpenFolder: (crumbs: Crumb[]) => void;
}

/**
 * Форма фильтров и таблица результатов поиска.
 *
 * Поиск запускается кнопкой «Найти» (или Enter): до этого запросы не уходят.
 * Клик по строке получает путь папки файла и открывает её на карте — дерево и
 * таблица синхронизируются тем же механизмом, что и навигация из карты.
 */
export default function SearchPanel({
  scanning,
  scanRun,
  onOpenFolder,
}: SearchPanelProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [form, setForm] = useState<SearchForm>(empty_search_form);
  // Отправленный запрос: null — пользователь ещё не искал.
  const [submitted, setSubmitted] = useState<SearchQuery | null>(null);
  // Папка, к которой идёт переход: защищает от двойного клика и показывает
  // занятость, пока ядро собирает цепочку крошек.
  const [opening, setOpening] = useState<number | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);

  const update = (patch: Partial<SearchForm>) => {
    setForm((current) => ({ ...current, ...patch }));
  };

  const query = useInfiniteQuery({
    queryKey: ["search", scanRun, submitted],
    // `enabled` не пускает запрос до первого «Найти»; защитная ветка ниже нужна
    // только чтобы тип страницы не зависел от null.
    queryFn: ({ pageParam }) =>
      submitted === null
        ? Promise.resolve({ rows: [], total: 0 })
        : searchFiles(submitted, pageParam, PAGE_SIZE),
    initialPageParam: 0,
    getNextPageParam: (lastPage, allPages) => {
      const loaded = allPages.reduce((sum, page) => sum + page.rows.length, 0);
      if (lastPage.rows.length === 0) return undefined;
      return loaded < lastPage.total ? loaded : undefined;
    },
    enabled: submitted !== null && !scanning,
    staleTime: Number.POSITIVE_INFINITY,
  });

  const rows = useMemo(
    () => query.data?.pages.flatMap((page) => page.rows) ?? emptyRows,
    [query.data],
  );
  const total = query.data?.pages[0]?.total ?? 0;

  const table = useTable({ features, columns, data: rows });
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

  // Новый запрос — читаем выдачу сверху.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [submitted]);

  const submit = () => {
    if (scanning) return;
    setOpenError(null);
    setSubmitted(build_search_query(form));
  };

  const reset = () => {
    setForm(empty_search_form());
    setSubmitted(null);
    setOpenError(null);
  };

  const openFolder = (folderId: number) => {
    if (opening !== null) return;
    setOpening(folderId);
    setOpenError(null);
    getPathCrumbs(folderId)
      .then((crumbs) => {
        if (crumbs.length > 0) onOpenFolder(crumbs);
      })
      .catch((err: unknown) => {
        setOpenError(`Не удалось перейти к папке: ${String(err)}`);
      })
      .finally(() => setOpening(null));
  };

  const padTop = virtualItems[0]?.start ?? 0;
  const padBottom = Math.max(
    0,
    rowVirtualizer.getTotalSize() - (lastVirtual?.end ?? 0),
  );

  const firstPending = query.isPending;
  const message = scanning
    ? "Поиск станет доступен после завершения сканирования."
    : submitted === null
      ? "Задайте фильтры и нажмите «Найти». Пустые фильтры — все файлы."
      : firstPending
        ? "Ищем…"
        : query.error
          ? `Не удалось выполнить поиск: ${String(query.error)}`
          : total === 0
            ? "Ничего не найдено — ослабьте фильтры."
            : null;

  return (
    <section className="flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-card p-3">
      <header className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-sm font-medium">Поиск файлов</h2>
        <span className="text-[11px] text-muted-foreground">
          имя — подстрока без учёта регистра · «Найти» или Enter · крупное сверху
        </span>
      </header>

      <form
        className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <label className="flex min-w-0 flex-col gap-1 sm:col-span-2">
          <span className="text-xs text-muted-foreground">Имя содержит</span>
          <input
            type="text"
            value={form.name}
            onChange={(event) => update({ name: event.target.value })}
            placeholder="например, отчёт"
            aria-label="Имя содержит"
            disabled={scanning}
            className={INPUT_CLASS}
          />
        </label>

        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-xs text-muted-foreground">Размер на диске</span>
          <div className="flex items-center gap-1">
            <input
              type="text"
              inputMode="decimal"
              value={form.sizeFrom}
              onChange={(event) => update({ sizeFrom: event.target.value })}
              placeholder="от"
              aria-label="Размер от"
              disabled={scanning}
              className={cn(INPUT_CLASS, "w-full")}
            />
            <span aria-hidden="true" className="text-muted-foreground">
              —
            </span>
            <input
              type="text"
              inputMode="decimal"
              value={form.sizeTo}
              onChange={(event) => update({ sizeTo: event.target.value })}
              placeholder="до"
              aria-label="Размер до"
              disabled={scanning}
              className={cn(INPUT_CLASS, "w-full")}
            />
            <Select
              items={SIZE_UNITS.map((unit) => ({
                value: unit.value,
                label: unit.label,
              }))}
              value={form.sizeUnit}
              onValueChange={(next) => {
                const unit = String(next);
                if (unit === "mib" || unit === "gib") {
                  update({ sizeUnit: unit as SizeUnit });
                }
              }}
            >
              <SelectTrigger
                size="sm"
                aria-label="Единица размера"
                disabled={scanning}
                className="w-[70px] shrink-0"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SIZE_UNITS.map((unit) => (
                  <SelectItem key={unit.value} value={unit.value}>
                    {unit.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        <div className="flex min-w-0 flex-col gap-1">
          <span className="text-xs text-muted-foreground">Дата изменения</span>
          <div className="flex items-center gap-1">
            <input
              type="date"
              lang="ru"
              value={form.dateFrom}
              onChange={(event) => update({ dateFrom: event.target.value })}
              aria-label="Дата от"
              disabled={scanning}
              className={cn(INPUT_CLASS, "w-full")}
            />
            <span aria-hidden="true" className="text-muted-foreground">
              —
            </span>
            <input
              type="date"
              lang="ru"
              value={form.dateTo}
              onChange={(event) => update({ dateTo: event.target.value })}
              aria-label="Дата до"
              disabled={scanning}
              className={cn(INPUT_CLASS, "w-full")}
            />
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2 sm:col-span-2 lg:col-span-4">
          <Button type="submit" disabled={scanning}>
            Найти
          </Button>
          <Button type="button" variant="outline" onClick={reset}>
            Сбросить
          </Button>
          {has_filters(form) ? null : (
            <span className="text-[11px] text-muted-foreground">
              Фильтров нет — будут все файлы
            </span>
          )}
          {openError ? (
            <span role="alert" className="text-[11px] text-destructive">
              {openError}
            </span>
          ) : null}
        </div>
      </form>

      <div
        ref={scrollRef}
        className="max-h-[420px] min-h-[160px] overflow-auto rounded border border-border"
      >
        <table className="w-full table-fixed border-collapse text-xs">
          <thead className="sticky top-0 z-10 bg-card">
            {table.getHeaderGroups().map((headerGroup) => (
              <tr key={headerGroup.id} className="border-b border-border">
                {headerGroup.headers.map((header) => (
                  <th
                    key={header.id}
                    className={cn(
                      "h-8 px-2 text-left align-middle font-medium whitespace-nowrap text-muted-foreground",
                      COLUMN_CLASS[header.column.id] ?? "",
                    )}
                  >
                    {header.isPlaceholder ? null : (
                      <table.FlexRender header={header} />
                    )}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {padTop > 0 ? (
              <tr aria-hidden="true">
                <td
                  colSpan={columns.length}
                  style={{ height: padTop, padding: 0 }}
                />
              </tr>
            ) : null}

            {tableRows.length === 0 ? (
              <tr>
                <td
                  colSpan={columns.length}
                  className="py-12 text-center text-muted-foreground"
                >
                  {message ?? "Ничего не найдено."}
                </td>
              </tr>
            ) : (
              virtualItems.map((item) => {
                const row = tableRows[item.index];
                if (!row) return null;
                const isOpening = opening === row.original.folder_id;
                return (
                  <tr
                    key={row.id}
                    style={{ height: item.size }}
                    onClick={() => openFolder(row.original.folder_id)}
                    title={`Открыть папку ${row.original.folder_path} на карте`}
                    className={cn(
                      "cursor-pointer border-b border-border/50 hover:bg-muted/40",
                      isOpening && "bg-muted/40",
                    )}
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

        {message && tableRows.length > 0 && !scanning ? (
          <p className="p-3 text-xs text-muted-foreground">{message}</p>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <span className="tabular-nums">
          {scanning || submitted === null || firstPending
            ? "Найдено: —"
            : `Показано ${formatCount(rows.length)} из ${formatCount(total)}`}
        </span>
        <span>
          {scanning
            ? "Идёт сканирование"
            : submitted === null
              ? "Поиск ещё не запускался"
              : firstPending
                ? "Ищем…"
                : query.error
                  ? "Ошибка поиска"
                  : opening !== null
                    ? "Открываем папку…"
                    : isFetchingNextPage
                      ? "Загружаем следующую страницу…"
                      : hasNextPage
                        ? "Прокрутите вниз — догрузим ещё"
                        : total === 0
                          ? "Ничего не найдено"
                          : "Клик по строке — открыть папку на карте"}
        </span>
      </div>
    </section>
  );
}
