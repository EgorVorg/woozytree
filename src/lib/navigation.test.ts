// Навигация по пути: карта/дерево/поиск приносят полную цепочку, и путь
// обязан заменяться целиком — тест фиксирует это намерение без React.
import { describe, expect, it } from "vitest";

import { path_to_crumb, path_up, replace_path } from "./navigation";
import type { Crumb } from "./types";

const ROOT: Crumb = { id: 0, name: "C:\\" };
const USERS: Crumb = { id: 7, name: "Users" };
const MUSIC: Crumb = { id: 9, name: "Music" };

describe("replace_path", () => {
  it("заменяет путь, а не дописывает его", () => {
    // Клик по Music из карты: пришла цепочка C:\ > Users > Music, хотя текущий
    // путь — C:\ > Users > Music > Albums. Дописывание дало бы дубли.
    const current = [ROOT, USERS, MUSIC, { id: 10, name: "Albums" }];
    const next = [ROOT, USERS, MUSIC];

    expect(replace_path(current, next)).toEqual(next);
    expect(replace_path(current, next)).toHaveLength(3);
  });

  it("пустая цепочка оставляет путь прежним", () => {
    expect(replace_path([ROOT, USERS], [])).toEqual([ROOT, USERS]);
  });

  it("возвращает копии звеньев, а не тот же массив", () => {
    const next = [ROOT, USERS];
    const result = replace_path([], next);
    expect(result).toEqual(next);
    expect(result).not.toBe(next);
    expect(result[0]).not.toBe(ROOT);
  });
});

describe("path_to_crumb", () => {
  it("отрезает всё глубже выбранной крошки", () => {
    expect(path_to_crumb([ROOT, USERS, MUSIC], USERS.id)).toEqual([
      ROOT,
      USERS,
    ]);
    expect(path_to_crumb([ROOT, USERS, MUSIC], ROOT.id)).toEqual([ROOT]);
    expect(path_to_crumb([ROOT, USERS, MUSIC], MUSIC.id)).toEqual([
      ROOT,
      USERS,
      MUSIC,
    ]);
  });

  it("неизвестная крошка не меняет путь", () => {
    expect(path_to_crumb([ROOT, USERS], 999)).toEqual([ROOT, USERS]);
  });
});

describe("path_up", () => {
  it("поднимает на уровень вверх, но не с корня", () => {
    expect(path_up([ROOT, USERS, MUSIC])).toEqual([ROOT, USERS]);
    expect(path_up([ROOT, USERS])).toEqual([ROOT]);
    expect(path_up([ROOT])).toEqual([ROOT]);
    expect(path_up([])).toEqual([]);
  });
});
