import { describe, expect, it } from "vitest";

import { same_query_scope } from "./query";

describe("same_query_scope", () => {
  it("разрешает placeholder при смене только сортировки", () => {
    expect(
      same_query_scope(["files", 2, 7, "name", true], ["files", 2, 7]),
    ).toBe(true);
  });

  it("запрещает placeholder при смене папки или скана", () => {
    expect(
      same_query_scope(["files", 2, 7, "name", true], ["files", 2, 8]),
    ).toBe(false);
    expect(
      same_query_scope(["files", 1, 7, "name", true], ["files", 2, 7]),
    ).toBe(false);
    expect(
      same_query_scope(["file_types", 2, 8, "name", true], ["files", 2, 7]),
    ).toBe(false);
  });

  it("без прошлого ключа или при коротком ключе placeholder не нужен", () => {
    expect(same_query_scope(undefined, ["files", 1, 0])).toBe(false);
    expect(same_query_scope(["files", 1], ["files", 1, 0])).toBe(false);
  });
});
