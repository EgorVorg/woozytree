//! Операции над путями: корзина, безвозвратное удаление, открытие, проводник.
//!
//! Функции принимают готовые абсолютные пути и не ходят в индекс: путь
//! приходит из окна (его строит `ScanIndex::real_path_of`), а проверка
//! «абсолютный/существует» живёт в командах `lib.rs`. Здесь — только действие.
//!
//! Ошибки не паникуют и не прерывают обработку списка: каждая попадает в отчёт
//! понятным текстом («Файл не найден: …», «Нет доступа: …»).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::model::{DeleteReport, ScanError};

/// Код возврата `ShellExecuteW` больше 32 — успех; 0..=32 — коды ошибок.
const SHELL_EXECUTE_OK: isize = 32;

/// Удалить пути: в корзину (`permanent == false`) или безвозвратно.
///
/// Ошибка по одному пути попадает в `errors` и не мешает удалить остальные:
/// в отчёте видно и сколько удалено, и что именно не получилось.
pub fn delete_paths(paths: &[PathBuf], permanent: bool) -> DeleteReport {
    let mut report = DeleteReport {
        deleted: 0,
        errors: Vec::new(),
    };
    for path in paths {
        let result = if permanent {
            remove_permanently(path)
        } else {
            move_to_trash(path)
        };
        match result {
            Ok(()) => report.deleted = report.deleted.saturating_add(1),
            Err(message) => report.errors.push(message),
        }
    }
    report
}

/// Отправить путь в корзину системным API (`trash`), а не удалением.
fn move_to_trash(path: &Path) -> Result<(), String> {
    // Отдельная проверка существования: у `trash` ошибка канонизации пути
    // звучит технически, а пользователю нужен понятный «Файл не найден».
    if fs::symlink_metadata(path).is_err() {
        return Err(not_found(path));
    }
    trash::delete(path)
        .map_err(|err| format!("Не удалось удалить в корзину «{}»: {err}", path.display()))
}

/// Удалить путь безвозвратно: файл, символическую ссылку или дерево папки.
fn remove_permanently(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|err| path_error("удалить", path, &err))?;
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else if metadata.file_type().is_symlink() && path.is_dir() {
        // Ссылка на папку: удаляем саму ссылку, а не содержимое её цели.
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|err| path_error("удалить", path, &err))
}

/// Открыть путь приложением по умолчанию (ShellExecuteW, операция «open»).
pub fn open_path(path: &Path) -> Result<(), ScanError> {
    shell_execute("open", path, None)
}

/// Показать путь в проводнике с выделением файла: `explorer.exe /select,<path>`.
pub fn reveal_in_explorer(path: &Path) -> Result<(), ScanError> {
    let params = format!("/select,\"{}\"", path.display());
    shell_execute("open", Path::new("explorer.exe"), Some(&params))
}

/// Вызвать `ShellExecuteW` и превратить код ошибки в понятный текст.
fn shell_execute(operation: &str, target: &Path, params: Option<&str>) -> Result<(), ScanError> {
    let operation = wide(operation);
    let target_text = target.to_string_lossy();
    let target_wide = wide(&target_text);
    let params = params.map(wide);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target_wide.as_ptr()),
            params
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code > SHELL_EXECUTE_OK {
        return Ok(());
    }
    Err(shell_error(code, &target_text))
}

/// Текст ошибки `ShellExecuteW`: коды 2/3 — путь не найден, 5 — нет доступа.
fn shell_error(code: isize, target: &str) -> ScanError {
    let message = match code {
        0 => format!("Не удалось открыть «{target}»: недостаточно памяти для операции"),
        2 | 3 => format!("Файл не найден: {target}"),
        5 => format!("Нет доступа: {target}"),
        other => format!("Не удалось открыть «{target}» (код ошибки Windows {other})"),
    };
    ScanError::Io(message)
}

/// Понятный текст ошибки файловой операции для окна.
fn path_error(action: &str, path: &Path, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => not_found(path),
        io::ErrorKind::PermissionDenied => format!("Нет доступа: {}", path.display()),
        _ => format!("Не удалось {action} «{}»: {err}", path.display()),
    }
}

