//! Индекс сканирования: дерево папок в памяти с агрегированными итогами.

use std::collections::HashMap;

use crate::model::{FileKey, FileMeta, FolderId, FolderSummary, FolderTotals};

/// Результат добавления файла в индекс.
pub enum AddOutcome {
    Counted,
    HardLinkDuplicate,
}

struct Node {
    name: String,
    parent: Option<FolderId>,
    totals: FolderTotals,
    children: Vec<FolderId>,
}

fn zero_totals() -> FolderTotals {
    FolderTotals {
        size_on_disk: 0,
        size_logical: 0,
        file_count: 0,
        folder_count: 0,
    }
}

pub struct ScanIndex {
    nodes: Vec<Node>,
    // (родитель, имя) -> id, чтобы folder() не создавал дубликат папки
    folders_by_name: HashMap<(FolderId, String), FolderId>,
    // Ключи уже учтённых файлов: по ним распознаются жёсткие ссылки.
    seen_files: HashMap<FileKey, ()>,
}

impl ScanIndex {
    pub fn new(root_name: &str) -> ScanIndex {
        let root = Node {
            name: root_name.to_string(),
            parent: None,
            totals: zero_totals(),
            children: Vec::new(),
        };
        ScanIndex {
            nodes: vec![root],
            folders_by_name: HashMap::new(),
            seen_files: HashMap::new(),
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
        let outcome = if self.seen_files.insert(meta.file_key.clone(), ()).is_some() {
            AddOutcome::HardLinkDuplicate
        } else {
            AddOutcome::Counted
        };
        let counted = matches!(outcome, AddOutcome::Counted);

        let mut cursor = Some(parent);
        while let Some(current) = cursor {
            let totals = &mut self.nodes[current as usize].totals;
            totals.file_count += 1;
            if counted {
                totals.size_on_disk += meta.size_on_disk;
                totals.size_logical += meta.size_logical;
            }
            cursor = self.nodes[current as usize].parent;
        }

        outcome
    }

    pub fn folder_totals(&self, id: FolderId) -> FolderTotals {
        self.nodes[id as usize].totals.clone()
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
    use crate::model::{FileKey, FileMeta};

    fn meta(name: &str, size: u64, file_index: u64) -> FileMeta {
        FileMeta {
            name: name.to_string(),
            size_on_disk: size,
            size_logical: size,
            modified: 0,
            file_key: FileKey {
                volume_serial: 1,
                file_index,
            },
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
}
