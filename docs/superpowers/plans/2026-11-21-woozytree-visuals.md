# WoozyTree — План 2: Живая карта и файлы (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WoozyTree показывает главное из WizTree: treemap-карту занятости, растущую вживую по мере сканирования, дерево папок и таблицу файлов с сортировкой и пометками жёстких ссылок.

**Architecture:** Ядро (Rust) становится инкрементальным: дерево строится батчами во время чтения MFT/обхода, события `scan://top`/`scan://treemap` уходят во фронт таймером раз в секунду, отмена прерывает и чтение тоже. Раскладку treemap считает Rust (алгоритм squarified) и отдаёт готовые прямоугольники; фронт рисует их на canvas с hover/кликом/зумом. Дерево и таблица файлов — React + TanStack Table/Virtual; файлы отдаются страницами, чтобы папки с сотнями тысяч файлов не вешали интерфейс.

**Tech Stack:** Rust (edition 2024), Tauri 2, React + TypeScript + Vite, shadcn/ui (пресет `b6ZR0yFgO`), TanStack Table + TanStack Virtual (+ TanStack Query для запросов данных), canvas 2D.

**Spec:** [docs/superpowers/specs/2026-11-21-woozytree-design.md](../specs/2026-11-21-woozytree-design.md) + решения Плана 1 (см. Global Constraints и Review Focus).

**Roadmap:** План 3 — поиск/фильтры, типы файлов, действия с файлами (удаление, открытие, «показать в проводнике»), CSV, настройки отображения.

## Global Constraints

- Только Windows 10/11. Приложение запускается с правами администратора (манифест `requireAdministrator`).
- NTFS-диски сканируются через прямое чтение MFT; не-NTFS, сетевые диски и отдельные папки — обходом папок.
- Скорость (из спецификации): диск ~1 ТБ с 1–2 млн файлов — полное сканирование NTFS за единицы секунд; первые данные на экране — в первую секунду; интерфейс не подвисает. **Живость (требование Плана 2):** карта и топ-папки обновляются вживую, не реже раза в секунду во время сканирования; счётчик папок живой (не 0 до конца).
- Жёсткие ссылки: размер засчитывается один раз — в первую найденную ссылку; остальные ссылки видны в таблице с фактическим размером и пометкой «жёсткая ссылка», но не входят в итоги.
- Тесты, которые создают и удаляют файлы, работают только во временных папках. Никаких операций записи/удаления на реальных данных пользователя.
- Интерфейс: тёмная тема, тексты на русском, компоненты shadcn/ui; таблица файлов — на TanStack Table + TanStack Virtual.
- Код простой и понятный; короткие комментарии на русском там, где логика неочевидна.
- Коммит после каждой задачи. Никакой работы поверх незакоммиченного предыдущего шага.
- Тесты: `cargo test --manifest-path src-tauri/Cargo.toml --lib` (bin-цели несут манифест администратора; тестов у них нет). Сборка проверяется с `--no-bundle`.
- cargo офлайн: CARGO_HOME=<repo>/.superpowers/cargo-home, CARGO_NET_OFFLINE=true; новые крейты — только по согласованию с контролёром.
- Отмена сканирования прерывает и разбор, и чтение диска (прерывание чтения — часть этого плана).
- Отменённый скан показывается как «Остановлено» с пометкой «данные частичные» (уже есть; не ломать).

## Review Focus

1. **Память и объёмы данных.** Индекс теперь хранит каждый файл (1.25 млн на C:); пиковая память и размер ответов IPC не должны вырасти квадратично или зависать UI. → Тест `get_files_returns_paged_rows` (Задача 2) + замер памяти на C: в отчёте Задачи 5/6.
2. **Папки с сотнями тысяч файлов.** `Users`, `Windows` содержат сотни тысяч файлов: таблица не грузит всё разом. → Тест `get_files_respects_limit_offset` (Задача 2), ручная проверка на папке с 100k+ файлов (Задача 5).
3. **Treemap под нагрузкой живых обновлений.** Прямоугольники меняются под курсором во время сканирования; клик/зум не должен «уезжать» в другую папку. → Тест `layout_is_deterministic_for_same_snapshot` (Задача 3) + правило UI: во время сканирования карта обновляется, но навигация работает по зафиксированному на момент клика id папки (ручная проверка Задачи 4).
4. **Сортировка и проценты файлов.** `percent_of_parent` у файлов, порядок сортировки стабилен, жёсткие ссылки не задваивают сортировку по размеру. → Тесты `child_files_sorts_stably` и `hard_linked_row_is_visible_but_not_counted` (Задача 2).
5. **Мгновенная отмена.** «Остановить» прерывает чтение диска, а не только разбор. → Тест `read_records_stops_promptly_on_cancel` (Задача 1): чтение фикстуры с отменой после N записей возвращается быстро и с неполным счётчиком; ручное подтверждение «Остановить» < 1 с (Задача 6).

