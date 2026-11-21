// Тесты чистых функций карты: хит-тест точка→плитка и разметка вложенности.
// Контракт координат — нормированные 0..1 (см. `TreemapRect` в Rust).
import { describe, expect, it } from "vitest";

import { analyze_rects, hit_test, is_other } from "./treemap";
import type { TreemapRect } from "./types";

/** Плитка с координатами; по умолчанию — папка нулевого размера-заглушки. */
function tile(
  overrides: Partial<TreemapRect> & Pick<TreemapRect, "x" | "y" | "w" | "h">,
): TreemapRect {
  return {
    id: 0,
    name: "плитка",
    size: 0,
    is_dir: false,
    ...overrides,
  };
}

describe("hit_test", () => {
  it("hit_test_finds_tile_under_point", () => {
    const left = tile({ x: 0, y: 0, w: 0.5, h: 0.5, name: "слева" });
    const right = tile({ x: 0.5, y: 0, w: 0.5, h: 0.5, name: "справа" });
    const rects = [left, right];

    expect(hit_test(rects, 0.25, 0.25)).toBe(left);
    expect(hit_test(rects, 0.75, 0.25)).toBe(right);
  });

  it("hit_test_prefers_topmost_nested_tile", () => {
    // Родитель лежит в списке раньше ребёнка: ребёнок нарисован поверх.
    const parent = tile({ x: 0, y: 0, w: 1, h: 1, name: "родитель" });
    const child = tile({ x: 0.1, y: 0.1, w: 0.2, h: 0.2, name: "ребёнок" });

    expect(hit_test([parent, child], 0.2, 0.2)).toBe(child);
    // Вне ребёнка попадание достаётся родителю.
    expect(hit_test([parent, child], 0.8, 0.8)).toBe(parent);
  });

  it("hit_test_handles_shared_boundary", () => {
    // Общая граница входит в обе плитки: побеждает последняя (верхняя) в списке.
    const left = tile({ x: 0, y: 0, w: 0.5, h: 1, name: "слева" });
    const right = tile({ x: 0.5, y: 0, w: 0.5, h: 1, name: "справа" });

    expect(hit_test([left, right], 0.5, 0.5)).toBe(right);
    // Правая и нижняя границы контейнера тоже включены.
    expect(hit_test([left, right], 1, 1)).toBe(right);
    // Левая граница принадлежит только первой плитке.
    expect(hit_test([left, right], 0, 0)).toBe(left);
  });

  it("hit_test_skips_zero_area_tiles", () => {
    const real = tile({ x: 0, y: 0, w: 1, h: 1, name: "настоящая" });
    const empty = tile({ x: 1, y: 1, w: 0, h: 0, name: "нулевая" });

    expect(hit_test([real, empty], 1, 1)).toBe(real);
  });

  it("hit_test_returns_null_in_gaps", () => {
    const rects = [tile({ x: 0.1, y: 0.1, w: 0.2, h: 0.2, name: "одна" })];

    expect(hit_test(rects, 0.8, 0.8)).toBeNull();
    expect(hit_test(rects, 0.5, 0.05)).toBeNull();
    expect(hit_test([], 0.5, 0.5)).toBeNull();
  });
});

describe("analyze_rects", () => {
  it("analyze_rects_marks_depth_and_parent_percent", () => {
    // Контейнер: папка A (75 %) и папка B (25 %); внутри A лежит файл (50 % A).
    const folderA = tile({
      x: 0,
      y: 0,
      w: 0.75,
      h: 1,
      id: 1,
      name: "A",
      size: 300,
      is_dir: true,
    });
    const folderB = tile({
      x: 0.75,
      y: 0,
      w: 0.25,
      h: 1,
      id: 2,
      name: "B",
      size: 100,
      is_dir: true,
    });
    const fileA = tile({
      x: 0,
      y: 0,
      w: 0.75,
      h: 1,
      name: "a.bin",
      size: 150,
    });

    const analyzed = analyze_rects([folderA, folderB, fileA]);
    expect(analyzed).toHaveLength(3);

    expect(analyzed[0]).toMatchObject({
      depth: 0,
      parent_name: null,
      parent_size: 400,
      percent_of_parent: 0.75,
      has_children: true,
    });
    expect(analyzed[1]).toMatchObject({
      depth: 0,
      parent_name: null,
      parent_size: 400,
      percent_of_parent: 0.25,
      has_children: false,
    });
    expect(analyzed[2]).toMatchObject({
      depth: 1,
      parent_name: "A",
      parent_size: 300,
      percent_of_parent: 0.5,
      has_children: false,
    });
  });

  it("analyze_rects_handles_empty_input", () => {
    expect(analyze_rects([])).toEqual([]);
  });
});

describe("is_other", () => {
  it("is_other_matches_only_placeholder_rect", () => {
    expect(is_other(tile({ x: 0, y: 0, w: 1, h: 1, name: "прочее" }))).toBe(
      true,
    );
    expect(
      is_other(
        tile({ x: 0, y: 0, w: 1, h: 1, name: "прочее", is_dir: true, id: 7 }),
      ),
    ).toBe(false);
    expect(
      is_other(tile({ x: 0, y: 0, w: 1, h: 1, name: "прочее.txt", size: 10 })),
    ).toBe(false);
  });
});
