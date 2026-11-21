//! Сборка приложения WoozyTree: команды Tauri и мост между сканером и окном.

pub mod model;
pub mod scan;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use model::{DriveInfo, FolderSummary, ScanError, ScanProgress, ScanTotals, TreemapRect};
use scan::index::ScanIndex;
use scan::{ScanService, ScanSink, ScanTarget, drive_root, run_scan};

/// Состояние приложения: сервис сканирования живёт всё время работы окна.
struct AppState {
    service: Arc<ScanService>,
}

/// Итоговое событие сканирования: отправляем его после сохранения индекса.
enum FinalEvent {
    Done(ScanTotals),
    Error(String),
}

/// Приёмник событий: каждое событие уходит в окно под своим именем.
struct TauriSink {
    app: AppHandle,
    /// Сервис и флаг запуска: события устаревшего (уже отменённого новым
    /// запуском) сканирования в окно не отправляются.
    service: Arc<ScanService>,
    cancel: Arc<AtomicBool>,
    /// Итог откладывается: по `scan://done` фронтенд может сразу запросить
    /// дочерние папки, и готовый индекс должен быть уже доступен.
    final_event: Option<FinalEvent>,
    /// Когда последний раз отправляли прогресс: окно не должно захлёбываться.
    last_progress: Option<Instant>,
}

/// Ядро сообщает прогресс каждые 1000 записей MFT — это до ~150 событий в
/// секунду. Столько перерисовок окно не переваривает: счётчики не успевают
/// отрисоваться, а нажатия кнопок обрабатываются с задержкой. Для «живых»
/// цифр достаточно 10 обновлений в секунду; итоговые значения приходят
/// событием `scan://done` и не зависят от этого ограничения.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

impl TauriSink {
    fn new(app: AppHandle, service: Arc<ScanService>, cancel: Arc<AtomicBool>) -> TauriSink {
        TauriSink {
            app,
            service,
            cancel,
            final_event: None,
            last_progress: None,
        }
    }

    /// Этот запуск всё ещё актуален (новый скан не начат).
    fn is_current(&self) -> bool {
        self.service.is_current(&self.cancel)
    }

    /// Отправить отложенные итоги (или ошибку) в окно.
    fn emit_final(&mut self) {
        let final_event = self.final_event.take();
        if !self.is_current() {
            return;
        }
        match final_event {
            Some(FinalEvent::Done(totals)) => {
                let _ = self.app.emit("scan://done", totals);
            }
            Some(FinalEvent::Error(text)) => {
                let _ = self.app.emit("scan://error", text);
            }
            None => {}
        }
    }
}

