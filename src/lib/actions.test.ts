import { describe, expect, it } from "vitest";

import {
  ALIAS_PERMANENT_REASON,
  error_text,
  files_word,
  folders_in_label,
  HARD_LINK_PERMANENT_REASON,
  permanent_delete_allowed,
  permanent_delete_question,
  permanent_delete_reason,
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

describe("folders_in_label", () => {
  it("даёт форму «в N папке/папках»", () => {
    expect(folders_in_label(1)).toBe("папке");
    expect(folders_in_label(21)).toBe("папке");
    expect(folders_in_label(101)).toBe("папке");
    expect(folders_in_label(2)).toBe("папках");
    expect(folders_in_label(5)).toBe("папках");
    expect(folders_in_label(11)).toBe("папках");
    expect(folders_in_label(0)).toBe("папках");
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

describe("permanent_delete_allowed", () => {
  it("запрещает безвозвратное удаление для жёстких ссылок и коротких имён", () => {
    expect(permanent_delete_allowed(false)).toBe(true);
    expect(permanent_delete_allowed(true)).toBe(false);
    expect(permanent_delete_allowed(false, true)).toBe(false);
    expect(permanent_delete_allowed(true, true)).toBe(false);
  });

  it("объясняет причину запрета по-русски", () => {
    expect(HARD_LINK_PERMANENT_REASON).toContain("вторичных ссылок");
    expect(HARD_LINK_PERMANENT_REASON).toContain("основную строку файла");
    expect(ALIAS_PERMANENT_REASON).toContain("короткого имени");
    expect(ALIAS_PERMANENT_REASON).toContain("второе имя того же файла");
  });

  it("выбирает причину по пометке строки", () => {
    expect(permanent_delete_reason(false)).toBeNull();
    expect(permanent_delete_reason(true)).toBe(HARD_LINK_PERMANENT_REASON);
    // У алиаса обычно стоит и hard_linked, но объяснение — про короткое имя.
    expect(permanent_delete_reason(true, true)).toBe(ALIAS_PERMANENT_REASON);
    expect(permanent_delete_reason(false, true)).toBe(ALIAS_PERMANENT_REASON);
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
