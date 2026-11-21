# WoozyTree — План 1: Сканер (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Работающее приложение WoozyTree с быстрым ядром сканирования: чтение MFT для NTFS-дисков, обход папок для всего остального, индекс файлов в памяти Rust с дедупликацией жёстких ссылок и минимальный тёмный интерфейс, где видно прогресс сканирования и самые тяжёлые папки.

**Architecture:** Tauri 2: Rust сканирует диск в фоновом потоке, строит `ScanIndex` в памяти и отправляет события во фронтенд (события Tauri). Фронтенд — React + минимальные экраны (панель прогресса, таблица топ-папок). Полная treemap-карта, дерево и таблица файлов — в Плане 2.

**Tech Stack:** Rust (edition 2021), Tauri 2, React + TypeScript + Vite, shadcn/ui (пресет `b6ZR0yFgO`, тёмная тема), крейты: `mft`, `windows`, `serde`, `tempfile` (dev), `embed-resource` (build).

**Spec:** [docs/superpowers/specs/2026-11-21-woozytree-design.md](../specs/2026-11-21-woozytree-design.md)

**Roadmap:** План 1 (этот) — сканер. План 2 — treemap-карта, дерево папок, таблица файлов. План 3 — поиск/фильтры, типы файлов, действия с файлами, CSV, настройки.

## Global Constraints

- Только Windows 10/11. Приложение запускается с правами администратора (манифест `requireAdministrator`).
- NTFS-диски сканируются через прямое чтение MFT; не-NTFS, сетевые диски и отдельные папки — обходом папок.
- Скорость (из спецификации): диск ~1 ТБ с 1–2 млн файлов — полное сканирование NTFS за единицы секунд; первые данные на экране — в первую секунду; интерфейс не подвисает.
- Жёсткие ссылки: размер засчитывается один раз — в первую найденную ссылку; остальные — 0 байт в итогах + пометка.
- Тесты, которые создают и удаляют файлы, работают только во временных папках. Никаких операций записи/удаления на реальных данных пользователя.
- Интерфейс: тёмная тема, тексты на русском, компоненты shadcn/ui.
- Код простой и понятный; короткие комментарии на русском там, где логика неочевидна.
- Коммит после каждой задачи. Никакой работы поверх незакоммиченного предыдущего шага.

## Review Focus

