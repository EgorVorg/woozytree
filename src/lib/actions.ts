// Чистые помощники действий с файлами: формы слова «файл» и текст
// подтверждения. Вынесены из компонента, чтобы проверяться vitest без DOM.

/**
 * Слово «файл» в форме, согласованной с числом: 1 файл, 2 файла, 5 файлов.
 *
 * Русские правила: 11–14 — всегда «файлов», последняя цифра 1 — «файл»
 * (кроме 11), 2–4 — «файла» (кроме 12–14).
 */
export function files_word(count: number): string {
  const absolute = Math.abs(count);
  const lastTwo = absolute % 100;
  const last = absolute % 10;
  if (last === 1 && lastTwo !== 11) return "файл";
  if (last >= 2 && last <= 4 && (lastTwo < 12 || lastTwo > 14)) return "файла";
  return "файлов";
}

/**
 * Слово «папка» в предложном падеже для фразы «в N папках»: «в 1 папке»,
 * «в 2 папках», «в 5 папках». В предложном падеже множественное число всегда
 * «папках», поэтому различается только форма для одного.
 */
export function folders_in_label(count: number): string {
  return count % 10 === 1 && count % 100 !== 11 ? "папке" : "папках";
}

/**
 * Вопрос подтверждения безвозвратного удаления.
 *
 * Мультивыделения сейчас нет (одно действие — один файл), но форма слова
 * считается от числа: диалог обязан оставаться грамотным и при N > 1.
 */
export function permanent_delete_question(count: number): string {
  return `Безвозвратно удалить ${count} ${files_word(count)}?`;
}

/**
 * Почему безвозвратное удаление недоступно для строки-дубликата.
 *
 * `hard_linked` в индексе — это и повторные жёсткие ссылки, и DOS-алиасы
 * коротких имён (8.3): удаление такого имени стирает единственный файл, поэтому
 * безвозвратное удаление для него отключено (в корзину — можно, восстановимо).
 */
export const HARD_LINK_PERMANENT_REASON =
  "Безвозвратное удаление для вторичных ссылок отключено — используйте основную строку файла";

/**
 * Почему безвозвратное удаление недоступно для короткого DOS-имени.
 *
 * Короткое имя (8.3) — второе имя того же файла, а не отдельная копия:
 * безвозвратное удаление по нему уничтожило бы файл, хотя пользователь видел
 * «всего лишь» строку-алиас.
 */
export const ALIAS_PERMANENT_REASON =
  "Безвозвратное удаление для короткого имени (8.3) отключено — это второе имя того же файла, используйте основную строку";

/**
 * Разрешено ли безвозвратное удаление для строки файла.
 *
 * Мультивыделения нет, поэтому решение зависит от пометок строки: для
 * вторичных жёстких ссылок и коротких DOS-имён безвозвратное удаление
 * запрещено, чтобы не потерять единственный файл.
 */
export function permanent_delete_allowed(
  hardLinked: boolean,
  alias = false,
): boolean {
  return !hardLinked && !alias;
}

/**
 * Причина запрета безвозвратного удаления или `null`, если оно разрешено.
 *
 * Алиас проверяется первым: у DOS-имени обычно стоит и `hard_linked`, но
 * пользователю важнее объяснить, что это второе имя, а не «ссылка».
 */
export function permanent_delete_reason(
  hardLinked: boolean,
  alias = false,
): string | null {
  if (alias) return ALIAS_PERMANENT_REASON;
  if (hardLinked) return HARD_LINK_PERMANENT_REASON;
  return null;
}

/** Понятный текст ошибки из промиса: строка, Error или что-то ещё. */
export function error_text(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

/**
 * Скопировать текст в буфер обмена.
 *
 * Основной путь — `navigator.clipboard.writeText` (требование брифа). Он может
 * отказать, если окно не в фокусе или буфер недоступен, поэтому есть запасной
 * путь через `execCommand`: пользователь не должен остаться без копирования.
 */
export async function copy_text(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Буфер через навигатор недоступен — пробуем старый способ ниже.
    }
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.top = "0";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  const copied = document.execCommand("copy");
  document.body.removeChild(area);
  if (!copied) {
    throw new Error("Не удалось скопировать путь в буфер обмена");
  }
}

