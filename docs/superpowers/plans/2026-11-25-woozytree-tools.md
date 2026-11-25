# WoozyTree — План 3: Инструменты и паритет (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Закрыть функциональный паритет с WizTree: поиск и фильтры, группировка по типам файлов, действия с файлами (корзина/удаление/открытие/проводник/путь), экспорт CSV, настройки отображения, сканирование «Все диски» и отдельной папки — плюс финальная полировка и документация.

**Architecture:** Ядро (Rust) расширяется запросами поверх готового индекса (поиск, типы файлов, пути папок) и командами действий (корзина через крейт `trash`, открытие/проводник через Win32 ShellExecute, CSV через крейт `csv`). Интерфейс получает три вкладки — «Карта», «Типы файлов», «Поиск» — как в оригинальном WizTree; настройки отображения живут на фронте (localStorage) и параметрами уходят в команды карты/таблицы. Удаление — безопасное по умолчанию (корзина), «навсегда» — только с явным подтверждением.

**Tech Stack:** Rust (edition 2024), Tauri 2 (+ tauri-plugin-dialog для системных диалогов), React + TypeScript + Vite, shadcn/ui (пресет `b6ZR0yFgO`), TanStack Table/Virtual/Query, крейты `trash` и `csv`.

**Spec:** [docs/superpowers/specs/2026-11-21-woozytree-design.md](../specs/2026-11-21-woozytree-design.md) + решения Планов 1–2 (Global Constraints, Review Focus).

**Roadmap:** это финальный план паритета; после него — свободное развитие (оптимизации из списка Плана 2, установщик, мультиязычность).

## Global Constraints

- Только Windows 10/11. Приложение запускается с правами администратора (манифест `requireAdministrator`).
- NTFS-диски — через прямое чтение MFT; не-NTFS, сеть и отдельные папки — обходом. Сканирование за единицы секунд; живая карта/топ ≥1 раз/с; интерфейс не подвисает.
- Жёсткие ссылки: размер — один раз; остальные ссылки видны с пометкой и НЕ входят в итоги (процент таких строк — 0.0/«—», не 100%).
- **Безопасность данных (антицель):** удаление файлов — только по явному действию пользователя; по умолчанию в корзину (можно восстановить); «навсегда» — только после подтверждения в отдельном диалоге. Тесты, создающие/удаляющие файлы, — только во временных папках; никогда на реальных данных пользователя.
- Интерфейс: тёмная тема, русские тексты, shadcn/ui; таблицы — TanStack Table + Virtual; запросы — TanStack Query. Настройки отображения хранятся в localStorage.
- Код простой; русские комментарии где логика неочевидна.
- Коммит после каждой задачи. Тесты: `cargo test --manifest-path src-tauri/Cargo.toml --lib` (bin-цели несут манифест администратора) и `npx vitest run`. Сборка — с `--no-bundle`.
- cargo офлайн: CARGO_HOME=<repo>/.superpowers/cargo-home, CARGO_NET_OFFLINE=true. Новые крейты (согласовано): `trash` (корзина), `csv` (уже в кэше как зависимость mft), `tauri-plugin-dialog` (системные диалоги; + npm-пакет). Загрузку новых крейтов выполняет контролёр до задач.
- Документация (README, структура, скриншоты) обновляется в задаче полировки — это часть сдачи.

## Review Focus

1. **Удаление файлов — главный риск проекта.** Корзина по умолчанию, «навсегда» — только с подтверждением; удаляется только то, что выбрано; после удаления данные явно помечаются устаревшими. → Тесты `delete_moves_to_recycle_bin` и `permanent_delete_removes_only_targets` (Задача 4, только temp-папки) + ручная проверка на специально заведённой мусорной папке.
2. **Поиск на 1.26 млн файлов.** Не должен вешать UI: команда async, страницы результатов, цель < 1 с на C:. → Тест `search_files_pages_and_filters` (синтетика) + замер на C: (Задача 2).
3. **«Все диски» — корректная агрегация.** Каждый диск — подпапка корня; итоги корня включают все диски; «Остановить» останавливает весь процесс. → Тест `scan_into_subfolder_aggregates_under_target` (Задача 1) + ручная проверка с двумя томами (временный VHD, как в Плане 2).
4. **CSV-экспорт не ломает данные.** Экранирование запятых/кавычек/переносов, кириллица, BOM для Excel; число строк сходится с таблицей. → Тест `export_csv_escapes_specials` (Задача 5, temp) + открытие файла в Excel вручную.
5. **Настройки отображения согласованы и безобидны.** Переключатель «на диске/фактический», «файлы на карте», порог мелочи — меняют все экраны одинаково; порог влияет только на отображение, не на итоги. → Чистая функция фильтрации порога + vitest (Задача 6) + ручная сверка экранов.

