//! Сервис сканирования: выбор движка, события для интерфейса и общее состояние.
//!
//! Движок выбирается по типу цели: NTFS-том читается напрямую через `$MFT`,
//! всё остальное (не-NTFS, сетевые диски, отдельная папка) — обходом папок.

pub mod index;
pub mod mft;
pub mod volume;
pub mod walk;

#[cfg(test)]
mod test_support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetLogicalDriveStringsW, GetVolumeInformationW,
};
use windows::core::PCWSTR;

use crate::model::{
    DriveInfo, FolderId, FolderSummary, ScanError, ScanProgress, ScanTotals, SizeMode, TreemapRect,
};
use crate::scan::index::ScanIndex;
use crate::scan::mft::scan_ntfs;
use crate::scan::walk::walk;

// Что сканировать, — часть модели данных: `ScanTarget` живёт в `model.rs`,
// здесь он реэкспортируется, чтобы прежние пути `scan::ScanTarget` работали.
pub use crate::model::ScanTarget;

/// Сколько дочерних папок корня отдавать в живом топе как кандидатов.
///
/// Окно показывает топ-20, но сортирует его по выбранному режиму размера
/// («на диске» или «фактический»). Если ядро пришлёт ровно 20 папок,
/// отобранных по занятому месту, папка с большим фактическим размером, но
/// скромным занятым может не попасть в список вовсе. Поэтому кандидатов
/// больше — 100: их хватает, чтобы топ-20 по любому режиму был честным, а
/// JSON живого события остаётся небольшим.
const TOP_FOLDER_CANDIDATES: usize = 100;

/// Период живых событий (топ папок и карта) в приложении — раз в секунду.
///
/// Тесты передают `Duration::ZERO` через [`run_scan_with_interval`], чтобы
/// получать события на каждом батче движка, а не ждать секунду.
const LIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Бюджет прямоугольников живой карты: столько же отдаёт команда `get_treemap`
/// для уровня корня. Больше — дороже JSON и отрисовка, меньше — теряется детализация.
const LIVE_TREEMAP_RECTS: usize = 2000;

/// Настройки отображения, зафиксированные на момент старта сканирования.
///
/// Настройки живут на фронтенде, но живую карту строит ядро по событиям скана.
/// Снимок передаётся в `start_scan`: без него живая карта рисовалась бы с
/// параметрами по умолчанию (размер на диске, файлы видны, без порога), а
/// пользователь видел бы одно, пока идёт скан, и другое — после него.
#[derive(Debug, Clone, Copy)]
pub struct DisplayOptions {
    pub size_mode: SizeMode,
    pub include_files: bool,
    pub min_tile_bytes: u64,
}

impl Default for DisplayOptions {
    fn default() -> DisplayOptions {
        DisplayOptions {
            size_mode: SizeMode::OnDisk,
            include_files: true,
            min_tile_bytes: 0,
        }
    }
}

/// Приёмник событий сканирования (в приложении — отправка в окно).
pub trait ScanSink {
    fn progress(&mut self, p: &ScanProgress);
    fn top_folders(&mut self, rows: &[FolderSummary]);
    /// Живая карта: те же прямоугольники, что отдаёт команда `get_treemap`,
    /// для корня сканирования и с тем же бюджетом.
    fn treemap(&mut self, rects: &[TreemapRect]);
    fn done(&mut self, r: Result<&ScanTotals, &ScanError>);
}

/// Периодические события для интерфейса: топ папок и карта не чаще раза в
/// `interval` и не реже раза в секунду на живом сканировании.
struct LiveEmitter {
    interval: Duration,
    last_emit: Instant,
    /// Снимок настроек отображения на старте скана: живая карта строится по нему.
    display: DisplayOptions,
}

impl LiveEmitter {
    fn new(interval: Duration, display: DisplayOptions) -> LiveEmitter {
        LiveEmitter {
            interval,
            last_emit: Instant::now(),
            display,
        }
    }

    /// Прогресс уходит всегда, топ и карта — по таймеру. При нулевом интервале
    /// (тесты) событие приходит на каждом батче движка.
    ///
    /// Размер для экрана берём из индекса, а не из цифры движка: движок считает
    /// «сырые» байты, где повторные жёсткие ссылки учтены дважды. Индекс их
    /// дедуплицирует, поэтому прогресс на экране совпадает с живым топом и
    /// итогами, а не расходится с ними на десятки процентов.
    fn on_progress(&mut self, sink: &mut dyn ScanSink, progress: &ScanProgress, index: &ScanIndex) {
        let live = ScanProgress {
            size_on_disk: index.folder_totals(index.root()).size_on_disk,
            ..progress.clone()
        };
        sink.progress(&live);
        if self.interval.is_zero() || self.last_emit.elapsed() >= self.interval {
            self.emit(sink, index);
        }
    }

    fn emit(&mut self, sink: &mut dyn ScanSink, index: &ScanIndex) {
        let rows = top_rows(index);
        sink.top_folders(&rows);
        sink.treemap(&live_treemap(index, self.display));
        self.last_emit = Instant::now();
    }
}

/// Живая карта корня сканирования: та же раскладка, что отдаёт `get_treemap`.
///
/// Параметры отображения — снимок [`DisplayOptions`] со старта скана: живая
/// карта уважает выбранный размер, скрытие файлов и порог мелочи. После
/// завершения скана карта всё равно перезапрашивается командой с актуальными
/// настройками (они могли измениться во время скана).
fn live_treemap(index: &ScanIndex, display: DisplayOptions) -> Vec<TreemapRect> {
    crate::treemap::layout::treemap_for_folder(
        index,
        index.root(),
        LIVE_TREEMAP_RECTS,
        display.size_mode,
        display.include_files,
        display.min_tile_bytes,
    )
}

/// Дочерние папки корня для живого топа: кандидаты по занятому месту.
///
/// Окно само выбирает из них топ-20 по выбранному режиму размера.
fn top_rows(index: &ScanIndex) -> Vec<FolderSummary> {
    index
        .child_folders(index.root())
        .into_iter()
        .take(TOP_FOLDER_CANDIDATES)
        .collect()
}

