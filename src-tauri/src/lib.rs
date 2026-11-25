//! Сборка приложения WoozyTree: команды Tauri и мост между сканером и окном.

pub mod files;
pub mod model;
pub mod scan;
pub mod treemap;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use files::ops;
use model::{
    CrumbDto, DeleteReport, DriveInfo, FilePage, FileSort, FileTypeRow, FolderSummary, ScanError,
    ScanProgress, ScanTotals, SearchPage, SearchQuery, SizeMode, TreemapRect, TypeSort,
};
use scan::index::{ScanIndex, valid_path_segment};
use scan::{DisplayOptions, ScanService, ScanSink, ScanTarget, drive_root, run_scan_with_display};

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

/// Имя корневой папки индекса: буква диска, имя выбранной папки или «Все диски».
fn root_name(target: &ScanTarget) -> String {
    match target {
        ScanTarget::Drive(letter) => drive_root(letter),
        ScanTarget::Folder(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        // «Все диски» — общий корень: в нём лежат подпапки с именами томов.
        ScanTarget::AllDrives(_) => "Все диски".to_string(),
    }
}

/// Реальный путь корня сканирования — база для путей действий с файлами.
///
/// У диска это «C:\», у папки — её путь, у «Все диски» — пусто: там базой
/// становится подпапка тома «X:» (см. `ScanIndex::real_path_of`).
fn real_root_of(target: &ScanTarget) -> String {
    match target {
        ScanTarget::Drive(letter) => drive_root(letter),
        ScanTarget::Folder(path) => path.to_string_lossy().into_owned(),
        ScanTarget::AllDrives(_) => String::new(),
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
    size_mode: String,
    include_files: bool,
    min_tile_bytes: u64,
) -> Result<(), String> {
    let service = Arc::clone(&state.service);
    let size_mode = SizeMode::from_name(&size_mode)?;
    // Настройки отображения — снимок на старте: живые карта и топ строятся по
    // ним, а не по значениям по умолчанию (после скана карта перезапросится с
    // актуальными настройками).
    let display = DisplayOptions {
        size_mode,
        include_files,
        min_tile_bytes,
    };
    // Новый запуск: прошлый просим остановиться, чтобы два скана не спорили.
    let cancel = service.begin();
    let index_name = root_name(&target);
    // Реальный корень знает только сервис: индекс хранит имена, а действиям с
    // файлами нужны абсолютные пути («C:\», путь папки или пусто у «Все диски»).
    let real_root = real_root_of(&target);

    std::thread::Builder::new()
        .name("woozytree-scan".to_string())
        .spawn(move || {
            let mut index = ScanIndex::new(&index_name);
            index.set_root_real_path(real_root);
            let mut sink = TauriSink::new(app, Arc::clone(&service), Arc::clone(&cancel));
            let result = run_scan_with_display(&target, &mut index, &mut sink, &cancel, display);
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

/// Проверить, что папка с таким id есть в готовом индексе.
///
/// Раньше здесь сравнивался id с `folder_count`, то есть проверка опиралась на
/// неявную связь «число папок = последний id». Теперь индекс отвечает прямо:
/// есть ли узел с таким id. Чужой id отсекается без паники.
fn check_folder_id(index: &ScanIndex, folder_id: u32) -> Result<(), String> {
    if !index.has_folder(folder_id) {
        return Err(format!("Папка с id {folder_id} не найдена."));
    }
    Ok(())
}

#[tauri::command]
fn get_children(state: State<'_, AppState>, folder_id: u32) -> Result<Vec<FolderSummary>, String> {
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    Ok(index.child_folders(folder_id))
}

#[tauri::command]
fn get_scan_totals(state: State<'_, AppState>) -> Result<ScanTotals, String> {
    state
        .service
        .totals()
        .ok_or_else(|| "Сканирование ещё не завершено.".to_string())
}

/// Колонка сортировки файлов из строки окна.
///
/// Неизвестное имя — ошибка на русском, а не паника: окно могло сохранить
/// старую настройку или прислать мусор, приложение должно остаться живым.
fn file_sort_from_name(name: &str) -> Result<FileSort, String> {
    match name {
        "name" => Ok(FileSort::Name),
        "size_on_disk" => Ok(FileSort::SizeOnDisk),
        "size_logical" => Ok(FileSort::SizeLogical),
        "modified" => Ok(FileSort::Modified),
        other => Err(format!(
            "Неизвестная сортировка «{other}»: ожидается name, size_on_disk, size_logical или modified."
        )),
    }
}

/// Колонка сортировки таблицы типов из строки окна.
///
/// Времени изменения здесь нет: у группы расширений его не существует, поэтому
/// допустимы только четыре имени. Неизвестное имя — ошибка на русском, а не
/// паника: окно могло сохранить старую настройку или прислать мусор.
fn type_sort_from_name(name: &str) -> Result<TypeSort, String> {
    match name {
        "name" => Ok(TypeSort::Name),
        "count" => Ok(TypeSort::Count),
        "size_on_disk" => Ok(TypeSort::SizeOnDisk),
        "size_logical" => Ok(TypeSort::SizeLogical),
        other => Err(format!(
            "Неизвестная сортировка «{other}»: ожидается name, count, size_on_disk или size_logical."
        )),
    }
}

/// Сколько строк таблицы файлов отдаём одной командой максимум.
///
/// Окно просит 2000 строк на страницу; запас сверху — защита от случайно
/// огромного запроса, из-за которого ответ IPC раздулся бы на сотни мегабайт.
const FILES_ROWS_LIMIT: usize = 10_000;

/// Кламп размера страницы файлов: окно не может запросить больше лимита.
fn files_page_limit(limit: usize) -> usize {
    limit.min(FILES_ROWS_LIMIT)
}

/// Страница таблицы файлов папки: сортировка и срез делает индекс.
///
/// Команда асинхронная: сортировка большой папки (десятки тысяч имён) — это
/// десятки миллисекунд CPU (в debug-сборке — до сотен), а главный поток окна
/// не должен ждать их при прокрутке и смене колонки.
///
/// `size_mode` — настройка отображения с фронтенда: по ней считается колонка
/// `%` (доля строки от выбранного размера родителя). Сортировка приходит
/// отдельным параметром: клик по заголовку выбирает любую колонку.
#[tauri::command(async)]
fn get_files(
    state: State<'_, AppState>,
    folder_id: u32,
    sort: String,
    desc: bool,
    offset: usize,
    limit: usize,
    size_mode: String,
) -> Result<FilePage, String> {
    let sort = file_sort_from_name(&sort)?;
    let size_mode = SizeMode::from_name(&size_mode)?;
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    let limit = files_page_limit(limit);
    Ok(FilePage {
        total: index.file_count(folder_id),
        rows: index.child_files(folder_id, sort, desc, offset, limit, size_mode),
    })
}

/// Сколько строк выдачи поиска отдаём одной командой максимум: страница
/// таблицы — 2000, запас сверху; защита от случайно огромного ответа IPC.
const SEARCH_ROWS_LIMIT: usize = 5_000;

/// Поиск файлов по готовому индексу: фильтры и страница — в ядре.
///
/// Команда асинхронная: линейный проход по 1.26 млн файлов и сортировка
/// найденного занимают десятки-сотни миллисекунд, а главный поток окна не
/// должен ждать. Пустые фильтры валидны — это «всё подряд».
#[tauri::command(async)]
fn search_files(
    state: State<'_, AppState>,
    q: SearchQuery,
    offset: usize,
    limit: usize,
) -> Result<SearchPage, String> {
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;
    let limit = limit.min(SEARCH_ROWS_LIMIT);
    let (rows, total) = index.search_files(&q, offset, limit);
    Ok(SearchPage { rows, total })
}

/// Цепочка папок от корня до указанной: путь для перехода из поиска на карту.
///
/// Команда асинхронная, как и остальные запросы к индексу: подъём по родителям
/// дешёвый, но окно не должно ждать даже его синхронно.
#[tauri::command(async)]
fn get_path_crumbs(state: State<'_, AppState>, folder_id: u32) -> Result<Vec<CrumbDto>, String> {
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    Ok(index
        .ancestors(folder_id)
        .into_iter()
        .map(|(id, name)| CrumbDto { id, name })
        .collect())
}

/// Сколько прямоугольников карты максимум отдаём одной командой: защита от
/// случайно огромного ответа IPC и зависания отрисовки.
const TREEMAP_RECTS_LIMIT: usize = 20_000;

/// Раскладка карты для папки: готовые прямоугольники с нормированными 0..1
/// координатами. Рекурсия внутрь крупных папок — до бюджета `max_rects`.
///
/// Команда асинхронная: раскладка 2000 прямоугольников — это единицы-десятки
/// миллисекунд CPU, и главный поток окна не должен на них ждать.
///
/// Параметры отображения (`size_mode`, `include_files`, `min_tile_bytes`) —
/// осознанное расширение сигнатуры Плана 2: настройки живут на фронтенде и
/// приходят аргументами. Порог мелочи влияет ТОЛЬКО на карту, итоги папки не
/// меняются.
#[tauri::command(async)]
fn get_treemap(
    state: State<'_, AppState>,
    folder_id: u32,
    max_rects: usize,
    size_mode: String,
    include_files: bool,
    min_tile_bytes: u64,
) -> Result<Vec<TreemapRect>, String> {
    let size_mode = SizeMode::from_name(&size_mode)?;
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    let max_rects = max_rects.min(TREEMAP_RECTS_LIMIT);
    Ok(treemap::layout::treemap_for_folder(
        &index,
        folder_id,
        max_rects,
        size_mode,
        include_files,
        min_tile_bytes,
    ))
}

/// Сколько плиток карты типов максимум отдаём одной командой.
///
/// Расширений на диске тысячи, а не миллионы, но ответ IPC всё равно ограничен:
/// карте типов хватает нескольких сотен крупнейших, остальные — в таблице.
const TYPES_RECTS_LIMIT: usize = 5_000;

/// Агрегаты по расширениям в поддереве папки — таблица вкладки «Типы файлов».
///
/// Команда асинхронная: обход поддерева (на C: — 1.26 млн файлов) и агрегация
/// по `HashMap` занимают десятки-сотни миллисекунд, окно ждать не должно.
#[tauri::command(async)]
fn get_file_types(
    state: State<'_, AppState>,
    folder_id: u32,
    sort: String,
    desc: bool,
) -> Result<Vec<FileTypeRow>, String> {
    let sort = type_sort_from_name(&sort)?;
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    Ok(index.file_types(folder_id, sort, desc))
}

/// Плитки карты типов: расширения как плоский список прямоугольников 0..1.
///
/// Раскладка — общий `layout_treemap`, поэтому плитки покрывают контейнер без
/// пересечений. Команда асинхронная: сортировка тысяч групп и раскладка — это
/// единицы-десятки миллисекунд CPU. `size_mode` — настройка отображения: и
/// порядок плиток, и их площадь считаются по выбранному размеру.
#[tauri::command(async)]
fn get_types_treemap(
    state: State<'_, AppState>,
    folder_id: u32,
    max_rects: usize,
    size_mode: String,
) -> Result<Vec<TreemapRect>, String> {
    let size_mode = SizeMode::from_name(&size_mode)?;
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    let max_rects = max_rects.min(TYPES_RECTS_LIMIT);
    Ok(index.types_treemap(folder_id, max_rects, size_mode))
}

/// Проверить путь действия из окна.
///
/// Отвергаются: пустой путь; путь с пробелами по краям (обрезать нельзя —
/// «a.txt » и «a.txt» могут быть разными файлами); относительный путь;
/// компоненты `.` и `..` (уводят из папки, а `remove_dir_all` по такому пути
/// стёр бы чужой каталог); корень диска; путь, у которого родитель не
/// существует или не папка. Окно присылает только пути файлов из индекса, но
/// команда не должна верить окну на слово.
fn action_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Не указан путь файла.".to_string());
    }
    if path.trim() != path {
        return Err(format!(
            "Путь не должен начинаться или заканчиваться пробелами: «{path}»"
        ));
    }
    let path_buf = PathBuf::from(path);
    if !path_buf.is_absolute() {
        return Err(format!("Путь должен быть абсолютным: {path}"));
    }
    // `Path::components` нормализует «.» и «..» (они исчезают из цепочки),
    // поэтому проверяем сырые сегменты исходной строки.
    for segment in path.split(|symbol| symbol == '\\' || symbol == '/') {
        if segment == "." || segment == ".." {
            return Err(format!("Путь не должен содержать «.» или «..»: {path}"));
        }
    }
    let Some(parent) = path_buf.parent() else {
        return Err(format!("Нельзя выполнить действие с корнем диска: {path}"));
    };
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.is_dir() => {}
        _ => return Err(format!("Папка не найдена: {}", parent.display())),
    }
    Ok(path_buf)
}

/// Путь действия, который сейчас существует: открывать и показывать нечего,
/// если файл уже удалён или переименован.
fn existing_action_path(path: &str) -> Result<PathBuf, String> {
    let path = action_path(path)?;
    if std::fs::symlink_metadata(&path).is_err() {
        return Err(format!("Файл не найден: {}", path.display()));
    }
    Ok(path)
}

/// Проверить путь для безвозвратного удаления: существует и это файл.
///
/// Папки запрещены: безвозвратное удаление папки — это `remove_dir_all`, и одна
/// ошибка в запросе стоила бы целого дерева. Ссылка на папку тоже отвергается.
fn permanent_delete_target(path: &str) -> Result<PathBuf, String> {
    let path = action_path(path)?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("Файл не найден: {}", path.display()))?;
    if metadata.is_dir() || (metadata.file_type().is_symlink() && path.is_dir()) {
        return Err(format!(
            "Безвозвратно удалять папки нельзя: {}",
            path.display()
        ));
    }
    Ok(path)
}

/// Текст ошибки действия без общего префикса: сообщения `files::ops` уже
/// готовы для окна («Файл не найден: …», «Нет доступа: …»).
fn action_error_text(err: &ScanError) -> String {
    match err {
        ScanError::VolumeAccess(message) | ScanError::Io(message) | ScanError::Parse(message) => {
            message.clone()
        }
    }
}

/// Реальный абсолютный путь файла в папке индекса — для действий в окне.
///
/// Индекс хранит только имена, а удалению и открытию нужен путь. Имя проверяется
/// на `.`/`..`, разделители и двоеточие (иначе через него можно было бы
/// выпросить путь за пределами папки), а пара (folder_id, имя) должна реально
/// существовать в индексе: путь выдаётся только для проиндексированного файла.
#[tauri::command(async)]
fn get_file_path(
    state: State<'_, AppState>,
    folder_id: u32,
    file_name: String,
) -> Result<String, String> {
    let index = state
        .service
        .index()
        .ok_or_else(|| "Индекс ещё не готов: сначала завершите сканирование.".to_string())?;

    check_folder_id(&index, folder_id)?;

    if !valid_path_segment(&file_name) {
        return Err(format!("Недопустимое имя файла: «{file_name}»"));
    }
    if !index.has_file(folder_id, &file_name) {
        return Err(format!("Файл не найден в папке индекса: «{file_name}»"));
    }

    let path = index.real_path_of(folder_id, Some(&file_name));
    if path.is_empty() {
        return Err(format!("Не удалось построить путь для файла «{file_name}»"));
    }
    Ok(path)
}

/// Удалить пути: в корзину (`permanent == false`) или безвозвратно.
///
/// Команда асинхронная: `remove_dir_all` большого дерева или обращение к
/// корзине может занять заметное время, а главный поток окна ждать не должен.
/// Безвозвратное удаление принимает только существующие файлы; корзина —
/// любые проверенные пути (её можно отменить восстановлением).
#[tauri::command(async)]
fn delete_paths(paths: Vec<String>, permanent: bool) -> Result<DeleteReport, String> {
    let mut checked = Vec::with_capacity(paths.len());
    for path in &paths {
        checked.push(if permanent {
            permanent_delete_target(path)?
        } else {
            action_path(path)?
        });
    }
    Ok(ops::delete_paths(&checked, permanent))
}

/// Открыть файл приложением по умолчанию.
#[tauri::command(async)]
fn open_path(path: String) -> Result<(), String> {
    let path = existing_action_path(&path)?;
    ops::open_path(&path).map_err(|err| action_error_text(&err))
}

/// Показать файл в проводнике с выделением.
#[tauri::command(async)]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    let path = existing_action_path(&path)?;
    ops::reveal_in_explorer(&path).map_err(|err| action_error_text(&err))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Системные диалоги: выбор папки для сканирования (и сохранение CSV).
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            service: Arc::new(ScanService::new()),
        })
        .invoke_handler(tauri::generate_handler![
            list_drives,
            start_scan,
            cancel_scan,
            get_children,
            get_files,
            get_scan_totals,
            get_treemap,
            search_files,
            get_path_crumbs,
            get_file_types,
            get_types_treemap,
            get_file_path,
            delete_paths,
            open_path,
            reveal_in_explorer
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

    #[test]
    fn file_sort_maps_window_column_names() {
        assert!(matches!(file_sort_from_name("name"), Ok(FileSort::Name)));
        assert!(matches!(
            file_sort_from_name("size_on_disk"),
            Ok(FileSort::SizeOnDisk)
        ));
        assert!(matches!(
            file_sort_from_name("size_logical"),
            Ok(FileSort::SizeLogical)
        ));
        assert!(matches!(
            file_sort_from_name("modified"),
            Ok(FileSort::Modified)
        ));
    }

    #[test]
    fn unknown_file_sort_is_a_russian_error_not_a_panic() {
        let text = file_sort_from_name("size").expect_err("неизвестная колонка должна отвергаться");
        assert!(text.contains("«size»"), "{text}");
        assert!(
            text.contains("size_on_disk") && text.contains("modified"),
            "ошибка должна перечислять допустимые колонки: {text}"
        );
    }

    #[test]
    fn type_sort_maps_window_column_names() {
        assert!(matches!(type_sort_from_name("name"), Ok(TypeSort::Name)));
        assert!(matches!(type_sort_from_name("count"), Ok(TypeSort::Count)));
        assert!(matches!(
            type_sort_from_name("size_on_disk"),
            Ok(TypeSort::SizeOnDisk)
        ));
        assert!(matches!(
            type_sort_from_name("size_logical"),
            Ok(TypeSort::SizeLogical)
        ));
        // Времени изменения у группы расширений нет: колонка не принимается.
        let text = type_sort_from_name("modified").expect_err("modified неприменим к типам файлов");
        assert!(text.contains("«modified»"), "{text}");
        assert!(
            text.contains("count") && text.contains("size_logical"),
            "ошибка должна перечислять допустимые колонки: {text}"
        );
    }

    #[test]
    fn root_name_of_all_drives_is_shared_root() {
        let target = ScanTarget::AllDrives(vec!["C:".to_string(), "D:".to_string()]);
        assert_eq!(root_name(&target), "Все диски");
        // У одиночного диска имя корня прежнее — буква с обратным слэшем.
        assert_eq!(root_name(&ScanTarget::Drive("C:".to_string())), "C:\\");
    }

    #[test]
    fn all_drives_target_comes_from_frontend_as_object() {
        // Внешняя разметка serde: все варианты — объекты, имена — snake_case.
        let target: ScanTarget =
            serde_json::from_str(r#"{"all_drives":["C:","D:"]}"#).expect("all_drives из окна");
        match target {
            ScanTarget::AllDrives(letters) => assert_eq!(letters, ["C:", "D:"]),
            other => panic!("ожидался AllDrives, получено {other:?}"),
        }
        let folder: ScanTarget =
            serde_json::from_str(r#"{"folder":"C:\\Temp"}"#).expect("папка из окна");
        assert!(matches!(folder, ScanTarget::Folder(path) if path.ends_with("Temp")));
    }

    #[test]
    fn action_path_rejects_dot_components() {
        // «..» и «.» уводят путь из папки: с permanent=true это remove_dir_all
        // по чужому каталогу, поэтому такие пути обязаны отвергаться.
        assert!(action_path("C:\\Temp\\..").is_err());
        assert!(action_path("C:\\Temp\\.").is_err());
        assert!(action_path("C:\\Temp\\..\\Windows").is_err());

        let dir = crate::files::test_dir();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"data").expect("файл создаётся");
        assert!(
            action_path(&file.to_string_lossy()).is_ok(),
            "обычный абсолютный путь файла допустим"
        );
    }

    #[test]
    fn action_path_rejects_surrounding_whitespace_without_trimming() {
        // trim() молча переписал бы имя («a.txt » → «a.txt» — другой файл),
        // поэтому путь с пробелами по краям отвергается, а не обрезается.
        assert!(action_path(" C:\\Temp\\a.txt").is_err());
        assert!(action_path("C:\\Temp\\a.txt ").is_err());
        assert!(action_path("C:\\Temp\\a.txt\n").is_err());
    }

    #[test]
    fn permanent_delete_rejects_folders_and_allows_files() {
        let dir = crate::files::test_dir();
        let folder = dir.path().join("папка-навсегда");
        std::fs::create_dir(&folder).expect("папка создаётся");
        let file = dir.path().join("файл.txt");
        std::fs::write(&file, b"data").expect("файл создаётся");

        let folder_text = folder.to_string_lossy().into_owned();
        let error =
            permanent_delete_target(&folder_text).expect_err("безвозвратно удалять папки нельзя");
        assert!(
            error.contains("папки нельзя"),
            "ошибка должна объяснять запрет: {error}"
        );
        assert!(
            permanent_delete_target(&file.to_string_lossy()).is_ok(),
            "файл удалять можно"
        );
        assert!(
            permanent_delete_target(&dir.path().join("нет.txt").to_string_lossy()).is_err(),
            "несуществующий файл не проходит проверку существования"
        );
    }

    /// Валидатор сегмента пути один на ядро: те же правила, что у
    /// `real_path_of` и `get_file_path`.
    #[test]
    fn path_segment_validation_rejects_dots_slashes_and_spaces() {
        assert!(valid_path_segment("a.txt"));
        assert!(
            valid_path_segment("a b.txt"),
            "пробелы внутри имени легальны"
        );
        assert!(!valid_path_segment(""), "пустое имя недопустимо");
        assert!(!valid_path_segment("."));
        assert!(!valid_path_segment(".."));
        assert!(!valid_path_segment("a\\b"));
        assert!(!valid_path_segment("a/b"));
        assert!(!valid_path_segment("x:y"));
        assert!(
            !valid_path_segment(" a.txt"),
            "пробел в начале не обрезается"
        );
        assert!(
            !valid_path_segment("a.txt "),
            "пробел в конце не обрезается"
        );
    }

    #[test]
    fn real_root_of_maps_scan_target() {
        // Диск — «C:\», папка — её путь, «Все диски» — пусто (база — подпапка тома).
        assert_eq!(real_root_of(&ScanTarget::Drive("C:".to_string())), "C:\\");
        assert_eq!(
            real_root_of(&ScanTarget::Folder(PathBuf::from("C:\\Temp\\Тест"))),
            "C:\\Temp\\Тест"
        );
        assert_eq!(
            real_root_of(&ScanTarget::AllDrives(vec!["C:".to_string()])),
            ""
        );
    }

    #[test]
    fn action_path_rejects_relative_and_drive_root() {
        // Временная папка теста, а не жёстко прописанный C:\Temp: чистый клон
        // не обязан иметь чужие каталоги, и тест не должен от них зависеть.
        let dir = crate::files::test_dir();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"data").expect("файл создаётся");
        let valid = file.to_string_lossy().into_owned();
        assert!(
            action_path(&valid).is_ok(),
            "абсолютный путь файла допустим"
        );
        assert!(
            action_path(&dir.path().join("нет-папки").join("a.txt").to_string_lossy()).is_err(),
            "путь в несуществующей папке отвергается"
        );
        assert!(
            action_path("относительный\\файл.txt").is_err(),
            "относительный путь отвергается"
        );
        assert!(action_path("").is_err(), "пустой путь отвергается");
        assert!(
            action_path("C:\\").is_err(),
            "корень диска нельзя удалять или открывать как файл"
        );
        assert!(
            action_path("D:/").is_err(),
            "корень диска с другим слэшем тоже"
        );
    }

    /// Лимит страницы файлов: окно не может запросить больше защиты IPC.
    #[test]
    fn files_page_limit_is_clamped() {
        assert_eq!(files_page_limit(0), 0);
        assert_eq!(files_page_limit(2_000), 2_000);
        assert_eq!(files_page_limit(FILES_ROWS_LIMIT), FILES_ROWS_LIMIT);
        assert_eq!(
            files_page_limit(FILES_ROWS_LIMIT * 3),
            FILES_ROWS_LIMIT,
            "запрос больше лимита урезается до защиты IPC"
        );
    }

    #[test]
    fn existing_action_path_reports_missing_file() {
        let dir = crate::files::test_dir();
        let existing = dir.path().join("есть.bin");
        std::fs::write(&existing, b"data").expect("файл создаётся");
        let existing_text = existing.to_string_lossy().into_owned();
        assert!(existing_action_path(&existing_text).is_ok());

        let missing = dir.path().join("нет.bin");
        let error = existing_action_path(&missing.to_string_lossy())
            .expect_err("несуществующий путь должен отвергаться");
        assert!(error.contains("Файл не найден"), "{error}");
        assert!(
            error.contains("нет.bin"),
            "в ошибке должен быть путь: {error}"
        );
    }
}
