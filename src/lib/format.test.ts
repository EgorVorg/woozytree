// Тесты форматирования размеров: одна десятичная, единицы ГиБ/МиБ/КиБ.
import { describe, expect, it } from "vitest";

import { formatPercent, formatSize } from "./format";

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
