import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";

import FileTable from "@/components/FileTable";
import FileTypesPanel from "@/components/FileTypesPanel";
import FolderTree from "@/components/FolderTree";
import ScanStatus, { Breadcrumbs, type Crumb } from "@/components/ScanStatus";
import SearchPanel from "@/components/SearchPanel";
import Tabs, { type AppTab } from "@/components/Tabs";
import Toolbar from "@/components/Toolbar";
import TopFolders from "@/components/TopFolders";
import Treemap from "@/components/Treemap";
import type { TreeRoot } from "@/lib/files";
import {
  cancelScan,
  listDrives,
  onDone,
  onError,
  onProgress,
  onTop,
  pickFolder,
  startScan,
} from "@/lib/ipc";
import { scanPercent } from "@/lib/progress";
import { root_crumb_name } from "@/lib/treemap";
import type {
  DriveInfo,
  FolderSummary,
  ScanChoice,
  ScanProgress,
  ScanTarget,
  ScanTotals,
} from "@/lib/types";

const EMPTY_PROGRESS: ScanProgress = {
  files: 0,
  folders: 0,
  size_on_disk: 0,
  elapsed_ms: 0,
};

/** Диск по умолчанию: системный C:, иначе первый в списке. */
function preferredLetter(drives: DriveInfo[]): string {
  const preferred =
    drives.find((drive) => drive.letter.toUpperCase() === "C:") ?? drives[0];
  return preferred?.letter ?? "";
}

/** Занятое место диска: столько байт индекс может покрыть в лучшем случае. */
function usedBytes(drive: DriveInfo | undefined): number {
  if (!drive) return 0;
  return Math.max(0, drive.total_bytes - drive.free_bytes);
}

/**
 * Занятое место всего выбранного охвата на момент старта — база процентов.
 *
 * Для папки базы нет (её размер заранее неизвестен), поэтому делить не на что:
 * бар покажет 0 % до завершения и 100 % по итогам.
 */
function scanBaseBytes(choice: ScanChoice, drives: DriveInfo[]): number {
  if (choice.kind === "folder") return 0;
  if (choice.kind === "all") {
    return drives.reduce((sum, drive) => sum + usedBytes(drive), 0);
  }
  return usedBytes(drives.find((drive) => drive.letter === choice.letter));
}

/**
 * Имя корневой хлебной крошки: «C:\» для диска, «Все диски» для общего охвата,
 * путь папки — для сканирования папки.
 */
function rootCrumb(choice: ScanChoice): string {
  switch (choice.kind) {
    case "all":
      return "Все диски";
    case "folder":
      // Корень диска («C:\») не обрезаем, у остальных путей убираем хвостовой
      // разделитель — иначе крошка выглядела бы как «C:\Temp\».
      return choice.path.length > 3
        ? choice.path.replace(/[\\/]+$/, "")
        : choice.path;
    default:
      return root_crumb_name(choice.letter);
  }
}

/** Цель сканирования для ядра по выбору в тулбаре. */
function scanTargetOf(choice: ScanChoice, drives: DriveInfo[]): ScanTarget {
  switch (choice.kind) {
    case "all":
      // «Все диски» — буквы из list_drives: ядро развернёт их в подпапки.
      return { all_drives: drives.map((drive) => drive.letter) };
    case "folder":
      return { folder: choice.path };
    default:
      return { drive: choice.letter };
  }
}