---

## File Structure

```
src-tauri/src/
├─ model.rs                  — + SearchQuery/SearchRow/SearchPage, FileTypeRow, DeleteReport, SizeMode
├─ scan/index.rs             — + path_of(folder), поиск по имени/размеру/дате, файлы по расширениям
├─ scan/mod.rs               — + ScanTarget::AllDrives, сканирование в подпапку (все диски)
├─ scan/mft.rs, walk.rs      — параметр-папка назначения (into: FolderId)
├─ files/ops.rs              — НОВОЕ: удаление (trash/permanent), open, reveal (Win32 ShellExecute)
├─ export/csv.rs             — НОВОЕ: запись CSV (крейт csv, BOM UTF-8)
└─ lib.rs                    — команды search_files/get_file_types/get_types_treemap/delete_paths/
                               open_path/reveal_in_explorer/export_csv; плагин dialog

src/
├─ lib/types.ts, ipc.ts      — новые DTO/обёртки
├─ lib/settings.ts           — НОВОЕ: настройки отображения (localStorage) + чистые функции
├─ components/
│  ├─ Tabs.tsx               — НОВОЕ: вкладки «Карта | Типы файлов | Поиск»
│  ├─ SearchPanel.tsx        — НОВОЕ: фильтры + таблица результатов (TanStack)
│  ├─ FileTypesPanel.tsx     — НОВОЕ: таблица расширений + treemap по типам
│  ├─ FileActionsMenu.tsx    — НОВОЕ: контекстное меню действий + AlertDialog «навсегда»
│  ├─ SettingsPopover.tsx    — НОВОЕ: настройки отображения
│  └─ FileTable/Treemap/...  — + действия, настройки, экспорт
└─ App.tsx                   — вкладки, «Все диски»/«Папка…» в селекторе, плашка «данные устарели»

docs/superpowers/plans/…     — этот план; README — в Задаче 7
```

---

### Task 1: «Все диски» и сканирование отдельной папки

**Files:**
- Modify: `src-tauri/src/model.rs` (`ScanTarget`), `src-tauri/src/scan/mod.rs`, `src-tauri/src/scan/mft.rs`, `src-tauri/src/scan/walk.rs`, `src-tauri/src/lib.rs`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/components/Toolbar.tsx`, `src/App.tsx`
- Test: тесты в `scan/mod.rs`

**Interfaces:**
- Consumes: `run_scan`/`ScanService`/движки (Планы 1–2), `list_drives`.
- Produces (Задачи 2–7):

```rust
// model.rs — РАСШИРЕННЫЙ ScanTarget (serde externally-tagged по умолчанию: {"drive": "C"} | {"folder": "C:\\Temp"} | {"all_drives": ["C:", "D:"]} — все варианты объекты, TS-тип прост):
#[derive(Debug, Clone, serde::Deserialize)]
pub enum ScanTarget {
    Drive(String),
    Folder(PathBuf),
    AllDrives(Vec<String>), // буквы томов от фронтенда (list_drives)
}