---

## File Structure

```
src-tauri/src/
├─ model.rs                  — + FileRow, FileSort, TreemapRect (DTO для фронтенда)
├─ scan/
│  ├─ index.rs               — + хранение файлов, child_files (страницы, сортировка)
│  ├─ mft.rs                 — инкрементальная вставка батчами, живые top/treemap-события
│  ├─ walk.rs                — то же для обхода; cancel-просмотр
│  ├─ volume.rs              — read_records принимает cancel (прерывание чтения)
│  └─ mod.rs                 — ScanSink + top/treemap-таймер, run_scan
├─ treemap/
│  └─ layout.rs              — squarified-раскладка (чистая математика, юнит-тесты)
└─ lib.rs                    — команды get_files/get_treemap, события scan://treemap

src/
├─ lib/types.ts, ipc.ts      — TS-типы/обёртки + пагинация
├─ components/
│  ├─ Treemap.tsx            — canvas-карта: hover, клик-зум, хлебные крошки
│  ├─ FolderTree.tsx         — дерево папок (раскрывается по требованию)
│  ├─ FileTable.tsx          — таблица файлов (TanStack Table + Virtual, страницы)
│  └─ Toolbar/ScanStatus/TopFolders — существующие; полировка по Задаче 6
└─ App.tsx                   — компоновка: тулбар, карта, снизу дерево + таблица
```

---

### Task 1: Инкрементальное сканирование и живые события

**Files:**
- Modify: `src-tauri/src/model.rs` (НОВОЕ: DTO `TreemapRect` — создаётся здесь, раскладку даст Задача 3)
- Modify: `src-tauri/src/scan/volume.rs` (сигнатура `read_records`)
- Modify: `src-tauri/src/scan/mft.rs`, `src-tauri/src/scan/walk.rs`, `src-tauri/src/scan/mod.rs`
- Test: модульные тесты в тех же файлах

**Interfaces:**
- Consumes: `ScanIndex`, `ScanSink`, `ScanProgress`, `ScanTotals`, `FileMeta` (План 1).
- Produces (Задачи 2–6 полагаются на это):