/// Что делать с ошибкой: показать фатально или предложить обход папок.
#[derive(Debug, PartialEq, Eq)]
pub enum FallbackAction {
    ShowFatal,
    OfferWalkFallback,
}

/// Классифицировать ошибку сканирования.
///
/// Нет доступа к тому (нет прав администратора, не NTFS) — не приговор: тём же
/// охватом можно пройтись обходом папок. Остальные ошибки (ввод-вывод, разбор)
/// обходом не лечатся.
pub fn classify_error(err: &ScanError) -> FallbackAction {
    match err {
        ScanError::VolumeAccess(_) => FallbackAction::OfferWalkFallback,
        ScanError::Io(_) | ScanError::Parse(_) => FallbackAction::ShowFatal,
    }
}

/// Нехватку прав на том лечим тем же охватом через обход папок.
///
/// Быстрый движок NTFS требует прав администратора; обход читает тот же корень
/// обычными API файловой системы и работает без них. Ошибку показываем, только
/// если не удался и обход, — иначе пользователь увидел бы «нет доступа» при
/// успешном результате.
fn with_walk_fallback(
    fast: Result<ScanTotals, ScanError>,
    walk: impl FnOnce() -> Result<ScanTotals, ScanError>,
) -> Result<ScanTotals, ScanError> {
    match fast {
        Err(ScanError::VolumeAccess(fast_error)) => walk().map_err(|walk_error| {
            ScanError::VolumeAccess(format!(
                "{fast_error}; обход папок тоже не удался: {}",
                error_message(&walk_error)
            ))
        }),
        other => other,
    }
}

/// Текст ошибки без префикса варианта — для составных сообщений.
fn error_message(err: &ScanError) -> &str {
    match err {
        ScanError::VolumeAccess(message) | ScanError::Io(message) | ScanError::Parse(message) => {
            message
        }
    }
}

/// Прогресс и итоги наружу в одном месте: движки только сообщают цифры.
///
/// Живые события (топ папок и карта) отправляются таймером раз в секунду.
/// Движок держит `&mut ScanIndex` всё время работы, поэтому срез дерева
/// приходит в колбэк прогресса вместе с индексом — иначе `run_scan` не смог бы
/// снять топ, пока идёт сканирование.
pub fn run_scan(
    target: &ScanTarget,
    index: &mut ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
) -> Result<ScanTotals, ScanError> {
    run_scan_with_display(target, index, sink, cancel, DisplayOptions::default())
}

/// [`run_scan`] со снимком настроек отображения для живых событий.
///
/// Приложение передаёт сюда настройки со старта скана: живая карта и топ
/// строятся по ним, а не по значениям по умолчанию.
pub fn run_scan_with_display(
    target: &ScanTarget,
    index: &mut ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    display: DisplayOptions,
) -> Result<ScanTotals, ScanError> {
    run_scan_with_interval_display(target, index, sink, cancel, LIVE_INTERVAL, display)
}

/// Сканирование нескольких корней в подпапки корня индекса («Все диски»).
///
/// `roots` — пары «имя подпапки → цель сканирования»: каждый том получает в
/// корне индекса собственную папку (например, «C:»), а его записи достраиваются
/// уже в неё. Итоги складываются, прогресс — общий счётчик по всем томам.
///
/// Нечитаемый том (пустой оптический привод, отключённый сетевой диск,
/// залоченный BitLocker) не прерывает скан: он учитывается как пропущенный том
/// (`skipped += 1`), остальные тома сканируются. Ошибка возвращается, только
/// если не прочитан ни один том.
///
/// Имя корня индекса задаёт вызывающий (`ScanIndex::new`): для приложения это
/// «Все диски» (см. `crate::root_name`).
pub fn run_scan_all(
    index: &mut ScanIndex,
    roots: &[(String, ScanTarget)],
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
) -> Result<ScanTotals, ScanError> {
    run_scan_all_with_interval_display(
        index,
        roots,
        sink,
        cancel,
        LIVE_INTERVAL,
        DisplayOptions::default(),
    )
}

/// `run_scan` с явным периодом живых событий: тесты передают `Duration::ZERO`
/// и получают топ/карту на каждом батче, не дожидаясь секунды.
#[cfg(test)]
fn run_scan_with_interval(
    target: &ScanTarget,
    index: &mut ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    live_interval: Duration,
) -> Result<ScanTotals, ScanError> {
    run_scan_with_interval_display(
        target,
        index,
        sink,
        cancel,
        live_interval,
        DisplayOptions::default(),
    )
}

/// [`run_scan_with_interval`] со снимком настроек отображения.
fn run_scan_with_interval_display(
    target: &ScanTarget,
    index: &mut ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    live_interval: Duration,
    display: DisplayOptions,
) -> Result<ScanTotals, ScanError> {
    let mut live = LiveEmitter::new(live_interval, display);

    // «Все диски» — не один движок, а последовательность томов: каждый том
    // сканируется в свою подпапку, поэтому дальше идёт отдельный путь.
    if let ScanTarget::AllDrives(letters) = target {
        let roots: Vec<(String, ScanTarget)> = letters
            .iter()
            .map(|letter| (drive_folder_name(letter), ScanTarget::Drive(letter.clone())))
            .collect();
        return run_scan_all_with_interval_display(
            index,
            &roots,
            sink,
            cancel,
            live_interval,
            display,
        );
    }

    let into = index.root();
    let mut report = |p: &ScanProgress, current: &ScanIndex| live.on_progress(sink, p, current);
    let result = scan_one(target, index, into, cancel, &mut report);
    drop(report);

    finish_scan(result, index, sink, cancel, display)
}

/// `run_scan_all` с явным периодом живых событий (см. [`run_scan_with_interval`]).
#[cfg(test)]
fn run_scan_all_with_interval(
    index: &mut ScanIndex,
    roots: &[(String, ScanTarget)],
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    live_interval: Duration,
) -> Result<ScanTotals, ScanError> {
    run_scan_all_with_interval_display(
        index,
        roots,
        sink,
        cancel,
        live_interval,
        DisplayOptions::default(),
    )
}