// scan/mft.rs, walk.rs — ДОБАВЛЯЕТСЯ параметр назначения (осознанное расширение интерфейсов,
// как waiver Плана 2; старые вызовы обновляются):
pub fn walk(root: &Path, index: &mut ScanIndex, into: FolderId, cancel: &AtomicBool, progress: &mut dyn FnMut(&ScanProgress, &ScanIndex)) -> Result<WalkStats, ScanError>;
pub fn scan_ntfs_reader(reader: &VolumeReader, index: &mut ScanIndex, into: FolderId, cancel: &AtomicBool, progress: &mut dyn FnMut(&ScanProgress, &ScanIndex)) -> Result<ScanTotals, ScanError>;
// поведение: все записи достраиваются под папку `into` (корень для Drive — root()); для AllDrives
// сервис создаёт подпапку с именем диска (напр. «C:») под корнем «Все диски» и сканирует в неё.
```

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/scan/mod.rs`:
- `scan_into_subfolder_aggregates_under_target` — сканирование temp-папки `into` = отдельная подпапка индекса → файлы и итоги лежат под ней, корень содержит только эту подпапку.
- `all_drives_scan_uses_drive_subfolders` — `run_scan(ScanTarget::AllDrives([temp1, temp2]))`-эквивалент через внутренний API → корень «Все диски» содержит две подпапки с независимыми итогами; `ScanTotals` — сумма.
- `cancel_all_drives_stops_everything` — отмена на втором томе → процесс завершён, итоги частичные (`cancelled == true`).

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Параметр `into` в обоих движках (вставка под указанную папку; корень записи №5 MFT не создаёт свой узел, дети идут в `into`). `run_scan`/`ScanService`: `AllDrives` — последовательно по томам, каждый — в свою подпапку (имя = буква, напр. «C:»), общий прогресс (номер тома + общий счётчик), `ScanTotals` — суммы; `Folder` — walk в `root()`; `Drive` — как раньше. UI: в селекторе — «Все диски» (первым) и пункт «Папка…» (диалог выбора папки — плагин `dialog`, см. примечание ниже); крошки для AllDrives: корень «Все диски» (`C:\ >`-стиль не используется — имя корня «Все диски»).

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — PASS, без предупреждений. Ручная проверка: скан «Все диски» (или временный VHD как второй том — паттерн Задачи 6 Плана 2) — корень содержит подпапки дисков, итоги суммарные; скан папки через «Папка…» работает.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src docs
git commit -m "feat(scanner): сканирование «Все диски» и отдельной папки"
```

Примечание: новые зависимости этой задачи — `tauri-plugin-dialog` (crate + `@tauri-apps/plugin-dialog`); контролёр предзагрузит крейт в кэш до старта.

---

### Task 2: Поиск и фильтры

**Files:**
- Modify: `src-tauri/src/model.rs`, `src-tauri/src/scan/index.rs`, `src-tauri/src/lib.rs`
- Create: `src/components/SearchPanel.tsx`, `src/components/Tabs.tsx`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx`
- Test: тесты в `index.rs`; vitest для чистых функций фильтра, если появятся

**Interfaces:**
- Consumes: `ScanIndex` (файлы, сортировка), `FileRow`, `path_of` (новый).
- Produces (Задачи 3–7):

```rust
// scan/index.rs:
impl ScanIndex {
    pub fn path_of(&self, folder: FolderId) -> String; // "C:\Users\georg" — от корня, разделитель '\'
    pub fn search_files(&self, q: &SearchQuery, offset: usize, limit: usize) -> (Vec<SearchRow>, u32); // (строки, total)
}

// model.rs:
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SearchQuery {
    pub name: String,            // подстрока без учёта регистра (правила сравнения — как у сортировки: ASCII + кириллица + «ё»≈«е»)
    pub min_size: u64,           // 0 — без нижней границы
    pub max_size: Option<u64>,   // None — без верхней границы (u64::MAX нельзя передавать из JS — теряет точность!)
    pub modified_after: Option<i64>,  // Unix мс; None — без границы
    pub modified_before: Option<i64>, // Unix мс; None — без границы
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchRow {
    pub name: String,
    pub folder_path: String,
    pub folder_id: u32,      // для перехода к папке
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,
    pub hard_linked: bool,
}
#[derive(Debug, serde::Serialize)]
pub struct SearchPage { pub rows: Vec<SearchRow>, pub total: u32 }

// lib.rs:
#[tauri::command(async)] fn search_files(q: SearchQuery, offset: usize, limit: usize) -> Result<SearchPage, String>;
```

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/scan/index.rs`:
- `search_files_pages_and_filters` — 30 файлов с разными именами/размерами/датами: фильтр по имени (кириллица без учёта регистра), по диапазону размера, по дате; пагинация (offset/limit) и `total`.
- `search_matches_name_case_insensitively` — «ОТЧЁТ» находится по запросу «отчёт», «Ёж» по «еж».
- `path_of_builds_native_path` — цепочка папок → «C:\Users\georg»-подобная строка (для temp-корня — путь от корня индекса).

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Поиск линейный по файлам индекса (1.26 млн — быстро; async-команда не блокирует UI). `Tabs.tsx`: вкладки «Карта | Типы файлов | Поиск» (shadcn `tabs`) — «Типы файлов» пока заглушка (Задача 3). `SearchPanel.tsx`: поля (имя, размер от/до в МиБ/ГиБ, дата от/до), кнопка «Найти», таблица результатов (TanStack Table + Virtual, страницы 2 000): Имя, Папка, Размер, На диске, Изменён, «жёсткая ссылка»; клик по строке → переход в папку (`folder_id`) на вкладке «Карта» (синхронизация с деревом/таблицей — механизм Плана 2).

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — PASS. Замер на C: (scan_bench или временная команда): поиск по подстроке — < 1 с от старта до готовности страницы (зафиксировать в отчёте). Ручная проверка: вкладка «Поиск», кириллица без регистра, фильтры, переход к файлу.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src
git commit -m "feat(search): поиск и фильтры по имени, размеру и дате"
```

