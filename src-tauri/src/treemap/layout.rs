//! Squarified-раскладка treemap и данные для карты.
//!
//! Алгоритм — Bruls/van Wijk («squarified»): элементы сортируются по размеру
//! убыв., контейнер делится на ряды вдоль короткой стороны, ряд растёт, пока
//! «неквадратность» его плиток уменьшается. Координаты — нормированные 0..1.

use crate::model::{FileSort, FolderId, SizeMode, TreemapRect};
use crate::scan::index::ScanIndex;

/// Прямоугольник в нормированных координатах контейнера (0..1).
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Элемент раскладки: папка или файл.
pub struct TreemapItem {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Элементы меньше 1/100 контейнера считаются мелочью и собираются в «прочее».
const SMALL_SHARE: u64 = 100;

/// Сколько детей показывать отдельными плитками, если в «прочее» уходит больше
/// половины родителя: иначе зум в такую папку упирается в одну серую плитку.
const FALLBACK_TILES: usize = 100;

/// Имя прямоугольника, в который собрана мелочь.
const OTHER_NAME: &str = "прочее";

/// Squarified-раскладка: items сортируются по size desc, rect-контейнер делится
/// на плитки. Возвращает пары (индекс элемента, прямоугольник); порядок — по
/// убыванию size. Элементы нулевого размера получают нулевую площадь в правом
/// нижнем углу контейнера (их место в результате сохраняется).
pub fn layout_treemap(items: &[TreemapItem], container: Rect) -> Vec<(usize, Rect)> {
    if items.is_empty() {
        return Vec::new();
    }
    if !(container.w > 0.0) || !(container.h > 0.0) {
        // Вырожденный контейнер: площади нет, но каждый элемент получает плитку.
        let empty = Rect {
            x: container.x,
            y: container.y,
            w: 0.0,
            h: 0.0,
        };
        return (0..items.len()).map(|index| (index, empty)).collect();
    }

    // Стабильная сортировка: равные размеры сохраняют порядок входного среза,
    // поэтому раскладка детерминирована для одного и того же снимка.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&left, &right| items[right].size.cmp(&items[left].size));

    let total: u64 = items.iter().map(|item| item.size).sum();
    if total == 0 {
        let empty = Rect {
            x: container.x + container.w,
            y: container.y + container.h,
            w: 0.0,
            h: 0.0,
        };
        return order.into_iter().map(|index| (index, empty)).collect();
    }

    let area = container.w * container.h;
    let scale = area / total as f64;
    let areas: Vec<f64> = order
        .iter()
        .map(|&index| items[index].size as f64 * scale)
        .collect();
    // Нули стоят в конце: размеры отсортированы по убыванию.
    let positive = areas.iter().take_while(|&&value| value > 0.0).count();
    let laid = layout_rows(&areas[..positive], container);

    let mut rects = Vec::with_capacity(items.len());
    for (&index, rect) in order.iter().zip(laid) {
        rects.push((index, rect));
    }
    let empty = Rect {
        x: container.x + container.w,
        y: container.y + container.h,
        w: 0.0,
        h: 0.0,
    };
    for &index in &order[positive..] {
        rects.push((index, empty));
    }
    rects
}

/// Разложить площади по рядам. Позиции соседних плиток вычисляются прибавлением
/// уже посчитанной стороны, поэтому общая граница у них побитово одна и та же —
/// пересечений нет даже на уровне погрешности f64.
fn layout_rows(areas: &[f64], container: Rect) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(areas.len());
    let mut rem = container;
    let mut start = 0;

