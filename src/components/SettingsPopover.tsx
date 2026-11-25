// Настройки отображения в тулбаре: режим размера, файлы на карте и порог мелочи.
//
// Значения применяются сразу (родитель перерисовывает карту/таблицу/топ) и
// сохраняются в localStorage — после перезапуска окно откроется с ними же.
import { useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";
import {
  min_tile_mib,
  set_min_tile_mib,
  update_settings,
  type DisplaySettings,
} from "@/lib/settings";
import type { SizeMode } from "@/lib/types";

interface SettingsPopoverProps {
  settings: DisplaySettings;
  /** Новые настройки: применяются сразу и сохраняются в localStorage. */
  onChange: (settings: DisplaySettings) => void;
}

/** Пункт радио «Размер»: клик по подписи тоже переключает режим. */
function SizeOption({
  value,
  label,
  onSelect,
}: {
  value: SizeMode;
  label: string;
  onSelect: (value: SizeMode) => void;
}) {
  return (
    <div
      className="flex cursor-pointer items-center gap-1.5"
      onClick={() => onSelect(value)}
    >
      <RadioGroupItem value={value} aria-label={label} />
      <span className="text-xs">{label}</span>
    </div>
  );
}

/**
 * Кнопка «Настройки» и поповер с тремя настройками.
 *
 * Поле порога живёт локальной строкой: пока пользователь печатает («1.», пусто),
 * настройка не дёргается на каждый символ — значение применяется, когда из него
 * получается число (или 0 у пустого поля).
 */
export default function SettingsPopover({
  settings,
  onChange,
}: SettingsPopoverProps) {
  const [minTileText, setMinTileText] = useState(() =>
    String(min_tile_mib(settings)),
  );
  // Последнее значение, которое поле само записало в настройки. Синхронизация
  // по эффекту нужна только тогда, когда настройка изменилась снаружи: иначе
  // набор «1.» нормализовался бы в «1» и десятичная дробь не набиралась.
  const lastPushed = useRef<number | null>(null);

  useEffect(() => {
    if (settings.minTileBytes !== lastPushed.current) {
      setMinTileText(String(min_tile_mib(settings)));
      lastPushed.current = settings.minTileBytes;
    }
  }, [settings.minTileBytes]);

  const applyThreshold = (text: string) => {
    setMinTileText(text);
    const mib = text.trim() === "" ? 0 : Number(text.replace(",", "."));
    if (!Number.isFinite(mib)) {
      return;
    }
    const next = set_min_tile_mib(settings, mib);
    lastPushed.current = next.minTileBytes;
    onChange(next);
  };

  return (
    <Popover>
      <PopoverTrigger
        render={<Button variant="outline" size="sm" />}
        aria-label="Настройки отображения"
      >
        Настройки
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 space-y-4">
        <div className="space-y-0.5">
          <PopoverTitle>Настройки отображения</PopoverTitle>
          <PopoverDescription>
            Применяются сразу и сохраняются между запусками
          </PopoverDescription>
        </div>

        <fieldset className="space-y-2">
          <legend className="mb-1 text-xs text-muted-foreground">Размер</legend>
          <RadioGroup
            value={settings.sizeMode}
            onValueChange={(value) =>
              onChange(
                update_settings(settings, { sizeMode: value as SizeMode }),
              )
            }
            aria-label="Что считать размером"
          >
            <SizeOption
              value="on_disk"
              label="на диске"
              onSelect={(sizeMode) =>
                onChange(update_settings(settings, { sizeMode }))
              }
            />
            <SizeOption
              value="logical"
              label="фактический"
              onSelect={(sizeMode) =>
                onChange(update_settings(settings, { sizeMode }))
              }
            />
          </RadioGroup>
        </fieldset>

        <div className="flex items-center justify-between gap-3">
          <span className="text-xs">Показывать файлы на карте</span>
          <Switch
            checked={settings.showFilesOnMap}
            onCheckedChange={(checked) =>
              onChange(update_settings(settings, { showFilesOnMap: checked }))
            }
            aria-label="Показывать файлы на карте"
          />
        </div>

        <div className="space-y-1">
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="settings-min-tile" className="text-xs">
              Не показывать мельче (МиБ)
            </label>
            <Input
              id="settings-min-tile"
              type="number"
              inputMode="decimal"
              min={0}
              step={0.1}
              value={minTileText}
              onChange={(event) => applyThreshold(event.target.value)}
              className="w-20 text-right"
              title="Плитки мельче порога собираются в «прочее»; итоги не меняются"
            />
          </div>
          <p className="text-[11px] text-muted-foreground">
            Мелочь собирается в плитку «прочее» — только на карте. Итоги в
            статусе, дереве и таблице не меняются.
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
}
