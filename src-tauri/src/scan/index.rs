//! Индекс сканирования: дерево папок с агрегированными итогами и плоский список
//! файлов — основа таблицы файлов и живой карты.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::model::{
    FileKey, FileMeta, FileRow, FileSort, FileTypeRow, FolderId, FolderSummary, FolderTotals,
    SearchQuery, SearchRow, TreemapRect, TypeSort,
};
use crate::treemap::layout::{Rect, TreemapItem, layout_treemap};

/// Результат добавления файла в индекс.
pub enum AddOutcome {
    Counted,
    HardLinkDuplicate,
}

/// Один файл (одно имя на диске) в плоском списке индекса.
///
/// Запись занимает 56 байт: на C: таких больше миллиона, поэтому лишние байты
/// на запись — это десятки мегабайт. Имя — единственная аллокация на файл.
/// Порядок и размеры папки здесь не дублируются: к записи всегда приходят через
/// `Node::files`, а итоги живут в узлах дерева.
struct FileRecord {
    name: String,
    size_on_disk: u64,
    size_logical: u64,
    modified: i64,
    /// Папка, в которой лежит имя; нужна для проверки целостности плоского
    /// списка и списков папок (в отладочных сборках).
    parent: FolderId,
    hard_linked: bool,
}

struct Node {
    name: String,
    parent: Option<FolderId>,
    totals: FolderTotals,
    children: Vec<FolderId>,
    /// Индексы файлов этой папки в `ScanIndex::files` в порядке вставки.
    files: Vec<u32>,
}

fn zero_totals() -> FolderTotals {
    FolderTotals {
        size_on_disk: 0,
        size_logical: 0,
        file_count: 0,
        folder_count: 0,
    }
}

/// Нормализовать реальный путь корня сканирования.
///
/// `\\?\C:\Temp` → `C:\Temp`: verbatim-префикс понимает Win32, но не проводник
/// и не пользователь, которому путь показывают. Хвостовой слэш у папки лишний
/// (склейка с именем добавляет разделитель сама), а у корня диска («C:\») он —
/// часть пути, поэтому короткие пути не трогаем.
fn normalize_root_path(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if let Some(rest) = text.strip_prefix("\\\\?\\") {
        // «\\?\UNC\server\share» — это сетевой путь «\\server\share».
        text = match rest.strip_prefix("UNC\\") {
            Some(unc) => format!("\\\\{unc}"),
            None => rest.to_string(),
        };
    }
    if text.len() > 3 {
        while text.ends_with('\\') || text.ends_with('/') {
            text.pop();
        }
    }
    text
}