    while start < areas.len() {
        if !(rem.w > 0.0) || !(rem.h > 0.0) {
            // Места не осталось: оставшиеся плитки нулевые, но их число
            // сохраняется (контракт «по паре на элемент»).
            let empty = Rect {
                x: container.x + container.w,
                y: container.y + container.h,
                w: 0.0,
                h: 0.0,
            };
            for _ in start..areas.len() {
                rects.push(empty);
            }
            break;
        }

        // Ряд идёт вдоль короткой стороны: у широкого контейнера это столбец.
        let vertical = rem.w >= rem.h;
        let side = if vertical { rem.h } else { rem.w };
        let (end, row_area) = row_end(areas, start, side);
        let last_row = end == areas.len();

        if vertical {
            let thickness = if last_row { rem.w } else { row_area / side };
            let thickness = thickness.clamp(0.0, rem.w);
            if thickness <= 0.0 {
                let empty = Rect {
                    x: container.x + container.w,
                    y: container.y + container.h,
                    w: 0.0,
                    h: 0.0,
                };
                for _ in start..areas.len() {
                    rects.push(empty);
                }
                break;
            }
            let bottom = rem.y + rem.h;
            let mut y = rem.y;
            for index in start..end {
                // Последняя плитка ряда забирает остаток, остальные не выходят за
                // границу ряда даже при ошибке округления.
                let limit = (bottom - y).max(0.0);
                let height = if index + 1 == end {
                    limit
                } else {
                    (areas[index] / thickness).min(limit).max(0.0)
                };
                rects.push(Rect {
                    x: rem.x,
                    y,
                    w: thickness,
                    h: height,
                });
                y += height;
            }
            rem.x += thickness;
            // Защита от бесконечного цикла: при очень маленькой толщине
            // вычитание может не изменить остаток (погрешность f64).
            let next = rem.w - thickness;
            rem.w = if next < rem.w { next.max(0.0) } else { 0.0 };
        } else {
            let thickness = if last_row { rem.h } else { row_area / side };
            let thickness = thickness.clamp(0.0, rem.h);
            if thickness <= 0.0 {
                let empty = Rect {
                    x: container.x + container.w,
                    y: container.y + container.h,
                    w: 0.0,
                    h: 0.0,
                };
                for _ in start..areas.len() {
                    rects.push(empty);
                }
                break;
            }
            let right = rem.x + rem.w;
            let mut x = rem.x;
            for index in start..end {
                let limit = (right - x).max(0.0);
                let width = if index + 1 == end {
                    limit
                } else {
                    (areas[index] / thickness).min(limit).max(0.0)
                };
                rects.push(Rect {
                    x,
                    y: rem.y,
                    w: width,
                    h: thickness,
                });
                x += width;
            }
            rem.y += thickness;
            let next = rem.h - thickness;
            rem.h = if next < rem.h { next.max(0.0) } else { 0.0 };
        }
        start = end;
    }
    rects
}

/// Длина ряда: сколько площадей с `start` укладывать вместе и их сумма.
/// Ряд растёт, пока худшее соотношение сторон его плиток не ухудшается.
fn row_end(areas: &[f64], start: usize, side: f64) -> (usize, f64) {
    let mut end = start + 1;
    let mut sum = areas[start];
    let mut best = worst_aspect(&areas[start..end], sum, side);
    while end < areas.len() {
        let candidate_sum = sum + areas[end];
        let candidate = worst_aspect(&areas[start..=end], candidate_sum, side);
        if candidate <= best {
            end += 1;
            sum = candidate_sum;
            best = candidate;
        } else {
            break;
        }
    }
    (end, sum)
}

/// Худшее соотношение сторон плиток ряда: максимум обратных отношений
/// «длина плитки вдоль короткой стороны / толщина ряда» и наоборот.
fn worst_aspect(areas: &[f64], sum: f64, side: f64) -> f64 {
    let mut smallest = f64::MAX;
    let mut largest = 0.0f64;
    for &value in areas {
        smallest = smallest.min(value);
        largest = largest.max(value);
    }
    if !(sum > 0.0) || !(side > 0.0) || !(smallest > 0.0) {
        return f64::INFINITY;
    }
    let side_sq = side * side;
    (largest * side_sq / (sum * sum)).max(sum * sum / (smallest * side_sq))
}

/// Общая функция данных для команды `get_treemap` и живых событий карты.
///
/// Раскладывает детей папки (папки и файлы, убыв. размера) в квадрат 0..1.
/// Крупные папки рекурсивно раскладываются внутри себя, пока хватает бюджета
/// `max_rects` и пока их дети не мельче ~1 % родителя; остаток мелочи — один
/// прямоугольник «прочее» (id = 0, is_dir = false). Если в «прочее» уходит больше
/// половины родителя (все дети мелкие или один крупный и сотни мелких),
/// отдельными плитками показываются крупнейшие дети (до `FALLBACK_TILES`), чтобы
/// зум не упирался в одну серую плитку. Файлы получают id = 0.
///
/// Параметры отображения — осознанное расширение Плана 2 (настройки живут на
/// фронтенде и приходят аргументами команды):
/// - `size_mode` — раскладывать по занятому месту или по фактическому размеру;
/// - `include_files` — `false` оставляет на карте только папки;
/// - `min_tile_bytes` — плитки мельче порога складываются в «прочее» (это
///   ТОЛЬКО отображение: итоги, счётчики и проценты папки не меняются).
pub fn treemap_for_folder(
    index: &ScanIndex,
    folder: FolderId,
    max_rects: usize,
    size_mode: SizeMode,
    include_files: bool,
    min_tile_bytes: u64,
) -> Vec<TreemapRect> {
    let mut rects = Vec::with_capacity(max_rects.min(4096));
    if max_rects == 0 {
        return rects;
    }
    let mut budget = max_rects;
    let root = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    expand_folder(
        index,
        folder,
        root,
        &mut budget,
        &mut rects,
        size_mode,
        include_files,
        min_tile_bytes,
    );
    rects
}