/**
 * Кэш запросов дерева и таблицы на всё время жизни окна.
 *
 * Данные индекса после сканирования не меняются, поэтому свежесть не
 * протухает; ключи включают номер скана, так что новый скан не показывает
 * папки и файлы прошлого.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: Number.POSITIVE_INFINITY,
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

function App() {
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [drivesBusy, setDrivesBusy] = useState(false);
  const [choice, setChoice] = useState<ScanChoice | null>(null);
  const [scanning, setScanning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [cancelled, setCancelled] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [totals, setTotals] = useState<ScanTotals | null>(null);
  const [top, setTop] = useState<FolderSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Скан завершился ошибкой: прогресс и итоги очищены, статус — «Ошибка».
  const [scanError, setScanError] = useState(false);
  // Занятое место диска на момент старта скана. Проценты считаются от него, а
  // не от текущего выбранного диска: иначе смена диска после остановки
  // пересчитывала бы процент уже отсканированного объёма.
  const [scanUsed, setScanUsed] = useState(0);
  // Путь от корня сканирования до открытой папки; последний элемент — текущая.
  const [path, setPath] = useState<Crumb[]>([]);
  // Открытая вкладка: карта, типы файлов (Задача 3) или поиск.
  const [tab, setTab] = useState<AppTab>("map");
  // Номер запуска сканирования: смена ключа сбрасывает карту к новому скану.
  const [scanRun, setScanRun] = useState(0);

  const handleUp = useCallback(() => {
    setPath((current) =>
      current.length > 1 ? current.slice(0, -1) : current,
    );
  }, []);

  // Крошка: отрезать всё, что глубже выбранного звена пути.
  const handleCrumb = useCallback((id: number) => {
    setPath((current) => {
      const index = current.findIndex((crumb) => crumb.id === id);
      return index >= 0 ? current.slice(0, index + 1) : current;
    });
  }, []);

  // Клик по папке на карте: карта приносит полный путь вместе с предками
  // вложенной плитки (Music внутри Users → C:\ > Users > Music), поэтому путь
  // заменяется целиком — как при клике в дереве, без «хвоста» и дублей.
  const handleMapNavigate = useCallback((crumbs: Crumb[]) => {
    if (crumbs.length > 0) setPath(crumbs);
  }, []);

  // Клик в дереве: узел приносит всю цепочку предков — путь заменяется целиком,
  // поэтому переход в соседнюю ветку не оставляет «хвоста» прежнего пути.
  const handleTreeNavigate = useCallback((crumbs: Crumb[]) => {
    if (crumbs.length > 0) setPath(crumbs);
  }, []);

  // Клик по строке поиска: ядро отдало цепочку папки (`get_path_crumbs`) —
  // открываем её на карте. Дерево и таблица синхронизируются тем же `path`, что
  // и при навигации из карты (механизм Плана 2).
  const handleSearchNavigate = useCallback((crumbs: Crumb[]) => {
    if (crumbs.length === 0) return;
    setPath(crumbs);
    setTab("map");
  }, []);

  // Alt+↑ — на уровень вверх (дублирует кнопку «Вверх»).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.altKey || event.key !== "ArrowUp") return;
      event.preventDefault();
      handleUp();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleUp]);

  // Перечитать список дисков: кнопка в тулбаре и первичная загрузка. Выбранный
  // диск сохраняется, если он ещё существует; иначе — C: или первый доступный.
  // «Все диски» и папка от списка не зависят и сохраняются как есть.
  const handleRefreshDrives = useCallback(() => {
    setDrivesBusy(true);
    listDrives()
      .then((list) => {
        setDrives(list);
        setChoice((current) => {
          if (current === null || current.kind !== "drive") {
            return current ?? { kind: "drive", letter: preferredLetter(list) };
          }
          return list.some((drive) => drive.letter === current.letter)
            ? current
            : { kind: "drive", letter: preferredLetter(list) };
        });
      })
      .catch((err: unknown) => setError(String(err)))
      .finally(() => setDrivesBusy(false));
  }, []);

  // Список дисков — при открытии окна и по кнопке «Обновить список дисков».
  useEffect(() => {
    handleRefreshDrives();
  }, [handleRefreshDrives]);

  // Подписка на события сканирования; отписка — при закрытии окна.
  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    const subscribe = <T,>(
      register: (cb: (payload: T) => void) => Promise<() => void>,
      handler: (payload: T) => void,
    ) => {
      void register(handler).then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    subscribe(onProgress, (next) => setProgress(next));
    subscribe(onTop, (rows) => setTop(rows));
    subscribe(onDone, (result) => {
      setTotals(result);
      setCancelled(result.cancelled);
      setScanError(false);
      setProgress({
        files: result.files,
        folders: result.folders,
        size_on_disk: result.size_on_disk,
        elapsed_ms: result.elapsed_ms,
      });
      setScanning(false);
      setStopping(false);
    });
    subscribe(onError, (message) => {
      setError(message);
      // Ошибка — не «Готово»: недостоверные прогресс и итоги убираем, статус
      // показывает «Ошибка», а не последний удачный результат. Карту и панели
      // тоже прячем: иначе под «данные не собраны» остались бы строки прошлого
      // индекса, а корень дерева уже показывал бы 0 Б.
      setScanError(true);
      setProgress(null);
      setTotals(null);
      setTop([]);
      setPath([]);
      setCancelled(false);
      setScanning(false);
      setStopping(false);
    });

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  // Заполненность бара — доля занятого места диска, уже попавшая в индекс.
  // База (`scanUsed`) зафиксирована на старте скана: смена выбранного диска
  // после остановки не должна пересчитывать процент уже собранных данных.
  const percent = scanPercent({
    scannedBytes: progress?.size_on_disk ?? 0,
    scannedUsed: scanUsed,
    scanning,
    cancelled,
    finished: totals !== null,
  });

  const handleSelectDrive = useCallback((letter: string) => {
    setChoice({ kind: "drive", letter });
  }, []);

  const handleSelectAllDrives = useCallback(() => {
    setChoice({ kind: "all" });
  }, []);

  // «Папка…»: системный диалог; отмена диалога оставляет прежний выбор.
  const handlePickFolder = useCallback(() => {
    pickFolder()
      .then((path) => {
        if (path) setChoice({ kind: "folder", path });
      })
      .catch((err: unknown) => setError(String(err)));
  }, []);

  const handleScan = () => {
    if (choice === null) return;
    const target = scanTargetOf(choice, drives);
    setError(null);
    setScanError(false);
    setTotals(null);
    setTop([]);
    setProgress(EMPTY_PROGRESS);
    setCancelled(false);
    setScanning(true);
    setStopping(false);
    // База процентов — занятое место всего охвата; дальше она не меняется, даже
    // если пользователь переключит селектор после остановки скана.
    setScanUsed(scanBaseBytes(choice, drives));
    // Новая карта начинается с корня охвата: «C:\», «Все диски» или путь папки.
    setPath([{ id: 0, name: rootCrumb(choice) }]);
    // Новый скан — показываем карту: результаты прошлого поиска уже не к месту.
    setTab("map");
    setScanRun((run) => run + 1);
    startScan(target).catch((err: unknown) => {
      setError(String(err));
      setScanError(true);
      setProgress(null);
      setTotals(null);
      setTop([]);
      setPath([]);
      setScanning(false);
      setStopping(false);
    });
  };

  const handleStop = () => {
    if (!scanning || stopping) return;
    setStopping(true);
    cancelScan().catch((err: unknown) => {
      setError(String(err));
      setStopping(false);
    });
  };

  const folderId = path.length > 0 ? path[path.length - 1].id : 0;

  // Корень дерева: имя из пути, итоги — из завершённого сканирования. Пока
  // индекс не готов, дерево показывает только корень и честную подсказку.
  const rootSummary = useMemo<TreeRoot>(
    () => ({
      name: path.length > 0 ? path[0].name : "",
      size_on_disk: totals?.size_on_disk ?? 0,
      file_count: totals?.files ?? 0,
      folder_count: totals?.folders ?? 0,
    }),
    [path, totals],
  );

  return (
    <QueryClientProvider client={queryClient}>
      <main className="min-h-screen bg-background text-foreground">
        <div className="mx-auto flex max-w-6xl flex-col gap-4 p-6">
          <header className="flex items-baseline justify-between gap-4">
            <h1 className="text-xl font-semibold tracking-tight">WoozyTree</h1>
            <span className="text-xs text-muted-foreground">
              Быстрый анализ занятого места на диске
            </span>
          </header>

          <Toolbar
            drives={drives}
            choice={choice}
            scanning={scanning}
            stopping={stopping}
            refreshing={drivesBusy}
            onSelectDrive={handleSelectDrive}
            onSelectAllDrives={handleSelectAllDrives}
            onPickFolder={handlePickFolder}
            onScan={handleScan}
            onStop={handleStop}
            onRefreshDrives={handleRefreshDrives}
          />

          {error ? (
            <div
              role="alert"
              className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              {error}
            </div>
          ) : null}

          <Tabs
            value={tab}
            onChange={setTab}
            map={
              path.length === 0 ? (
                <section className="rounded-lg border border-border bg-card px-4 py-6 text-sm text-muted-foreground">
                  Карта занятости появится здесь сразу после начала сканирования.
                </section>
              ) : (
                <div className="flex min-w-0 flex-col gap-4">
                  <Breadcrumbs
                    path={path}
                    onNavigate={handleCrumb}
                    onUp={handleUp}
                  />
                  <Treemap
                    key={scanRun}
                    folderId={folderId}
                    path={path}
                    onNavigate={handleMapNavigate}
                    scanning={scanning}
                    cancelled={cancelled}
                  />
                  <div className="grid min-w-0 grid-cols-1 gap-4 lg:grid-cols-[minmax(240px,320px)_minmax(0,1fr)]">
                    <FolderTree
                      path={path}
                      root={rootSummary}
                      scanning={scanning}
                      scanRun={scanRun}
                      onNavigate={handleTreeNavigate}
                    />
                    <FileTable
                      folderId={folderId}
                      scanning={scanning}
                      scanRun={scanRun}
                    />
                  </div>
                </div>
              )
            }
            types={
              path.length === 0 ? (
                <section className="rounded-lg border border-border bg-card px-4 py-6 text-sm text-muted-foreground">
                  Карта типов появится здесь сразу после начала сканирования.
                </section>
              ) : (
                <FileTypesPanel
                  folderId={folderId}
                  scanning={scanning}
                  scanRun={scanRun}
                />
              )
            }
            search={
              <SearchPanel
                scanning={scanning}
                scanRun={scanRun}
                onOpenFolder={handleSearchNavigate}
              />
            }
          />

          <ScanStatus
            progress={progress}
            percent={percent}
            scanning={scanning}
            stopping={stopping}
            cancelled={cancelled}
            failed={scanError}
            skipped={totals?.skipped ?? 0}
          />

          <TopFolders rows={top} />
        </div>
      </main>
    </QueryClientProvider>
  );
}

export default App;