/// [`run_scan_all_with_interval`] со снимком настроек отображения.
fn run_scan_all_with_interval_display(
    index: &mut ScanIndex,
    roots: &[(String, ScanTarget)],
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    live_interval: Duration,
    display: DisplayOptions,
) -> Result<ScanTotals, ScanError> {
    let started = Instant::now();
    let mut live = LiveEmitter::new(live_interval, display);
    let mut totals = ScanTotals {
        files: 0,
        folders: 0,
        size_on_disk: 0,
        size_logical: 0,
        skipped: 0,
        cancelled: false,
        elapsed_ms: 0,
    };
    // Сколько уже собрано с завершённых томов: прогресс следующего тома
    // продолжается с общей суммы, а не начинается с нуля.
    let mut done_files = 0u64;
    let mut done_folders = 0u64;
    // Тома, которые прочитать не удалось, и тома, которые прочитаны: по ним
    // решается, ошибка это или просто пропуск.
    let mut failed_volumes: Vec<String> = Vec::new();
    let mut scanned_volumes = 0u32;

    for (name, target) in roots {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        // Подпапка тома создаётся до сканирования: в неё движок достраивает
        // записи, поэтому даже пустой том виден в корне.
        let into = index.folder(index.root(), name);
        let volume = {
            let mut report = |p: &ScanProgress, current: &ScanIndex| {
                let progress = ScanProgress {
                    files: done_files.saturating_add(p.files),
                    folders: done_folders.saturating_add(p.folders),
                    size_on_disk: p.size_on_disk,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                };
                live.on_progress(sink, &progress, current);
            };
            scan_one(target, index, into, cancel, &mut report)
        };

        match volume {
            Ok(volume) => {
                scanned_volumes += 1;
                done_files = done_files.saturating_add(volume.files);
                done_folders = done_folders.saturating_add(volume.folders);
                totals.files = totals.files.saturating_add(volume.files);
                totals.folders = totals.folders.saturating_add(volume.folders);
                totals.size_on_disk = totals.size_on_disk.saturating_add(volume.size_on_disk);
                totals.size_logical = totals.size_logical.saturating_add(volume.size_logical);
                totals.skipped = totals.skipped.saturating_add(volume.skipped);
            }
            // Нечитаемый том — не приговор всему скану: пустой оптический привод,
            // отключённый сетевой диск, залоченный BitLocker не должны стирать
            // уже собранные данные остальных томов. Подпапка тома уже создана и
            // остаётся пустой, а сам том учитывается как пропущенный (+1 к skipped).
            Err(err) => {
                failed_volumes.push(format!("{name} — {}", error_message(&err)));
                totals.skipped = totals.skipped.saturating_add(1);
            }
        }
    }

    // Ошибка — только если не прочитан ни один том и при этом не было отмены:
    // иначе сообщение «ни один том» было бы неправдой или перебивало «Остановлено».
    if scanned_volumes == 0 && !failed_volumes.is_empty() && !cancel.load(Ordering::Relaxed) {
        let err = ScanError::Io(format!(
            "не удалось просканировать ни один том: {}",
            failed_volumes.join("; ")
        ));
        sink.done(Err(&err));
        return Err(err);
    }

    totals.cancelled = cancel.load(Ordering::Relaxed);
    totals.elapsed_ms = started.elapsed().as_millis() as u64;
    finish_scan(Ok(totals), index, sink, cancel, display)
}

/// Итоговые события сканирования: флаг частичности, топ, карта и `done`.
fn finish_scan(
    result: Result<ScanTotals, ScanError>,
    index: &ScanIndex,
    sink: &mut dyn ScanSink,
    cancel: &AtomicBool,
    display: DisplayOptions,
) -> Result<ScanTotals, ScanError> {
    match result {
        Ok(mut totals) => {
            // Флаг отмены — единственный достоверный признак частичности на момент
            // завершения: движок мог остановиться уже после нажатия «Остановить».
            totals.cancelled = cancel.load(Ordering::Relaxed);
            // Итоговые события: топ и карта — ещё раз, по завершении.
            let rows = top_rows(index);
            sink.top_folders(&rows);
            sink.treemap(&live_treemap(index, display));
            sink.done(Ok(&totals));
            Ok(totals)
        }
        Err(err) => {
            sink.done(Err(&err));
            Err(err)
        }
    }
}

/// Сканирование одной цели в папку `into`: выбор движка и запасной обход.
///
/// `into` — узел индекса, под который достраиваются записи (для одиночного
/// диска и папки — корень индекса, для «Все диски» — подпапка с именем тома).
fn scan_one(
    target: &ScanTarget,
    index: &mut ScanIndex,
    into: FolderId,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress, &ScanIndex),
) -> Result<ScanTotals, ScanError> {
    match target {
        // NTFS-том читаем напрямую; всё остальное — обходом.
        ScanTarget::Drive(letter) if is_ntfs(letter) => {
            let fast = scan_ntfs(letter, index, into, cancel, progress);
            // Нет прав на том — не тупик: тот же охват обходом, без ошибки в окне.
            let root = PathBuf::from(drive_root(letter));
            with_walk_fallback(fast, move || {
                scan_folder_into(&root, index, into, cancel, progress)
            })
        }
        ScanTarget::Drive(letter) => {
            let root = PathBuf::from(drive_root(letter));
            scan_folder_into(&root, index, into, cancel, progress)
        }
        ScanTarget::Folder(path) => scan_folder_into(path, index, into, cancel, progress),
        // «Все диски» — не отдельная цель движка, а набор томов; вложение этого
        // варианта в подпапку было бы ошибкой вызывающего кода.
        ScanTarget::AllDrives(_) => Err(ScanError::Io(
            "«Все диски» нельзя сканировать во вложенную папку".to_string(),
        )),
    }
}

