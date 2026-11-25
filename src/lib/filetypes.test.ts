import { describe, expect, it } from "vitest";

import {
  DEFAULT_TYPE_SORT,
  default_type_sort,
  extension_label,
  next_type_sort,
  size_sort_column,
  tile_extension,
} from "./filetypes";

describe("extension_label", () => {
  it("для файла без расширения показывает «(нет)»", () => {
    expect(extension_label("")).toBe("(нет)");
  });

  it("обычное расширение не меняет", () => {
    expect(extension_label("exe")).toBe("exe");
  });
});

describe("tile_extension", () => {
  it("плитка «(нет)» — это пустое расширение", () => {
    expect(tile_extension("(нет)")).toBe("");
  });

  it("имя плитки-расширения возвращается как есть", () => {
    expect(tile_extension("mp4")).toBe("mp4");
  });
});

describe("next_type_sort", () => {  it("повторный клик по той же колонке разворачивает порядок", () => {
    const next = next_type_sort({ sort: "size_on_disk", desc: true }, "size_on_disk");
    expect(next).toEqual({ sort: "size_on_disk", desc: false });
    expect(next_type_sort(next, "size_on_disk")).toEqual({
      sort: "size_on_disk",
      desc: true,
    });
  });

  it("новая колонка-размер начинается с крупного, имя — с возрастания", () => {
    expect(next_type_sort(DEFAULT_TYPE_SORT, "count")).toEqual({
      sort: "count",
      desc: true,
    });
    expect(next_type_sort(DEFAULT_TYPE_SORT, "size_logical")).toEqual({
      sort: "size_logical",
      desc: true,
    });
    expect(next_type_sort(DEFAULT_TYPE_SORT, "name")).toEqual({
      sort: "name",
      desc: false,
    });
  });
});

describe("default_type_sort", () => {
  it("следует режиму размера, как таблица файлов", () => {
    expect(size_sort_column("on_disk")).toBe("size_on_disk");
    expect(size_sort_column("logical")).toBe("size_logical");
    expect(default_type_sort("on_disk")).toEqual({
      sort: "size_on_disk",
      desc: true,
    });
    expect(default_type_sort("logical")).toEqual({
      sort: "size_logical",
      desc: true,
    });
  });
});
