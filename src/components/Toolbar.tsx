import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { DriveInfo } from "@/lib/types";

interface ToolbarProps {
  drives: DriveInfo[];
  selected: string;
  scanning: boolean;
  stopping: boolean;
  onSelect: (letter: string) => void;
  onScan: () => void;
  onStop: () => void;
}

/** Верхняя панель: выбор диска и кнопки пуска/остановки. */
export default function Toolbar({
  drives,
  selected,
  scanning,
  stopping,
  onSelect,
  onScan,
  onStop,
}: ToolbarProps) {
  const items = drives.map((drive) => ({
    value: drive.letter,
    label: `${drive.letter} — ${drive.label || "без метки"} (${drive.fs || "?"})`,
  }));

  return (
    <section className="flex flex-wrap items-end gap-3 rounded-lg border border-border bg-card p-4">
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted-foreground">
          Диск для сканирования
        </span>
        <Select
          items={items}
          value={selected}
          onValueChange={(value) => onSelect(String(value ?? ""))}
        >
          <SelectTrigger className="w-80" aria-label="Диск для сканирования">
            <SelectValue placeholder="Выберите диск" />
          </SelectTrigger>
          <SelectContent>
            {items.map((item) => (
              <SelectItem key={item.value} value={item.value}>
                {item.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="ml-auto flex items-center gap-2">
        <Button onClick={onScan} disabled={scanning || drives.length === 0}>
          Сканировать
        </Button>
        <Button variant="outline" onClick={onStop} disabled={!scanning || stopping}>
          Остановить
        </Button>
      </div>
    </section>
  );
}