---

### Task 3: Типы файлов

**Files:**
- Modify: `src-tauri/src/model.rs`, `src-tauri/src/scan/index.rs`, `src-tauri/src/lib.rs`
- Create: `src/components/FileTypesPanel.tsx`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx` (вкладка)
- Test: тесты в `index.rs`

**Interfaces:**
- Consumes: файлы индекса, `layout_treemap`/`TreemapRect` (План 2).
- Produces:

```rust
// model.rs:
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileTypeRow {
    pub extension: String,   // без точки; "" — «без расширения» (показывать «(нет)»)
    pub count: u32,
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub percent_of_parent: f32,
}
#[derive(Debug, Clone, Copy)]
pub enum TypeSort { Name, Count, SizeOnDisk, SizeLogical } // сортировка таблицы типов (Modified неприменим)

// scan/index.rs:
impl ScanIndex {
    pub fn file_types(&self, folder: FolderId, sort: TypeSort, desc: bool) -> Vec<FileTypeRow>; // агрегаты по расширениям рекурсивно
}

// lib.rs:
#[tauri::command(async)] fn get_file_types(folder_id: u32, sort: String, desc: bool) -> Result<Vec<FileTypeRow>, String>;
#[tauri::command(async)] fn get_types_treemap(folder_id: u32, max_rects: usize) -> Result<Vec<TreemapRect>, String>; // плитки = расширения (id=0, is_dir=false, имя=расширение)
```

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/scan/index.rs`:
- `file_types_aggregate_by_extension` — файлы `.txt`/`.TXT`/`.rs`/без расширения → 3 группы (txt: 2, rs: 1, «(нет)»: 1), размеры суммарны, `percent_of_parent` — от итога папки.
- `file_types_are_recursive` — вложенные папки входят в агрегаты корня.
- `types_treemap_covers_container` — `get_types_treemap`-логика (через `layout_treemap`) даёт плитки, покрывающие контейнер; бюджет соблюдён.

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Агрегация по расширению (без учёта регистра; «ё» не требуется — расширения латинские, но сравнение унифицировано). `FileTypesPanel.tsx`: treemap по типам (переиспользовать canvas-компонент `Treemap.tsx` режимом «плоский» или отдельным простым рендером — решение за исполнителем, зафиксировать в отчёте) + таблица расширений (сортировка по клику); размеры ГиБ/МиБ/КиБ, «0 Б».

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — PASS. Ручная проверка: вкладка «Типы файлов» на C: — карта расширений + таблица (exe/dll/…), сортировка, суммы сходятся с итогами.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src
git commit -m "feat(types): группировка по типам файлов с treemap и таблицей"
```

---

### Task 4: Действия с файлами (корзина, удаление, открытие, проводник, путь)

**Files:**
- Create: `src-tauri/src/files/ops.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/model.rs`
- Create: `src/components/FileActionsMenu.tsx`
- Modify: `src/components/FileTable.tsx`, `src/components/Treemap.tsx` (вызов меню), `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx` (плашка «данные устарели»)
- Test: тесты в `files/ops.rs` (ТОЛЬКО temp-папки!)

**Interfaces:**
- Consumes: `ScanIndex` (пути: `path_of` + имя файла), `FileRow`.
- Produces:

```rust
// model.rs:
#[derive(Debug, serde::Serialize)]
pub struct DeleteReport { pub deleted: u32, pub errors: Vec<String> }