1. Запуск без прав администратора или недоступный том: пользователь должен увидеть понятную русскую ошибку и предложение сканировать обходом, а не стек вызовов. → Тест `classify_error_maps_volume_access_to_fallback` (Задача 6).
2. Битые записи MFT и «мёртвые» симлинки не должны ронять сканер: пропуск и подсчёт пропущенного. → Тесты `parse_record_skips_corrupt_record_without_panic` (Задача 5) и `test_walk_tolerates_dangling_junction` (Задача 3).
3. Junction-циклы (папка-ссылка, указывающая на предка) не должны зацикливать обход или задваивать размеры. → Тест `test_walk_does_not_follow_junctions` (Задача 3).
4. Юникод-имена (кириллица, эмодзи) и длинные пути (`\\?\`, больше 260 символов) должны работать. → Тесты `test_walk_handles_unicode_and_long_paths` (Задача 3), `test_parse_record_reads_unicode_name` (Задача 5).
5. Жёсткие ссылки не должны задваивать размеры — итоги должны сходиться с занятым местом Windows. → Тесты `add_file_deduplicates_hard_links` (Задача 2), `test_walk_counts_hard_link_once` (Задача 3).

---

## File Structure

```
WoozyTree/
├─ package.json, vite.config.ts, tsconfig.json, index.html, styles.css   (каркас Vite)
├─ src/                                    — фронтенд (React + TS)
│  ├─ main.tsx, App.tsx                    — точка входа и компоновка экрана
│  ├─ lib/types.ts                         — TS-типы, зеркало Rust DTO
│  ├─ lib/ipc.ts                           — обёртки над Tauri invoke/event
│  └─ components/
│     ├─ ui/…                              — компоненты shadcn/ui
│     ├─ Toolbar.tsx                       — выбор диска + кнопка сканирования
│     ├─ ScanStatus.tsx                    — прогресс и счётчики
│     └─ TopFolders.tsx                    — таблица самых тяжёлых папок
├─ src-tauri/
│  ├─ Cargo.toml, build.rs, app.manifest, tauri.conf.json, capabilities/default.json
│  ├─ src/
│  │  ├─ main.rs, lib.rs                   — сборка Tauri, регистрация команд
│  │  ├─ model.rs                          — общие типы (DTO, ошибки)
│  │  └─ scan/
│  │     ├─ mod.rs                         — ScanService: фоновое сканирование, события
│  │     ├─ index.rs                       — ScanIndex: дерево папок, агрегация
│  │     ├─ walk.rs                        — обход папок (запасной режим)
│  │     ├─ volume.rs                      — сырой доступ к тому, поток записей MFT
│  │     └─ mft.rs                         — разбор записей MFT, быстрое сканирование NTFS
│  ├─ src/bin/dump_mft.rs                  — утилита: снять слепок записей MFT для тестов
│  └─ tests/fixtures/                      — слепки записей MFT (фикстуры для тестов)
└─ docs/superpowers/{specs,plans,spikes}/
```

Файлы, которые меняются вместе, живут вместе: всё про сканирование — в `src-tauri/src/scan/`. Фронтенд минимальный: План 2 добавит `Treemap.tsx`, `FolderTree.tsx`, `FileTable.tsx`.

---

### Task 1: Каркас проекта — инструменты, Tauri, тема, права админа

**Files:**
- Create: корневые файлы проекта (`package.json`, `vite.config.ts`, `tsconfig.json`, `index.html`, `src/main.tsx`, `src/App.tsx`, `src/styles.css`)
- Create: `src-tauri/` (Cargo.toml, build.rs, app.manifest, tauri.conf.json, capabilities/default.json)
- Create: `.gitignore`, `docs/superpowers/spikes/` (пустая папка)

**Interfaces:**
- Consumes: нет (первая задача).
- Produces: запускаемое окно приложения с тёмной темой и запросом прав админа; git-репозиторий; команды `cargo test --manifest-path src-tauri/Cargo.toml` и `npm run tauri dev` работают.

- [ ] **Step 1: Установить Rust и проверить инструменты сборки Windows**

Rust на компьютере не установлен. Выполнить:

```powershell
winget install --id Rustlang.Rustup --source winget
```

Закрыть и открыть терминал, затем:

```powershell
rustup default stable
winget install --id Microsoft.VisualStudio.2022.BuildTools --source winget --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools"
cargo --version
```

Expected: `cargo 1.9x` (или новее). Сборка Tauri требует MSVC (Visual Studio Build Tools с компонентом C++) и WebView2 (в Windows 10/11 обычно уже есть).

- [ ] **Step 2: Инициализировать git и .gitignore**

```powershell
git init
```

`.gitignore`: `node_modules/`, `dist/`, `src-tauri/target/`, `app-tmp/`, `.DS_Store`.

Expected: `git status` показывает репозиторий без `node_modules` и `target`.

- [ ] **Step 3: Создать каркас Tauri 2 + React + TypeScript**

`create-tauri-app` отказывается работать в непустой папке (в репозитории уже есть `docs/`), поэтому создаём во временной папке и переносим:

```powershell
npm create tauri-app@latest app-tmp -t react-ts -m npm -y
```

Перенести содержимое `app-tmp/` в корень репозитория (включая скрытые файлы), удалить `app-tmp/`.

- [ ] **Step 4: Настроить идентичность приложения в `src-tauri/tauri.conf.json`**

Точные значения: `"productName": "WoozyTree"`, `"identifier": "com.woozytree.app"`, окно: `"title": "WoozyTree"`, `"width": 1280`, `"height": 800`, `"minWidth": 900`, `"minHeight": 600`, `"center": true`. В `bundle` ничего не менять (установщик — вне этого плана).

- [ ] **Step 5: Добавить запрос прав администратора (манифест Windows)**

Создать `src-tauri/app.manifest` (точная копия):

```xml
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
```

Создать `src-tauri/build.rs` (точная копия):

```rust
fn main() {
    // Манифест требует запуска от администратора — без этого нельзя читать MFT.
    embed_resource::compile("app.manifest", embed_resource::NONE);
    tauri_build::build()
}
```

В `src-tauri/Cargo.toml` добавить в `[build-dependencies]`: `embed-resource = "3"`.

- [ ] **Step 6: Применить пресет shadcn `b6ZR0yFgO` и включить тёмную тему**

```powershell
npx shadcn@latest init --preset b6ZR0yFgO
```

Если флаг `--preset` отклонён — применить пресет к уже созданному проекту: `npx shadcn@latest apply --preset b6ZR0yFgO`.

Затем в `index.html` на элемент `<html>` добавить класс `dark`, чтобы тёмная тема была всегда (спецификация: тёмная тема из коробки).

- [ ] **Step 7: Проверить сборку и запуск**

```powershell
npm run tauri build -- --debug
```

Expected: сборка проходит; запуск собранного exe показывает запрос UAC (права администратора), затем пустое окно WoozyTree с тёмной темой. Пресет проверяется командой `npx shadcn@latest preset resolve` — Expected: в выводе `code b6ZR0yFgO`.

- [ ] **Step 8: Commit**

```powershell
git add -A
git commit -m "chore: каркас Tauri 2 + React, тёмная тема shadcn (пресет b6ZR0yFgO), запуск от администратора"
```

---

### Task 2: Модель данных и индекс `ScanIndex` (агрегация, жёсткие ссылки)

**Files:**
- Create: `src-tauri/src/model.rs`
- Create: `src-tauri/src/scan/index.rs`
- Modify: `src-tauri/src/lib.rs` (объявить модули)

**Interfaces:**
- Consumes: нет.
- Produces (используются в Задачах 3–6):

```rust
// model.rs
pub type FolderId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey { pub volume_serial: u32, pub file_index: u64 } // уникальный ключ файла на диске

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub name: String,        // имя файла без пути
    pub size_on_disk: u64,   // байт, занятых на диске
    pub size_logical: u64,   // фактический размер
    pub modified: i64,       // время изменения, Unix-мс
    pub file_key: FileKey,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderTotals {
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub file_count: u32,
    pub folder_count: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderSummary {   // DTO для фронтенда
    pub id: FolderId,
    pub name: String,
    pub size_on_disk: u64,
    pub file_count: u32,
    pub folder_count: u32,
    pub percent_of_parent: f32, // доля от размера родителя, 0.0..=1.0
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanProgress { pub files: u64, pub folders: u64, pub size_on_disk: u64, pub elapsed_ms: u64 }

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanTotals { pub files: u64, pub folders: u64, pub size_on_disk: u64, pub size_logical: u64, pub skipped: u32, pub elapsed_ms: u64 }

#[derive(Debug, Clone, serde::Serialize)]
pub struct DriveInfo { pub letter: String, pub label: String, pub fs: String, pub total_bytes: u64, pub free_bytes: u64 }

#[derive(Debug)]
pub enum ScanError { VolumeAccess(String), Io(String), Parse(String) }

// scan/index.rs
pub enum AddOutcome { Counted, HardLinkDuplicate }

pub struct ScanIndex { /* … */ }

impl ScanIndex {
    pub fn new(root_name: &str) -> ScanIndex;
    pub fn root(&self) -> FolderId;
    pub fn folder(&mut self, parent: FolderId, name: &str) -> FolderId; // создать или получить
    pub fn add_file(&mut self, parent: FolderId, meta: FileMeta) -> AddOutcome;
    pub fn folder_totals(&self, id: FolderId) -> FolderTotals;
    pub fn child_folders(&self, id: FolderId) -> Vec<FolderSummary>; // по убыванию size_on_disk
}
```

- [ ] **Step 1: Написать падающие тесты для `ScanIndex`**

Тесты в `src-tauri/src/scan/index.rs` (`#[cfg(test)] mod tests`):

- `add_file_aggregates_folder_totals` — добавить в папку файлы 100 и 50 байт → `folder_totals` вернёт `size_on_disk: 150`, `file_count: 2`.
- `add_file_deduplicates_hard_links` — два `add_file` с одинаковым `FileKey` → второй вернёт `HardLinkDuplicate`, итоги: `size_on_disk` первого файла, `file_count: 2`.
- `child_folders_are_sorted_by_size_desc` — три папки с размерами 10/30/20 → `child_folders` вернёт порядок 30, 20, 10.
- `percent_of_parent_is_fraction_of_parent_size` — папка 25 из родителя 100 → `percent_of_parent == 0.25`; у папки с пустым родителем (размер 0) — `percent_of_parent == 0.0`, не NaN.
- `folder_totals_count_nested_folders` — вложенная структура: итоги корня включают размеры и счётчики всех потомков.

- [ ] **Step 2: Запустить тесты и убедиться, что они падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: FAIL — `ScanIndex` не существует.

- [ ] **Step 3: Реализовать `model.rs` и `ScanIndex`**

Дедупликация жёстких ссылок: внутри `ScanIndex` хранится `HashMap<FileKey, ()>` (или признак у существующей записи файла); повторный `add_file` с тем же `FileKey` не добавляет размер. Счётчики `file_count` считают все ссылки, `size_on_disk` — только первые. Поясняющий комментарий в коде — почему так (спецификация: жёсткие ссылки не задваивают размер).

- [ ] **Step 4: Запустить тесты и убедиться, что они проходят**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: все тесты PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/model.rs src-tauri/src/scan/index.rs src-tauri/src/lib.rs
git commit -m "feat(scanner): модель данных и ScanIndex с дедупликацией жёстких ссылок"
```

---

### Task 3: Обход папок `walk.rs` (запасной режим сканирования)

**Files:**
- Create: `src-tauri/src/scan/walk.rs`
- Modify: `src-tauri/src/scan/mod.rs` (объявить модуль)

**Interfaces:**
- Consumes: `ScanIndex`, `FileMeta`, `FileKey`, `ScanProgress` (Задача 2).
- Produces (Задачи 5–6):

```rust
// scan/walk.rs
#[derive(Debug, Default)]
pub struct WalkStats { pub skipped: u32, pub reparse_skipped: u32 }

pub fn walk(
    root: &Path,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<WalkStats, ScanError>;
```

`FileKey` для обхода: `volume_serial` и `file_index` из `GetFileInformationByHandle` (крейт `windows`).

- [ ] **Step 1: Написать падающие тесты для `walk`**

Тесты в `src-tauri/src/scan/walk.rs`, каждому — своя временная папка (`tempfile::tempdir`), данные создаются в коде теста:

- `test_walk_sums_file_sizes` — файлы 10 и 32 байта → итоги папки: `size_on_disk` ≥ 42, `file_count: 2`.
- `test_walk_handles_unicode_and_long_paths` — файл `файл 🚀 тест.txt` и вложенная цепочка папок с полным путём больше 260 символов → оба файла в индексе с верными размерами; запуск и по пути с префиксом `\\?\`, и без него.
- `test_walk_counts_hard_link_once` — `std::fs::hard_link` создал второе имя файла → итоги по размеру равны одному файлу, второй файл в индексе с пометкой дубликата.
- `test_walk_does_not_follow_junctions` — создать junction (`cmd /c mklink /J`) из подпапки на её предка → `walk` завершается, размеры и счётчики не задвоены, `reparse_skipped ≥ 1`.
- `test_walk_tolerates_dangling_junction` — junction на удалённую папку → `walk` завершается `Ok`, `skipped ≥ 1`.
- `test_walk_stops_on_cancel` — `cancel` выставлен в `progress`-колбэке → `walk` возвращается досрочно, без ошибки.

- [ ] **Step 2: Запустить тесты и убедиться, что они падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: FAIL — `walk` не существует.

- [ ] **Step 3: Реализовать `walk`**

Обход итеративный (явный стек `Vec<PathBuf>`, не рекурсия). Reparse-точки (junction, симлинки) не посещаются: признак — `FILE_ATTRIBUTE_REPARSE_POINT` из метаданных; каждая такая точка учитывается в `reparse_skipped`. Ошибки чтения отдельных записей не прерывают обход: запись в `skipped`, продолжение. Прогресс-колбэк вызывается примерно раз в 1000 файлов. Комментарий в коде: почему не заходим в reparse-точки (циклы и двойной счёт).

- [ ] **Step 4: Запустить тесты и убедиться, что они проходят**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: все тесты PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/scan/walk.rs src-tauri/src/scan/mod.rs
git commit -m "feat(scanner): обход папок с обходом junction-циклов и дедупликацией жёстких ссылок"
```

---

### Task 4: Спайк чтения MFT + утилита `dump_mft` + `volume.rs`

**Files:**
- Create: `src-tauri/src/bin/dump_mft.rs`
- Create: `src-tauri/src/scan/volume.rs`
- Create: `src-tauri/tests/fixtures/sample.mft`, `src-tauri/tests/fixtures/corrupt.mft`, `src-tauri/tests/fixtures/boot.bin`
- Create: `docs/superpowers/spikes/2026-11-21-mft-reading.md`

**Interfaces:**
- Consumes: `ScanError` (Задача 2).
- Produces (Задача 5):

```rust
// scan/volume.rs
pub struct VolumeReader { /* … */ }

impl VolumeReader {
    pub fn open(drive_letter: &str) -> Result<VolumeReader, ScanError>; // "C" -> открытый том \\.\C:
    pub fn from_fixture(path: &Path, record_size: usize) -> Result<VolumeReader, ScanError>; // для тестов: записи читаются из файла-слепка
    pub fn record_size(&self) -> usize;                                  // размер записи MFT, обычно 1024
    pub fn read_records(&self, sink: &mut dyn FnMut(&[u8])) -> Result<u64, ScanError>; // поток «сырых» записей; возвращает их количество
}
```

- [ ] **Step 1: Написать спайк — замер двух способов чтения MFT**

Пробная (throwaway) проверка прямо в коде `dump_mft` / временном примере: (а) прямое чтение данных $MFT (загрузочный сектор → размещение $MFT → последовательное чтение), (б) перебор записей через `FSCTL_GET_NTFS_FILE_RECORD`. Замерить на `C:`: время полного чтения всех записей обоими способами.

Записать выводы в `docs/superpowers/spikes/2026-11-21-mft-reading.md`: выбранный способ, тайминги, число записей на `C:`, грабли (если были). Решение по выбору: остаётся способ, укладывающийся в Global Constraints (единицы секунд на диск ~1 ТБ); если оба укладываются — более простой.

- [ ] **Step 2: Написать падающие тесты разбора загрузочного сектора и runlist**

Тесты в `src-tauri/src/scan/volume.rs`:

- `parse_boot_sector_reads_mft_params` — 512 байт из `tests/fixtures/boot.bin` → размер записи MFT (1024), размещение $MFT.
- `parse_runlist_decodes_extents` — синтетический runlist (закодированные вручную 2–3 прогона) → вектор `(lcn, length)` как в эталоне.
- `from_fixture_reads_dumped_records` — `VolumeReader::from_fixture("tests/fixtures/sample.mft", 1024)` → `read_records` выдаёт 2000 записей по 1024 байта.

- [ ] **Step 3: Запустить тесты и убедиться, что они падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: FAIL.

- [ ] **Step 4: Реализовать `volume.rs` и утилиту `dump_mft`**

`dump_mft` — консольная утилита того же крейта (`src-tauri/src/bin/dump_mft.rs`): `dump_mft <буква_тома> <выходной_файл> [--records N] [--corrupt]`. Пишет последовательность сырых записей MFT в файл; `--corrupt` портит байты последней записи (фикстура `corrupt.mft`). Через утилиту получить фикстуры: `sample.mft` (с `C:`, например `--records 2000`), `corrupt.mft` (та же выборка с `--corrupt`), `boot.bin` (первые 512 байт тома — утилита сохраняет их по флагу `--boot`, добавить этот флаг).

Ошибки доступа к тому (`ScanError::VolumeAccess`) — с понятным сообщением: том не удалось открыть (нет прав / нет тома).

- [ ] **Step 5: Запустить тесты, проверить утилиту на реальном диске**

Run: `cargo test --manifest-path src-tauri/Cargo.toml` — Expected: PASS.
Run (от администратора): `cargo run --manifest-path src-tauri/Cargo.toml --bin dump_mft -- C: src-tauri\tests\fixtures\sample.mft --records 2000 --boot` — Expected: файл `sample.mft` размером `2000 * record_size` байт и `boot.bin` (512 байт); затем `cargo run --manifest-path src-tauri/Cargo.toml --bin dump_mft -- C: src-tauri\tests\fixtures\corrupt.mft --records 2000 --corrupt` — Expected: `corrupt.mft` готов; тайминг полного чтения записан в спайк-заметки.

- [ ] **Step 6: Commit**

```powershell
git add src-tauri/src/scan/volume.rs src-tauri/src/bin/dump_mft.rs src-tauri/tests/fixtures docs/superpowers/spikes
git commit -m "feat(scanner): сырое чтение MFT, утилита dump_mft и фикстуры; спайк-заметки"
```

---

### Task 5: Разбор записей MFT и быстрое сканирование NTFS (`mft.rs`)

**Files:**
- Create: `src-tauri/src/scan/mft.rs`
- Modify: `src-tauri/src/scan/mod.rs`

**Interfaces:**
- Consumes: `VolumeReader` (Задача 4), `ScanIndex`, `FileMeta`, `FileKey` (Задача 2).
- Produces (Задача 6):

```rust
// scan/mft.rs
#[derive(Debug)]
pub struct MftFileInfo {
    pub name: String,
    pub parent_ref: u64,   // номер записи-родителя в MFT (младшие 48 бит ссылки)
    pub size_on_disk: u64,
    pub size_logical: u64,
    pub modified: i64,
    pub file_key: FileKey, // file_index = номер записи MFT
    pub is_directory: bool,
}

pub fn parse_record(record: &[u8]) -> Vec<MftFileInfo>; // по элементу на каждый $FILE_NAME записи (жёсткие ссылки дают несколько элементов с одним file_key); пусто — мусорная/неполная запись

pub fn scan_ntfs_reader(
    reader: &VolumeReader,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanTotals, ScanError>;

pub fn scan_ntfs(
    drive_letter: &str,
    index: &mut ScanIndex,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<ScanTotals, ScanError>;
```

`scan_ntfs` — тонкая обёртка над `scan_ntfs_reader`: открывает том через `VolumeReader::open` и делегирует (так тесты гоняют `scan_ntfs_reader` на фикстурах без реального диска). Парсинг записей — крейт `mft` (если спайк показал, что он тянет нужные атрибуты: имена, размеры, родители) — иначе ручной разбор атрибутов `$FILE_NAME` и `$DATA`.

- [ ] **Step 1: Написать падающие тесты разбора записей**

Тесты в `src-tauri/src/scan/mft.rs` (читают фикстуры из `tests/fixtures/`):

- `test_parse_record_reads_unicode_name` — синтетическая запись (тестовый помощник `make_record`, собирающий байты записи MFT) с именем `файл 🚀 тест.txt` → `parse_record` вернёт один элемент с этим именем и корректными размерами.
- `test_parse_record_returns_all_hard_link_names` — синтетическая запись с двумя атрибутами `$FILE_NAME` (жёсткие ссылки) → `parse_record` вернёт 2 элемента с одним `file_key`, но разными именами и `parent_ref`.
- `parse_record_skips_corrupt_record_without_panic` — последняя запись `corrupt.mft` → `parse_record` вернёт пустой `Vec`, остальные записи парсятся без паники.
- `test_scan_ntfs_builds_index_with_valid_parents` — `scan_ntfs_reader` с `VolumeReader::from_fixture("tests/fixtures/sample.mft", 1024)` → каждый файл в индексе имеет родителя-папку; счётчики ненулевые.
- `test_scan_ntfs_counts_hard_links_once` — синтетическая запись с двумя `$FILE_NAME`, записанная во временный файл-слепок → `scan_ntfs_reader` засчитал размер один раз, обе ссылки видны.

- [ ] **Step 2: Запустить тесты и убедиться, что они падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: FAIL.

- [ ] **Step 3: Реализовать `parse_record` и `scan_ntfs`**

Алгоритм `scan_ntfs` (определяет не подпись, а порядок): прочитать все записи через `VolumeReader` → собрать `HashMap<u64 /*parent_ref*/, …>` → второй проход: создавать папки (`index.folder`) и файлы (`index.add_file`) в порядке, где родитель встречается раньше детей (корень — запись MFT №5); записи с неразрешимым родителем — в корень с увеличением `ScanTotals.skipped`. Отмена (`cancel`) проверяется между порциями чтения.

- [ ] **Step 4: Запустить тесты и убедиться, что они проходят**

Run: `cargo test --manifest-path src-tauri/Cargo.toml` — Expected: все PASS.
Run (от администратора) — замер скорости на реальном диске: временный `main`/тестовый бинарь сканирует `C:` через `scan_ntfs` и печатает `ScanTotals` с таймингом. Expected по Global Constraints: единицы секунд; итог `size_on_disk` близок к занятому месту `C:` (у `fsutil volume diskfree C:` — в пределах ~5% с учётом служебных файлов). Результат — в спайк-заметки.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/scan/mft.rs src-tauri/src/scan/mod.rs docs/superpowers/spikes
git commit -m "feat(scanner): разбор записей MFT и быстрое сканирование NTFS"
```

---

### Task 6: `ScanService`, команды Tauri и минимальный интерфейс

**Files:**
- Create: `src-tauri/src/scan/mod.rs` (дописать: `ScanService`, `ScanSink`, `run_scan`, `classify_error`)
- Modify: `src-tauri/src/lib.rs` (команды Tauri)
- Create: `src/lib/types.ts`, `src/lib/ipc.ts`
- Create: `src/components/Toolbar.tsx`, `src/components/ScanStatus.tsx`, `src/components/TopFolders.tsx`
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes: `walk`, `scan_ntfs`, `ScanIndex` (Задачи 2–5).
- Produces (План 2): события Tauri `scan://progress` (`ScanProgress`), `scan://top` (`Vec<FolderSummary>`, раз в секунду), `scan://done` (`ScanTotals`), `scan://error` (строка — русское сообщение об ошибке); команды:

```rust
#[tauri::command] fn list_drives() -> Vec<DriveInfo>;
#[tauri::command] fn start_scan(app: AppHandle, target: ScanTarget) -> Result<(), String>;
#[tauri::command] fn cancel_scan();
#[tauri::command] fn get_children(folder_id: u32) -> Result<Vec<FolderSummary>, String>;
#[tauri::command] fn get_scan_totals() -> Result<ScanTotals, String>;

pub enum ScanTarget { Drive(String), Folder(PathBuf) } // serde: {"drive": "C"} | {"folder": "C:\\Temp"}
pub trait ScanSink { fn progress(&mut self, p: &ScanProgress); fn top_folders(&mut self, rows: &[FolderSummary]); fn done(&mut self, r: Result<&ScanTotals, &ScanError>); }
pub fn run_scan(target: &ScanTarget, index: &mut ScanIndex, sink: &mut dyn ScanSink, cancel: &AtomicBool) -> Result<ScanTotals, ScanError>;
pub fn classify_error(err: &ScanError) -> FallbackAction;
pub enum FallbackAction { ShowFatal, OfferWalkFallback }
```

Выбор движка: NTFS → `scan_ntfs`, иначе (или для `Folder`) → `walk`. `classify_error`: `ScanError::VolumeAccess` → `OfferWalkFallback`, остальное → `ShowFatal`.

- [ ] **Step 1: Написать падающие тесты для `run_scan` и `classify_error`**

Тесты в `src-tauri/src/scan/mod.rs`:

- `run_scan_on_folder_walks_and_reports_progress` — `run_scan` на временной папке с 3 файлами, `sink` пишет в `Vec` → есть `progress`, итоговый `ScanTotals` с `files: 3`.
- `run_scan_drive_ntfs_vs_folder` — `ScanTarget::Folder` всегда идёт через `walk` (на тестовой папке результат тот же, что у прямого `walk`).
- `classify_error_maps_volume_access_to_fallback` — `ScanError::VolumeAccess` → `OfferWalkFallback`; `ScanError::Io` → `ShowFatal`.
- `cancel_scan_stops_run` — `cancel` до старта → возврат без ошибки, `ScanTotals.skipped == 0`.

- [ ] **Step 2: Запустить тесты и убедиться, что они падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: FAIL.

- [ ] **Step 3: Реализовать `run_scan`, `classify_error` и команды Tauri**

`start_scan` запускает фоновый поток (не блокирует UI): создаёт `ScanIndex`, зовёт `run_scan`, события отправляет через `app.emit` (обёртка `TauriSink` над `ScanSink`). Глобальное состояние — `Mutex<Option<Arc<AtomicBool>>>` для отмены и `Mutex<Option<Arc<ScanIndex>>>` для `get_children`. Событие `scan://top` — раз в секунду: топ-20 дочерних папок корня. Ошибки уходят событием `scan://error` с текстом на русском.

- [ ] **Step 4: Реализовать минимальный интерфейс**

- `src/lib/types.ts` — TS-типы, зеркало `FolderSummary`, `ScanProgress`, `ScanTotals`, `DriveInfo`, `ScanTarget`.
- `src/lib/ipc.ts` — функции `listDrives()`, `startScan(target)`, `cancelScan()`, `getChildren(id)`, `onProgress(cb)`, `onTop(cb)`, `onDone(cb)`, `onError(cb)` поверх `invoke`/`listen` из `@tauri-apps/api`.
- `Toolbar.tsx` — select с дисками (из `list_drives`) + кнопки «Сканировать» / «Остановить» (shadcn `button`, `select`).
- `ScanStatus.tsx` — прогресс-бар (shadcn `progress`) + строка «Файлов: N, папок: N, размер: X ГБ, время: N с».
- `TopFolders.tsx` — таблица (shadcn `table`): имя, размер на диске, файлов, папок, % от родителя. Обновляется по событию `scan://top`.
- `App.tsx` — компоновка: `Toolbar`, `ScanStatus`, `TopFolders`; тёмная тема.

Компоненты shadcn ставятся по одному: `npx shadcn@latest add button select progress table`.

- [ ] **Step 5: Ручная проверка целиком**

Run: `npm run tauri dev` (от администратора). Expected: UAC → окно; выбор `C:` → «Сканировать» → счётчики растут, топ-папки заполняются в первую секунду; сканирование `C:` завершается за единицы секунд; «Остановить» прерывает; окно не подвисает; интерфейс тёмный, тексты на русском.

- [ ] **Step 6: Commit**

```powershell
git add -A
git commit -m "feat(scanner): ScanService, команды Tauri и минимальный интерфейс сканирования"
```
