import { describe, expect, it } from "vitest";

import {
  error_text,
  files_word,
  permanent_delete_question,
} from "./actions";

describe("files_word", () => {
  it("согласует слово «файл» с числом", () => {
    expect(files_word(1)).toBe("файл");
    expect(files_word(2)).toBe("файла");
    expect(files_word(4)).toBe("файла");
    expect(files_word(5)).toBe("файлов");
    expect(files_word(11)).toBe("файлов");
    expect(files_word(12)).toBe("файлов");
    expect(files_word(21)).toBe("файл");
    expect(files_word(22)).toBe("файла");
    expect(files_word(25)).toBe("файлов");
    expect(files_word(111)).toBe("файлов");
    expect(files_word(0)).toBe("файлов");
  });
});

describe("permanent_delete_question", () => {
  it("спрашивает про безвозвратное удаление с числом выбранного", () => {
    expect(permanent_delete_question(1)).toBe(
      "Безвозвратно удалить 1 файл?",
    );
    expect(permanent_delete_question(3)).toBe(
      "Безвозвратно удалить 3 файла?",
    );
    expect(permanent_delete_question(10)).toBe(
      "Безвозвратно удалить 10 файлов?",
    );
  });
});

describe("error_text", () => {
  it("достаёт текст из строки, Error и прочего", () => {
    expect(error_text("Файл не найден: C:\\temp\\a.bin")).toBe(
      "Файл не найден: C:\\temp\\a.bin",
    );
    expect(error_text(new Error("Нет доступа"))).toBe("Нет доступа");
    expect(error_text(42)).toBe("42");
  });
});
