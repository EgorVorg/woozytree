// Настройки отображения: чистые функции + хранение в localStorage.
//
// Настройки живут на фронтенде (в ядре их нет): параметры уходят аргументами
// команд карты/таблицы. Ключ хранилища один — `woozytree.settings`; при чтении
// мусор (испорченный JSON, чужие типы, отрицательный порог) заменяется
// значениями по умолчанию, а не роняет окно.
import type { SizeMode } from "./types";

/** Что считать размером: занятое на диске или фактический размер. */
export type { SizeMode };

/** Настройки отображения целиком. */
export interface DisplaySettings {
  sizeMode: SizeMode;
  /** Показывать файлы плитками на карте (иначе — только папки). */
  showFilesOnMap: boolean;
  /** Плитки мельче этого размера складываются в «прочее»; 0 — без порога. */
  minTileBytes: number;
}

/** Ключ localStorage с настройками. */
export const SETTINGS_KEY = "woozytree.settings";

/** Значения по умолчанию: на диске, файлы видны, порога нет. */
export const DEFAULT_SETTINGS: DisplaySettings = {
  sizeMode: "on_disk",
  showFilesOnMap: true,
  minTileBytes: 0,
};

const MIB = 1024 * 1024;

/** Минимум хранилища, которого достаточно настройкам (localStorage подходит). */
export interface SettingsStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Хранилище браузера; в тестах/SSR его может не быть — тогда null. */
function browser_storage(): SettingsStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Доступ к localStorage запрещён настройками приватности — работаем без него.
    return null;
  }
}

function is_record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Порог в байтах: неотрицательное целое; мусор и дроби приводятся к норме. */
function normalize_min_tile_bytes(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return 0;
  }
  return Math.min(Math.floor(value), Number.MAX_SAFE_INTEGER);
}

/**
 * Привести произвольное значение к валидным настройкам.
 *
 * Каждое поле проверяется отдельно: испорченный ключ не обнуляет остальные
 * (например, старый файл без `minTileBytes` сохраняет выбранный режим).
 */
export function normalize_settings(value: unknown): DisplaySettings {
  if (!is_record(value)) {
    return { ...DEFAULT_SETTINGS };
  }
  const sizeMode =
    value.sizeMode === "logical" || value.sizeMode === "on_disk"
      ? value.sizeMode
      : DEFAULT_SETTINGS.sizeMode;
  return {
    sizeMode,
    showFilesOnMap:
      typeof value.showFilesOnMap === "boolean"
        ? value.showFilesOnMap
        : DEFAULT_SETTINGS.showFilesOnMap,
    minTileBytes: normalize_min_tile_bytes(value.minTileBytes),
  };
}

/** Разобрать сырую строку localStorage; пусто/мусор — значения по умолчанию. */
export function parse_settings(raw: string | null): DisplaySettings {
  if (raw === null || raw.trim() === "") {
    return { ...DEFAULT_SETTINGS };
  }
  try {
    return normalize_settings(JSON.parse(raw));
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

/** Прочитать настройки; без хранилища (тесты, SSR) — значения по умолчанию. */
export function load_settings(
  storage: SettingsStorage | null = browser_storage(),
): DisplaySettings {
  if (!storage) {
    return { ...DEFAULT_SETTINGS };
  }
  try {
    return parse_settings(storage.getItem(SETTINGS_KEY));
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

/**
 * Сохранить настройки. Ошибка записи (приватный режим, квота) не роняет окно:
 * настройки применятся до перезапуска, но не переживут его.
 */
export function save_settings(
  settings: DisplaySettings,
  storage: SettingsStorage | null = browser_storage(),
): void {
  if (!storage) {
    return;
  }
  try {
    storage.setItem(SETTINGS_KEY, JSON.stringify(normalize_settings(settings)));
  } catch {
    // Запись недоступна — молча продолжаем: приложение работает и без неё.
  }
}

/** Новые настройки из текущих и патча; результат нормализуется. */
export function update_settings(
  current: DisplaySettings,
  patch: Partial<DisplaySettings>,
): DisplaySettings {
  return normalize_settings({ ...current, ...patch });
}

/** Порог в мегабайтах для поля ввода (обратная операция к `set_min_tile_mib`). */
export function min_tile_mib(settings: DisplaySettings): number {
  if (settings.minTileBytes <= 0) {
    return 0;
  }
  return Math.round((settings.minTileBytes / MIB) * 100) / 100;
}

/** Настройки с порогом, заданным в мегабайтах; меньше нуля — без порога. */
export function set_min_tile_mib(
  settings: DisplaySettings,
  mib: number,
): DisplaySettings {
  const bytes =
    Number.isFinite(mib) && mib > 0
      ? Math.min(Math.round(mib * MIB), Number.MAX_SAFE_INTEGER)
      : 0;
  return update_settings(settings, { minTileBytes: bytes });
}
