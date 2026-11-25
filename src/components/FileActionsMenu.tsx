// Контекстное меню файла: открыть, показать в проводнике, скопировать путь,
// удалить в корзину или безвозвратно.
//
// Одно меню обслуживает и строку таблицы файлов, и выбранную плитку карты:
// родитель даёт папку индекса, имя файла и точку правого клика, а компонент
// сам спрашивает у ядра реальный путь (`get_file_path`) — индекс хранит только
// имена. Безвозвратное удаление всегда идёт через диалог подтверждения:
// это единственное действие, которое нельзя отменить.
import {
  ArrowSquareOutIcon,
  CopyIcon,
  FolderOpenIcon,
  TrashIcon,
} from "@phosphor-icons/react";
import { useCallback, useMemo, useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import {
  copy_text,
  error_text,
  HARD_LINK_PERMANENT_REASON,
  permanent_delete_allowed,
  permanent_delete_question,
} from "@/lib/actions";
import { deletePaths, getFilePath, openPath, revealInExplorer } from "@/lib/ipc";
import type { DeleteReport } from "@/lib/types";

/** Файл, для которого открыто меню, и точка клика в координатах окна. */
export interface FileActionTarget {
  /** Папка индекса, в которой лежит файл. */
  folderId: number;
  /** Имя файла без пути. */
  fileName: string;
  /** Строка помечена «жёсткая ссылка» (в т.ч. DOS-алиас): безвозвратно нельзя. */
  hardLinked?: boolean;
  /** Координаты правого клика: clientX/clientY. */
  x: number;
  y: number;
}

export interface FileActionsMenuProps {
  /** Открытый файл; `null` — меню закрыто. */
  target: FileActionTarget | null;
  /** Меню просит закрыться (клик мимо, Escape, выбор пункта). */
  onOpenChange: (open: boolean) => void;
  /** Удаление состоялось: родитель показывает плашку «данные устарели». */
  onDeleted: (report: DeleteReport) => void;
  /** Ошибка действия — понятный текст для красной плашки. */
  onError: (message: string) => void;
  /** Нейтральное сообщение, например «путь скопирован». */
  onNotice: (message: string) => void;
}

/**
 * Виртуальный якорь меню: точка правого клика.
 *
 * Меню открывается программно, без кнопки-триггера, поэтому Base UI нужен
 * якорь с координатами, а не элемент: так попап появляется ровно под курсором.
 */
function anchor_at(x: number, y: number): { getBoundingClientRect: () => DOMRect } {
  const rect = new DOMRect(x, y, 0, 0);
  return { getBoundingClientRect: () => rect };
}

export default function FileActionsMenu({
  target,
  onOpenChange,
  onDeleted,
  onError,
  onNotice,
}: FileActionsMenuProps) {
  // Диалог безвозвратного удаления живёт отдельно от меню: меню закрывается,
  // а подтверждение остаётся открытым со своим файлом.
  const [confirm, setConfirm] = useState<FileActionTarget | null>(null);
  const [busy, setBusy] = useState(false);

  const anchor = useMemo(
    () => (target ? anchor_at(target.x, target.y) : null),
    [target],
  );
  // Вторичные ссылки и короткие имена (8.3) нельзя удалять безвозвратно:
  // единственный файл исчез бы навсегда. В корзину — по-прежнему можно.
  const permanentAllowed = permanent_delete_allowed(target?.hardLinked ?? false);

  /** Показать итог удаления: ошибки — в красную плашку, успех — родителю. */
  const reportDelete = useCallback(
    (result: DeleteReport) => {
      for (const message of result.errors) onError(message);
      if (result.deleted > 0) onDeleted(result);
    },
    [onDeleted, onError],
  );

  /** Выполнить действие с реальным путём файла; ошибки — в плашку. */
  const withResolved = useCallback(
    async (
      goal: FileActionTarget,
      action: (path: string) => Promise<void>,
    ) => {
      setBusy(true);
      try {
        const path = await getFilePath(goal.folderId, goal.fileName);
        await action(path);
      } catch (err) {
        onError(error_text(err));
      } finally {
        setBusy(false);
      }
    },
    [onError],
  );

  const handleOpen = () => {
    if (target) void withResolved(target, openPath);
  };

  const handleReveal = () => {
    if (target) void withResolved(target, revealInExplorer);
  };

  const handleCopy = () => {
    if (!target) return;
    void withResolved(target, async (path) => {
      await copy_text(path);
      onNotice(`Путь скопирован: ${path}`);
    });
  };

  const handleTrash = () => {
    if (!target) return;
    void withResolved(target, async (path) => {
      reportDelete(await deletePaths([path], false));
    });
  };

  const askPermanent = () => {
    if (!target) return;
    setConfirm({ ...target });
    onOpenChange(false);
  };

  const handlePermanent = () => {
    const goal = confirm;
    setConfirm(null);
    if (!goal) return;
    void withResolved(goal, async (path) => {
      reportDelete(await deletePaths([path], true));
    });
  };

  return (
    <>
      <DropdownMenu
        open={target !== null}
        onOpenChange={onOpenChange}
        modal={false}
      >
        {anchor ? (
          <DropdownMenuContent
            anchor={anchor}
            side="bottom"
            align="start"
            sideOffset={2}
            className="min-w-56"
          >
            {target ? (
              <DropdownMenuGroup>
                <DropdownMenuLabel className="flex flex-col gap-0.5">
                  <span
                    className="max-w-56 truncate text-xs text-foreground"
                    title={target.fileName}
                  >
                    {target.fileName}
                  </span>
                  <span className="text-[10px]">действия с файлом</span>
                </DropdownMenuLabel>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={busy} onClick={handleOpen}>
                  <ArrowSquareOutIcon />
                  Открыть
                </DropdownMenuItem>
                <DropdownMenuItem disabled={busy} onClick={handleReveal}>
                  <FolderOpenIcon />
                  Показать в проводнике
                </DropdownMenuItem>
                <DropdownMenuItem disabled={busy} onClick={handleCopy}>
                  <CopyIcon />
                  Копировать путь
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={busy} onClick={handleTrash}>
                  <TrashIcon />
                  Удалить в корзину
                </DropdownMenuItem>
                {permanentAllowed ? (
                  <DropdownMenuItem
                    variant="destructive"
                    disabled={busy}
                    onClick={askPermanent}
                  >
                    <TrashIcon />
                    Удалить навсегда…
                  </DropdownMenuItem>
                ) : (
                  // Для вторичной ссылки/короткого имени пункта нет вовсе:
                  // безвозвратное удаление стёрло бы единственный файл.
                  <DropdownMenuLabel
                    className="max-w-56 text-[10px] leading-snug text-muted-foreground"
                    title={HARD_LINK_PERMANENT_REASON}
                  >
                    {HARD_LINK_PERMANENT_REASON}
                  </DropdownMenuLabel>
                )}
              </DropdownMenuGroup>
            ) : null}
          </DropdownMenuContent>
        ) : null}
      </DropdownMenu>

      <AlertDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirm ? permanent_delete_question(1) : "Безвозвратное удаление"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm ? `Файл «${confirm.fileName}». ` : ""}
              Это действие нельзя отменить.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Отмена</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={busy}
              onClick={handlePermanent}
            >
              Удалить навсегда
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
