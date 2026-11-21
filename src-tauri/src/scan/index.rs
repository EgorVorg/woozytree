//! Индекс сканирования: дерево папок с агрегированными итогами и плоский список
//! файлов — основа таблицы файлов и живой карты.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::model::{FileKey, FileMeta, FileRow, FileSort, FolderId, FolderSummary, FolderTotals};

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

/// Сравнение имён для сортировки — по-русски, как в проводнике: регистр не
/// различается (включая кириллицу), «ё» стоит наравне с «е». Порядок символов —
/// по коду Unicode: латиница раньше кириллицы, а после снятия регистра строчные
/// «а»…«я» идут по алфавиту.
///
/// ASCII-имена (типичный случай) сравниваются побайтово, без аллокаций: быстрый
/// путь включается, пока различающиеся байты ASCII. На не-ASCII (кириллица)
/// регистр снимается посимвольно через `char::to_lowercase` — тоже без аллокаций.
///
/// При полном совпадении без учёта регистра сравниваем исходные байты: порядок
/// однозначен («A.txt» раньше «a.txt», «еж.txt» раньше «ёж.txt»).
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
            return a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
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

pub struct ScanIndex {
    nodes: Vec<Node>,
    // (родитель, имя) -> id, чтобы folder() не создавал дубликат папки
    folders_by_name: HashMap<(FolderId, String), FolderId>,
    // Ключи уже учтённых файлов: по ним распознаются жёсткие ссылки.
    seen_files: HashMap<FileKey, ()>,
    // Плоский список файлов: одно имя на диске — одна запись.
    files: Vec<FileRecord>,
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
        }
    }

    pub fn root(&self) -> FolderId {
        0
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileKey, FileMeta, FileSort};

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
}
