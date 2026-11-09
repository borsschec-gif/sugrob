//! Папки на рабочем столе: чистая логика из `DesktopIcons.swift`.
use super::Rect;

/// Контур верха иконки из её прозрачности. Всё в долях стороны картинки.
#[derive(Clone, Debug, PartialEq)]
pub struct IconShape {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    /// Для каждого столбца от левого края контура: насколько верх ниже самой высокой точки.
    pub drops: Vec<f32>,
}

/// Строит контур по альфа-каналу, построчно сверху вниз. Порог тот же, что на Mac.
pub fn icon_shape(alpha: &[u8], width: usize, height: usize) -> Option<IconShape> {
    if width == 0 || height == 0 || alpha.len() < width * height {
        return None;
    }
    let tops: Vec<Option<usize>> = (0..width).map(|x| (0..height).find(|y| alpha[y * width + x] > 128)).collect();
    let first = tops.iter().position(|t| t.is_some())?;
    let last = tops.iter().rposition(|t| t.is_some())?;
    let highest = tops.iter().flatten().copied().min()?;
    if last <= first {
        return None;
    }
    let (w, h) = (width as f32, height as f32);
    let drops = tops[first..=last].iter().map(|t| (t.unwrap_or(highest) - highest) as f32 / h).collect();
    Some(IconShape { left: first as f32 / w, top: highest as f32 / h, width: (last - first + 1) as f32 / w, drops })
}

impl IconShape {
    /// Рамка снега и профиль для картинки в прямоугольнике `image`:
    /// индекс профиля — точки вдоль ширины, значение — точки ниже самой высокой.
    pub fn place(&self, image: Rect) -> (Rect, Vec<f32>) {
        let frame = Rect {
            x: image.x + self.left * image.w,
            y: image.y + self.top * image.h,
            w: self.width * image.w,
            h: image.h * (1.0 - self.top),
        };
        let count = frame.w.ceil().max(1.0) as usize + 1;
        let columns = self.drops.len();
        let profile = (0..count)
            .map(|point| {
                let column = (point as f32 / frame.w.max(1.0) * columns as f32) as usize;
                self.drops.get(column.min(columns.saturating_sub(1))).copied().unwrap_or(0.0) * image.h
            })
            .collect();
        (frame, profile)
    }
}

/// Снег держат только настоящие папки: не архивы, не файлы и не ярлыки (как на Mac).
pub fn holds_folder_snow(is_folder: bool, is_filesystem: bool, is_stream: bool, is_link: bool) -> bool {
    is_folder && is_filesystem && !is_stream && !is_link
}

/// Устойчивый номер по пути (FNV-1a). Старшие биты заняты: HWND в них не попадает.
pub fn desktop_icon_id(path: &str) -> u64 {
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    hash & 0x0FFF_FFFF_FFFF_FFFF | 0xE000_0000_0000_0000
}

/// Система сообщает размер значка либо в пикселях, либо в единицах 96 dpi. Какой из двух
/// на самом деле, видно по самому короткому элементу: значок плюс одна строка подписи.
pub fn icon_side(reported: f32, scale: f32, shortest_item: f32, label_height: f32) -> f32 {
    let estimate = shortest_item - label_height;
    let scaled = reported * scale;
    if (scaled - estimate).abs() < (reported - estimate).abs() {
        scaled
    } else {
        reported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Папка 8×6: ушко слева выше, справа верх ниже.
    fn folder_alpha() -> Vec<u8> {
        let rows = ["........", ".###....", ".######.", ".######.", ".######.", "........"];
        rows.iter().flat_map(|r| r.bytes().map(|b| if b == b'#' { 255 } else { 0 })).collect()
    }

    #[test]
    fn shape_finds_silhouette() {
        let shape = icon_shape(&folder_alpha(), 8, 6).unwrap();
        assert_eq!(shape.left, 1.0 / 8.0);
        assert_eq!(shape.top, 1.0 / 6.0);
        assert_eq!(shape.width, 6.0 / 8.0);
        assert_eq!(shape.drops.len(), 6);
        assert_eq!(shape.drops[0], 0.0);
        assert_eq!(shape.drops[5], 1.0 / 6.0);
    }

    #[test]
    fn empty_or_broken_bitmap_gives_nothing() {
        assert_eq!(icon_shape(&[0; 48], 8, 6), None);
        assert_eq!(icon_shape(&[255; 10], 8, 6), None);
        assert_eq!(icon_shape(&[], 0, 0), None);
    }

    #[test]
    fn soft_edge_does_not_count() {
        let mut alpha = folder_alpha();
        alpha[0] = 128;
        assert_eq!(icon_shape(&alpha, 8, 6).unwrap().left, 1.0 / 8.0);
    }

    #[test]
    fn place_converts_to_points() {
        let shape = icon_shape(&folder_alpha(), 8, 6).unwrap();
        let (frame, profile) = shape.place(Rect { x: 100.0, y: 200.0, w: 48.0, h: 48.0 });
        assert_eq!(frame, Rect { x: 106.0, y: 208.0, w: 36.0, h: 40.0 });
        assert_eq!(profile.len(), 37);
        assert_eq!(profile[0], 0.0);
        // Правая часть контура ниже левой на одну строку картинки: 8 точек.
        assert_eq!(profile[35], 8.0);
        assert_eq!(profile[36], 8.0);
    }

    #[test]
    fn only_real_folders_hold_snow() {
        assert!(holds_folder_snow(true, true, false, false));
        assert!(!holds_folder_snow(true, true, true, false), "архив");
        assert!(!holds_folder_snow(false, true, true, false), "файл");
        assert!(!holds_folder_snow(true, true, false, true), "ярлык");
        assert!(!holds_folder_snow(true, false, false, false), "корзина и прочие виртуальные");
    }

    #[test]
    fn icon_id_is_stable_and_far_from_hwnd() {
        let id = desktop_icon_id(r"C:\Users\me\Desktop\Папка");
        assert_eq!(id, desktop_icon_id(r"C:\Users\me\Desktop\Папка"));
        assert_ne!(id, desktop_icon_id(r"C:\Users\me\Desktop\Другая"));
        assert!(id >> 60 == 0xE && id > u32::MAX as u64);
    }

    #[test]
    fn side_picks_nearest_to_measured() {
        // 150 %: система отдаёт 48, элемент 72 + строка.
        assert_eq!(icon_side(48.0, 1.5, 100.0, 28.0), 72.0);
        // 150 %: система уже отдаёт пиксели.
        assert_eq!(icon_side(72.0, 1.5, 100.0, 28.0), 72.0);
        assert_eq!(icon_side(48.0, 1.0, 70.0, 20.0), 48.0);
    }
}