// files/ops.rs:
pub fn delete_paths(paths: &[PathBuf], permanent: bool) -> DeleteReport; // permanent=false → корзина (крейт trash)
pub fn open_path(path: &Path) -> Result<(), ScanError>;                  // ShellExecuteW "open"
pub fn reveal_in_explorer(path: &Path) -> Result<(), ScanError>;         // explorer.exe /select,<path>

// lib.rs (пути — строки от фронтенда; валидация: абсолютный путь, существование):
#[tauri::command(async)] fn delete_paths(paths: Vec<String>, permanent: bool) -> Result<DeleteReport, String>;
#[tauri::command(async)] fn open_path(path: String) -> Result<(), String>;
#[tauri::command(async)] fn reveal_in_explorer(path: String) -> Result<(), String>;
```

- [ ] **Step 1: Написать падающие тесты (только temp-папки!)**

В `src-tauri/src/files/ops.rs`:
- `delete_moves_to_recycle_bin` — два файла в tempdir → `delete_paths(permanent=false)` → файлы исчезли из папки, `deleted == 2`, ошибок нет (восстановление из корзины вручную не проверяем — главное: исчезли и без ошибок).
- `permanent_delete_removes_only_targets` — 3 файла, удаляем 2 → удалены ровно 2, третий на месте; `DeleteReport.deleted == 2`.
- `delete_reports_errors_without_panicking` — один путь не существует → `errors.len() == 1`, остальные удалены.

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

`delete_paths`: `permanent=false` → `trash::delete` на каждый путь (ошибка по пути — в `errors`, не прерывает); `permanent=true` → `std::fs::remove_file`/`remove_dir_all`. `open_path`/`reveal_in_explorer` — Win32 `ShellExecuteW` (крейт `windows` уже есть). UI: `FileActionsMenu.tsx` — контекстное меню (shadcn `dropdown-menu`, правый клик по строке таблицы и по выбранной файловой плитке карты): «Открыть», «Показать в проводнике», «Копировать путь» (navigator.clipboard), «Удалить в корзину», «Удалить навсегда…» — последний открывает AlertDialog (shadcn `alert-dialog`): «Безвозвратно удалить N файлов? Это действие нельзя отменить.» с кнопками «Отмена»/«Удалить навсегда» (деструктивная). После любого удаления — плашка «Данные устарели — изменился диск» + кнопка «Пересканировать» (проще и честнее, чем патчить индекс).

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — PASS, без предупреждений. Ручная проверка — НА СПЕЦИАЛЬНО ЗАВЕДЁННОЙ МУСОРНОЙ ПАПКЕ (создать папку с тестовыми файлами, например `C:\Temp\woozzytree-delete-test\`; НИКОГДА не на реальных данных): удалить в корзину (файлы в корзине — проверить глазами), восстановить; удалить навсегда с подтверждением; «Отмена» в диалоге ничего не удаляет; «Открыть»/«Показать в проводнике»/«Копировать путь» работают. Скриншот диалога подтверждения: `.superpowers/tools/plan3-delete-dialog.png`.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src
git commit -m "feat(actions): корзина, удаление с подтверждением, открытие и проводник"
```

---

### Task 5: Экспорт CSV

**Files:**
- Create: `src-tauri/src/export/csv.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/model.rs`
- Modify: `src/components/FileTable.tsx` (кнопка «Экспорт CSV»), `src/lib/types.ts`, `src/lib/ipc.ts`
- Test: тесты в `export/csv.rs` (temp-папки)

**Interfaces:**
- Consumes: `child_files`/`file_count`/`path_of`, `FileSort`.
- Produces:

```rust
// export/csv.rs:
pub fn write_files_csv(path: &Path, folder: FolderId, index: &ScanIndex, sort: FileSort, desc: bool) -> Result<u32, ScanError>; // возвращает число строк; заголовок: Имя;Папка;Размер (байт);На диске (байт);Изменён;Жёсткая ссылка

// lib.rs:
#[tauri::command(async)] fn export_csv(path: String, folder_id: u32, sort: String, desc: bool) -> Result<u32, String>;
```

