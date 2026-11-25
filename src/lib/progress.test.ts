// Процент заполнения прогресс-бара: единая формула для живого и остановленного
// скана. База — занятое место СКАНИРУЕМОГО диска на момент старта, а не текущий
// выбранный диск: иначе смена диска после остановки пересчитывала бы процент уже
// собранных данных.
import { describe, expect, it } from "vitest";

import { scanPercent } from "./progress";

describe("scanPercent", () => {
  it("percent_is_relative_to_scanned_volume", () => {
    expect(
      scanPercent({
        scannedBytes: 250,
        scannedUsed: 1000,
        scanning: true,
        cancelled: false,
        finished: false,
      }),
    ).toBe(25);
  });

  it("live_percent_never_reaches_one_hundred", () => {
    expect(
      scanPercent({
        scannedBytes: 1200,
        scannedUsed: 1000,
        scanning: true,
        cancelled: false,
        finished: false,
      }),
    ).toBe(99);
  });

  it("cancelled_scan_keeps_partial_percent", () => {
    expect(
      scanPercent({
        scannedBytes: 625,
        scannedUsed: 1000,
        scanning: false,
        cancelled: true,
        finished: false,
      }),
    ).toBe(63);
  });

  it("completed_scan_is_one_hundred", () => {
    expect(
      scanPercent({
        scannedBytes: 800,
        scannedUsed: 1000,
        scanning: false,
        cancelled: false,
        finished: true,
      }),
    ).toBe(100);
  });

  it("no_data_is_zero", () => {
    expect(
      scanPercent({
        scannedBytes: 0,
        scannedUsed: 0,
        scanning: true,
        cancelled: false,
        finished: false,
      }),
    ).toBe(0);
    expect(
      scanPercent({
        scannedBytes: 0,
        scannedUsed: 1000,
        scanning: false,
        cancelled: false,
        finished: false,
      }),
    ).toBe(0);
  });
});
