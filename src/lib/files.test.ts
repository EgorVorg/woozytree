// Тесты чистой логики дерева и таблицы файлов: разворачивание дерева,
// цепочка предков для навигации, допустимые колонки сортировки.
import { describe, expect, it } from "vitest";

import {
  flatten_tree,
  is_file_sort,
  tree_chain,
  type TreeRoot,
} from "./files";
import type { FolderSummary } from "./types";

const ROOT: TreeRoot = {
  name: "C:\\",
  size_on_disk: 10_000,
  file_count: 500,
  folder_count: 4,
};

/** Сводка папки: размеры и счётчики — только для проверки переноса в строку. */
function folder(
  id: number,
  name: string,
  size: number,
  fileCount: number,
  folderCount: number,
): FolderSummary {
  return {
    id,
    name,
    size_on_disk: size,
    size_logical: size,
    file_count: fileCount,
    folder_count: folderCount,
    percent_of_parent: size / ROOT.size_on_disk,
  };
}

function childrenMap(
  entries: Array<[number, FolderSummary[]]>,
): Map<number, FolderSummary[]> {
  return new Map(entries);
}

describe("flatten_tree", () => {
  it("flatten_tree_lists_root_and_expanded_children", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([
        [
          0,
          [
            folder(1, "Windows", 6_000, 300, 2),
            folder(2, "Users", 3_000, 150, 1),
          ],
        ],
      ]),
      new Set([0]),
      new Set(),
    );

    expect(rows.map((row) => [row.id, row.depth])).toEqual([
      [0, 0],
      [1, 1],
      [2, 1],
    ]);
    expect(rows[0]).toMatchObject({
      name: "C:\\",
      size_on_disk: 10_000,
      file_count: 500,
      folder_count: 4,
      has_children: true,
      expanded: true,
    });
    expect(rows[1]).toMatchObject({
      name: "Windows",
      size_on_disk: 6_000,
      file_count: 300,
      folder_count: 2,
      has_children: true,
      expanded: false,
    });
  });

  it("flatten_tree_skips_children_of_collapsed_node", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([
        [0, [folder(1, "Windows", 6_000, 300, 1)]],
        [1, [folder(3, "System32", 4_000, 200, 0)]],
      ]),
      new Set([0]),
      new Set(),
    );

    expect(rows.map((row) => row.id)).toEqual([0, 1]);
  });

  it("flatten_tree_descends_only_through_expanded_nodes", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([
        [0, [folder(1, "Windows", 6_000, 300, 1)]],
        [1, [folder(3, "System32", 4_000, 200, 0)]],
      ]),
      new Set([0, 1]),
      new Set(),
    );

    expect(rows.map((row) => [row.id, row.depth])).toEqual([
      [0, 0],
      [1, 1],
      [3, 2],
    ]);
  });

  it("flatten_tree_marks_unloaded_children_as_loading", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([]),
      new Set([0]),
      new Set([0]),
    );

    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ loading: true, expanded: true });
  });

  it("flatten_tree_does_not_wait_for_children_of_leaf", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([[0, [folder(1, "Windows", 6_000, 300, 0)]]]),
      new Set([0, 1]),
      new Set([1]),
    );

    expect(rows).toHaveLength(2);
    expect(rows[1]).toMatchObject({
      has_children: false,
      expanded: true,
      loading: false,
    });
  });
});

describe("tree_chain", () => {
  it("tree_chain_rebuilds_path_from_flattened_rows", () => {
    const rows = flatten_tree(
      ROOT,
      childrenMap([
        [0, [folder(1, "Windows", 6_000, 300, 1)]],
        [1, [folder(3, "System32", 4_000, 200, 0)]],
      ]),
      new Set([0, 1]),
      new Set(),
    );

    expect(tree_chain(rows, 3)).toEqual([
      { id: 0, name: "C:\\" },
      { id: 1, name: "Windows" },
      { id: 3, name: "System32" },
    ]);
    expect(tree_chain(rows, 0)).toEqual([{ id: 0, name: "C:\\" }]);
  });

  it("tree_chain_returns_null_for_unknown_folder", () => {
    const rows = flatten_tree(ROOT, childrenMap([]), new Set([0]), new Set());
    expect(tree_chain(rows, 42)).toBeNull();
  });
});

describe("is_file_sort", () => {
  it("is_file_sort_accepts_only_core_columns", () => {
    for (const column of ["name", "size_on_disk", "size_logical", "modified"]) {
      expect(is_file_sort(column)).toBe(true);
    }
    for (const column of ["percent_of_parent", "size", "", "NAME"]) {
      expect(is_file_sort(column)).toBe(false);
    }
  });
});
