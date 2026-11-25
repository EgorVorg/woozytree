import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";

import FileTable from "@/components/FileTable";
import FolderTree from "@/components/FolderTree";
import ScanStatus, { Breadcrumbs, type Crumb } from "@/components/ScanStatus";
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
  startScan,
} from "@/lib/ipc";
import { root_crumb_name } from "@/lib/treemap";
import type {
  DriveInfo,
  FolderSummary,
  ScanProgress,
  ScanTotals,
} from "@/lib/types";

const EMPTY_PROGRESS: ScanProgress = {
  files: 0,
  folders: 0,
  size_on_disk: 0,
  elapsed_ms: 0,
};

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
  const [selected, setSelected] = useState("");
  const [scanning, setScanning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [cancelled, setCancelled] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [totals, setTotals] = useState<ScanTotals | null>(null);
  const [top, setTop] = useState<FolderSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Путь от корня сканирования до открытой папки; последний элемент — текущая.
  const [path, setPath] = useState<Crumb[]>([]);
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

  // Клик по папке на карте: имя нужно для хлебных крошек.
  const handleMapNavigate = useCallback((id: number, name?: string) => {
    setPath((current) => [...current, { id, name: name ?? `Папка ${id}` }]);
  }, []);

  // Клик в дереве: узел приносит всю цепочку предков — путь заменяется целиком,
  // поэтому переход в соседнюю ветку не оставляет «хвоста» прежнего пути.
  const handleTreeNavigate = useCallback((crumbs: Crumb[]) => {
    if (crumbs.length > 0) setPath(crumbs);
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

  // Список дисков — один раз при открытии окна.
  useEffect(() => {
    let alive = true;
    listDrives()
      .then((list) => {
        if (!alive) return;
        setDrives(list);
        const preferred =
          list.find((drive) => drive.letter.toUpperCase() === "C:") ?? list[0];
        if (preferred) setSelected(preferred.letter);
      })
      .catch((err: unknown) => {
        if (alive) setError(String(err));
      });
    return () => {
      alive = false;
    };
  }, []);

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
  // У остановленного скана остаётся частичное значение: 100 % означало бы,
  // что данные полные, а их такими показывать нельзя.
  const percent = (() => {
    const drive = drives.find((item) => item.letter === selected);
    const used = drive ? drive.total_bytes - drive.free_bytes : 0;
    const live =
      used > 0 && progress
        ? Math.min(99, Math.round((progress.size_on_disk / used) * 100))
        : 0;
    if (cancelled) return live;
    if (!scanning) {
      return totals ? 100 : 0;
    }
    return live;
  })();

  const handleScan = () => {
    if (!selected) return;
    setError(null);
    setTotals(null);
    setTop([]);
    setProgress(EMPTY_PROGRESS);
    setCancelled(false);
    setScanning(true);
    setStopping(false);
    // Новая карта начинается с корня выбранного диска:
    // «C:» → «C:\» (без двойного двоеточия).
    setPath([{ id: 0, name: root_crumb_name(selected) }]);
    setScanRun((run) => run + 1);
    startScan({ drive: selected }).catch((err: unknown) => {
      setError(String(err));
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
            selected={selected}
            scanning={scanning}
            stopping={stopping}
            onSelect={setSelected}
            onScan={handleScan}
            onStop={handleStop}
          />

          {error ? (
            <div
              role="alert"
              className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              {error}
            </div>
          ) : null}

          {path.length === 0 ? (
            <section className="rounded-lg border border-border bg-card px-4 py-6 text-sm text-muted-foreground">
              Карта занятости появится здесь сразу после начала сканирования.
            </section>
          ) : (
            <>
              <Breadcrumbs
                path={path}
                onNavigate={handleCrumb}
                onUp={handleUp}
              />
              <Treemap
                key={scanRun}
                folderId={folderId}
                onNavigate={handleMapNavigate}
                scanning={scanning}
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
            </>
          )}

          <ScanStatus
            progress={progress}
            percent={percent}
            scanning={scanning}
            stopping={stopping}
            cancelled={cancelled}
            skipped={totals?.skipped ?? 0}
          />

          <TopFolders rows={top} />
        </div>
      </main>
    </QueryClientProvider>
  );
}

export default App;