/// Имя подпапки тома в корне «Все диски»: «C:» (буква и двоеточие).
fn is_drive_segment(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Сравнение имён для сортировки — по-русски, как в проводнике: регистр не
/// различается (включая кириллицу), «ё» стоит наравне с «е». Порядок символов —
/// по коду Unicode: латиница раньше кириллицы, а после снятия регистра строчные
/// «а»…«я» идут по алфавиту. Различие только в регистре сравнение не решает:
/// «apple.txt» раньше «Apricot.txt», «a» раньше «AB», «readme.md» раньше
/// «RELEASE.txt».
///
/// ASCII-имена (типичный случай) сравниваются побайтово, без аллокаций: пока
/// различие приходится на ASCII-байты, ответ берётся из них; байты, различающиеся
/// только регистром, пропускаются (`continue`), а не решают сравнение. На
/// не-ASCII (кириллица) регистр снимается посимвольно через `char::to_lowercase`
/// — тоже без аллокаций.
///
/// Tie-break по исходным байтам срабатывает только тогда, когда имена совпали
/// целиком без учёта регистра (и с «ё» ≈ «е»): только в этом случае порядок иначе
/// не определён («A.txt» раньше «a.txt», «еж.txt» раньше «ёж.txt»).
///
/// Это не полное locale-сравнение: знаки препинания и пробелы не игнорируются —
/// для имён файлов, где имена различаются буквами, этого достаточно.
fn compare_names(left: &str, right: &str) -> Ordering {
    compare_names_folded(left, right).then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

/// Первичное сравнение имён: регистр и «ё»/«е» не различаются.
fn compare_names_folded(left: &str, right: &str) -> Ordering {
    let (left_bytes, right_bytes) = (left.as_bytes(), right.as_bytes());
    let shared = left_bytes.len().min(right_bytes.len());
    for index in 0..shared {
        let (a, b) = (left_bytes[index], right_bytes[index]);
        if a == b {
            continue;
        }
        if a.is_ascii() && b.is_ascii() {
            match a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()) {
                // Различие только в регистре сравнение не решает: иначе имена
                // сортировались бы по регистру («AB» раньше «a», «Apricot.txt»
                // раньше «apple.txt»), а не по алфавиту.
                Ordering::Equal => continue,
                order => return order,
            }
        }
        // Не-ASCII: байтовый порядок в UTF-8 не совпадает с алфавитным (заглавная
        // «З» = 0xD0 0x97 идёт раньше строчной «а» = 0xD0 0xB0), поэтому дальше
        // сравниваем символы.
        return compare_lowered_chars(left, right);
    }
    // Байтовый префикс совпал — короткое имя раньше. Для UTF-8 это совпадает с
    // посимвольным сравнением: префикс байт = префикс символов.
    left_bytes.len().cmp(&right_bytes.len())
}

/// Посимвольное сравнение: регистр снят, «ё» приведена к «е».
fn compare_lowered_chars(left: &str, right: &str) -> Ordering {
    let mut left_chars = lowered_chars(left);
    let mut right_chars = lowered_chars(right);
    loop {
        match (left_chars.next(), right_chars.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (Some(a), Some(b)) => return a.cmp(&b),
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

/// Символы имени в нижнем регистре, «ё» приведена к «е» (в русской сортировке
/// они на одном месте); аллокаций нет: `char::to_lowercase` — итератор.
fn lowered_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
}

/// Подстрока без учёта регистра по правилам сортировки: ASCII, кириллица и
/// «ё» ≈ «е». Пустой образец совпадает с любым именем («всё подряд»).
///
/// Функция чистая и годится для разовых проверок; поиск по миллиону файлов
/// строит [`NameNeedle`] один раз на запрос, а не на каждое имя.
pub fn name_contains(haystack: &str, needle: &str) -> bool {
    NameNeedle::new(needle).matches(haystack)
}

/// Предвычисленный образец поиска: нормализованные символы (нижний регистр,
/// «ё» → «е») и таблица префикс-функции KMP.
///
/// Нормализация — та же, что у сортировки ([`lowered_chars`]), поэтому поиск и
/// порядок имён не расходятся. Считать образец один раз важно: вызов на
/// 1.26 млн файлов не должен аллоцировать вектор символов на каждое имя.
struct NameNeedle {
    /// Образец в нижнем регистре, «ё» → «е».
    chars: Vec<char>,
    /// Префикс-функция KMP: без неё частичное совпадение пришлось бы откатывать
    /// перебором и можно было бы пропустить наложение («aab» в «aaab»).
    prefix: Vec<usize>,
    /// Образец целиком ASCII — значит, в ASCII-имени достаточно байтового поиска.
    ascii: bool,
}

impl NameNeedle {
    fn new(text: &str) -> NameNeedle {
        let chars: Vec<char> = lowered_chars(text).collect();
        let prefix = prefix_table(&chars);
        let ascii = chars.iter().all(|c| c.is_ascii());
        NameNeedle {
            chars,
            prefix,
            ascii,
        }
    }

    /// Совпадает ли имя с образцом (пустой образец — совпадает всегда).
    fn matches(&self, haystack: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        // ASCII-имя (типичный случай на диске) — байтовый поиск без итераторов
        // по символам и без аллокаций.
        if haystack.is_ascii() {
            // Не-ASCII образец не может совпасть с ASCII-именем: снятие регистра
            // ASCII-символа остаётся ASCII.
            if !self.ascii {
                return false;
            }
            return ascii_contains(haystack.as_bytes(), &self.chars);
        }
        contains_folded(haystack, &self.chars, &self.prefix)
    }
}

/// Таблица префикс-функции KMP для нормализованного образца.
fn prefix_table(needle: &[char]) -> Vec<usize> {
    let mut prefix = vec![0usize; needle.len()];
    let mut matched = 0usize;
    for index in 1..needle.len() {
        while matched > 0 && needle[index] != needle[matched] {
            matched = prefix[matched - 1];
        }
        if needle[index] == needle[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    prefix
}

/// Поиск ASCII-образца в ASCII-имени: регистр снимается по байтам.
fn ascii_contains(haystack: &[u8], needle: &[char]) -> bool {
    let length = needle.len();
    if length > haystack.len() {
        return false;
    }
    let first = needle[0] as u8;
    for start in 0..=haystack.len() - length {
        if haystack[start].to_ascii_lowercase() != first {
            continue;
        }
        let candidate = &haystack[start..start + length];
        if candidate
            .iter()
            .zip(needle)
            .all(|(byte, expected)| byte.to_ascii_lowercase() == *expected as u8)
        {
            return true;
        }
    }
    false
}

/// Поиск по не-ASCII имени: KMP по нормализованным символам.
fn contains_folded(haystack: &str, needle: &[char], prefix: &[usize]) -> bool {
    let mut matched = 0usize;
    for c in lowered_chars(haystack) {
        while matched > 0 && needle[matched] != c {
            matched = prefix[matched - 1];
        }
        if needle[matched] == c {
            matched += 1;
            if matched == needle.len() {
                return true;
            }
        }
    }
    false
}

/// Порядок с учётом направления сортировки.
fn ordered(order: Ordering, desc: bool) -> Ordering {
    if desc { order.reverse() } else { order }
}

/// Порядок строк таблицы файлов.
///
/// `desc` разворачивает только основную колонку; равные значения всегда идут по
/// возрастанию имени, чтобы порядок не зависел от направления сортировки.
fn compare_files(left: &FileRecord, right: &FileRecord, sort: FileSort, desc: bool) -> Ordering {
    let primary = match sort {
        // Имя — сама колонка сортировки, поэтому desc разворачивает и его.
        FileSort::Name => return ordered(compare_names(&left.name, &right.name), desc),
        FileSort::SizeOnDisk => left.size_on_disk.cmp(&right.size_on_disk),
        FileSort::SizeLogical => left.size_logical.cmp(&right.size_logical),
        FileSort::Modified => left.modified.cmp(&right.modified),
    };
    ordered(primary, desc).then_with(|| compare_names(&left.name, &right.name))
}

/// Расширение файла без точки в нижнем регистре; `""` — расширения нет.
///
/// Расширение — текст после последней точки. Точка в начале имени (`.gitignore`)
/// расширением не считается: это скрытый файл без расширения, как в проводнике;
/// точка в конце (`file.`) тоже даёт пустое расширение. Регистр снимается
/// `to_lowercase`, чтобы `.TXT` и `.txt` попали в одну группу.
fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Имя группы без расширения в таблице и на карте типов.
const NO_EXTENSION_NAME: &str = "(нет)";

/// Суммы по одной группе расширений: счётчик и размеры.
#[derive(Default)]
struct TypeAgg {
    count: u32,
    size_on_disk: u64,
    size_logical: u64,
}

/// Порядок строк таблицы типов.
///
/// `desc` разворачивает только основную колонку; равные значения всегда идут по
/// возрастанию имени — так порядок групп не зависит от направления сортировки.
fn compare_type_rows(
    left: &FileTypeRow,
    right: &FileTypeRow,
    sort: TypeSort,
    desc: bool,
) -> Ordering {
    let primary = match sort {
        TypeSort::Name => return ordered(compare_names(&left.extension, &right.extension), desc),
        TypeSort::Count => left.count.cmp(&right.count),
        TypeSort::SizeOnDisk => left.size_on_disk.cmp(&right.size_on_disk),
        TypeSort::SizeLogical => left.size_logical.cmp(&right.size_logical),
    };
    ordered(primary, desc).then_with(|| compare_names(&left.extension, &right.extension))
}

/// Порядок результатов поиска: крупное сверху, при равных размерах — по имени
/// (правила проводника), затем по позиции в индексе. Позиция последним ключом
/// делает порядок полным, поэтому страницы не зависят от стабильности сортировки.
fn compare_search_positions(files: &[FileRecord], left: u32, right: u32) -> Ordering {
    let left_file = &files[left as usize];
    let right_file = &files[right as usize];
    right_file
        .size_on_disk
        .cmp(&left_file.size_on_disk)
        .then_with(|| compare_names(&left_file.name, &right_file.name))
        .then_with(|| left.cmp(&right))
}

pub struct ScanIndex {
    nodes: Vec<Node>,
    // (родитель, имя) -> id, чтобы folder() не создавал дубликат папки
    folders_by_name: HashMap<(FolderId, String), FolderId>,
    // Ключи уже учтённых файлов: по ним распознаются жёсткие ссылки.
    seen_files: HashMap<FileKey, ()>,
    // Плоский список файлов: одно имя на диске — одна запись.
    files: Vec<FileRecord>,
    // Реальный путь корня сканирования: из него строятся пути действий с
    // файлами (удаление, открытие, проводник). Пустой у «Все диски».
    root_real_path: String,
}

impl ScanIndex {
    pub fn new(root_name: &str) -> ScanIndex {
        let root = Node {
            name: root_name.to_string(),
            parent: None,
            totals: zero_totals(),
            children: Vec::new(),
            files: Vec::new(),
        };
        ScanIndex {
            nodes: vec![root],
            folders_by_name: HashMap::new(),
            seen_files: HashMap::new(),
            files: Vec::new(),
            root_real_path: String::new(),
        }
    }

    pub fn root(&self) -> FolderId {
        0
    }

    /// Цепочка от корня индекса до папки включительно: (id, имя).
    ///
    /// Нужна для перехода из поиска: клик по строке результата получает путь
    /// папки и открывает её на карте (`get_path_crumbs`). Узлы индекса помнят
    /// родителя, поэтому цепочка собирается подъёмом до корня.
    pub fn ancestors(&self, folder: FolderId) -> Vec<(FolderId, String)> {
        let mut chain = Vec::new();
        let mut cursor = Some(folder);
        while let Some(id) = cursor {
            let node = &self.nodes[id as usize];
            chain.push((id, node.name.clone()));
            cursor = node.parent;
        }
        chain.reverse();
        chain
    }

    /// Путь папки от корня индекса, разделитель `\`: «C:\Users\georg».
    ///
    /// Корень диска уже кончается слэшем («C:\»), поэтому второй разделитель
    /// перед первым именем не ставится. У корня-папки в индексе хранится только
    /// имя, так что путь относительный — от корня сканирования.
    pub fn path_of(&self, folder: FolderId) -> String {
        let mut path = String::new();
        for (index, (_, name)) in self.ancestors(folder).into_iter().enumerate() {
            if index > 0 && !path.ends_with('\\') {
                path.push('\\');
            }
            path.push_str(&name);
        }
        path
    }

    /// Реальный путь корня сканирования — база для путей действий с файлами.
    ///
    /// Вызывающий сервис кладёт сюда «C:\» для диска, путь выбранной папки для
    /// папки и пустую строку для «Все диски» (там база — подпапка тома «X:»).
    pub fn set_root_real_path(&mut self, s: String) {
        self.root_real_path = normalize_root_path(&s);
    }

    /// Реальный (абсолютный) путь папки или файла в ней — для удаления,
    /// открытия и проводника.
    ///
    /// `path_of` даёт путь от корня индекса («C:\Users» или «Users» у папки) —
    /// для действий он не годится: индекс хранит только имена. Здесь к пути
    /// корня добавляется относительный подпуть, а у «Все диски» базой становится
    /// подпапка тома: «C:» → «C:\», дальше обычная склейка. Имя файла
    /// добавляется последним звеном; путь всегда без `\\?\` — так его понимают
    /// проводник и `ShellExecuteW`.
    pub fn real_path_of(&self, folder: FolderId, file_name: Option<&str>) -> String {
        let mut base = self.root_real_path.clone();
        // Корень индекса уже учтён в root_real_path: его имя — не звено пути.
        for (_, name) in self.ancestors(folder).into_iter().skip(1) {
            if base.is_empty() {
                // «Все диски»: первый сегмент — том, база «C:\».
                base = if is_drive_segment(&name) {
                    format!("{name}\\")
                } else {
                    name
                };
            } else {
                if !base.ends_with('\\') {
                    base.push('\\');
                }
                base.push_str(&name);
            }
        }
        let Some(name) = file_name else {
            return base;
        };
        if base.is_empty() {
            return name.to_string();
        }
        if !base.ends_with('\\') {
            base.push('\\');
        }
        base.push_str(name);
        base
    }

    /// Создать папку или вернуть уже существующую с тем же именем у того же родителя.
    pub fn folder(&mut self, parent: FolderId, name: &str) -> FolderId {
        let key = (parent, name.to_string());
        if let Some(&id) = self.folders_by_name.get(&key) {
            return id;
        }

        let id = self.nodes.len() as FolderId;
        self.nodes.push(Node {
            name: name.to_string(),
            parent: Some(parent),
            totals: zero_totals(),
            children: Vec::new(),
            files: Vec::new(),
        });
        self.folders_by_name.insert(key, id);
        self.nodes[parent as usize].children.push(id);

        // Новая папка — счётчик папок растёт у всех предков.
        let mut cursor = Some(parent);
        while let Some(current) = cursor {
            self.nodes[current as usize].totals.folder_count += 1;
            cursor = self.nodes[current as usize].parent;
        }

        id
    }

    pub fn add_file(&mut self, parent: FolderId, meta: FileMeta) -> AddOutcome {
        // Жёсткие ссылки не задваивают размер: байты засчитываются только первой
        // ссылке на файл, а счётчик файлов растёт у каждой (это разные имена на диске).
        let FileMeta {
            name,
            size_on_disk,
            size_logical,
            modified,
            file_key,
        } = meta;
        let outcome = if self.seen_files.insert(file_key, ()).is_some() {
            AddOutcome::HardLinkDuplicate
        } else {
            AddOutcome::Counted
        };
        let counted = matches!(outcome, AddOutcome::Counted);

        // Запись помним всегда: повторная ссылка видна в таблице с фактическим
        // размером и пометкой `hard_linked`, но в итоги папки не входит.
        let position = self.files.len() as u32;
        self.files.push(FileRecord {
            name,
            size_on_disk,
            size_logical,
            modified,
            parent,
            hard_linked: !counted,
        });
        self.nodes[parent as usize].files.push(position);

        let mut cursor = Some(parent);
        while let Some(current) = cursor {
            let totals = &mut self.nodes[current as usize].totals;
            totals.file_count += 1;
            if counted {
                totals.size_on_disk += size_on_disk;
                totals.size_logical += size_logical;
            }
            cursor = self.nodes[current as usize].parent;
        }

        outcome
    }

    pub fn folder_totals(&self, id: FolderId) -> FolderTotals {
        self.nodes[id as usize].totals.clone()
    }

    /// Число файлов-ссылок непосредственно в папке — для пагинации таблицы.
    pub fn file_count(&self, folder: FolderId) -> u32 {
        self.nodes[folder as usize].files.len() as u32
    }

    /// Страница файлов папки: сортировка при выдаче, затем срез `offset..offset+limit`.
    ///
    /// Сортируем весь список папки на каждый запрос: папка одного уровня, а её
    /// список — плоские индексы по 4 байта (100 тыс. файлов — 400 КБ копии).
    /// Кэш отсортированных порядков не держим, чтобы не платить памятью за все
    /// папки сразу и не ломать инварианты при живом сканировании.
    pub fn child_files(
        &self,
        folder: FolderId,
        sort: FileSort,
        desc: bool,
        offset: usize,
        limit: usize,
    ) -> Vec<FileRow> {
        if limit == 0 {
            return Vec::new();
        }
        let node = &self.nodes[folder as usize];
        if offset >= node.files.len() {
            return Vec::new();
        }

        let mut order = node.files.clone();
        order.sort_by(|&left, &right| {
            let left = &self.files[left as usize];
            let right = &self.files[right as usize];
            compare_files(left, right, sort, desc)
        });

        let end = offset.saturating_add(limit).min(order.len());
        let parent_size = node.totals.size_on_disk;
        order[offset..end]
            .iter()
            .map(|&position| {
                let file = &self.files[position as usize];
                // Плоский список и список папки обязаны совпадать: расхождение
                // означало бы, что страница показывает чужой файл.
                debug_assert_eq!(file.parent, folder);
                // У дубликата жёсткой ссылки, чья первая копия лежит в другой
                // папке, размер не входит в итог этой папки — отношение могло бы
                // превысить 1.0. Контракт DTO — 0.0..=1.0, его и держим.
                let percent_of_parent = if parent_size == 0 {
                    0.0
                } else {
                    (file.size_on_disk as f32 / parent_size as f32).min(1.0)
                };
                FileRow {
                    name: file.name.clone(),
                    size_on_disk: file.size_on_disk,
                    size_logical: file.size_logical,
                    modified: file.modified,
                    percent_of_parent,
                    hard_linked: file.hard_linked,
                }
            })
            .collect()
    }

    /// Дочерние папки для фронтенда, по убыванию занятого места.
    pub fn child_folders(&self, id: FolderId) -> Vec<FolderSummary> {
        let node = &self.nodes[id as usize];
        let parent_size = node.totals.size_on_disk;

        let mut children: Vec<FolderSummary> = node
            .children
            .iter()
            .map(|&child| {
                let child_node = &self.nodes[child as usize];
                let percent_of_parent = if parent_size == 0 {
                    0.0
                } else {
                    child_node.totals.size_on_disk as f32 / parent_size as f32
                };
                FolderSummary {
                    id: child,
                    name: child_node.name.clone(),
                    size_on_disk: child_node.totals.size_on_disk,
                    file_count: child_node.totals.file_count,
                    folder_count: child_node.totals.folder_count,
                    percent_of_parent,
                }
            })
            .collect();

        children.sort_by(|a, b| b.size_on_disk.cmp(&a.size_on_disk));
        children
    }

    /// Поиск файлов по фильтрам: линейный проход по плоскому списку, затем
    /// сортировка найденных по размеру на диске и срез страницы.
    ///
    /// Линейный проход по 1.26 млн записей — доли секунды даже в debug-сборке,
    /// а команда `search_files` асинхронная, поэтому окно не ждёт. Сортируется
    /// только найденное (а не все файлы) — так первая страница сразу показывает
    /// самое крупное. `total` — сколько всего подходит под фильтры.
    ///
    /// Фильтр размера — по занятому месту (`size_on_disk`, основная метрика
    /// приложения); фактический размер в выдаче — отдельное поле. Границы дат
    /// включительные; файлы с неизвестным временем (0) при заданной дате не
    /// показываются, чтобы не выдаваться за «очень старые».
    pub fn search_files(
        &self,
        q: &SearchQuery,
        offset: usize,
        limit: usize,
    ) -> (Vec<SearchRow>, u32) {
        let needle = NameNeedle::new(&q.name);
        let by_date = q.modified_after.is_some() || q.modified_before.is_some();
        let mut matches: Vec<u32> = Vec::new();
        for (position, file) in self.files.iter().enumerate() {
            if !needle.matches(&file.name) {
                continue;
            }
            if file.size_on_disk < q.min_size {
                continue;
            }
            if let Some(max_size) = q.max_size {
                if file.size_on_disk > max_size {
                    continue;
                }
            }
            if by_date {
                if file.modified <= 0 {
                    continue;
                }
                if let Some(after) = q.modified_after {
                    if file.modified < after {
                        continue;
                    }
                }
                if let Some(before) = q.modified_before {
                    if file.modified > before {
                        continue;
                    }
                }
            }
            matches.push(position as u32);
        }

        let total = matches.len() as u32;
        if limit == 0 || offset >= matches.len() {
            return (Vec::new(), total);
        }

        // Полная сортировка нужна только когда страница забирает весь хвост.
        // Иначе достаточно отобрать `end` самых крупных (select_nth_unstable) и
        // отсортировать только их: первая страница не ждёт сортировки 1.26 млн
        // строк, а порядок страниц остаётся тем же (порядок полный).
        let end = offset.saturating_add(limit).min(matches.len());
        if end < matches.len() {
            matches.select_nth_unstable_by(end, |left, right| {
                compare_search_positions(&self.files, *left, *right)
            });
        }
        matches[..end]
            .sort_unstable_by(|left, right| compare_search_positions(&self.files, *left, *right));

        let rows = matches[offset..end]
            .iter()
            .map(|&position| {
                let file = &self.files[position as usize];
                SearchRow {
                    name: file.name.clone(),
                    folder_path: self.path_of(file.parent),
                    folder_id: file.parent,
                    size_on_disk: file.size_on_disk,
                    size_logical: file.size_logical,
                    modified: file.modified,
                    hard_linked: file.hard_linked,
                }
            })
            .collect();
        (rows, total)
    }

    /// Агрегаты по расширениям рекурсивно по поддереву папки — таблица вкладки
    /// «Типы файлов».
    ///
    /// Обход — по папкам поддерева, а не по всему плоскому списку файлов: для
    /// открытой подпапки не нужно проверять миллион чужих записей. Группы
    /// копятся в `HashMap` (расширений на диске тысячи, а не миллионы), затем
    /// сортируются один раз. Счётчик включает повторные жёсткие ссылки (это
    /// разные имена), а размеры дубликатов не суммируются — как в итогах папки,
    /// поэтому сумма строк сходится с деревом.
    pub fn file_types(&self, folder: FolderId, sort: TypeSort, desc: bool) -> Vec<FileTypeRow> {
        let mut groups: HashMap<String, TypeAgg> = HashMap::new();
        let mut stack = vec![folder];
        while let Some(id) = stack.pop() {
            let node = &self.nodes[id as usize];
            for &position in &node.files {
                let file = &self.files[position as usize];
                let aggregate = groups.entry(extension_of(&file.name)).or_default();
                aggregate.count += 1;
                if !file.hard_linked {
                    aggregate.size_on_disk += file.size_on_disk;
                    aggregate.size_logical += file.size_logical;
                }
            }
            stack.extend(node.children.iter().copied());
        }

        let parent_size = self.nodes[folder as usize].totals.size_on_disk;
        let mut rows: Vec<FileTypeRow> = groups
            .into_iter()
            .map(|(extension, aggregate)| FileTypeRow {
                extension,
                count: aggregate.count,
                size_on_disk: aggregate.size_on_disk,
                size_logical: aggregate.size_logical,
                // Сумма размеров групп не больше итога папки (дубликаты жёстких
                // ссылок не входят), но контракт DTO — 0.0..=1.0: ограничиваем.
                percent_of_parent: if parent_size == 0 {
                    0.0
                } else {
                    (aggregate.size_on_disk as f32 / parent_size as f32).min(1.0)
                },
            })
            .collect();
        rows.sort_by(|left, right| compare_type_rows(left, right, sort, desc));
        rows
    }

    /// Плоская карта типов: плитки-расширения в контейнере 0..1.
    ///
    /// Используется тот же `layout_treemap`, что и у основной карты, поэтому
    /// плитки не пересекаются и покрывают весь контейнер. Бюджет `max_rects`
    /// ограничивает число плиток: на диске расширений тысячи, а окну хватает
    /// крупнейших — остальные видны в таблице ниже. Группы нулевого размера
    /// (пустые файлы) на карту не попадают: площади у них нет.
    pub fn types_treemap(&self, folder: FolderId, max_rects: usize) -> Vec<TreemapRect> {
        if max_rects == 0 {
            return Vec::new();
        }
        let mut rows = self.file_types(folder, TypeSort::SizeOnDisk, true);
        rows.retain(|row| row.size_on_disk > 0);
        rows.truncate(max_rects);
        if rows.is_empty() {
            return Vec::new();
        }

        let items: Vec<TreemapItem> = rows
            .iter()
            .map(|row| TreemapItem {
                // Все плитки — типы, а не папки и не файлы: id = 0, is_dir = false.
                id: 0,
                name: if row.extension.is_empty() {
                    NO_EXTENSION_NAME.to_string()
                } else {
                    row.extension.clone()
                },
                size: row.size_on_disk,
                is_dir: false,
            })
            .collect();
        let container = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };

        layout_treemap(&items, container)
            .into_iter()
            .map(|(position, tile)| {
                let item = &items[position];
                TreemapRect {
                    id: 0,
                    name: item.name.clone(),
                    size: item.size,
                    is_dir: false,
                    x: tile.x,
                    y: tile.y,
                    w: tile.w,
                    h: tile.h,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileKey, FileMeta, FileSort, SearchQuery, SearchRow, TypeSort};

    /// Файл с полным набором полей: размер на диске, логический размер и время.
    fn meta_full(
        name: &str,
        size_on_disk: u64,
        size_logical: u64,
        modified: i64,
        file_index: u64,
    ) -> FileMeta {
        FileMeta {
            name: name.to_string(),
            size_on_disk,
            size_logical,
            modified,
            file_key: FileKey {
                volume_serial: 1,
                file_index,
            },
        }
    }

    fn meta(name: &str, size: u64, file_index: u64) -> FileMeta {
        meta_full(name, size, size, 0, file_index)
    }

    /// Запрос «всё подряд»: пустые фильтры валидны и означают полную выдачу.
    fn query(name: &str) -> SearchQuery {
        SearchQuery {
            name: name.to_string(),
            min_size: 0,
            max_size: None,
            modified_after: None,
            modified_before: None,
        }
    }

    #[test]
    fn add_file_aggregates_folder_totals() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();

        assert!(matches!(
            index.add_file(root, meta("a.bin", 100, 1)),
            AddOutcome::Counted
        ));
        assert!(matches!(
            index.add_file(root, meta("b.bin", 50, 2)),
            AddOutcome::Counted
        ));

        let totals = index.folder_totals(root);
        assert_eq!(totals.size_on_disk, 150);
        assert_eq!(totals.file_count, 2);
    }

    #[test]
    fn add_file_deduplicates_hard_links() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();

        assert!(matches!(
            index.add_file(root, meta("first.txt", 4096, 42)),
            AddOutcome::Counted
        ));
        // Вторая жёсткая ссылка на тот же файл: размер не должен задвоиться.
        assert!(matches!(
            index.add_file(root, meta("second.txt", 4096, 42)),
            AddOutcome::HardLinkDuplicate
        ));

        let totals = index.folder_totals(root);
        assert_eq!(totals.size_on_disk, 4096);
        assert_eq!(totals.file_count, 2);
    }

    #[test]
    fn child_folders_are_sorted_by_size_desc() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();

        let small = index.folder(root, "small");
        let large = index.folder(root, "large");
        let medium = index.folder(root, "medium");
        index.add_file(small, meta("small.bin", 10, 1));
        index.add_file(large, meta("large.bin", 30, 2));
        index.add_file(medium, meta("medium.bin", 20, 3));

        let children = index.child_folders(root);
        let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["large", "medium", "small"]);
        let sizes: Vec<u64> = children.iter().map(|c| c.size_on_disk).collect();
        assert_eq!(sizes, [30, 20, 10]);
    }

    #[test]
    fn percent_of_parent_is_fraction_of_parent_size() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let parent = index.folder(root, "parent");
        let child = index.folder(parent, "child");

        index.add_file(parent, meta("direct.bin", 75, 1));
        index.add_file(child, meta("nested.bin", 25, 2));

        let children = index.child_folders(parent);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].percent_of_parent, 0.25);

        // Пустой родитель: делить на ноль нельзя, доля должна быть 0.0, а не NaN.
        let empty_parent = index.folder(root, "empty-parent");
        index.folder(empty_parent, "empty-child");
        let empty_children = index.child_folders(empty_parent);
        assert_eq!(empty_children.len(), 1);
        assert_eq!(empty_children[0].percent_of_parent, 0.0);
        assert!(!empty_children[0].percent_of_parent.is_nan());
    }

    #[test]
    fn folder_totals_count_nested_folders() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let first = index.folder(root, "first");
        let second = index.folder(first, "second");
        let third = index.folder(second, "third");

        index.add_file(root, meta("root.bin", 5, 1));
        index.add_file(first, meta("first.bin", 100, 2));
        index.add_file(second, meta("second.bin", 50, 3));
        index.add_file(third, meta("third.bin", 25, 4));

        let totals = index.folder_totals(root);
        assert_eq!(totals.size_on_disk, 180);
        assert_eq!(totals.file_count, 4);
        assert_eq!(totals.folder_count, 3);

        let first_totals = index.folder_totals(first);
        assert_eq!(first_totals.size_on_disk, 175);
        assert_eq!(first_totals.file_count, 3);
        assert_eq!(first_totals.folder_count, 2);
    }

    #[test]
    fn get_files_returns_paged_rows() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "many");
        for i in 0..25u64 {
            // Размер растёт вместе с номером: порядок по размеру противоположен
            // порядку по имени, поэтому срез однозначно доказывает, что
            // сортировка применена до выборки страницы.
            index.add_file(
                folder,
                meta_full(&format!("file{i:02}"), i + 1, 25 - i, i as i64, i),
            );
        }

        assert_eq!(index.file_count(folder), 25);
        let rows = index.child_files(folder, FileSort::SizeOnDisk, true, 10, 10);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "file14", "file13", "file12", "file11", "file10", "file09", "file08", "file07",
                "file06", "file05"
            ],
            "страница — срез списка, отсортированного по размеру на диске"
        );

        // Доля считается от размера папки: сумма размеров файлов 1..=25 = 325.
        let first = &rows[0];
        assert_eq!(first.size_on_disk, 15);
        assert_eq!(first.percent_of_parent, 15.0_f32 / 325.0_f32);
    }

    #[test]
    fn get_files_respects_limit_offset() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "few");
        for i in 0..5u64 {
            index.add_file(folder, meta_full(&format!("f{i}"), 10, 10, 0, i));
        }

        // offset за пределами папки — страницы нет.
        assert!(
            index
                .child_files(folder, FileSort::Name, false, 10, 10)
                .is_empty()
        );
        // limit = 0 — пусто даже при корректном offset.
        assert!(
            index
                .child_files(folder, FileSort::Name, false, 0, 0)
                .is_empty()
        );
        // limit больше числа файлов — отдаём всё, что есть.
        assert_eq!(
            index
                .child_files(folder, FileSort::Name, false, 0, 100)
                .len(),
            5
        );
        // Хвост страницы: offset 3 при большом limit — только две последние строки.
        let tail = index.child_files(folder, FileSort::Name, false, 3, 100);
        let names: Vec<&str> = tail.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["f3", "f4"]);

        // Пустая папка: ноль файлов и пустая страница.
        let empty = index.folder(root, "empty");
        assert_eq!(index.file_count(empty), 0);
        assert!(
            index
                .child_files(empty, FileSort::Name, false, 0, 10)
                .is_empty()
        );

        // Нулевой родитель (файлы нулевого размера): доля 0.0, а не NaN.
        let zeros = index.folder(root, "zeros");
        index.add_file(zeros, meta_full("empty.bin", 0, 0, 0, 100));
        let zero_rows = index.child_files(zeros, FileSort::Name, false, 0, 10);
        assert_eq!(zero_rows.len(), 1);
        assert_eq!(zero_rows[0].percent_of_parent, 0.0);
        assert!(!zero_rows[0].percent_of_parent.is_nan());
    }

    #[test]
    fn child_files_sorts_stably() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "mixed");
        // Три файла одного размера на диске и один меньше — так проверяются и
        // основной ключ, и tie-break по имени при равных значениях.
        for (name, disk, logical, modified, key) in [
            ("b.txt", 100, 300, 200, 1),
            ("a.txt", 100, 100, 100, 2),
            ("c.txt", 100, 200, 300, 3),
            ("d.txt", 50, 400, 400, 4),
        ] {
            index.add_file(folder, meta_full(name, disk, logical, modified, key));
        }

        let names = |sort: FileSort, desc: bool| -> Vec<String> {
            index
                .child_files(folder, sort, desc, 0, 100)
                .iter()
                .map(|row| row.name.clone())
                .collect()
        };

        // Равные размеры — по имени (вставка была в порядке b, a, c, d).
        assert_eq!(
            names(FileSort::SizeOnDisk, false),
            ["d.txt", "a.txt", "b.txt", "c.txt"]
        );
        assert_eq!(
            names(FileSort::SizeOnDisk, true),
            ["a.txt", "b.txt", "c.txt", "d.txt"]
        );
        // Имя — самостоятельный ключ: desc разворачивает его.
        assert_eq!(
            names(FileSort::Name, false),
            ["a.txt", "b.txt", "c.txt", "d.txt"]
        );
        assert_eq!(
            names(FileSort::Name, true),
            ["d.txt", "c.txt", "b.txt", "a.txt"]
        );
        // Логический размер и время изменения — тоже с tie-break по имени.
        assert_eq!(
            names(FileSort::SizeLogical, false),
            ["a.txt", "c.txt", "b.txt", "d.txt"]
        );
        assert_eq!(
            names(FileSort::SizeLogical, true),
            ["d.txt", "b.txt", "c.txt", "a.txt"]
        );
        assert_eq!(
            names(FileSort::Modified, false),
            ["a.txt", "b.txt", "c.txt", "d.txt"]
        );
        assert_eq!(
            names(FileSort::Modified, true),
            ["d.txt", "c.txt", "b.txt", "a.txt"]
        );
    }

    #[test]
    fn hard_linked_row_is_visible_but_not_counted() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "links");

        assert!(matches!(
            index.add_file(folder, meta("first.txt", 4096, 7)),
            AddOutcome::Counted
        ));
        assert!(matches!(
            index.add_file(folder, meta("second.txt", 4096, 7)),
            AddOutcome::HardLinkDuplicate
        ));

        let rows = index.child_files(folder, FileSort::Name, false, 0, 10);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["first.txt", "second.txt"]);
        assert!(
            !rows[0].hard_linked,
            "первая ссылка считается обычным файлом"
        );
        assert!(
            rows[1].hard_linked,
            "у повторной ссылки должна быть пометка"
        );
        // Фактический размер виден у обеих строк...
        assert_eq!(rows[0].size_on_disk, 4096);
        assert_eq!(rows[1].size_on_disk, 4096);

        // ...но в итог папки входит только первая ссылка.
        let totals = index.folder_totals(folder);
        assert_eq!(totals.size_on_disk, 4096);
        assert_eq!(totals.file_count, 2);
        assert_eq!(index.file_count(folder), 2);

        let rows_sum: u64 = rows.iter().map(|row| row.size_on_disk).sum();
        assert_eq!(
            rows_sum - totals.size_on_disk,
            4096,
            "сумма по строкам больше итога папки ровно на размер дубликата"
        );
    }

    /// Раскладка записи — часть бюджета памяти: на C: файлов больше миллиона,
    /// поэтому рост записи на 8 байт — это +10 МБ на индексе.
    #[test]
    fn file_record_stays_compact() {
        assert_eq!(std::mem::size_of::<FileRecord>(), 56);
    }

    /// Имена по-русски: без учёта регистра «архив.zip» идёт раньше «Загрузки».
    /// Побайтовый компаратор ставил заглавную «З» (U+0417) впереди строчной «а»
    /// (U+0430), то есть сортировал не по алфавиту.
    #[test]
    fn test_sorts_cyrillic_alphabetically() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "russian");
        // Все файлы одного размера: порядок задаёт только имя (в т.ч. tie-break).
        for (position, name) in ["Загрузки", "архив.zip", "документы"].iter().enumerate()
        {
            index.add_file(folder, meta(name, 100, position as u64));
        }

        let names = |sort: FileSort, desc: bool| -> Vec<String> {
            index
                .child_files(folder, sort, desc, 0, 10)
                .iter()
                .map(|row| row.name.clone())
                .collect()
        };

        let ascending = ["архив.zip", "документы", "Загрузки"];
        assert_eq!(names(FileSort::Name, false), ascending);
        // Tie-break по имени действует и при сортировке по размеру.
        assert_eq!(names(FileSort::SizeOnDisk, false), ascending);
        assert_eq!(
            names(FileSort::Name, true),
            ["Загрузки", "документы", "архив.zip"]
        );
    }

    /// «ё» сортируется наравне с «е», как в проводнике: «ёж» раньше «Жук».
    /// При полном совпадении без учёта регистра порядок однозначный: «еж.txt»
    /// раньше «ёж.txt» (побайтовый tie-break, как и «A.txt» раньше «a.txt»).
    #[test]
    fn test_sorts_yo_like_ye() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "yo");
        for (position, name) in ["Жук.txt", "Ель.txt", "ёж.txt", "еж.txt"]
            .iter()
            .enumerate()
        {
            index.add_file(folder, meta(name, 100, position as u64));
        }

        let names: Vec<String> = index
            .child_files(folder, FileSort::Name, false, 0, 10)
            .iter()
            .map(|row| row.name.clone())
            .collect();
        assert_eq!(names, ["еж.txt", "ёж.txt", "Ель.txt", "Жук.txt"]);
    }

    /// ASCII без учёта регистра: различие только в регистре НЕ решает сравнение.
    /// Иначе «Apricot.txt» встало бы раньше «apple.txt», «AB» — раньше «a», а
    /// «RELEASE.txt» — раньше «readme.md», то есть сортировка шла бы по регистру,
    /// а не по алфавиту.
    #[test]
    fn test_sorts_ascii_case_insensitively() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "ascii-case");
        for (position, name) in [
            "apple.txt",
            "Apricot.txt",
            "a",
            "AB",
            "readme.md",
            "RELEASE.txt",
        ]
        .iter()
        .enumerate()
        {
            // Одинаковый размер: порядок задаёт имя (в т.ч. tie-break).
            index.add_file(folder, meta(name, 100, position as u64));
        }

        let names = |sort: FileSort, desc: bool| -> Vec<String> {
            index
                .child_files(folder, sort, desc, 0, 10)
                .iter()
                .map(|row| row.name.clone())
                .collect()
        };

        let ascending = [
            "a",
            "AB",
            "apple.txt",
            "Apricot.txt",
            "readme.md",
            "RELEASE.txt",
        ];
        assert_eq!(names(FileSort::Name, false), ascending);
        // Tie-break по имени при равных размерах идёт через тот же компаратор.
        assert_eq!(names(FileSort::SizeOnDisk, false), ascending);
        assert_eq!(
            names(FileSort::Name, true),
            [
                "RELEASE.txt",
                "readme.md",
                "Apricot.txt",
                "apple.txt",
                "AB",
                "a"
            ]
        );
    }

    /// Дубликат жёсткой ссылки, чья первая копия лежит в другой папке: размер
    /// второй ссылки не входит в итог этой папки, поэтому доля от родителя
    /// арифметически превысила бы 1.0. Инвариант DTO — 0.0..=1.0, его держим.
    #[test]
    fn hard_linked_percent_stays_within_one() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let first = index.folder(root, "first");
        let second = index.folder(root, "second");

        // Первая копия учтена в другой папке...
        index.add_file(first, meta("shared.bin", 4096, 7));
        // ...а вторая папка содержит свои файлы и ссылку-дубликат.
        index.add_file(second, meta("small.bin", 100, 8));
        index.add_file(second, meta("other.bin", 900, 9));
        index.add_file(second, meta("alias.bin", 4096, 7));

        let totals = index.folder_totals(second);
        assert_eq!(totals.size_on_disk, 1000, "дубликат в итог папки не входит");

        let rows = index.child_files(second, FileSort::Name, false, 0, 10);
        let alias = rows.iter().find(|row| row.name == "alias.bin").unwrap();
        assert!(alias.hard_linked);
        assert_eq!(
            alias.percent_of_parent, 1.0,
            "доля ограничена единицей: иначе полоска в UI вылезет за строку"
        );
        let small = rows.iter().find(|row| row.name == "small.bin").unwrap();
        assert_eq!(small.percent_of_parent, 0.1);
    }

    /// Поиск: 30 файлов с разными именами, размерами и датами — проверяем
    /// каждый фильтр, их сочетание, пагинацию (offset/limit) и `total`.
    #[test]
    fn search_files_pages_and_filters() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let documents = index.folder(root, "Документы");
        let reports = index.folder(documents, "Отчёты");
        let downloads = index.folder(root, "Загрузки");

        // По 10 файлов в каждой папке; размер и дата растут вместе с номером
        // файла, поэтому каждый фильтр отсекает заранее известный диапазон.
        let folders = [reports, documents, downloads];
        for group in 0..3u64 {
            for number in 1..=10u64 {
                let position = group * 10 + number - 1;
                let name = match group {
                    0 => format!("Отчёт {number:02}.txt"),
                    1 => format!("photo {number:02}.jpg"),
                    _ => format!("данные {number:02}.bin"),
                };
                let size = (position + 1) * 1_000;
                let modified = 1_000 + position as i64 * 1_000;
                index.add_file(
                    folders[group as usize],
                    meta_full(&name, size, size / 2, modified, position + 1),
                );
            }
        }

        // Имя: кириллица без учёта регистра — «Отчёт» находится по «ОТЧЁТ».
        let (rows, total) = index.search_files(&query("ОТЧЁТ"), 0, 100);
        assert_eq!(total, 10);
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|row| row.name.starts_with("Отчёт ")));

        // Размер на диске 5 000..=15 000 байт — позиции 4..=14, всего 11.
        let mut by_size = query("");
        by_size.min_size = 5_000;
        by_size.max_size = Some(15_000);
        assert_eq!(index.search_files(&by_size, 0, 100).1, 11);

        // Дата 8 000..=12 000 мс — позиции 7..=11, всего 5.
        let mut by_date = query("");
        by_date.modified_after = Some(8_000);
        by_date.modified_before = Some(12_000);
        assert_eq!(index.search_files(&by_date, 0, 100).1, 5);

        // Сочетание фильтров: «данные» + размер от 25 000 + дата не позже 28 000
        // — позиции 24..=27, всего 4.
        let mut combined = query("данные");
        combined.min_size = 25_000;
        combined.modified_before = Some(28_000);
        let (rows, total) = index.search_files(&combined, 0, 100);
        assert_eq!(total, 4);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.name.starts_with("данные ")));

        // Пагинация: страницы — срез одного и того же порядка, `total` честный.
        let (first_page, total) = index.search_files(&query(""), 0, 10);
        assert_eq!(total, 30);
        assert_eq!(first_page.len(), 10);
        assert_eq!(
            first_page[0].size_on_disk, 30_000,
            "первая страница начинается с самого крупного файла"
        );
        assert!(
            first_page
                .windows(2)
                .all(|pair| pair[0].size_on_disk >= pair[1].size_on_disk),
            "страница отсортирована по размеру на диске"
        );
        let (tail, total) = index.search_files(&query(""), 25, 10);
        assert_eq!(total, 30);
        assert_eq!(tail.len(), 5);
        let (beyond, total) = index.search_files(&query(""), 30, 10);
        assert!(beyond.is_empty());
        assert_eq!(total, 30);
        let (none, total) = index.search_files(&query(""), 0, 0);
        assert!(none.is_empty(), "limit = 0 — пустая страница");
        assert_eq!(total, 30);

        // Строка несёт папку файла: путь от корня индекса и id для перехода.
        let (rows, _) = index.search_files(&query("Отчёт 01"), 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].folder_id, reports);
        assert_eq!(rows[0].folder_path, "C:\\Документы\\Отчёты");
        assert_eq!(rows[0].size_on_disk, 1_000);
        assert_eq!(rows[0].size_logical, 500);
        assert_eq!(rows[0].modified, 1_000);

        // Противоречивые границы размера дают пустую выдачу, а не панику.
        let mut impossible = query("");
        impossible.min_size = 100;
        impossible.max_size = Some(50);
        assert_eq!(index.search_files(&impossible, 0, 10).1, 0);

        // Файл с неизвестным временем (0) не притворяется «очень старым»: при
        // заданной дате он не показывается.
        index.add_file(downloads, meta_full("без-даты.bin", 1_000, 1_000, 0, 999));
        assert_eq!(index.search_files(&by_date, 0, 100).1, 5);
        assert_eq!(index.search_files(&query(""), 0, 100).1, 31);
    }

    /// «ОТЧЁТ» находится по «отчёт», «Ёж» — по «еж»: те же правила, что и у
    /// сортировки (регистр и «ё» не различаются), и для ASCII тоже.
    #[test]
    fn search_matches_name_case_insensitively() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        for (position, name) in ["ОТЧЁТ.txt", "Ёж.txt", "ёжик.txt", "report.TXT"]
            .iter()
            .enumerate()
        {
            index.add_file(root, meta(name, 10, position as u64));
        }

        let names = |needle: &str| -> Vec<String> {
            index
                .search_files(&query(needle), 0, 100)
                .0
                .iter()
                .map(|row| row.name.clone())
                .collect()
        };

        assert_eq!(names("отчёт"), ["ОТЧЁТ.txt"]);
        assert_eq!(names("Отчёт"), ["ОТЧЁТ.txt"]);
        // «ё» и «е» — одна буква, как в сортировке; равные размеры — по имени.
        assert_eq!(names("еж"), ["Ёж.txt", "ёжик.txt"]);
        assert_eq!(names("ЕЖ"), ["Ёж.txt", "ёжик.txt"]);
        // ASCII: регистр не важен и здесь.
        assert_eq!(names("REPORT"), ["report.TXT"]);
        // Пустой запрос — «всё подряд».
        assert_eq!(names("").len(), 4);

        assert!(name_contains("ОТЧЁТ.txt", "отчёт"));
        assert!(name_contains("Ёж", "еж"));
        assert!(name_contains("report.TXT", "REPORT"));
        assert!(!name_contains("Ёж", "ёж2"));
        assert!(!name_contains("файл", "ЁЖ"));
        assert!(name_contains("что угодно", ""));
        assert!(name_contains("", ""));
    }

    /// Жёсткая ссылка в поиске видна отдельной строкой с пометкой.
    #[test]
    fn search_reports_hard_linked_rows() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        index.add_file(root, meta("shared.bin", 4_096, 7));
        index.add_file(root, meta("alias.bin", 4_096, 7));

        let (rows, total): (Vec<SearchRow>, u32) = index.search_files(&query(""), 0, 10);
        assert_eq!(total, 2);
        let alias = rows
            .iter()
            .find(|row| row.name == "alias.bin")
            .expect("повторная ссылка должна быть в выдаче");
        assert!(alias.hard_linked);
        let shared = rows
            .iter()
            .find(|row| row.name == "shared.bin")
            .expect("первая ссылка должна быть в выдаче");
        assert!(!shared.hard_linked);
    }

    /// Путь папки — от корня индекса, разделитель `\'`, без двойных слэшей у
    /// корня диска. Цепочка предков — для перехода из поиска (get_path_crumbs).
    #[test]
    fn path_of_builds_native_path() {
        // В приложении корень диска создаётся как «C:\» (drive_root), а не «C:».
        let mut index = ScanIndex::new("C:\\");
        let root = index.root();
        let users = index.folder(root, "Users");
        let georg = index.folder(users, "georg");

        assert_eq!(index.path_of(root), "C:\\");
        assert_eq!(index.path_of(users), "C:\\Users");
        assert_eq!(index.path_of(georg), "C:\\Users\\georg");
        assert_eq!(
            index.ancestors(georg),
            vec![
                (root, "C:\\".to_string()),
                (users, "Users".to_string()),
                (georg, "georg".to_string()),
            ]
        );

        // Корень-папка хранит только имя: путь строится от корня индекса.
        let mut folder_index = ScanIndex::new("WoozyTree-Тест");
        let folder_root = folder_index.root();
        let nested = folder_index.folder(folder_root, "вложенная");
        let deep = folder_index.folder(nested, "глубже");
        assert_eq!(
            folder_index.path_of(deep),
            "WoozyTree-Тест\\вложенная\\глубже"
        );
        assert_eq!(folder_index.path_of(folder_root), "WoozyTree-Тест");
    }

    /// Реальные пути для действий: корень диска — база, дальше относительный
    /// подпуть индекса. Файл прямо в корне диска не получает двойного слэша.
    #[test]
    fn real_path_of_drive_mode_uses_drive_root() {
        let mut index = ScanIndex::new("C:\\");
        index.set_root_real_path("C:\\".to_string());
        let root = index.root();
        let users = index.folder(root, "Users");
        let georg = index.folder(users, "georg");

        assert_eq!(index.real_path_of(root, None), "C:\\");
        assert_eq!(index.real_path_of(users, None), "C:\\Users");
        assert_eq!(index.real_path_of(georg, None), "C:\\Users\\georg");
        assert_eq!(
            index.real_path_of(georg, Some("отчёт.txt")),
            "C:\\Users\\georg\\отчёт.txt"
        );
        assert_eq!(
            index.real_path_of(root, Some("pagefile.sys")),
            "C:\\pagefile.sys"
        );
    }

    /// Режим «папка»: база — выбранная папка (без хвостового слэша и без
    /// verbatim-префикса `\\?\`, который ломает проводник и ShellExecuteW).
    #[test]
    fn real_path_of_folder_mode_uses_selected_folder() {
        let mut index = ScanIndex::new("WoozyTree-Тест");
        index.set_root_real_path("C:\\Temp\\WoozyTree-Тест".to_string());
        let root = index.root();
        let nested = index.folder(root, "вложенная");
        let deep = index.folder(nested, "глубже");

        assert_eq!(index.real_path_of(root, None), "C:\\Temp\\WoozyTree-Тест");
        assert_eq!(
            index.real_path_of(deep, None),
            "C:\\Temp\\WoozyTree-Тест\\вложенная\\глубже"
        );
        assert_eq!(
            index.real_path_of(deep, Some("файл.bin")),
            "C:\\Temp\\WoozyTree-Тест\\вложенная\\глубже\\файл.bin"
        );

        let mut normalized = ScanIndex::new("Temp");
        normalized.set_root_real_path("\\\\?\\C:\\Temp\\".to_string());
        assert_eq!(
            normalized.real_path_of(normalized.root(), None),
            "C:\\Temp",
            "verbatim-префикс и хвостовой слэш не должны попадать в путь действия"
        );
    }

    /// «Все диски»: общего реального пути нет, а подпапка тома «X:» становится
    /// базой «X:\» для себя и всех своих детей.
    #[test]
    fn real_path_of_all_drives_uses_volume_subfolder_as_base() {
        let mut index = ScanIndex::new("Все диски");
        index.set_root_real_path(String::new());
        let root = index.root();
        let c = index.folder(root, "C:");
        let users = index.folder(c, "Users");
        let d = index.folder(root, "D:");

        assert_eq!(
            index.real_path_of(root, None),
            "",
            "у общего корня «Все диски» реального пути нет"
        );
        assert_eq!(index.real_path_of(c, None), "C:\\");
        assert_eq!(index.real_path_of(users, None), "C:\\Users");
        assert_eq!(
            index.real_path_of(users, Some("файл.bin")),
            "C:\\Users\\файл.bin"
        );
        assert_eq!(index.real_path_of(d, None), "D:\\");
    }

    /// Агрегация по расширениям: `.txt`/`.TXT` — одна группа, `.rs` — вторая,
    /// файл без расширения — третья (`""`). Размеры суммарны, доля — от итога
    /// папки. Жёсткая ссылка-дубликат видна в счётчике, но размер не задваивает.
    #[test]
    fn file_types_aggregate_by_extension() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        index.add_file(root, meta_full("a.txt", 400, 300, 0, 1));
        index.add_file(root, meta_full("b.TXT", 200, 100, 0, 2));
        index.add_file(root, meta_full("main.rs", 300, 150, 0, 3));
        index.add_file(root, meta_full("README", 100, 50, 0, 4));

        let rows = index.file_types(root, TypeSort::SizeOnDisk, true);
        assert_eq!(rows.len(), 3, "txt, rs и «без расширения»");
        let extensions: Vec<&str> = rows.iter().map(|row| row.extension.as_str()).collect();
        assert_eq!(
            extensions,
            ["txt", "rs", ""],
            "по размеру на диске, убыв.; имя группы — без точки в нижнем регистре"
        );

        let txt = &rows[0];
        assert_eq!(txt.count, 2, ".txt и .TXT — одна группа");
        assert_eq!(txt.size_on_disk, 600);
        assert_eq!(txt.size_logical, 400);
        assert_eq!(txt.percent_of_parent, 0.6, "600 из 1000 байт папки");

        let rs = &rows[1];
        assert_eq!((rs.count, rs.size_on_disk, rs.size_logical), (1, 300, 150));
        assert_eq!(rs.percent_of_parent, 0.3);

        let no_extension = &rows[2];
        assert_eq!(no_extension.extension, "");
        assert_eq!(
            (
                no_extension.count,
                no_extension.size_on_disk,
                no_extension.size_logical
            ),
            (1, 100, 50),
        );
        assert_eq!(no_extension.percent_of_parent, 0.1);

        let percent_sum: f32 = rows.iter().map(|row| row.percent_of_parent).sum();
        assert!(
            (percent_sum - 1.0).abs() < 1e-6,
            "доли типов складываются в 100 % папки: {percent_sum}"
        );

        // Сортировка по имени без учёта регистра: пустое расширение — первым
        // (asc) и последним (desc).
        let by_name: Vec<String> = index
            .file_types(root, TypeSort::Name, false)
            .iter()
            .map(|row| row.extension.clone())
            .collect();
        assert_eq!(by_name, ["", "rs", "txt"]);
        let by_name_desc: Vec<String> = index
            .file_types(root, TypeSort::Name, true)
            .iter()
            .map(|row| row.extension.clone())
            .collect();
        assert_eq!(by_name_desc, ["txt", "rs", ""]);

        // По числу файлов: txt (2) первый; равные счётчики — по имени asc.
        let by_count: Vec<String> = index
            .file_types(root, TypeSort::Count, true)
            .iter()
            .map(|row| row.extension.clone())
            .collect();
        assert_eq!(by_count, ["txt", "", "rs"]);
        // По фактическому размеру: 400, 150, 50.
        let by_logical: Vec<String> = index
            .file_types(root, TypeSort::SizeLogical, true)
            .iter()
            .map(|row| row.extension.clone())
            .collect();
        assert_eq!(by_logical, ["txt", "rs", ""]);

        // Повторная жёсткая ссылка на a.txt: имя учитывается в счётчике, но
        // размер — нет (как в итогах папки), поэтому сумма не растёт.
        assert!(matches!(
            index.add_file(root, meta_full("alias.txt", 400, 300, 0, 1)),
            AddOutcome::HardLinkDuplicate
        ));
        let rows = index.file_types(root, TypeSort::SizeOnDisk, true);
        let txt = rows.iter().find(|row| row.extension == "txt").expect("txt");
        assert_eq!(txt.count, 3);
        assert_eq!(txt.size_on_disk, 600, "размер дубликата не задваивается");
        assert_eq!(txt.percent_of_parent, 0.6);
        let sum: u64 = rows.iter().map(|row| row.size_on_disk).sum();
        assert_eq!(
            sum,
            index.folder_totals(root).size_on_disk,
            "сумма по типам сходится с итогом папки"
        );
    }

    /// Агрегаты рекурсивны: файлы вложенных папок входят в типы родителя, а
    /// папка считает только своё поддерево.
    #[test]
    fn file_types_are_recursive() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let docs = index.folder(root, "docs");
        let deep = index.folder(docs, "deep");

        index.add_file(root, meta("root.txt", 100, 1));
        index.add_file(docs, meta("doc.pdf", 200, 2));
        index.add_file(deep, meta("nested.PDF", 300, 3));
        index.add_file(deep, meta("image.png", 400, 4));

        let rows = index.file_types(root, TypeSort::SizeOnDisk, true);
        let extensions: Vec<&str> = rows.iter().map(|row| row.extension.as_str()).collect();
        assert_eq!(extensions, ["pdf", "png", "txt"]);
        assert_eq!(rows[0].count, 2, ".pdf и .PDF — одна группа");
        assert_eq!(rows[0].size_on_disk, 500);
        assert_eq!(
            rows[0].percent_of_parent, 0.5,
            "500 из 1000 байт всего поддерева"
        );

        let sum: u64 = rows.iter().map(|row| row.size_on_disk).sum();
        assert_eq!(sum, index.folder_totals(root).size_on_disk);

        // Подпапка docs: 500 + 400 = 900 байт, доли — от её итога.
        let docs_rows = index.file_types(docs, TypeSort::SizeOnDisk, true);
        let docs_extensions: Vec<&str> =
            docs_rows.iter().map(|row| row.extension.as_str()).collect();
        assert_eq!(docs_extensions, ["pdf", "png"]);
        assert_eq!(docs_rows[0].size_on_disk, 500);
        assert_eq!(docs_rows[0].percent_of_parent, 500.0 / 900.0);
        assert_eq!(
            docs_rows.iter().map(|row| row.size_on_disk).sum::<u64>(),
            index.folder_totals(docs).size_on_disk
        );
    }

    /// Карта типов — плоская раскладка расширений тем же `layout_treemap`, что
    /// и основная карта: плитки покрывают контейнер, бюджет соблюдён, имена —
    /// расширения («(нет)» для файлов без расширения), id = 0.
    #[test]
    fn types_treemap_covers_container() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        for (name, size, key) in [
            ("a.txt", 500u64, 1u64),
            ("b.TXT", 300, 2),
            ("main.rs", 100, 3),
            ("data.bin", 60, 4),
            ("README", 40, 5),
        ] {
            index.add_file(root, meta(name, size, key));
        }

        // Полная выдача: четыре расширения (txt, rs, bin и «(нет)»), доли от
        // 1000 байт.
        let rects = index.types_treemap(root, 10);
        assert_eq!(rects.len(), 4);
        assert_eq!(
            rects
                .iter()
                .map(|rect| rect.name.as_str())
                .collect::<Vec<_>>(),
            ["txt", "rs", "bin", "(нет)"],
            "порядок — по убыванию размера, безымянное расширение — «(нет)»"
        );
        assert!(rects.iter().all(|rect| rect.id == 0 && !rect.is_dir));
        let txt = rects.iter().find(|rect| rect.name == "txt").expect("txt");
        assert!((txt.w * txt.h - 0.8).abs() < 1e-9, "800 из 1000 байт");

        // Бюджет: не больше max_rects плиток, и они покрывают контейнер.
        let limited = index.types_treemap(root, 3);
        assert_eq!(limited.len(), 3);
        let covered: f64 = limited.iter().map(|rect| rect.w * rect.h).sum();
        assert!(
            (covered - 1.0).abs() < 1e-9,
            "плитки покрывают весь контейнер: {covered}"
        );
        for rect in &limited {
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.x + rect.w <= 1.0 + 1e-9);
            assert!(rect.y + rect.h <= 1.0 + 1e-9);
        }

        // Нет файлов или бюджет 0 — пусто, без паники.
        let empty = index.folder(root, "empty");
        assert!(index.types_treemap(empty, 10).is_empty());
        assert!(index.types_treemap(root, 0).is_empty());
    }
}