/// Разложить детей одной папки и, пока есть бюджет, спуститься в крупные дочерние.
fn expand_folder(
    index: &ScanIndex,
    folder: FolderId,
    rect: Rect,
    budget: &mut usize,
    out: &mut Vec<TreemapRect>,
    size_mode: SizeMode,
    include_files: bool,
    min_tile_bytes: u64,
) {
    if *budget == 0 {
        return;
    }
    let totals = index.folder_totals(folder);
    let parent_size = size_mode.size(totals.size_on_disk, totals.size_logical);
    if parent_size == 0 {
        return;
    }

    let mut items = children(index, folder, size_mode, include_files);
    if items.is_empty() {
        return;
    }
    // Папки и файлы в одном порядке: размер убыв., при равенстве — сначала
    // папки, потом по имени. Так результат не зависит от порядка обхода.
    items.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| right.is_dir.cmp(&left.is_dir))
            .then_with(|| left.name.cmp(&right.name))
    });

    let total_items = items.len();
    // Сумма размеров всех кандидатов: база и для порога видимости, и для
    // «прочего». При включённых файлах она совпадает с итогом папки (дубликаты
    // жёстких ссылок и нулевые размеры не входят ни туда, ни туда), при
    // выключенных — содержит только папки: файлы не подмешиваются в «прочее».
    let item_total: u64 = items.iter().map(|item| item.size).sum();
    // Дети мельче 1 % кандидатов (и всё, что не влезло в бюджет) уходят в
    // «прочее». Порог пользователя (`min_tile_bytes`) — абсолютный: он важнее
    // доли, поэтому настройка «не показывать мельче N» режет и крупных соседей.
    let visible = items
        .iter()
        .take_while(|item| item.size >= min_tile_bytes && is_visible(item.size, item_total))
        .count();
    let mut shown = visible;
    let mut collapses = shown < total_items;
    // «Прочее» не должно быть главной плиткой папки: если в нём остаётся больше
    // половины (например, у папки один крупный ребёнок, а сотни мелких),
    // показываем крупнейших детей отдельными плитками до FALLBACK_TILES. Явный
    // порог отключает эту эвристику: пользователь сам попросил спрятать мелочь.
    if collapses && min_tile_bytes == 0 {
        let shown_size: u64 = items.iter().take(shown).map(|item| item.size).sum();
        let other_size = item_total.saturating_sub(shown_size);
        if other_size.saturating_mul(2) > item_total {
            shown = total_items.min(FALLBACK_TILES).max(shown);
        }
    }
    if shown == 0 && min_tile_bytes == 0 {
        shown = total_items.min(FALLBACK_TILES);
    }
    collapses = shown < total_items;
    if shown + usize::from(collapses) > *budget {
        shown = budget.saturating_sub(1).min(shown);
        collapses = true;
    }
    if shown == 0 && !collapses {
        return;
    }

    let mut layout_items: Vec<TreemapItem> = items.into_iter().take(shown).collect();
    let shown_size: u64 = layout_items.iter().map(|item| item.size).sum();
    let other_size = if collapses {
        item_total.saturating_sub(shown_size)
    } else {
        0
    };
    let other_index = if other_size > 0 {
        let index = layout_items.len();
        layout_items.push(TreemapItem {
            id: 0,
            name: OTHER_NAME.to_string(),
            size: other_size,
            is_dir: false,
        });
        Some(index)
    } else {
        None
    };

    let laid = layout_treemap(&layout_items, rect);
    let mut nested: Vec<(FolderId, Rect)> = Vec::new();
    for (index, tile) in laid {
        if *budget == 0 {
            break;
        }
        if Some(index) == other_index {
            let size = layout_items[index].size;
            push_rect(out, budget, 0, OTHER_NAME.to_string(), size, false, tile);
            continue;
        }
        let item = &layout_items[index];
        let is_dir = item.is_dir;
        if push_rect(
            out,
            budget,
            item.id,
            item.name.clone(),
            item.size,
            is_dir,
            tile,
        ) && is_dir
        {
            nested.push((item.id as FolderId, tile));
        }
    }

    // Крупнейшие папки раскладываются внутри своих плиток, пока есть бюджет.
    for (id, tile) in nested {
        if *budget == 0 {
            break;
        }
        expand_folder(
            index,
            id,
            tile,
            budget,
            out,
            size_mode,
            include_files,
            min_tile_bytes,
        );
    }
}