Формат: UTF-8 **с BOM** (Excel), разделитель `;` (русский Excel), даты ISO-8601, булево «да»/«нет»; имена с `;`/`"`/переносами — в кавычках с удвоением кавычек (стандарт CSV). Диалог сохранения — плагин `dialog` (Задача 1).

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/export/csv.rs`:
- `export_csv_escapes_specials` — файлы `a;b.txt`, `кав"ычка.txt`, `много\nстрок.txt`, кириллица → прочитать файл обратно `csv`-крейтом → имена совпадают, полей 6, BOM на месте.
- `export_csv_counts_rows` — 25 файлов → возвращено 25, в файле 25 строк данных + заголовок.
- `export_csv_includes_hard_link_flag` — строка дубликата содержит «да».

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Запись всех строк папки (не только загруженных страниц) текущей сортировки. UI: кнопка «Экспорт CSV» в панели таблицы → `dialog.save()` (фильтр `*.csv`, имя по умолчанию `woozytree-<имя папки>.csv`) → команда → статус «Экспортировано N строк: <путь>» + кнопка «Показать в проводнике».

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — PASS. Ручная проверка: экспорт папки с кириллицей/спецсимволами → файл открыт в Excel (или Блокноте): заголовки, разделитель «;», кириллица читается, число строк сходится с таблицей.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src
git commit -m "feat(export): экспорт списка файлов в CSV с диалогом сохранения"
```

---

### Task 6: Настройки отображения

**Files:**
- Create: `src/lib/settings.ts`, `src/components/SettingsPopover.tsx`
- Modify: `src-tauri/src/model.rs`, `src-tauri/src/lib.rs` (параметры `get_treemap`/`get_types_treemap`/`get_files`), `src-tauri/src/treemap/layout.rs` (`treemap_for_folder` — параметры отображения)
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx`, `src/components/Treemap.tsx`, `src/components/FileTable.tsx`, `src/components/TopFolders.tsx`
- Test: тесты в `treemap/layout.rs` + vitest для `settings.ts`

**Interfaces:**
- Consumes: всё из Планов 1–2.
- Produces:

```rust
// model.rs:
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub enum SizeMode { OnDisk, Logical } // serde: "on_disk" | "logical"

// lib.rs — команды получают параметры отображения (ОСОЗНАННОЕ расширение сигнатур Плана 2; старые вызовы обновляются):
#[tauri::command(async)] fn get_treemap(folder_id: u32, max_rects: usize, size_mode: String, include_files: bool, min_tile_bytes: u64) -> Result<Vec<TreemapRect>, String>;
#[tauri::command(async)] fn get_types_treemap(folder_id: u32, max_rects: usize, size_mode: String) -> Result<Vec<TreemapRect>, String>;
// get_files (Плана 2) тоже получает `size_mode: String` — осознанное расширение сигнатуры: процент строки считается от выбранного размера:
// #[tauri::command(async)] fn get_files(folder_id: u32, sort: String, desc: bool, offset: usize, limit: usize, size_mode: String) -> Result<FilePage, String>;
// treemap_for_folder получает те же три параметра (SizeMode, include_files, min_tile_bytes):
// include_files=false — только папки; min_tile_bytes — плитки мельче складываются в «прочее» (ТОЛЬКО отображение, итоги не меняются).

// src/lib/settings.ts — чистые функции + хранение (localStorage, ключ "woozytree.settings"):
// { sizeMode: "on_disk" | "logical", showFilesOnMap: boolean, minTileBytes: number }
```

- [ ] **Step 1: Написать падающие тесты**

Rust (`treemap/layout.rs`):
- `min_tile_threshold_lumps_small_into_other` — 10 элементов, `min_tile_bytes` выше половины → крупные отдельно, мелочь в «прочее»; СУММА площадей и итоги не изменены.
- `include_files_false_shows_only_folders` — на смешанном индексе → в выдаче только папки.
- `size_mode_logical_changes_layout` — файлы с разными logical/allocated → порядок/площади соответствуют выбранному режиму.
JS (`settings.test.ts`):
- `settings_roundtrip_localstorage` — сохранение/чтение, значения по умолчанию, миграция мусора.

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`; `npx vitest run` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Параметры проходят через `get_treemap`/`get_types_treemap`/`treemap_for_folder`; `get_files` — `size_mode` влияет на сортировку по умолчанию и колонку `%` (от выбранного размера). UI: `SettingsPopover.tsx` (кнопка «Настройки» в тулбаре, shadcn `popover`): радио «Размер: на диске / фактический», свитч «Показывать файлы на карте», поле «Не показывать мельче (МиБ)»; изменения применяются сразу (перезапрос карты/таблицы), хранятся в localStorage.

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: оба прогона — PASS. Ручная проверка: переключение режимов меняет карту/таблицу/топ согласованно; порог собирает мелочь в «прочее», но СУММЫ в статусе/дереве не меняются; после перезапуска приложения настройки сохранены.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src src
git commit -m "feat(settings): настройки отображения — размер, файлы на карте, порог мелочи"
```