```rust
// volume.rs — ИЗМЕНЁННАЯ сигнатура (отмена прерывает чтение):
pub fn read_records(&self, cancel: &AtomicBool, sink: &mut dyn FnMut(&[u8])) -> Result<u64, ScanError>;

// scan/mod.rs — расширенный sink (таймер живых событий живёт в run_scan):
pub trait ScanSink {
    fn progress(&mut self, p: &ScanProgress);
    fn top_folders(&mut self, rows: &[FolderSummary]);   // как в Плане 1
    fn treemap(&mut self, rects: &[TreemapRect]);        // НОВОЕ: живая карта (Задача 3 подключит данные)
    fn done(&mut self, r: Result<&ScanTotals, &ScanError>);
}
// Поведение: run_scan вызывает sink.top_folders/sink.treemap с периодом live_interval (внутренний параметр;
// в проде — 1 с, тесты передают Duration::ZERO и получают события на каждом батче) во время сканирования
// и один раз по завершении; ScanProgress.folders — живой счётчик (число папок в индексе на момент события).

// mft.rs — инкрементальная вставка: записи вставляются в ScanIndex батчами по мере чтения/разбора,
// родители, пришедшие позже ребёнка, доустанавливаются волной после батча (или в конце) — итог идентичен двухпроходному.
```

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/scan/mft.rs` / `walk.rs` / `volume.rs`:
- `read_records_stops_promptly_on_cancel` — чтение фикстуры (`from_fixture`), `cancel` выставлен в колбэке после 100 записей → `read_records` возвращается с количеством ~100 (не 2000), без ошибки; замер: возврат быстрее секунды.
- `scan_ntfs_reader_is_incremental` — `progress.folders` больше 0 ДО завершения (есть хотя бы одно промежуточное событие с ненулевыми папками); итоги совпадают с прежними константами синтетической фикстуры.
- `run_scan_emits_live_top_and_treemap` — тестовый sink с `live_interval = Duration::ZERO` получает `top_folders`/`treemap` минимум один раз ДО `done` (синтетическая фикстура, несколько батчей); события упорядочены: до `done`.
- `incremental_equals_batch_build` — индекс, построенный инкрементально, даёт те же `folder_totals`, что и прежний двухпроходный путь (тот же набор записей).

- [ ] **Step 2: Запустить тесты и убедиться, что падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: FAIL (новые тесты не проходят / сигнатура `read_records` не совпадает).

- [ ] **Step 3: Реализовать**

`read_records` получает `cancel` (проверка между блоками чтения, возврат с частичным счётчиком — без ошибки). `mft.rs`: записи вставляются в индекс батчами (порядок батчей — порядок чтения; неразрешимые пока родители — в корень с последующей «перевязкой» волной; итоговое состояние обязано совпадать с прежним). `ScanProgress.folders` — число папок в индексе. `run_scan`: таймер живых событий (не чаще/не реже раза в секунду), вызовы `sink.top_folders` и `sink.treemap` (для Задачи 1 в `treemap` пока можно передавать пустой срез — данные подключит Задача 3; главное — ритм событий).

- [ ] **Step 4: Запустить тесты и убедиться, что проходят**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: PASS, вывод без предупреждений. Замер на C: (через `scan_bench`, с повышением): время сканирования не хуже 6.5 с, `folders` в прогрессе ненулевой, итоги совпадают с Планом 1 (±0.5% от прежних: 1 238 806+ файлов / 281 257+ папок — живой том меняется, сверять формат, не абсолют).

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src
git commit -m "feat(scanner): инкрементальный индекс, живые события и быстрая отмена чтения"
```

---

### Task 2: Файлы в индексе: `FileRow`, страницы, сортировка

**Files:**
- Modify: `src-tauri/src/model.rs`, `src-tauri/src/scan/index.rs`
- Modify: `src-tauri/src/scan/mft.rs`, `src-tauri/src/scan/walk.rs` (передают `FileMeta` в новые слоты индекса)
- Test: тесты в `index.rs`

**Interfaces:**
- Consumes: `ScanIndex::add_file(parent, FileMeta) -> AddOutcome` (План 1; `FileMeta` больше не выбрасывается).
- Produces (Задачи 3–6):

```rust
// model.rs
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileRow {
    pub name: String,
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,          // Unix мс
    pub percent_of_parent: f32, // доля от размера родителя, 0.0..=1.0
    pub hard_linked: bool,      // true у повторных ссылок (в итоги не входят)
}
#[derive(Debug, Clone, Copy)]
pub enum FileSort { Name, SizeOnDisk, SizeLogical, Modified }

// index.rs
impl ScanIndex {
    pub fn child_files(&self, folder: FolderId, sort: FileSort, desc: bool, offset: usize, limit: usize) -> Vec<FileRow>;
    pub fn file_count(&self, folder: FolderId) -> u32; // число файлов-ссылок непосредственно в папке (для пагинации)
}
```

Правила: сортировка стабильна (равные элементы — по имени как tie-break); `hard_linked: true` у строк, чей `AddOutcome` был `HardLinkDuplicate` (фактический размер показывается, в итоги не входит — так и считает индекс); `percent_of_parent` — от размера родителя, 0.0 при нулевом родителе.

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/scan/index.rs`:
- `get_files_returns_paged_rows` — папка с 25 файлами: `child_files(..., offset: 10, limit: 10)` → ровно 10 строк, корректный срез по текущей сортировке; `file_count` == 25.
- `get_files_respects_limit_offset` — граничные случаи: offset за пределами → пусто; limit = 0 → пусто; limit больше числа → всё.
- `child_files_sorts_stably` — файлы с одинаковым размером → порядок по имени; `FileSort::Modified`/`Name`/`SizeLogical` дают ожидаемые порядки (desc и asc).
- `hard_linked_row_is_visible_but_not_counted` — файл с двумя ссылками: обе строки видны, у второй `hard_linked == true`; сумма `size_on_disk` по строкам больше итога папки ровно на размер дубликата; `folder_totals` не задвоен.

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

`ScanIndex` хранит файлы: плоский `Vec` записей (имя, размеры, modified, признак дубликата, parent) + по папке — индексы файлов. Память: без лишних аллокаций на файл (имя — один String). `add_file` заполняет слот (дубликаты — с флагом, без вклада в агрегаты). Сортировка — при выдаче (папка одного уровня; сортировать каждый запрос допустимо — зафиксировать в отчёте фактическое время на 100k файлов).

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: PASS, без предупреждений. Замер: `scan_bench` на C: — пиковая память процесса записана в отчёт (ориентир: < 1.5 ГБ на 1.25 млн файлов), время сканирования не хуже 7 с.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src
git commit -m "feat(scanner): файлы в индексе — FileRow, страницы и сортировка"
```