/// Дети папки одним списком: папки и, если включено, файлы. Дубликаты жёстких
/// ссылок не входят в итоги родителя, поэтому и на карте не показываются.
///
/// Размер элемента и фильтр нулевых — в выбранном режиме: в «фактическом»
/// разрежённый файл может занимать 0 байт на диске, но иметь ненулевой
/// фактический размер, и наоборот.
fn children(
    index: &ScanIndex,
    folder: FolderId,
    size_mode: SizeMode,
    include_files: bool,
) -> Vec<TreemapItem> {
    let mut items = Vec::new();
    for child in index.child_folders(folder) {
        let size = size_mode.size(child.size_on_disk, child.size_logical);
        if size == 0 {
            continue;
        }
        items.push(TreemapItem {
            id: u64::from(child.id),
            name: child.name,
            size,
            is_dir: true,
        });
    }
    if include_files {
        let file_count = index.file_count(folder) as usize;
        if file_count > 0 {
            for row in
                index.child_files(folder, FileSort::SizeOnDisk, true, 0, file_count, size_mode)
            {
                if row.hard_linked {
                    continue;
                }
                let size = size_mode.size(row.size_on_disk, row.size_logical);
                if size == 0 {
                    continue;
                }
                items.push(TreemapItem {
                    id: 0,
                    name: row.name,
                    size,
                    is_dir: false,
                });
            }
        }
    }
    items
}

/// Элемент занимает не меньше 1 % контейнера: только такие показываем отдельно.
fn is_visible(size: u64, parent_size: u64) -> bool {
    size.saturating_mul(SMALL_SHARE) >= parent_size
}