/// Скан папки (или не-NTFS диска) обходом в папку `into`.
fn scan_folder_into(
    root: &Path,
    index: &mut ScanIndex,
    into: FolderId,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress, &ScanIndex),
) -> Result<ScanTotals, ScanError> {
    let started = Instant::now();
    let stats = walk(root, index, into, cancel, progress)?;
    let totals = index.folder_totals(into);

    Ok(ScanTotals {
        files: u64::from(totals.file_count),
        folders: u64::from(totals.folder_count),
        size_on_disk: totals.size_on_disk,
        size_logical: totals.size_logical,
        // В skipped входит всё, что не попало в индекс: недоступные записи и
        // reparse-каталоги. Дубликаты жёстких ссылок — не пропуски, они учтены
        // отдельным счётчиком `WalkStats::hard_link_duplicates`.
        skipped: stats.skipped.saturating_add(stats.reparse_skipped),
        cancelled: cancel.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Общее состояние сервиса: отмена текущего запуска, готовый индекс и итоги.
///
/// Запуск сканирования создаёт новый флаг отмены, поэтому `cancel_scan` не может
/// случайно остановить уже завершённое сканирование, а результат устаревшего
/// запуска не затирает свежий индекс.
pub struct ScanService {
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    index: Mutex<Option<Arc<ScanIndex>>>,
    totals: Mutex<Option<ScanTotals>>,
}

impl Default for ScanService {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanService {
    pub fn new() -> ScanService {
        ScanService {
            cancel: Mutex::new(None),
            index: Mutex::new(None),
            totals: Mutex::new(None),
        }
    }

    /// Начать новый запуск: старый останавливается, возвращается его флаг отмены.
    pub fn begin(&self) -> Arc<AtomicBool> {
        self.cancel();
        let flag = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().unwrap() = Some(Arc::clone(&flag));
        flag
    }

    /// Попросить текущий запуск остановиться (если он есть).
    pub fn cancel(&self) {
        if let Some(flag) = self.cancel.lock().unwrap().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Актуален ли запуск с этим флагом (не начат ли уже следующий).
    pub fn is_current(&self, flag: &Arc<AtomicBool>) -> bool {
        let active = self.cancel.lock().unwrap();
        active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, flag))
    }

    /// Сохранить результат, если этот запуск всё ещё актуален.
    pub fn finish(
        &self,
        flag: &Arc<AtomicBool>,
        index: Arc<ScanIndex>,
        totals: Option<ScanTotals>,
    ) {
        let active = self.cancel.lock().unwrap();
        let is_current = active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, flag));
        if !is_current {
            return;
        }
        *self.index.lock().unwrap() = Some(index);
        *self.totals.lock().unwrap() = totals;
    }

    /// Готовый индекс после сканирования (во время сканирования его ещё нет).
    pub fn index(&self) -> Option<Arc<ScanIndex>> {
        self.index.lock().unwrap().clone()
    }

    /// Итоги последнего успешного сканирования.
    pub fn totals(&self) -> Option<ScanTotals> {
        self.totals.lock().unwrap().clone()
    }
}

/// Корень тома для Win32-вызовов: `C`/`C:`/`C:\` → `C:\`.
pub(crate) fn drive_root(drive_letter: &str) -> String {
    let letter = drive_letter.trim_end_matches('\\').trim_end_matches(':');
    format!("{letter}:\\")
}

/// Имя подпапки тома в корне «Все диски»: `C`/`C:`/`C:\` → `C:`.
///
/// WizTree называет диски в корне «Все диски» как «C:», «D:» — так же и здесь.
fn drive_folder_name(drive_letter: &str) -> String {
    drive_root(drive_letter).trim_end_matches('\\').to_string()
}

/// Лежит ли на диске файловая система NTFS.
///
/// Проверка именно через `GetVolumeInformationW`: буква диска может быть и
/// сетевой, и FAT32/exFAT — для них быстрый разбор `$MFT` неприменим.
pub fn is_ntfs(drive_letter: &str) -> bool {
    let root = to_wide(&drive_root(drive_letter));
    let mut fs_name = [0u16; 16];
    let read_ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_name[..]),
        )
        .is_ok()
    };
    read_ok && from_wide(&fs_name).eq_ignore_ascii_case("NTFS")
}

/// Логические диски с меткой, файловой системой и занятым местом.
pub fn list_drives() -> Vec<DriveInfo> {
    // Буфера хватает на 26 букв с разделителями: 26 * 4 u16.
    let mut buffer = vec![0u16; 512];
    let len = unsafe { GetLogicalDriveStringsW(Some(buffer.as_mut_slice())) } as usize;
    if len == 0 {
        return Vec::new();
    }

    // Строки идут через нули и завершаются двойным нулём; пустая строка — конец.
    let mut drives = Vec::new();
    let mut start = 0usize;
    while start < len {
        let Some(offset) = buffer[start..len].iter().position(|&c| c == 0) else {
            break;
        };
        let end = start + offset;
        if end == start {
            break;
        }
        let root = from_wide(&buffer[start..=end]);
        start = end + 1;

        let root_wide = to_wide(&root);
        let mut label = [0u16; 64];
        let mut fs_name = [0u16; 16];
        let info_ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(root_wide.as_ptr()),
                Some(&mut label[..]),
                None,
                None,
                None,
                Some(&mut fs_name[..]),
            )
            .is_ok()
        };

        // Свободное место есть не у всех дисков (например, у пустого привода),
        // поэтому нули в этом случае — допустимый ответ, а не ошибка.
        let mut total_bytes = 0u64;
        let mut free_bytes = 0u64;
        let size_ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root_wide.as_ptr()),
                None,
                Some(&mut total_bytes as *mut u64),
                Some(&mut free_bytes as *mut u64),
            )
            .is_ok()
        };

        drives.push(DriveInfo {
            letter: root.trim_end_matches('\\').to_string(),
            label: if info_ok {
                from_wide(&label)
            } else {
                String::new()
            },
            fs: if info_ok {
                from_wide(&fs_name)
            } else {
                String::new()
            },
            total_bytes: if size_ok { total_bytes } else { 0 },
            free_bytes: if size_ok { free_bytes } else { 0 },
        });
    }

    drives
}