---

### Task 3: Раскладка treemap в Rust и команда `get_treemap`

**Files:**
- Create: `src-tauri/src/treemap/layout.rs`
- Modify: `src-tauri/src/lib.rs` (команда), `src-tauri/src/scan/mod.rs` (данные для живой карты)
- Test: тесты в `layout.rs`

**Interfaces:**
- Consumes: `ScanIndex` (Задачи 1–2: агрегаты папок и файлы), `FileRow`/`FolderSummary`.
- Produces (Задача 4):

```rust
// treemap/layout.rs — чистая математика, без Tauri:
#[derive(Debug, Clone, Copy)]
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 } // нормированные координаты 0..1

pub struct TreemapItem { pub id: u64, pub name: String, pub size: u64, pub is_dir: bool }

/// Squarified-раскладка: items сортируются по size desc, rect-контейнер делится на плитки.
/// Возвращает пары (индекс элемента, прямоугольник); порядок — по убыванию size.
pub fn layout_treemap(items: &[TreemapItem], container: Rect) -> Vec<(usize, Rect)>;

// model.rs — DTO для фронтенда (координаты уже нормированы 0..1; id: для папок — FolderId, для файлов — 0, отличать по is_dir):
#[derive(Debug, Clone, serde::Serialize)]
pub struct TreemapRect { pub id: u64, pub name: String, pub size: u64, pub is_dir: bool, pub x: f64, pub y: f64, pub w: f64, pub h: f64 }

// lib.rs:
#[tauri::command] fn get_treemap(folder_id: u32, max_rects: usize) -> Result<Vec<TreemapRect>, String>;

// treemap/layout.rs — общая функция данных (ею пользуются и команда, и живые события Задачи 1):
pub fn treemap_for_folder(index: &ScanIndex, folder: FolderId, max_rects: usize) -> Vec<TreemapRect>;
// рекурсивно раскладывает детей (папки и файлы) до бюджета max_rects (крупное — мельче, мелочь — одним
// прямоугольником «прочее», id = 0); live-события scan://treemap шлют срез корня сканирования с тем же бюджетом.
```

- [ ] **Step 1: Написать падающие тесты**

В `src-tauri/src/treemap/layout.rs`:
- `layout_covers_container_area` — сумма площадей плиток равна площади контейнера (±1e-9).
- `layout_rects_do_not_overlap` — попарно пересечение площадей == 0 для 50 псевдослучайных наборов.
- `layout_is_deterministic_for_same_snapshot` — два вызова на одном срезе → идентичные прямоугольники.
- `layout_orders_by_size_desc` — первый элемент — самый крупный; минимальный размер плитки пропорционален размеру (монотонность: больший размер → не меньшая площадь).
- `budget_collapses_small_items` — `treemap_for_folder` с бюджетом 10 на дерево из 100 элементов (через тестовый `ScanIndex`) → ≤ 10 прямоугольников, крупные отдельно, мелочь собрана в «прочее» (id = 0).

- [ ] **Step 2: Запустить тесты — должны падать**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: FAIL.

- [ ] **Step 3: Реализовать**

Классический squarified алгоритм (Bruls/van Wijk) — нормализованные координаты. Рекурсивная укладка для `get_treemap`: дети папки укладываются в её прямоугольник; при исчерпании бюджета — остаток одним прямоугольником «прочее» (id = 0). Живая карта: `run_scan` (Задача 1) шлёт `scan://treemap` для корня с бюджетом ~2000.

