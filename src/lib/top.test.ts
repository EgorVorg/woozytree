import { describe, expect, it } from "vitest";

import { rank_top_folders } from "./top";
import type { FolderSummary } from "./types";

/** Папка с двумя размерами и остальными полями по умолчанию. */
function folder(
  id: number,
  name: string,
  size_on_disk: number,
  size_logical: number,
): FolderSummary {
  return {
    id,
    name,
    size_on_disk,
    size_logical,
    file_count: 1,
    folder_count: 0,
    percent_of_parent: 0,
  };
}

const ROWS = [
  folder(1, "диск-крупный", 900, 100),
  folder(2, "факт-крупный", 100, 900),
  folder(3, "средний", 500, 500),
  folder(4, "крошка", 1, 1),
];

describe("rank_top_folders", () => {
  it("сортирует по занятому месту в режиме «на диске»", () => {
    expect(rank_top_folders(ROWS, "on_disk").map((row) => row.name)).toEqual([
      "диск-крупный",
      "средний",
      "факт-крупный",
      "крошка",
    ]);
  });

  it("сортирует по фактическому размеру в режиме «фактический»", () => {
    expect(rank_top_folders(ROWS, "logical").map((row) => row.name)).toEqual([
      "факт-крупный",
      "средний",
      "диск-крупный",
      "крошка",
    ]);
  });

  it("ограничивает вывод и не мутирует вход", () => {
    const input = [...ROWS];
    expect(rank_top_folders(input, "on_disk", 2)).toHaveLength(2);
    expect(input.map((row) => row.id)).toEqual([1, 2, 3, 4]);
    expect(rank_top_folders(ROWS, "on_disk", 0)).toEqual([]);
  });

  it("при равных размерах сортирует по имени", () => {
    const tied = [folder(1, "б", 10, 10), folder(2, "а", 10, 10)];
    expect(rank_top_folders(tied, "on_disk").map((row) => row.name)).toEqual([
      "а",
      "б",
    ]);
  });
});
