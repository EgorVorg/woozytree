// Настройки отображения: значения по умолчанию, сохранение в localStorage и
// устойчивость к мусору (испорченный JSON, чужие типы, отрицательный порог).
import { describe, expect, it } from "vitest";

import {
  DEFAULT_SETTINGS,
  SETTINGS_KEY,
  load_settings,
  min_tile_mib,
  normalize_settings,
  save_settings,
  set_min_tile_mib,
  update_settings,
  type DisplaySettings,
} from "./settings";

/** localStorage-подобное хранилище: тесты не зависят от браузерного окружения. */
class MemoryStorage {
  private readonly items = new Map<string, string>();

  getItem(key: string): string | null {
    return this.items.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.items.set(key, value);
  }

  removeItem(key: string): void {
    this.items.delete(key);
  }

  /** Сырое содержимое ключа — для проверки формата записи. */
  raw(key: string): string | null {
    return this.getItem(key);
  }
}

const MIB = 1024 * 1024;

describe("settings", () => {
  it("settings_roundtrip_localstorage", () => {
    const storage = new MemoryStorage();

    // Пустое хранилище — дефолты: на диске, файлы на карте, без порога.
    expect(load_settings(storage)).toEqual(DEFAULT_SETTINGS);
    expect(load_settings(storage)).toEqual({
      sizeMode: "on_disk",
      showFilesOnMap: true,
      minTileBytes: 0,
    });

    // Сохранение и чтение: значения переживают перезапуск без потерь.
    const custom: DisplaySettings = {
      sizeMode: "logical",
      showFilesOnMap: false,
      minTileBytes: 5 * MIB,
    };
    save_settings(custom, storage);
    expect(storage.raw(SETTINGS_KEY)).toBe(JSON.stringify(custom));
    expect(load_settings(storage)).toEqual(custom);

    // Частичное обновление: патч сливается с текущими, а не с дефолтами.
    const patched = update_settings(custom, { minTileBytes: 0 });
    expect(patched).toEqual({
      sizeMode: "logical",
      showFilesOnMap: false,
      minTileBytes: 0,
    });
    save_settings(patched, storage);
    expect(load_settings(storage)).toEqual(patched);
  });

  it("garbage_becomes_defaults", () => {
    const storage = new MemoryStorage();
    for (const garbage of [
      "",
      "not json",
      "null",
      "42",
      '"on_disk"',
      "[1, 2]",
      '{"sizeMode": "weird"}',
      '{"sizeMode": 7, "showFilesOnMap": "yes", "minTileBytes": -5}',
      '{"minTileBytes": "много"}',
    ]) {
      storage.setItem(SETTINGS_KEY, garbage);
      expect(load_settings(storage), `мусор: ${garbage}`).toEqual(DEFAULT_SETTINGS);
    }

    // Испорченный только один ключ — остальные валидные поля сохраняются.
    storage.setItem(
      SETTINGS_KEY,
      '{"sizeMode": "logical", "showFilesOnMap": 1, "minTileBytes": 1024}',
    );
    expect(load_settings(storage)).toEqual({
      sizeMode: "logical",
      showFilesOnMap: true,
      minTileBytes: 1024,
    });

    // Не-конечные и дробные значения: NaN/Infinity не проходят, дробь округляется
    // вниз до байта.
    expect(
      normalize_settings({ minTileBytes: Number.NaN }).minTileBytes,
    ).toBe(0);
    expect(
      normalize_settings({ minTileBytes: Number.POSITIVE_INFINITY }).minTileBytes,
    ).toBe(0);
    expect(normalize_settings({ minTileBytes: 100.9 }).minTileBytes).toBe(100);
    expect(normalize_settings({ minTileBytes: -100 }).minTileBytes).toBe(0);
  });

  it("min_tile_mib_roundtrips_bytes", () => {
    const withThreshold = set_min_tile_mib(DEFAULT_SETTINGS, 1.5);
    expect(withThreshold.minTileBytes).toBe(Math.round(1.5 * MIB));
    expect(min_tile_mib(withThreshold)).toBe(1.5);

    expect(min_tile_mib(DEFAULT_SETTINGS)).toBe(0);
    // Отрицательное/нечисловое поле — 0 МиБ, без отрицательного порога.
    expect(min_tile_mib(set_min_tile_mib(DEFAULT_SETTINGS, -3))).toBe(0);
    expect(set_min_tile_mib(DEFAULT_SETTINGS, Number.NaN).minTileBytes).toBe(0);
  });
});