fn not_found(path: &Path) -> String {
    format!("Файл не найден: {}", path.display())
}

/// Строка в UTF-16 с завершающим нулём — так Win32 ждёт пути.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Создать файл в тестовой папке и вернуть его путь.
    fn dir_file(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("тестовый файл должен создаться");
        path
    }

    /// Корзина: файлы исчезают из папки, ошибок нет, `deleted` — ровно два.
    ///
    /// Тест кладёт настоящие (мелкие) файлы в корзину пользователя — это
    /// допустимо: восстановление вручную проверяется отдельно, а здесь важно,
    /// что путь проходит через системный API корзины, а не через удаление.
    #[test]
    fn delete_moves_to_recycle_bin() {
        let dir = super::super::test_dir();
        let first = dir_file(dir.path(), "в-корзину-1.bin", b"one");
        let second = dir_file(dir.path(), "в-корзину-2.bin", b"two");

        let report = delete_paths(&[first.clone(), second.clone()], false);

        assert_eq!(
            report.deleted, 2,
            "оба файла обязаны уйти в корзину: {report:?}"
        );
        assert!(
            report.errors.is_empty(),
            "удаление в корзину не должно давать ошибок: {report:?}"
        );
        assert!(
            !first.exists() && !second.exists(),
            "файлы должны исчезнуть из папки"
        );
    }

    /// Безвозвратное удаление: ровно два из трёх файлов, третий не тронут.
    #[test]
    fn permanent_delete_removes_only_targets() {
        let dir = super::super::test_dir();
        let first = dir_file(dir.path(), "удалить-1.bin", b"one");
        let second = dir_file(dir.path(), "удалить-2.bin", b"two");
        let keep = dir_file(dir.path(), "оставить.bin", b"three");

        let report = delete_paths(&[first.clone(), second.clone()], true);

        assert_eq!(
            report.deleted, 2,
            "должны удалиться ровно два файла: {report:?}"
        );
        assert!(
            report.errors.is_empty(),
            "ошибок быть не должно: {report:?}"
        );
        assert!(!first.exists(), "первый файл должен исчезнуть");
        assert!(!second.exists(), "второй файл должен исчезнуть");
        assert!(keep.exists(), "третий файл не должен пострадать");
    }

    /// Ошибка по несуществующему пути попадает в отчёт и не мешает удалить
    /// остальные пути: паники быть не должно.
    #[test]
    fn delete_reports_errors_without_panicking() {
        let dir = super::super::test_dir();
        let existing = dir_file(dir.path(), "существующий.bin", b"data");
        let missing = dir.path().join("нет-такого-файла.bin");

        let report = delete_paths(&[existing.clone(), missing], true);

        assert_eq!(
            report.deleted, 1,
            "существующий файл обязан удалиться: {report:?}"
        );
        assert_eq!(
            report.errors.len(),
            1,
            "несуществующий путь даёт ровно одну ошибку: {report:?}"
        );
        assert!(
            report.errors[0].contains("Файл не найден"),
            "ошибка должна быть понятной: {}",
            report.errors[0]
        );
        assert!(
            report.errors[0].contains("нет-такого-файла.bin"),
            "в ошибке должен быть путь: {}",
            report.errors[0]
        );
        assert!(!existing.exists(), "существующий файл удалён");
    }

    /// Папка удаляется целиком (безвозвратно) — дерево, а не только верхний
    /// уровень: безвозвратное удаление папки должно убирать и содержимое.
    #[test]
    fn permanent_delete_removes_folder_tree() {
        let dir = super::super::test_dir();
        let folder = dir.path().join("папка-на-удаление");
        fs::create_dir_all(folder.join("вложенная")).expect("папки должны создаться");
        dir_file(&folder, "верхний.bin", b"one");
        dir_file(&folder.join("вложенная"), "глубокий.bin", b"two");

        let report = delete_paths(&[folder.clone()], true);

        assert_eq!(
            report.deleted, 1,
            "папка считается одним удалённым путём: {report:?}"
        );
        assert!(
            report.errors.is_empty(),
            "ошибок быть не должно: {report:?}"
        );
        assert!(!folder.exists(), "папка должна исчезнуть целиком");
    }
}
