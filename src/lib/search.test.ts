// Тесты чистой логики панели поиска: форма фильтров → запрос к ядру.
import { describe, expect, it } from "vitest";

import {
  build_search_query,
  date_end_ms,
  date_start_ms,
  empty_search_form,
  has_filters,
  size_bound,
} from "./search";

describe("size_bound", () => {
  it("size_bound_converts_mib_and_gib_to_bytes", () => {
    expect(size_bound("1.5", "mib")).toBe(1_572_864);
    expect(size_bound("1.5", "gib")).toBe(1_610_612_736);
    // Запятая как десятичный разделитель — привычный ввод по-русски.
    expect(size_bound("2,5", "mib")).toBe(2_621_440);
    expect(size_bound("0", "mib")).toBe(0);
    expect(size_bound("", "mib")).toBeNull();
    expect(size_bound("   ", "gib")).toBeNull();
    expect(size_bound("abc", "mib")).toBeNull();
    expect(size_bound("-5", "mib")).toBeNull();
    expect(size_bound("Infinity", "gib")).toBeNull();
  });
});

describe("dates", () => {
  it("date_bounds_use_local_day_start_and_end", () => {
    const start = date_start_ms("2024-03-05");
    const end = date_end_ms("2024-03-05");
    expect(start).toBe(new Date(2024, 2, 5, 0, 0, 0, 0).getTime());
    expect(end).toBe(new Date(2024, 2, 5, 23, 59, 59, 999).getTime());
    expect(start).not.toBeNull();
    expect(end).not.toBeNull();
    expect((end ?? 0) - (start ?? 0)).toBe(86_400_000 - 1);
    expect(date_start_ms("")).toBeNull();
    expect(date_end_ms("")).toBeNull();
    expect(date_start_ms("05.03.2024")).toBeNull();
    expect(date_end_ms("2024-13-40")).toBeNull();
  });
});

describe("build_search_query", () => {
  it("empty_filters_mean_everything", () => {
    const query = build_search_query(empty_search_form());
    expect(query).toEqual({
      name: "",
      min_size: 0,
      max_size: null,
      modified_after: null,
      modified_before: null,
    });
  });

  it("form_filters_become_query_bounds", () => {
    const query = build_search_query({
      name: "  отчёт  ",
      sizeFrom: "10",
      sizeTo: "2",
      sizeUnit: "gib",
      dateFrom: "2024-03-05",
      dateTo: "2024-04-06",
    });
    expect(query.name).toBe("отчёт");
    expect(query.min_size).toBe(10 * 1024 ** 3);
    // Верхняя граница — Option: пустая строка становится null, а не u64::MAX.
    expect(query.max_size).toBe(2 * 1024 ** 3);
    expect(query.modified_after).toBe(new Date(2024, 2, 5).getTime());
    expect(query.modified_before).toBe(
      new Date(2024, 3, 6, 23, 59, 59, 999).getTime(),
    );
  });

  it("upper_size_bound_absent_is_null_not_max", () => {
    const query = build_search_query({
      ...empty_search_form(),
      sizeFrom: "1",
    });
    expect(query.min_size).toBe(1024 ** 2);
    expect(query.max_size).toBeNull();
  });
});

describe("has_filters", () => {
  it("has_filters_detects_any_non_empty_field", () => {
    expect(has_filters(empty_search_form())).toBe(false);
    expect(has_filters({ ...empty_search_form(), name: "еж" })).toBe(true);
    expect(has_filters({ ...empty_search_form(), sizeFrom: "1" })).toBe(true);
    expect(has_filters({ ...empty_search_form(), sizeTo: "1" })).toBe(true);
    expect(has_filters({ ...empty_search_form(), dateFrom: "2024-01-01" })).toBe(
      true,
    );
    expect(has_filters({ ...empty_search_form(), dateTo: "2024-01-01" })).toBe(
      true,
    );
  });
});