- [ ] **Step 4: Запустить тесты — должны проходить**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: PASS, без предупреждений. Замер: раскладка корня C: (1.25 млн записей, бюджет 2000) < 100 мс — зафиксировать в отчёте.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src
git commit -m "feat(treemap): squarified-раскладка в Rust и команда get_treemap"
```

---

### Task 4: Карта на canvas — hover, клик-зум, навигация

**Files:**
- Create: `src/components/Treemap.tsx`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx`, `src/components/ScanStatus.tsx` (крошки/возврат)
- Test: ручная проверка + скриншот (UI-задача; логика hit-test — чистые функции в `src/lib/treemap.ts` с юнит-тестами, если выносится)

**Interfaces:**
- Consumes: `get_treemap` / `scan://treemap` (Задача 3), `ScanTarget`, события Плана 1.
- Produces (Задача 5): компонент `<Treemap folderId={...} onNavigate={(id) => ...} />`; хит-тест точка→плитка.

- [ ] **Step 1: Реализовать компонент**

Canvas 2D с учётом devicePixelRatio; плитки — прямоугольники с цветовой шкалой по типу (папки — оттенки акцентного цвета пресета, файлы — нейтральные; границы 1px). Hover — подсветка + всплывающая подсказка (имя, размер, % — shadcn `tooltip`). Клик по папке — зум внутрь (`folderId` меняется, карта перерисовывается); клик по файлу — подсветка (действия — План 3). Хлебные крошки «C:\ > Users > …» + кнопка «Назад»/«Вверх» и колесо мыши (Alt+колесо — вверх?). Живые обновления: подписка на `scan://treemap` обновляет данные, НО если пользователь внутри папки — карта этой папки перезапрашивается `get_treemap`, а живой поток рисуется только на уровне корня (правило Review Focus №3). Тёмная тема, русские подписи.

- [ ] **Step 2: Проверка hit-test**

Логика hit-test выносится в чистые функции `src/lib/treemap.ts` (`hit_test(rects: TreemapRect[], x: number, y: number) -> TreemapRect | null`). Добавить dev-зависимость `vitest`; тесты: `hit_test_finds_tile_under_point` (точка внутри плитки → эта плитка; на границе — приоритет верхнего элемента, т.к. дети рисуются поверх родителей), `hit_test_returns_null_in_gaps` (пустая область → null). Запуск: `npx vitest run` — Expected: PASS.

- [ ] **Step 3: Ручная проверка + скриншот**

`npm run tauri build -- --debug --no-bundle`, запуск exe (с повышением через Start-Process -Verb RunAs, паттерн Плана 1): сканирование C: — карта растёт вживую (снимок в середине и после), hover показывает подсказки, клик входит в папку, «Назад» возвращает, UI не подвисает. Скриншоты: `.superpowers/tools/plan2-treemap-live.png`, `.superpowers/tools/plan2-treemap-zoom.png`.

- [ ] **Step 4: Commit**

```powershell
git add src docs package.json package-lock.json
git commit -m "feat(ui): treemap-карта на canvas — hover, зум, навигация, живые обновления"
```

---

### Task 5: Дерево папок и таблица файлов (TanStack)

