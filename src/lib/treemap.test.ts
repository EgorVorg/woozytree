// Тесты чистых функций карты: хит-тест точка→плитка и разметка вложенности.
// Контракт координат — нормированные 0..1 (см. `TreemapRect` в Rust).
import { describe, expect, it } from "vitest";

import {
  analyze_rects,
  crumbs_for_tile,
  folder_id_for_tile,
  hit_test,
  is_other,
  root_crumb_name,
  type Crumb,
} from "./treemap";
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
      h: 0.5,
      name: "a.bin",
      size: 200,
    });
    // Второй видимый ребёнок A: сумма видимых детей равна размеру папки, как
    // и в реальной выдаче, где дети покрывают родителя.
    const fileA2 = tile({
      x: 0,
      y: 0.5,
      w: 0.75,
      h: 0.5,
      name: "a2.bin",
      size: 100,
    });

    const analyzed = analyze_rects([folderA, folderB, fileA, fileA2]);
    expect(analyzed).toHaveLength(4);

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
      percent_of_parent: 2 / 3,
      has_children: false,
    });
    expect(analyzed[3]).toMatchObject({
      depth: 1,
      parent_name: "A",
      parent_size: 300,
      percent_of_parent: 1 / 3,
    });
  });

  it("analyze_rects_percent_matches_visible_area_when_children_partial", () => {
    // Скрытые файлы: у папки A виден только один ребёнок. Площадь ребёнка
    // занимает всю плитку A, поэтому доля — 1.0, а не «1/3 полной папки»:
    // площадь и процент обязаны считаться от одной базы (видимого).
    const folderA = tile({
      x: 0,
      y: 0,
      w: 1,
      h: 1,
      id: 1,
      name: "A",
      size: 300,
      is_dir: true,
    });
    const onlyChild = tile({
      x: 0,
      y: 0,
      w: 1,
      h: 1,
      id: 2,
      name: "B",
      size: 100,
      is_dir: true,
    });

    const analyzed = analyze_rects([folderA, onlyChild]);
    expect(analyzed[1]).toMatchObject({
      parent_size: 100,
      percent_of_parent: 1,
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

describe("crumbs_for_tile", () => {
  const ROOT: Crumb = { id: 0, name: "C:\\" };
  // Вид корня: Users — плитка верхнего уровня, Music и Documents — её дети.
  // Контракт выдачи: родитель идёт в списке раньше детей.
  const users = tile({
    id: 7,
    name: "Users",
    size: 1000,
    is_dir: true,
    x: 0.5,
    y: 0,
    w: 0.5,
    h: 1,
  });
  const music = tile({
    id: 9,
    name: "Music",
    size: 600,
    is_dir: true,
    x: 0.5,
    y: 0.05,
    w: 0.45,
    h: 0.5,
  });
  const documents = tile({
    id: 8,
    name: "Documents",
    size: 350,
    is_dir: true,
    x: 0.5,
    y: 0.6,
    w: 0.45,
    h: 0.4,
  });
  const rects = [users, music, documents];

  it("crumbs_for_tile_nested_folder_returns_three_links", () => {
    // Клик по Music из корня: онлайн-вид рисует её внутри Users, поэтому путь
    // обязан быть C:\ > Users > Music, а не C:\ > Music.
    expect(crumbs_for_tile(rects, music, [ROOT])).toEqual([
      ROOT,
      { id: 7, name: "Users" },
      { id: 9, name: "Music" },
    ]);
  });

  it("crumbs_for_tile_top_level_folder_extends_root", () => {
    // Папка верхнего уровня: корень + сама папка.
    expect(crumbs_for_tile(rects, users, [ROOT])).toEqual([
      ROOT,
      { id: 7, name: "Users" },
    ]);
  });

  it("crumbs_for_tile_file_is_not_navigation", () => {
    // Файл только подсвечивается: путь не меняется, клик не навигация.
    const file = tile({
      name: "track.mp3",
      size: 10,
      x: 0.5,
      y: 0.05,
      w: 0.1,
      h: 0.1,
    });

    expect(crumbs_for_tile([...rects, file], file, [ROOT])).toEqual([]);
  });

  it("crumbs_for_tile_other_tile_is_not_navigation", () => {
    // Сводная плитка «прочее» (id = 0, не папка) действий не даёт.
    const other = tile({ name: "прочее", x: 0.5, y: 0.9, w: 0.2, h: 0.1 });

    expect(crumbs_for_tile([...rects, other], other, [ROOT])).toEqual([]);
  });

  it("crumbs_for_tile_skips_duplicate_last_crumb", () => {
    // Текущий путь уже оканчивается этим звеном — не добавляем его второй раз.
    expect(
      crumbs_for_tile([users], users, [
        ROOT,
        { id: 7, name: "Users" },
      ]),
    ).toEqual([ROOT, { id: 7, name: "Users" }]);
  });

  it("crumbs_for_tile_finds_copy_of_tile_by_id", () => {
    // Плитка пришла из другого снимка данных (не та же ссылка, что в массиве):
    // путь обязан собраться по (id, is_dir), а не по identity объекта.
    const staleCopy = { ...music };
    expect(crumbs_for_tile(rects, staleCopy, [ROOT])).toEqual([
      ROOT,
      { id: 7, name: "Users" },
      { id: 9, name: "Music" },
    ]);
  });
});

describe("folder_id_for_tile", () => {
  // Вид открытой папки: Users (id 7), внутри Music (id 9), внутри неё файл.
  const users = tile({
    id: 7,
    name: "Users",
    size: 1000,
    is_dir: true,
    x: 0.5,
    y: 0,
    w: 0.5,
    h: 1,
  });
  const music = tile({
    id: 9,
    name: "Music",
    size: 600,
    is_dir: true,
    x: 0.5,
    y: 0.05,
    w: 0.45,
    h: 0.5,
  });
  const track = tile({
    name: "track.mp3",
    size: 10,
    x: 0.55,
    y: 0.1,
    w: 0.1,
    h: 0.1,
  });
  const topFile = tile({
    name: "readme.txt",
    size: 5,
    x: 0,
    y: 0,
    w: 0.1,
    h: 0.1,
  });
  const rects = [users, music, track, topFile];

  it("folder_id_for_tile_uses_nested_parent_folder", () => {
    // Файл нарисован внутри Music, а не в открытой папке: действия должны
    // получить id Music, иначе путь файла будет неверным.
    expect(folder_id_for_tile(rects, track, 0)).toBe(9);
  });

  it("folder_id_for_tile_top_level_file_belongs_to_open_folder", () => {
    expect(folder_id_for_tile(rects, topFile, 0)).toBe(0);
    expect(folder_id_for_tile(rects, topFile, 42)).toBe(42);
  });

  it("folder_id_for_tile_unknown_tile_falls_back_to_open_folder", () => {
    const stranger = tile({ name: "чужой.bin", x: 0.9, y: 0.9, w: 0.05, h: 0.05 });

    expect(folder_id_for_tile(rects, stranger, 5)).toBe(5);
  });
});

describe("root_crumb_name", () => {
  it("root_crumb_name_normalizes_drive_letter", () => {
    // DriveInfo.letter уже оканчивается двоеточием; шаблон не должен давать «C::\».
    expect(root_crumb_name("C:")).toBe("C:\\");
    expect(root_crumb_name("C:\\")).toBe("C:\\");
    expect(root_crumb_name("C")).toBe("C:\\");
    expect(root_crumb_name("D:")).toBe("D:\\");
    expect(root_crumb_name("")).toBe("");
  });
});