/// Строка в UTF-16 с завершающим нулём — так Win32 ждёт пути.
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 с завершающим нулём → `String` (обрезка по первому нулю).
fn from_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FolderSummary, ScanError, ScanProgress, ScanTotals, TreemapRect};
    use crate::scan::index::ScanIndex;
    use crate::scan::test_support::test_dir;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Приёмник событий сканирования: запоминает всё, что прислал `run_scan`,
    /// и порядок событий — по нему проверяется «живость» до `done`.
    #[derive(Default)]
    struct RecordingSink {
        progress: Vec<ScanProgress>,
        tops: Vec<Vec<FolderSummary>>,
        treemaps: Vec<Vec<TreemapRect>>,
        events: Vec<&'static str>,
        done: Option<Result<ScanTotals, String>>,
    }

    impl ScanSink for RecordingSink {
        fn progress(&mut self, p: &ScanProgress) {
            self.events.push("progress");
            self.progress.push(p.clone());
        }

        fn top_folders(&mut self, rows: &[FolderSummary]) {
            self.events.push("top");
            self.tops.push(rows.to_vec());
        }

        fn treemap(&mut self, rects: &[TreemapRect]) {
            self.events.push("treemap");
            self.treemaps.push(rects.to_vec());
        }

        fn done(&mut self, r: Result<&ScanTotals, &ScanError>) {
            self.events.push("done");
            self.done = Some(match r {
                Ok(totals) => Ok(totals.clone()),
                Err(err) => Err(format!("{err:?}")),
            });
        }
    }

    #[test]
    fn scan_service_keeps_only_current_run_results() {
        let service = ScanService::new();
        let old = service.begin();
        let current = service.begin();

        // Новый запуск обязан остановить прошлый, а его результат — не попасть
        // в состояние: иначе запоздавший поток затрёт свежий индекс.
        assert!(
            old.load(Ordering::Relaxed),
            "begin должен остановить предыдущий запуск"
        );
        assert!(service.is_current(&current));
        assert!(!service.is_current(&old));

        let mut stale = ScanIndex::new("root");
        stale.folder(stale.root(), "stale");
        service.finish(&old, Arc::new(stale), None);
        assert!(
            service.index().is_none(),
            "результат устаревшего запуска не должен сохраняться"
        );

        let mut fresh = ScanIndex::new("root");
        fresh.folder(fresh.root(), "fresh");
        service.finish(&current, Arc::new(fresh), None);
        let stored = service
            .index()
            .expect("результат текущего запуска должен сохраниться");
        assert_eq!(stored.child_folders(stored.root()).len(), 1);
    }

    #[test]
    fn run_scan_emits_live_top_and_treemap() {
        // Живые события не должны ждать `done`: на нескольких батчах обхода
        // sink обязан получить топ и карту ещё во время сканирования.
        let dir = test_dir();
        let sub = dir.path().join("с данными");
        std::fs::create_dir(&sub).unwrap();
        for index in 0..1000 {
            std::fs::write(sub.join(format!("f-{index:04}.bin")), b"x").unwrap();
        }

        let mut index = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();
        let totals = run_scan_with_interval(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
            Duration::ZERO,
        )
        .expect("сканирование папки должно завершиться без ошибки");

        assert_eq!(totals.files, 1000, "все файлы обязаны попасть в итоги");
        let done_at = sink
            .events
            .iter()
            .position(|event| *event == "done")
            .expect("событие done обязано быть");
        assert!(
            sink.events[..done_at].iter().any(|event| *event == "top"),
            "топ папок обязан прийти до done: {:?}",
            sink.events
        );
        assert!(
            sink.events[..done_at]
                .iter()
                .any(|event| *event == "treemap"),
            "живая карта обязана прийти до done: {:?}",
            sink.events
        );
        assert_eq!(
            sink.events.last(),
            Some(&"done"),
            "done — последнее событие сканирования: {:?}",
            sink.events
        );
        // Живой топ — не пустой срез: к первому событию папка с файлами уже есть.
        assert!(
            sink.tops.iter().any(|rows| rows
                .iter()
                .any(|row| row.name == "с данными" && row.file_count == 1000)),
            "живой топ обязан содержать отсканированную папку: {:?}",
            sink.tops
        );
    }

    #[test]
    fn live_treemap_follows_display_options_snapshot() {
        // Настройки на старте скана: фактический размер, файлы на карте скрыты.
        // Живая и итоговая карта обязаны строиться по этому снимку, а не по
        // умолчаниям: иначе во время скана пользователь видел бы одно, а после —
        // другое.
        let dir = test_dir();
        let sub = dir.path().join("папка");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("внутри.bin"), [1u8; 32]).unwrap();
        std::fs::write(dir.path().join("файл.bin"), [2u8; 64]).unwrap();

        let mut index = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();
        let display = DisplayOptions {
            size_mode: SizeMode::Logical,
            include_files: false,
            min_tile_bytes: 0,
        };
        run_scan_with_display(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
            display,
        )
        .expect("сканирование папки должно завершиться без ошибки");

        let rects = sink.treemaps.last().expect("итоговая карта обязана прийти");
        assert!(
            !rects.is_empty(),
            "у папки с файлом и подпапкой карта не пустая"
        );
        assert!(
            rects
                .iter()
                .all(|rect| rect.is_dir || rect.name == "прочее"),
            "при скрытых файлах на карте только папки (и сводная плитка): {rects:?}"
        );
        // В фактическом режиме размер плитки — логический (32 байта), а не
        // занятый на диске.
        let nested = rects
            .iter()
            .find(|rect| rect.name == "папка")
            .expect("подпапка обязана быть плиткой");
        assert_eq!(nested.size, 32, "размер плитки — фактический: {rects:?}");
    }

    /// Живой топ отдаёт больше 20 кандидатов: окно выбирает из них топ-20 по
    /// выбранному режиму размера, а не только по занятому месту.
    #[test]
    fn top_rows_sends_more_candidates_than_the_window_shows() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        for position in 0..(TOP_FOLDER_CANDIDATES + 20) {
            index.folder(root, &format!("папка-{position:03}"));
        }

        let rows = top_rows(&index);
        assert_eq!(
            rows.len(),
            TOP_FOLDER_CANDIDATES,
            "ядро обязано присылать запас кандидатов для пересортировки"
        );
        assert!(
            TOP_FOLDER_CANDIDATES > 20,
            "кандидатов заведомо больше, чем строк в таблице"
        );
    }

    #[test]
    fn run_scan_on_folder_walks_and_reports_progress() {
        let dir = test_dir();
        for name in ["a.bin", "b.bin", "c.bin"] {
            std::fs::write(dir.path().join(name), [0u8; 16]).unwrap();
        }

        let mut index = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();

        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("сканирование папки должно завершиться без ошибки");

        assert_eq!(totals.files, 3);
        assert!(
            !totals.cancelled,
            "обычный скан не должен помечаться отменённым"
        );
        assert_eq!(index.folder_totals(index.root()).file_count, 3);
        assert!(
            !sink.progress.is_empty(),
            "прогресс должен сообщаться хотя бы итоговым отчётом"
        );
        assert_eq!(
            sink.progress.last().map(|p| p.files),
            Some(3),
            "итоговый прогресс должен содержать число файлов"
        );
        assert!(
            sink.done.as_ref().is_some_and(|done| done.is_ok()),
            "по завершении sink должен получить итоги: {:?}",
            sink.done
        );
    }

    #[test]
    fn progress_size_comes_from_index_without_hard_link_doubles() {
        let dir = test_dir();
        let first = dir.path().join("first.bin");
        std::fs::write(&first, vec![0u8; 4096]).unwrap();
        std::fs::hard_link(&first, dir.path().join("second.bin"))
            .expect("NTFS должен поддерживать жёсткие ссылки");

        let mut index = ScanIndex::new("folder");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();
        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("сканирование папки должно завершиться без ошибки");

        assert_eq!(totals.files, 2, "обе жёсткие ссылки — отдельные имена");
        let indexed = index.folder_totals(index.root()).size_on_disk;
        assert_eq!(
            totals.size_on_disk, indexed,
            "итоги не должны задваивать жёсткую ссылку"
        );
        let live = sink.progress.last().expect("прогресс обязан прийти");
        assert_eq!(
            live.size_on_disk, indexed,
            "прогресс не должен показывать сырые байты движка: на экране он \
             обязан совпадать с живым топом и итогами"
        );
    }

    #[test]
    fn run_scan_drive_ntfs_vs_folder() {
        let dir = test_dir();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("nested.bin"), [1u8; 32]).unwrap();
        std::fs::write(dir.path().join("top.bin"), [2u8; 64]).unwrap();

        // Эталон: прямой обход той же папки.
        let mut direct = ScanIndex::new("folder");
        let direct_into = direct.root();
        let cancel = AtomicBool::new(false);
        let stats = walk::walk(
            dir.path(),
            &mut direct,
            direct_into,
            &cancel,
            &mut |_, _| {},
        )
        .expect("прямой обход должен пройти без ошибки");
        let direct_totals = direct.folder_totals(direct.root());

        let mut index = ScanIndex::new("folder");
        let mut sink = RecordingSink::default();
        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("ScanTarget::Folder всегда идёт через walk и не должен падать");

        assert_eq!(totals.files, u64::from(direct_totals.file_count));
        assert_eq!(totals.folders, u64::from(direct_totals.folder_count));
        assert_eq!(totals.size_on_disk, direct_totals.size_on_disk);
        assert_eq!(totals.size_logical, direct_totals.size_logical);
        // В skipped для обхода входит всё, что не попало в индекс, кроме
        // дубликатов жёстких ссылок (они — не пропуски).
        assert_eq!(totals.skipped, stats.skipped + stats.reparse_skipped);

        // Топ папок — дочерние папки корня, их присылают по завершении.
        let top = sink.tops.last().expect("топ папок должен быть отправлен");
        assert_eq!(top.len(), 1, "у корня ровно одна дочерняя папка: {top:?}");
        assert_eq!(top[0].name, "sub");
    }

    #[test]
    fn classify_error_maps_volume_access_to_fallback() {
        assert_eq!(
            classify_error(&ScanError::VolumeAccess("нет прав на том".to_string())),
            FallbackAction::OfferWalkFallback
        );
        assert_eq!(
            classify_error(&ScanError::Io("диск отвалился".to_string())),
            FallbackAction::ShowFatal
        );
        assert_eq!(
            classify_error(&ScanError::Parse("битая запись".to_string())),
            FallbackAction::ShowFatal
        );
    }

    fn sample_totals(files: u64, size_on_disk: u64) -> ScanTotals {
        ScanTotals {
            files,
            folders: 1,
            size_on_disk,
            size_logical: size_on_disk,
            skipped: 0,
            cancelled: false,
            elapsed_ms: 1,
        }
    }

    #[test]
    fn volume_access_falls_back_to_walk() {
        let mut walk_called = false;
        let result = with_walk_fallback(
            Err(ScanError::VolumeAccess("нет прав".into())),
            || {
                walk_called = true;
                Ok(sample_totals(7, 70))
            },
        );

        let totals = result.expect("обход обязан заменить неудачу быстрого движка");
        assert!(walk_called, "при VolumeAccess обход должен запускаться");
        assert_eq!(totals.files, 7);
        assert_eq!(totals.size_on_disk, 70);
    }

    #[test]
    fn successful_fast_scan_does_not_fall_back() {
        let mut walk_called = false;
        let result = with_walk_fallback(Ok(sample_totals(3, 30)), || {
            walk_called = true;
            Ok(sample_totals(9, 90))
        });

        assert_eq!(result.expect("успех должен пройти насквозь").files, 3);
        assert!(!walk_called, "успешный скан не должен повторяться обходом");
    }

    #[test]
    fn non_volume_errors_do_not_fall_back() {
        let mut walk_called = false;
        let result = with_walk_fallback(
            Err(ScanError::Io("диск отвалился".into())),
            || {
                walk_called = true;
                Ok(sample_totals(1, 1))
            },
        );

        assert!(!walk_called, "Io-ошибка обходом не лечится");
        assert!(matches!(result, Err(ScanError::Io(_))));
    }

    #[test]
    fn failed_walk_reports_both_causes() {
        let result = with_walk_fallback(
            Err(ScanError::VolumeAccess("нет прав".into())),
            || Err(ScanError::Io("папка недоступна".into())),
        );

        match result {
            Err(ScanError::VolumeAccess(message)) => {
                assert!(
                    message.contains("нет прав"),
                    "исходная причина должна остаться: {message}"
                );
                assert!(
                    message.contains("папка недоступна"),
                    "причина неудачи обхода должна остаться: {message}"
                );
            }
            other => panic!("ожидалась VolumeAccess с обеими причинами, получено {other:?}"),
        }
    }

    /// Приёмник-обёртка: просит отмену, как только общий прогресс перевалил за
    /// заданное число файлов. Нужен тесту «Все диски»: отмена должна случиться
    /// на втором томе, а не до первого.
    struct CancelOnProgress {
        cancel: Arc<AtomicBool>,
        cancel_above_files: u64,
        inner: RecordingSink,
    }

    impl ScanSink for CancelOnProgress {
        fn progress(&mut self, p: &ScanProgress) {
            if p.files > self.cancel_above_files {
                self.cancel.store(true, Ordering::Relaxed);
            }
            self.inner.progress(p);
        }

        fn top_folders(&mut self, rows: &[FolderSummary]) {
            self.inner.top_folders(rows);
        }

        fn treemap(&mut self, rects: &[TreemapRect]) {
            self.inner.treemap(rects);
        }

        fn done(&mut self, r: Result<&ScanTotals, &ScanError>) {
            self.inner.done(r);
        }
    }

    #[test]
    fn unreadable_volume_skips_and_continues() {
        // Один нечитаемый том (пустой привод, отключённый сетевой диск, BitLocker)
        // не должен убивать скан остальных: он учитывается как пропущенный том.
        let readable = test_dir();
        std::fs::write(readable.path().join("b.bin"), [7u8; 32]).unwrap();
        let missing = readable.path().join("нет-такой-папки");

        let roots = vec![
            ("A:".to_string(), ScanTarget::Folder(missing)),
            (
                "B:".to_string(),
                ScanTarget::Folder(readable.path().to_path_buf()),
            ),
        ];
        let mut index = ScanIndex::new("Все диски");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();

        let totals =
            run_scan_all_with_interval(&mut index, &roots, &mut sink, &cancel, Duration::ZERO)
                .expect("нечитаемый том не должен ронять весь скан «Все диски»");

        assert!(
            totals.skipped >= 1,
            "нечитаемый том обязан попасть в пропуски: {totals:?}"
        );
        assert_eq!(
            totals.files, 1,
            "файлы читаемого тома обязаны быть в итогах"
        );
        let children = index.child_folders(index.root());
        let by_name = |name: &str| {
            children
                .iter()
                .find(|child| child.name == name)
                .unwrap_or_else(|| panic!("в корне нет подпапки {name}: {children:?}"))
                .clone()
        };
        assert_eq!(by_name("B:").file_count, 1, "данные «B:» просканированы");
        assert_eq!(
            by_name("A:").file_count,
            0,
            "у пропущенного тома подпапка остаётся пустой"
        );
        assert_eq!(
            index.folder_totals(index.root()).file_count,
            1,
            "итоги — только по читаемым томам"
        );
        assert!(
            sink.done.as_ref().is_some_and(|done| done.is_ok()),
            "скан обязан завершиться успешно: {:?}",
            sink.done
        );
    }

    #[test]
    fn all_volumes_failed_is_an_error() {
        let dir = test_dir();
        let roots = vec![
            (
                "A:".to_string(),
                ScanTarget::Folder(dir.path().join("нет-1")),
            ),
            (
                "B:".to_string(),
                ScanTarget::Folder(dir.path().join("нет-2")),
            ),
        ];
        let mut index = ScanIndex::new("Все диски");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();

        let result =
            run_scan_all_with_interval(&mut index, &roots, &mut sink, &cancel, Duration::ZERO);

        match result {
            Err(ScanError::Io(message)) => {
                assert!(
                    message.contains("ни один том"),
                    "сообщение обязано объяснять причину: {message}"
                );
                assert!(
                    message.contains("A:"),
                    "в ошибке должен быть том: {message}"
                );
                assert!(
                    message.contains("B:"),
                    "в ошибке должен быть том: {message}"
                );
            }
            other => panic!("ожидалась ошибка «не прочитан ни один том», получено {other:?}"),
        }
        assert!(
            sink.done.as_ref().is_some_and(|done| done.is_err()),
            "окно обязано получить ошибку, а не пустые итоги"
        );
    }

    #[test]
    fn scan_into_subfolder_aggregates_under_target() {
        let dir = test_dir();
        let nested = dir.path().join("вложенная");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(dir.path().join("верхний.bin"), [1u8; 100]).unwrap();
        std::fs::write(nested.join("вложенный.bin"), [2u8; 32]).unwrap();

        let mut index = ScanIndex::new("корень");
        let into = index.folder(index.root(), "Цель");
        let cancel = AtomicBool::new(false);
        walk::walk(dir.path(), &mut index, into, &cancel, &mut |_, _| {})
            .expect("обход в подпапку должен пройти без ошибки");

        // Файлы и папки обязаны лежать под папкой назначения, а не в корне.
        let target = index.folder_totals(into);
        assert_eq!(target.file_count, 2, "файлы должны попасть в подпапку");
        assert_eq!(target.folder_count, 1, "вложенная папка — тоже под целью");
        assert_eq!(target.size_logical, 132);

        assert_eq!(
            index.file_count(index.root()),
            0,
            "прямо в корне файлов быть не должно"
        );
        let root = index.folder_totals(index.root());
        assert_eq!(
            root.file_count, target.file_count,
            "корень обязан агрегировать подпапку"
        );
        assert_eq!(
            root.folder_count,
            target.folder_count + 1,
            "корень считает саму папку назначения и всех её потомков"
        );

        let children = index.child_folders(index.root());
        assert_eq!(children.len(), 1, "корень содержит только папку-цель");
        assert_eq!(children[0].name, "Цель");
        assert_eq!(children[0].file_count, 2);
    }

    #[test]
    fn all_drives_scan_uses_drive_subfolders() {
        let first = test_dir();
        std::fs::write(first.path().join("a.bin"), [0u8; 10]).unwrap();
        std::fs::write(first.path().join("b.bin"), [0u8; 20]).unwrap();
        let second = test_dir();
        std::fs::write(second.path().join("c.bin"), [0u8; 40]).unwrap();

        let roots = vec![
            (
                "A:".to_string(),
                ScanTarget::Folder(first.path().to_path_buf()),
            ),
            (
                "B:".to_string(),
                ScanTarget::Folder(second.path().to_path_buf()),
            ),
        ];
        let mut index = ScanIndex::new("Все диски");
        let cancel = AtomicBool::new(false);
        let mut sink = RecordingSink::default();

        let totals =
            run_scan_all_with_interval(&mut index, &roots, &mut sink, &cancel, Duration::ZERO)
                .expect("скан всех дисков должен завершиться без ошибки");

        let children = index.child_folders(index.root());
        assert_eq!(
            children.len(),
            2,
            "корень «Все диски» содержит по подпапке на том: {children:?}"
        );
        let by_name = |name: &str| {
            children
                .iter()
                .find(|child| child.name == name)
                .unwrap_or_else(|| panic!("в корне нет подпапки {name}: {children:?}"))
                .clone()
        };
        let a = by_name("A:");
        let b = by_name("B:");
        assert_eq!(a.file_count, 2, "итоги первого тома — независимые");
        assert_eq!(b.file_count, 1, "итоги второго тома — независимые");

        let root = index.folder_totals(index.root());
        assert_eq!(root.file_count, 3, "корень суммирует файлы всех томов");
        assert_eq!(root.folder_count, 2, "два тома — две подпапки корня");
        assert_eq!(totals.files, 3);
        assert_eq!(totals.size_logical, 10 + 20 + 40);
        assert_eq!(
            totals.size_on_disk,
            a.size_on_disk + b.size_on_disk,
            "итоги скана — сумма независимых итогов томов"
        );
        assert_eq!(totals.size_on_disk, root.size_on_disk);
        assert!(!totals.cancelled);
        assert_eq!(
            sink.progress.last().map(|p| p.files),
            Some(3),
            "прогресс обязан быть общим счётчиком по всем томам"
        );
        assert!(
            sink.done.as_ref().is_some_and(|done| done.is_ok()),
            "по завершении sink должен получить итоги: {:?}",
            sink.done
        );
    }

    #[test]
    fn cancel_all_drives_stops_everything() {
        let first = test_dir();
        for index in 0..2 {
            std::fs::write(first.path().join(format!("a-{index}.bin")), [1u8; 16]).unwrap();
        }
        let second = test_dir();
        let second_total = 2000;
        for index in 0..second_total {
            std::fs::write(second.path().join(format!("b-{index:04}.bin")), [2u8; 16]).unwrap();
        }
        let third = test_dir();
        std::fs::write(third.path().join("c.bin"), [3u8; 16]).unwrap();

        let roots = vec![
            (
                "A:".to_string(),
                ScanTarget::Folder(first.path().to_path_buf()),
            ),
            (
                "B:".to_string(),
                ScanTarget::Folder(second.path().to_path_buf()),
            ),
            (
                "C:".to_string(),
                ScanTarget::Folder(third.path().to_path_buf()),
            ),
        ];
        let mut index = ScanIndex::new("Все диски");
        // Отмена — на втором томе: общий прогресс уже больше первого тома.
        let cancel = Arc::new(AtomicBool::new(false));
        let mut sink = CancelOnProgress {
            cancel: Arc::clone(&cancel),
            cancel_above_files: 2,
            inner: RecordingSink::default(),
        };

        let totals =
            run_scan_all_with_interval(&mut index, &roots, &mut sink, &cancel, Duration::ZERO)
                .expect("отмена — не ошибка: run_scan_all обязан вернуть Ok");

        assert!(
            totals.cancelled,
            "остановленный скан обязан помечать итоги как частичные"
        );
        assert!(
            cancel.load(Ordering::Relaxed),
            "отмена должна была сработать"
        );

        let children = index.child_folders(index.root());
        assert_eq!(
            children.len(),
            2,
            "третий том не должен начинаться после отмены: {children:?}"
        );
        let by_name = |name: &str| {
            children
                .iter()
                .find(|child| child.name == name)
                .unwrap_or_else(|| panic!("в корне нет подпапки {name}: {children:?}"))
                .clone()
        };
        assert_eq!(
            by_name("A:").file_count,
            2,
            "первый том обязан быть просканирован полностью"
        );
        let partial = by_name("B:").file_count;
        assert!(
            partial > 0 && partial < second_total,
            "второй том обязан остановиться посередине: {partial} из {second_total}"
        );
        assert_eq!(
            index.folder_totals(index.root()).file_count,
            2 + partial,
            "итоги корня — то, что успели до отмены"
        );
    }

    #[test]
    fn cancel_scan_stops_run() {
        let dir = test_dir();
        for i in 0..32 {
            std::fs::write(dir.path().join(format!("file-{i:02}.bin")), b"x").unwrap();
        }

        let mut index = ScanIndex::new("folder");
        // Отмена до старта: обход не должен ничего прочитать и не должен падать.
        let cancel = AtomicBool::new(true);
        let mut sink = RecordingSink::default();

        let totals = run_scan(
            &ScanTarget::Folder(dir.path().to_path_buf()),
            &mut index,
            &mut sink,
            &cancel,
        )
        .expect("отмена — не ошибка, run_scan обязан вернуть Ok");

        assert_eq!(totals.files, 0, "до отмены файлы не должны считаться");
        assert_eq!(totals.skipped, 0, "отмена не создаёт пропусков");
        assert!(
            totals.cancelled,
            "остановленный скан обязан помечать итоги как частичные"
        );
    }
}