impl ScanSink for TauriSink {
    fn progress(&mut self, p: &ScanProgress) {
        if !self.is_current() {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_progress
            .is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
        if !due {
            return;
        }
        self.last_progress = Some(now);
        let _ = self.app.emit("scan://progress", p);
    }

    fn top_folders(&mut self, rows: &[FolderSummary]) {
        if !self.is_current() {
            return;
        }
        let _ = self.app.emit("scan://top", rows);
    }

    fn treemap(&mut self, rects: &[TreemapRect]) {
        if !self.is_current() {
            return;
        }
        let _ = self.app.emit("scan://treemap", rects);
    }

    fn done(&mut self, r: Result<&ScanTotals, &ScanError>) {
        self.final_event = Some(match r {
            Ok(totals) => FinalEvent::Done(totals.clone()),
            Err(err) => FinalEvent::Error(error_text(err)),
        });
    }
}

/// Русский текст ошибки для окна.
///
/// Обход при нехватке прав запускается автоматически (`run_scan`), поэтому
/// отдельного совета «попробуйте обходом» здесь нет: при нехватке прав ошибка
/// показывается уже после того, как не удался и обход.
fn error_text(err: &ScanError) -> String {
    match err {
        ScanError::VolumeAccess(msg) => format!("Нет доступа к тому: {msg}"),
        ScanError::Io(msg) => format!("Ошибка ввода-вывода: {msg}"),
        ScanError::Parse(msg) => format!("Не удалось разобрать данные тома: {msg}"),
    }
}

/// Имя корневой папки индекса: буква диска или имя выбранной папки.
fn root_name(target: &ScanTarget) -> String {
    match target {
        ScanTarget::Drive(letter) => drive_root(letter),
        ScanTarget::Folder(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
    }
}

/// Итоги, которые можно сохранить для команды `get_scan_totals`.
///
/// Частичный результат отменённого скана не выдаём за финальный: окно получает
/// его событием `scan://done` с `cancelled == true` и само помечает данные.
fn final_totals(result: &Result<ScanTotals, ScanError>) -> Option<ScanTotals> {
    match result {
        Ok(totals) if !totals.cancelled => Some(totals.clone()),
        _ => None,
    }
}

// Команда асинхронная: Win32-опрос дисков может надолго зависнуть на отключённом
// сетевом диске и заморозить главный поток окна (Tauri по умолчанию выполняет
// синхронные команды на нём).
#[tauri::command(async)]
fn list_drives() -> Vec<DriveInfo> {
    scan::list_drives()
}

#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    target: ScanTarget,
) -> Result<(), String> {
    let service = Arc::clone(&state.service);
    // Новый запуск: прошлый просим остановиться, чтобы два скана не спорили.
    let cancel = service.begin();
    let index_name = root_name(&target);

    std::thread::Builder::new()
        .name("woozytree-scan".to_string())
        .spawn(move || {
            let mut index = ScanIndex::new(&index_name);
            let mut sink = TauriSink::new(app, Arc::clone(&service), Arc::clone(&cancel));
            let result = run_scan(&target, &mut index, &mut sink, &cancel);
            service.finish(&cancel, Arc::new(index), final_totals(&result));
            // Итоги уходят после сохранения индекса — без гонки с get_children.
            sink.emit_final();
        })
        .map_err(|err| format!("Не удалось запустить сканирование: {err}"))?;

    Ok(())
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    state.service.cancel();
}

#[tauri::command]
fn get_children(state: State<'_, AppState>, folder_id: u32) -> Result<Vec<FolderSummary>, String> {
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    // Идентификаторы папок выдаются по порядку (0 — корень), поэтому последний
    // известный id равен числу папок в корне. Так чужой id отсекается без паники.
    let max_id = index.folder_totals(index.root()).folder_count;
    if folder_id > max_id {
        return Err(format!("Папка с id {folder_id} не найдена."));
    }

    Ok(index.child_folders(folder_id))
}

#[tauri::command]
fn get_scan_totals(state: State<'_, AppState>) -> Result<ScanTotals, String> {
    state
        .service
        .totals()
        .ok_or_else(|| "Сканирование ещё не завершено.".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            service: Arc::new(ScanService::new()),
        })
        .invoke_handler(tauri::generate_handler![
            list_drives,
            start_scan,
            cancel_scan,
            get_children,
            get_scan_totals
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(cancelled: bool) -> ScanTotals {
        ScanTotals {
            files: 1,
            folders: 1,
            size_on_disk: 10,
            size_logical: 10,
            skipped: 0,
            cancelled,
            elapsed_ms: 1,
        }
    }

    #[test]
    fn final_totals_keeps_only_completed_run() {
        assert!(final_totals(&Ok(totals(false))).is_some());
        assert!(
            final_totals(&Ok(totals(true))).is_none(),
            "частичные итоги отменённого скана нельзя отдавать как финальные"
        );
        assert!(final_totals(&Err(ScanError::Io("диск отвалился".into()))).is_none());
    }

    #[test]
    fn volume_access_error_does_not_advise_manual_walk() {
        let text = error_text(&ScanError::VolumeAccess("нет прав".into()));
        assert!(text.contains("Нет доступа к тому: нет прав"), "{text}");
        assert!(
            !text.contains("Попробуйте сканировать папку обходом"),
            "обход теперь запускается автоматически, совет вручную недостижим: {text}"
        );
    }
}
