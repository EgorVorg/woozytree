import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { DriveInfo, ScanChoice } from "@/lib/types";

/** Значения служебных пунктов селектора: «Все диски» и «Папка…». */
const ALL_DRIVES = "__all_drives__";
const FOLDER = "__folder__";

interface ToolbarProps {
  drives: DriveInfo[];
  /** Текущий выбор: все диски, диск или папка (null — ещё не выбран). */
  choice: ScanChoice | null;
  scanning: boolean;
  stopping: boolean;
  /** Идёт перечитывание списка дисков: кнопка показывает занятость. */
  refreshing: boolean;
  onSelectDrive: (letter: string) => void;
  onSelectAllDrives: () => void;
  /** Открыть системный диалог выбора папки. */
  onPickFolder: () => void;
  onScan: () => void;
  onStop: () => void;
  /** Перезапросить `list_drives` (подключили или вынули диск). */
  onRefreshDrives: () => void;
}

/** Верхняя панель: выбор охвата, обновление списка дисков и кнопки пуска/остановки. */
export default function Toolbar({
  drives,
  choice,
  scanning,
  stopping,
  refreshing,
  onSelectDrive,
  onSelectAllDrives,
  onPickFolder,
  onScan,
  onStop,
  onRefreshDrives,
}: ToolbarProps) {
  // «Все диски» — первым пунктом, как в WizTree; «Папка…» — последним, потому
  // что он открывает диалог, а не выбирает готовое значение.
  const folderLabel =
    choice?.kind === "folder" ? `Папка: ${choice.path}` : "Папка…";
  const items = [
    { value: ALL_DRIVES, label: "Все диски" },
    ...drives.map((drive) => ({
      value: drive.letter,
      label: `${drive.letter} — ${drive.label || "без метки"} (${drive.fs || "?"})`,
    })),
    { value: FOLDER, label: folderLabel },
  ];

  const value =
    choice === null
      ? ""
      : choice.kind === "all"
        ? ALL_DRIVES
        : choice.kind === "folder"
          ? FOLDER
          : choice.letter;

  // «Все диски» и «Папка…» не зависят от списка дисков: папку можно сканировать
  // и тогда, когда список пуст (например, при ошибке перечисления).
  const canScan =
    choice !== null &&
    (choice.kind === "folder" ? choice.path.length > 0 : drives.length > 0);

  const handleValueChange = (next: unknown) => {
    const selected = String(next ?? "");
    if (selected === ALL_DRIVES) {
      onSelectAllDrives();
    } else if (selected === FOLDER) {
      onPickFolder();
    } else if (selected !== "") {
      onSelectDrive(selected);
    }
  };

  return (
    <section className="flex flex-wrap items-end gap-3 rounded-lg border border-border bg-card p-4">
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted-foreground">
          Что сканировать
        </span>
        <div className="flex flex-wrap items-center gap-2">
          <Select
            items={items}
            value={value}
            disabled={scanning}
            onValueChange={handleValueChange}
          >
            <SelectTrigger
              className="w-80"
              aria-label="Что сканировать"
              disabled={scanning}
              title={
                scanning
                  ? "Во время сканирования выбор не меняется — сначала остановите скан"
                  : undefined
              }
            >
              <SelectValue placeholder="Выберите диск, все диски или папку" />
            </SelectTrigger>
            <SelectContent>
              {items.map((item) => (
                <SelectItem key={item.value} value={item.value}>
                  {item.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Button
            variant="outline"
            size="sm"
            onClick={onRefreshDrives}
            disabled={refreshing}
            title="Перечитать список логических дисков"
          >
            {refreshing ? "Обновляем…" : "Обновить список дисков"}
          </Button>
        </div>
      </div>

      <div className="ml-auto flex items-center gap-2">
        <Button onClick={onScan} disabled={scanning || !canScan}>
          Сканировать
        </Button>
        <Button variant="outline" onClick={onStop} disabled={!scanning || stopping}>
          Остановить
        </Button>
      </div>
    </section>
  );
}
