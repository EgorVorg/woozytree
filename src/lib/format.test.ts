// Тесты форматирования размеров: одна десятичная, единицы ГиБ/МиБ/КиБ.
import { describe, expect, it } from "vitest";

import { formatDateTime, formatPercent, formatSize } from "./format";

const KIB = 1024;
const MIB = 1024 * KIB;
const GIB = 1024 * MIB;

describe("formatSize", () => {
  it("format_size_uses_kib_for_small_values", () => {
    expect(formatSize(512)).toBe("0.5 КиБ");
    expect(formatSize(1536)).toBe("1.5 КиБ");
  });

  it("format_size_uses_mib_and_gib", () => {
    expect(formatSize(5 * MIB)).toBe("5.0 МиБ");
    expect(formatSize(3.2 * GIB)).toBe("3.2 ГиБ");
  });

  it("format_size_keeps_one_decimal_for_zero_and_invalid", () => {
    expect(formatSize(0)).toBe("0.0 КиБ");
    expect(formatSize(Number.NaN)).toBe("0.0 КиБ");
  });
});

describe("formatPercent", () => {
  it("format_percent_uses_one_decimal", () => {
    expect(formatPercent(0.256)).toBe("25.6 %");
    expect(formatPercent(0)).toBe("0.0 %");
  });
});

describe("formatDateTime", () => {
  it("format_datetime_uses_day_month_year_and_time", () => {
    // Локальное время: формат не должен зависеть от часового пояса машины.
    const march = new Date(2024, 2, 5, 7, 8).getTime();
    expect(formatDateTime(march)).toBe("05.03.2024 07:08");
    const december = new Date(2023, 11, 31, 23, 59).getTime();
    expect(formatDateTime(december)).toBe("31.12.2023 23:59");
  });

  it("format_datetime_pads_single_digits", () => {
    const january = new Date(2025, 0, 1, 0, 0).getTime();
    expect(formatDateTime(january)).toBe("01.01.2025 00:00");
  });

  it("format_datetime_shows_dash_for_unknown_time", () => {
    expect(formatDateTime(0)).toBe("—");
    expect(formatDateTime(-1)).toBe("—");
    expect(formatDateTime(Number.NaN)).toBe("—");
  });
});