/// Положить плитку в результат, если бюджет ещё не исчерпан.
fn push_rect(
    out: &mut Vec<TreemapRect>,
    budget: &mut usize,
    id: u64,
    name: String,
    size: u64,
    is_dir: bool,
    rect: Rect,
) -> bool {
    if *budget == 0 {
        return false;
    }
    out.push(TreemapRect {
        id,
        name,
        size,
        is_dir,
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    });
    *budget -= 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileKey, FileMeta, SizeMode};
    use crate::scan::index::ScanIndex;

    /// Набор плиток: размеры по убыванию, id — позиция, чётные — «папки».
    fn sample_items(sizes: &[u64]) -> Vec<TreemapItem> {
        sizes
            .iter()
            .enumerate()
            .map(|(position, &size)| TreemapItem {
                id: position as u64,
                name: format!("item-{position}"),
                size,
                is_dir: position % 2 == 0,
            })
            .collect()
    }

    /// Детерминированный генератор: тесты не зависят от `rand` и часов.
    fn pseudo_random(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

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

    /// Файл, у которого занятое место и фактический размер различаются:
    /// сжатие/разрежённые файлы дают разный порядок в двух режимах.
    fn meta_sized(name: &str, on_disk: u64, logical: u64, file_index: u64) -> FileMeta {
        FileMeta {
            name: name.to_string(),
            size_on_disk: on_disk,
            size_logical: logical,
            modified: 0,
            file_key: FileKey {
                volume_serial: 1,
                file_index,
            },
        }
    }

    /// Площадь плитки.
    fn area(rect: Rect) -> f64 {
        rect.w * rect.h
    }

    /// Сумма площадей плиток равна площади контейнера (±1e-9).
    #[test]
    fn layout_covers_container_area() {
        let items = sample_items(&[5000, 3000, 2000, 1200, 800, 500, 250, 120, 60, 30]);
        for container in [
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            Rect {
                x: 0.05,
                y: 0.1,
                w: 0.7,
                h: 0.6,
            },
            Rect {
                x: 0.2,
                y: 0.0,
                w: 0.05,
                h: 0.9,
            },
        ] {
            let rects = layout_treemap(&items, container);
            assert_eq!(
                rects.len(),
                items.len(),
                "каждый элемент должен получить плитку"
            );
            let covered: f64 = rects.iter().map(|(_, rect)| area(*rect)).sum();
            let expected = container.w * container.h;
            assert!(
                (covered - expected).abs() <= 1e-9,
                "площадь {covered} != {expected} для контейнера {container:?}"
            );
        }
    }

    /// Плитки не пересекаются: 50 псевдослучайных наборов.
    #[test]
    fn layout_rects_do_not_overlap() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for round in 0..50 {
            let count = 1 + (pseudo_random(&mut state) % 80) as usize;
            let items: Vec<TreemapItem> = (0..count)
                .map(|position| TreemapItem {
                    id: position as u64,
                    name: format!("item-{position}"),
                    size: 1 + pseudo_random(&mut state) % 1_000_000,
                    is_dir: position % 3 == 0,
                })
                .collect();
            let rects = layout_treemap(
                &items,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
            );
            assert_eq!(rects.len(), count);
            for left in 0..rects.len() {
                for right in left + 1..rects.len() {
                    let a = rects[left].1;
                    let b = rects[right].1;
                    let overlap_w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                    let overlap_h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                    assert!(
                        overlap_w <= 0.0 || overlap_h <= 0.0,
                        "набор {round}: плитки {left} и {right} пересекаются на {overlap_w}x{overlap_h}"
                    );
                }
            }
        }
    }

    /// Два вызова на одном срезе дают побитово одинаковые прямоугольники.
    #[test]
    fn layout_is_deterministic_for_same_snapshot() {
        let items = sample_items(&[10, 10, 7, 7, 3, 1, 1, 1]);
        let container = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.8,
            h: 0.5,
        };
        let first = layout_treemap(&items, container);
        let second = layout_treemap(&items, container);
        assert_eq!(first.len(), second.len());
        for (left, right) in first.iter().zip(second.iter()) {
            assert_eq!(left.0, right.0, "порядок обязан совпасть");
            assert_eq!(left.1.x.to_bits(), right.1.x.to_bits());
            assert_eq!(left.1.y.to_bits(), right.1.y.to_bits());
            assert_eq!(left.1.w.to_bits(), right.1.w.to_bits());
            assert_eq!(left.1.h.to_bits(), right.1.h.to_bits());
        }
    }

    /// Порядок — по убыванию размера, площадь монотонна по размеру и
    /// пропорциональна ему.
    #[test]
    fn layout_orders_by_size_desc() {
        let sizes = [400u64, 300, 200, 100, 50, 25];
        let items = sample_items(&sizes);
        let container = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let rects = layout_treemap(&items, container);

        assert_eq!(rects.first().map(|(index, _)| *index), Some(0));
        let returned: Vec<u64> = rects.iter().map(|(index, _)| items[*index].size).collect();
        assert_eq!(returned, sizes, "порядок обязан быть по убыванию размера");

        let areas: Vec<f64> = rects.iter().map(|(_, rect)| area(*rect)).collect();
        assert!(
            areas.windows(2).all(|pair| pair[0] + 1e-12 >= pair[1]),
            "площади обязаны не расти вместе с порядком: {areas:?}"
        );

        let total: u64 = sizes.iter().sum();
        for (index, rect) in &rects {
            let expected = items[*index].size as f64 / total as f64;
            assert!(
                (area(*rect) - expected).abs() <= 1e-9,
                "площадь {} != доли размера {expected} (элемент {index})",
                area(*rect)
            );
        }
    }

    /// Бюджет 10 на дерево из 100 элементов: крупное — отдельно, мелочь —
    /// одним прямоугольником «прочее» (id = 0).
    #[test]
    fn budget_collapses_small_items() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let big = index.folder(root, "Крупная");
        for position in 0..20u64 {
            index.add_file(
                big,
                meta(
                    &format!("big-{position:02}.bin"),
                    50_000 - position * 1_000,
                    position + 1,
                ),
            );
        }
        for position in 0..99u64 {
            index.add_file(
                root,
                meta(&format!("small-{position:02}.bin"), 100, 100 + position),
            );
        }

        let rects = treemap_for_folder(&index, root, 10, SizeMode::OnDisk, true, 0);

        assert!(rects.len() <= 10, "бюджет 10 превышен: {}", rects.len());
        assert_eq!(
            rects.len(),
            10,
            "бюджет должен быть израсходован до последнего прямоугольника: {rects:?}"
        );

        let big_rect = rects
            .iter()
            .find(|rect| rect.is_dir && rect.id == u64::from(big))
            .expect("крупная папка обязана получить свою плитку");
        assert_eq!(big_rect.name, "Крупная");

        let others: Vec<&TreemapRect> = rects.iter().filter(|rect| rect.name == "прочее").collect();
        assert_eq!(
            others.len(),
            2,
            "по одному «прочее» на уровень (корень и большая папка): {rects:?}"
        );
        assert!(others.iter().all(|rect| rect.id == 0 && !rect.is_dir));
        assert!(
            others.iter().any(|rect| rect.size == 9_900),
            "мелочь корня (99 файлов по 100 байт) собрана в один прямоугольник"
        );

        let nested: Vec<&TreemapRect> = rects
            .iter()
            .filter(|rect| !rect.is_dir && rect.id == 0 && rect.name != "прочее")
            .collect();
        assert_eq!(
            nested.len(),
            7,
            "10 = папка + «прочее» корня + 7 файлов + «прочее» папки: {rects:?}"
        );
        for rect in &nested {
            assert!(
                rect.x + 1e-12 >= big_rect.x
                    && rect.y + 1e-12 >= big_rect.y
                    && rect.x + rect.w <= big_rect.x + big_rect.w + 1e-12
                    && rect.y + rect.h <= big_rect.y + big_rect.h + 1e-12,
                "плитка {} обязана лежать внутри папки: {rect:?} вне {big_rect:?}",
                rect.name
            );
        }

        for rect in &rects {
            assert!(
                rect.x >= -1e-12
                    && rect.y >= -1e-12
                    && rect.x + rect.w <= 1.0 + 1e-12
                    && rect.y + rect.h <= 1.0 + 1e-12,
                "координаты обязаны быть нормированы 0..1: {rect:?}"
            );
        }
    }

    /// Папка из одних мелких детей (все < 1 % родителя): зум внутрь не должен
    /// упираться в единственную серую плитку «прочее» — показываем топ-N детей,
    /// остальное по-прежнему собираем в «прочее».
    #[test]
    fn fallback_shows_top_children_when_all_are_small() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "Мелкая");
        // 150 файлов одного размера: каждый — ~0.67 % родителя, порог 1 % не пройден.
        for position in 0..150u64 {
            index.add_file(
                folder,
                meta(&format!("file-{position:03}.bin"), 4096, position + 1),
            );
        }

        let rects = treemap_for_folder(&index, folder, 200, SizeMode::OnDisk, true, 0);
        let files: Vec<&TreemapRect> = rects
            .iter()
            .filter(|rect| !rect.is_dir && rect.name != "прочее")
            .collect();

        assert_eq!(
            files.len(),
            FALLBACK_TILES,
            "должны показываться топ-{FALLBACK_TILES} файлов, а не одна серая плитка: {rects:?}"
        );
        assert!(files.iter().all(|rect| rect.size == 4096));
        assert!(
            files.iter().all(|rect| rect.w > 0.0 && rect.h > 0.0),
            "у показанных файлов обязана быть площадь"
        );

        let other = rects
            .iter()
            .find(|rect| rect.name == "прочее")
            .expect("остаток файлов обязан собраться в «прочее»");
        assert_eq!(other.size, (150 - FALLBACK_TILES as u64) * 4096);
        assert_eq!(rects.len(), FALLBACK_TILES + 1);

        // Тот же случай с тесным бюджетом: «прочее» резервируется, файлов — на 1 меньше.
        let tight = treemap_for_folder(&index, folder, 10, SizeMode::OnDisk, true, 0);
        assert_eq!(tight.len(), 10, "бюджет 10 превышен: {tight:?}");
        assert_eq!(
            tight
                .iter()
                .filter(|rect| !rect.is_dir && rect.name != "прочее")
                .count(),
            9
        );
    }

    /// Один крупный ребёнок и сотни мелких: «прочее» не должно покрывать больше
    /// половины родителя — крупнейшие мелкие дети показываются отдельными плитками.
    #[test]
    fn fallback_breaks_up_dominant_other() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "Смешанная");
        index.add_file(folder, meta("big.bin", 100_000, 1));
        for position in 0..150u64 {
            index.add_file(
                folder,
                meta(&format!("small-{position:03}.bin"), 2_000, position + 2),
            );
        }

        // 100 000 + 150 * 2 000 = 400 000 Б: крупный файл — 25 % (видим), мелочь —
        // 75 %, и без fallback она стала бы одной плиткой «прочее» на 3/4 карты.
        let rects = treemap_for_folder(&index, folder, 300, SizeMode::OnDisk, true, 0);
        let files: Vec<&TreemapRect> = rects
            .iter()
            .filter(|rect| !rect.is_dir && rect.name != "прочее")
            .collect();
        assert_eq!(
            files.len(),
            FALLBACK_TILES,
            "должны показываться топ-{FALLBACK_TILES} детей (крупный + мелкие)"
        );
        assert_eq!(files[0].name, "big.bin");

        let other = rects
            .iter()
            .find(|rect| rect.name == "прочее")
            .expect("остаток мелочи обязан собраться в «прочее»");
        assert_eq!(
            other.size,
            (150 - (FALLBACK_TILES as u64 - 1)) * 2_000,
            "в «прочее» остаются только дети за пределами топ-N"
        );
        assert!(
            other.size * 2 < 400_000,
            "«прочее» больше не покрывает половину родителя: {} байт",
            other.size
        );
        assert_eq!(rects.len(), FALLBACK_TILES + 1);
    }

    /// Порог мелочи складывает всё, что меньше `min_tile_bytes`, в «прочее»:
    /// крупные элементы остаются отдельными плитками, суммы площадей и итоги
    /// папки при этом не меняются (порог — только отображение).
    #[test]
    fn min_tile_threshold_lumps_small_into_other() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        for position in 0..5u64 {
            index.add_file(
                root,
                meta(&format!("big-{position}.bin"), 1_000, position + 1),
            );
        }
        for position in 0..5u64 {
            index.add_file(
                root,
                meta(&format!("small-{position}.bin"), 100, 100 + position),
            );
        }
        let before = index.folder_totals(root);
        assert_eq!(before.size_on_disk, 5_500);

        let rects = treemap_for_folder(&index, root, 100, SizeMode::OnDisk, true, 500);

        // 5 крупных файлов отдельными плитками + одна сводная «прочее».
        assert_eq!(
            rects.len(),
            6,
            "мелочь обязана собраться в одну плитку: {rects:?}"
        );
        assert_eq!(rects.iter().filter(|rect| rect.name == "прочее").count(), 1);
        let other = rects
            .iter()
            .find(|rect| rect.name == "прочее")
            .expect("сводная плитка обязана быть");
        assert_eq!(other.size, 500, "в «прочее» — ровно 5 файлов по 100 Б");
        assert!(!other.is_dir && other.id == 0);
        let bigs: Vec<&TreemapRect> = rects
            .iter()
            .filter(|rect| rect.name.starts_with("big-"))
            .collect();
        assert_eq!(bigs.len(), 5);
        assert!(bigs.iter().all(|rect| rect.size == 1_000));

        // Сумма площадей плиток — весь контейнер, включая «прочее».
        let covered: f64 = rects.iter().map(|rect| rect.w * rect.h).sum();
        assert!(
            (covered - 1.0).abs() <= 1e-9,
            "площадь карты {covered} != 1.0: порог не должен терять байты"
        );
        // Сумма размеров плиток верхнего уровня — итог папки: отображение не
        // переписывает данные.
        let shown: u64 = rects.iter().map(|rect| rect.size).sum();
        assert_eq!(
            shown, before.size_on_disk,
            "сумма плиток обязана совпасть с итогом папки"
        );

        // Без порога те же 10 элементов видны отдельно: контраст доказывает,
        // что дело именно в пороге.
        let without = treemap_for_folder(&index, root, 100, SizeMode::OnDisk, true, 0);
        assert_eq!(without.len(), 10);

        // Итоги после раскладки не изменились ни на байт.
        let after = index.folder_totals(root);
        assert_eq!(after.size_on_disk, before.size_on_disk);
        assert_eq!(after.size_logical, before.size_logical);
        assert_eq!(after.file_count, before.file_count);
        assert_eq!(after.folder_count, before.folder_count);
    }

    /// «Файлы на карте» выключены — в выдаче только папки; файлы не подмешиваются
    /// даже в «прочее», а карта по-прежнему покрывает контейнер.
    #[test]
    fn include_files_false_shows_only_folders() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let docs = index.folder(root, "Документы");
        index.add_file(docs, meta("a.bin", 2_000, 1));
        index.add_file(docs, meta("b.bin", 1_000, 2));
        for position in 0..3u64 {
            index.add_file(
                root,
                meta(&format!("root-{position}.bin"), 900, 10 + position),
            );
        }
        let before = index.folder_totals(root);

        let with_files = treemap_for_folder(&index, root, 100, SizeMode::OnDisk, true, 0);
        assert!(
            with_files
                .iter()
                .any(|rect| !rect.is_dir && rect.name == "root-0.bin"),
            "с включёнными файлами они обязаны быть на карте: {with_files:?}"
        );

        let folders_only = treemap_for_folder(&index, root, 100, SizeMode::OnDisk, false, 0);
        assert!(
            folders_only.iter().all(|rect| rect.is_dir),
            "при include_files=false на карте только папки: {folders_only:?}"
        );
        assert!(
            folders_only
                .iter()
                .any(|rect| rect.is_dir && rect.id == u64::from(docs)),
            "папка обязана остаться: {folders_only:?}"
        );
        assert!(
            folders_only.iter().all(|rect| rect.name != "прочее"),
            "файлы не должны попадать даже в «прочее»: {folders_only:?}"
        );
        let covered: f64 = folders_only.iter().map(|rect| rect.w * rect.h).sum();
        assert!(
            (covered - 1.0).abs() <= 1e-9,
            "карта обязана покрыть контейнер: {covered}"
        );

        let after = index.folder_totals(root);
        assert_eq!(after.size_on_disk, before.size_on_disk);
        assert_eq!(after.file_count, before.file_count);
    }

    /// Режим размера меняет раскладку: с «на диске» первой идёт одна плитка,
    /// с «фактическим» — другая, и площадь каждой соответствует выбранному размеру.
    #[test]
    fn size_mode_logical_changes_layout() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let folder = index.folder(root, "Смешанная");
        index.add_file(folder, meta_sized("a.bin", 100, 900, 1));
        index.add_file(folder, meta_sized("b.bin", 900, 100, 2));
        let before = index.folder_totals(folder);
        assert_eq!(before.size_on_disk, 1_000);
        assert_eq!(before.size_logical, 1_000);

        let disk = treemap_for_folder(&index, folder, 10, SizeMode::OnDisk, true, 0);
        assert_eq!(disk[0].name, "b.bin", "на диске крупнее b.bin: {disk:?}");
        assert_eq!(disk[0].size, 900);
        assert!((disk[0].w * disk[0].h - 0.9).abs() <= 1e-9);
        assert_eq!(disk[1].name, "a.bin");
        assert_eq!(disk[1].size, 100);

        let logical = treemap_for_folder(&index, folder, 10, SizeMode::Logical, true, 0);
        assert_eq!(
            logical[0].name, "a.bin",
            "фактически крупнее a.bin: {logical:?}"
        );
        assert_eq!(logical[0].size, 900);
        assert!((logical[0].w * logical[0].h - 0.9).abs() <= 1e-9);
        assert_eq!(logical[1].name, "b.bin");
        assert_eq!(logical[1].size, 100);

        // Раскладка — только отображение: итоги папки не пересчитаны.
        let after = index.folder_totals(folder);
        assert_eq!(after.size_on_disk, before.size_on_disk);
        assert_eq!(after.size_logical, before.size_logical);
        assert_eq!(after.file_count, before.file_count);
    }

    /// Дубликат жёсткой ссылки не входит в итоги папки, поэтому и на карте его
    /// быть не должно: иначе площадь плиток перестала бы соответствовать
    /// занятому месту.
    #[test]
    fn treemap_ignores_hard_link_duplicates() {
        let mut index = ScanIndex::new("C:");
        let root = index.root();
        let links = index.folder(root, "links");
        index.add_file(links, meta("first.bin", 4096, 7));
        index.add_file(links, meta("alias.bin", 4096, 7));

        let rects = treemap_for_folder(&index, root, 10, SizeMode::OnDisk, true, 0);
        assert_eq!(rects.len(), 2, "папка + один учтённый файл: {rects:?}");
        assert!(rects[0].is_dir);
        let file = rects
            .iter()
            .find(|rect| !rect.is_dir)
            .expect("файл обязан быть");
        assert_eq!(file.name, "first.bin");
        assert_eq!(file.size, 4096);
    }
}