**Files:**
- Create: `src/components/FolderTree.tsx`, `src/components/FileTable.tsx`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/App.tsx` (QueryClient-провайдер TanStack Query)
- Modify: `src-tauri/src/lib.rs` (команда `get_files`)
- Test: тесты `child_files`/`file_count` уже в Задаче 2; UI — ручная проверка + скриншот

**Interfaces:**
- Consumes: `child_files`/`file_count` (Задача 2), `get_children` (План 1), `FileRow`.
- Produces: `<FolderTree selected onNavigate />`, `<FileTable folderId sort onSort />`; команда:

```rust
#[tauri::command] fn get_files(folder_id: u32, sort: String, desc: bool, offset: usize, limit: usize) -> Result<FilePage, String>;
// model.rs: #[derive(serde::Serialize)] pub struct FilePage { pub rows: Vec<FileRow>, pub total: u32 }
```

- [ ] **Step 1: Реализовать `get_files` (Rust)**

Команда поверх `child_files`/`file_count`; сортировка принимается строкой (`"name"|"size_on_disk"|"size_logical"|"modified"`), неизвестная → `ScanError::Parse`-подобная ошибка на русском. Тест на команду-маппинг не нужен — логика покрыта Задачей 2; ручная проверка через UI.

- [ ] **Step 2: Реализовать `FolderTree`**

Левая панель: дерево папок текущего корня сканирования; раскрытие по клику (дочерние — `get_children` лениво); выделение синхронизировано с картой/таблицей; размеры в ГиБ/МиБ, счётчики. Клик по папке меняет `folderId` карты и таблицы. Виртуализация — TanStack Virtual (узлы дерева). Запросы данных — через TanStack Query (`useQuery` для `get_children`/`get_files` с кэшем по `folderId`): это и есть заявленный в стеке TanStack Query.

- [ ] **Step 3: Реализовать `FileTable`**

Нижняя/правая панель: TanStack Table (колонки: Имя, Размер, На диске, %, Изменён, пометка «жёсткая ссылка») + TanStack Virtual (виртуальный список). Сортировка по клику на заголовок (перезапрос `get_files` с новым sort/desc; на время перезапроса — индикатор «Сортируем…», старый порядок не выдаётся за новый). Пагинация: догрузка следующей страницы при прокрутке к концу (`offset += limit`; страницы по 2 000 строк — быстро для смены сортировки и догрузки; исходные 10 000 признаны замедляющими по замерам Задачи 5). Пустая папка — «Папка пуста». Размеры — ГиБ/МиБ/КиБ с одним знаком, даты — «дд.мм.гггг чч:мм».

- [ ] **Step 4: Ручная проверка + скриншот**

Сборка и запуск (паттерн Плана 1): дерево раскрывается, клик синхронизирует все три панели; папка с 100k+ файлов (`C:\Users` или `C:\Windows`) — таблица прокручивается плавно, догрузка страниц работает, сортировка мгновенная; жёсткая ссылка помечена (найти в `C:\Windows\WinSxS` — там hard links). Скриншот: `.superpowers/tools/plan2-filetable.png`. Замер памяти UI (Диспетчер задач) зафиксировать в отчёте.

- [ ] **Step 5: Commit**

```powershell
git add src docs
git commit -m "feat(ui): дерево папок и таблица файлов на TanStack Table/Virtual"
```

---

### Task 6: Полировка UI и финальная проверка

**Files:**
- Modify: `src/App.tsx`, `src/components/ScanStatus.tsx`, `src/components/Toolbar.tsx`
- Modify: мелочи по необходимости (без изменения движков)
- Test: ручная проверка чек-листа + скриншоты

**Interfaces:**
- Consumes: всё из Задач 1–5.
- Produces: готовый экран Плана 2.

- [ ] **Step 1: Реализовать пункты полировки (все из накопленного списка финального ревью)**

1. Процент отменённого скана не пересчитывать от нового выбранного диска: хранить процент от сканированного объёма (или фиксировать при остановке).
2. При `scan://error` очищать прогресс/итоги — состояние «Ошибка», не «Готово».
3. Кнопка «Обновить список дисков» в Toolbar (перезапрос `list_drives`).
4. Селектор дисков неактивен во время сканирования (или переключение с явной отменой).
5. `formatSize` показывает «0 Б» вместо «0.0 МиБ»; для файлов < 1 МиБ — КиБ.
6. При остановке — фокус/плашка «Данные частичные» уже есть; проверить, что и карта помечена (обрезанные данные визуально отличаются — приглушённая рамка или подпись).

- [ ] **Step 2: Финальный прогон тестов и сборки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` — Expected: все PASS, без предупреждений.
Run: `npm run build` и `npm run tauri build -- --debug --no-bundle` — Expected: exit 0.

- [ ] **Step 3: Финальная ручная проверка (чек-лист Плана 2)**

Запуск exe (с повышением): сканирование C: → карта и топ растут вживую (≥1 обновление/с), папки в счётчике живые; «Остановить» → «Остановлено», «данные частичные», < 1 с; зум по карте, дерево, таблица — всё синхронно; сортировка и пагинация на больших папках; смена диска после отмены не искажает проценты; тёмная тема, русские тексты. Скриншоты итогового экрана: `.superpowers/tools/plan2-final.png`.

- [ ] **Step 4: Commit**

```powershell
git add src
git commit -m "feat(ui): полировка интерфейса карты, дерева и таблицы"
```