---

### Task 7: Полировка, документация и чек-лист паритета

**Files:**
- Modify: мелочи по списку (`src/lib/*`, `src/components/*`, `src-tauri/src/scan/index.rs`, `src-tauri/src/lib.rs`)
- Modify: `README.md`, `docs/screenshot.png`
- Test: обновить/добавить затронутые тесты; финальный ручной чек-лист

**Interfaces:**
- Consumes: всё из Задач 1–6.
- Produces: готовый паритетный WoozyTree.

- [ ] **Step 1: Закрыть накопленный список мелочей (все пункты обязательны)**

1. `hard_linked`-строки: процент `0.0` (или «—») вместо cap 1.0 — обновить тест `hard_linked_percent_stays_within_one` → `hard_linked_rows_show_zero_percent`.
2. Размеры 1–1023 Б — «N Б» (не «0.0 КиБ»); обновить тест format.
3. Склонения: «в 1 папке / в 2 папках» (`foldersLabel` рядом с `filesLabel` в FolderTree).
4. `setError(null)` при успешном обновлении списка дисков (плашка ошибки не висит).
5. `get_files`: кламп `limit.min(10_000)`; заменить вывод max-id из `folder_count` на явную проверку существования папки.
6. Догрузка таблицы: `if (query.isPlaceholderData) return;` в эффекте страниц.
7. `crumbs_for_tile` — поиск по `id`+`is_dir` вместо `rects.indexOf` (устойчивость к рассинхрону).
8. Единый тип `Crumb` (убрать дубль ScanStatus/treemap); тест, что `handleMapNavigate` **заменяет** путь (`setPath(crumbs)`), а не дописывает.
9. Корневая карта: перезапрос `get_treemap` по переходу `scanning → false` (страховка от потери финального события).
10. Подписи на карте: у плитки-папки с детьми печатать её собственный размер рядом с именем чипа (убрать двусмысленность «размер ребёнка под чипом»).
11. `formatSize`/`foldersLabel` и пр. — прогнать `cargo fmt` отдельным коммитом (закрыть fmt-долг: сначала `cargo fmt` только для изменённых задачей файлов, затем — общий проход, если дифф приемлем; иначе — честная запись в отчёте).

- [ ] **Step 2: Документация**

README: все фичи паритета (карта/дерево/таблица/поиск/типы/действия/CSV/настройки/«Все диски»), структура проекта (включая files/, export/, settings.ts, новые компоненты), «Известные ограничения» финальные (точность, `$ATTRIBUTE_LIST`), снимок `docs/screenshot.png` — свежий экран с картой (и, если уместно, второй — с вкладкой «Поиск»). Пункт Roadmap «План 3» — заменить на «свободное развитие».

- [ ] **Step 3: Финальный прогон и чек-лист паритета**

1. Полный прогон: cargo (все), vitest (все), `npm run build`, `tauri build --debug --no-bundle` — чисто.
2. Чек-лист паритета с дизайн-документом §6 (пункт за пунктом, фактами): сканирование диска/«Все диски»/папки/повторное; treemap с подсказками/зумом/возвратом; дерево + таблица (имя, размер на диске, фактический, %, дата, число файлов/папок в дереве, сортировка); режим «Типы файлов» с treemap; поиск по имени/размеру/дате; действия (удалить в корзину/навсегда с подтверждением, открыть, показать в проводнике, копировать путь); экспорт CSV; настройки (два размера, файлы на карте, порог мелочи); жёсткие ссылки честно; живое сканирование; «Остановлено»/«Ошибка».
3. Скриншоты итогов: `.superpowers/tools/plan3-final-map.png`, `plan3-final-search.png`, `plan3-final-types.png`.

- [ ] **Step 4: Commit**

```powershell
git add -A
git commit -m "feat: полировка паритета, документация и финальная сверка с WizTree"
```
