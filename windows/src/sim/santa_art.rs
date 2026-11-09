//! Перенос `SantaArt.swift`: спрайты, нарисованные кодом. Тройка едет влево.
//! Координаты те же, что на Mac: начало внизу слева, y вверх.
use std::f32::consts::PI;

use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub const SANTA_WIDTH: f32 = 210.0;
pub const SANTA_HEIGHT: f32 = 76.0;
pub const SANTA_FRAME_COUNT: usize = 4;

/// Цвет из компонент sRGB, а не из байтов: так записано в оригинале.
#[derive(Clone, Copy)]
struct Rgb(f32, f32, f32, f32);

const fn rgb(r: f32, g: f32, b: f32) -> Rgb {
    Rgb(r, g, b, 1.0)
}

struct Canvas {
    pixmap: Pixmap,
    transform: Transform,
}

impl Canvas {
    fn new(width: u32, height: u32, scale: f32) -> Option<Self> {
        let pixmap = Pixmap::new(width, height)?;
        // Переворот по y: рисуем как в CoreGraphics, первая строка остаётся верхом.
        Some(Canvas { pixmap, transform: Transform::from_row(scale, 0.0, 0.0, -scale, 0.0, height as f32) })
    }

    fn paint(color: Rgb) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba(color.0, color.1, color.2, color.3).unwrap_or(Color::BLACK));
        paint
    }

    fn fill(&mut self, path: Option<Path>, color: Rgb) {
        if let Some(path) = path {
            self.pixmap.fill_path(&path, &Self::paint(color), FillRule::Winding, self.transform, None);
        }
    }

    fn stroke(&mut self, path: Option<Path>, color: Rgb, width: f32) {
        if let Some(path) = path {
            let stroke = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
            self.pixmap.stroke_path(&path, &Self::paint(color), &stroke, self.transform, None);
        }
    }

    fn fill_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.fill(Rect::from_xywh(x, y, w, h).and_then(PathBuilder::from_oval), color);
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.fill(Rect::from_xywh(x, y, w, h).map(PathBuilder::from_rect), color);
    }

    fn fill_rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgb) {
        self.fill(rounded(x, y, w, h, r), color);
    }

    /// Пиксели BGRA с премультиплицированной альфой, первая строка — верх картинки.
    fn into_bgra(self) -> Vec<u8> {
        let mut pixels = self.pixmap.take();
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        pixels
    }
}

fn polyline(points: &[(f32, f32)]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut b = PathBuilder::new();
    b.move_to(first.0, first.1);
    for p in rest {
        b.line_to(p.0, p.1);
    }
    b.finish()
}

fn polygon(points: &[(f32, f32)]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut b = PathBuilder::new();
    b.move_to(first.0, first.1);
    for p in rest {
        b.line_to(p.0, p.1);
    }
    b.close();
    b.finish()
}

fn quad(from: (f32, f32), control: (f32, f32), to: (f32, f32)) -> Option<Path> {
    let mut b = PathBuilder::new();
    b.move_to(from.0, from.1);
    b.quad_to(control.0, control.1, to.0, to.1);
    b.finish()
}

/// Дуга против часовой в координатах с y вверх, кубическими кусками не больше четверти круга.
fn arc(cx: f32, cy: f32, r: f32, start: f32, end: f32) -> Option<Path> {
    let segments = ((end - start).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = (end - start) / segments as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut b = PathBuilder::new();
    b.move_to(cx + r * start.cos(), cy + r * start.sin());
    for i in 0..segments {
        let (a0, a1) = (start + step * i as f32, start + step * (i + 1) as f32);
        let (c0, s0, c1, s1) = (a0.cos(), a0.sin(), a1.cos(), a1.sin());
        b.cubic_to(cx + r * (c0 - k * s0), cy + r * (s0 + k * c0), cx + r * (c1 + k * s1), cy + r * (s1 - k * c1), cx + r * c1, cy + r * s1);
    }
    b.finish()
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    const K: f32 = 0.552_284_8;
    let (right, top, c) = (x + w, y + h, r * (1.0 - K));
    let mut b = PathBuilder::new();
    b.move_to(x + r, y);
    b.line_to(right - r, y);
    b.cubic_to(right - c, y, right, y + c, right, y + r);
    b.line_to(right, top - r);
    b.cubic_to(right, top - c, right - c, top, right - r, top);
    b.line_to(x + r, top);
    b.cubic_to(x + c, top, x, top - c, x, top - r);
    b.line_to(x, y + r);
    b.cubic_to(x, y + c, x + c, y, x + r, y);
    b.close();
    b.finish()
}

const FUR_BLUE: Rgb = rgb(0.17, 0.35, 0.70);
const WHITE: Rgb = rgb(0.96, 0.97, 0.99);
const GOLD: Rgb = rgb(0.95, 0.76, 0.26);

/// BGRA с премультиплицированной альфой, первая строка — верх картинки.
pub fn make_santa_frame(frame: usize, scale: u32) -> Option<(Vec<u8>, u32, u32)> {
    let (width, height) = (SANTA_WIDTH as u32 * scale, SANTA_HEIGHT as u32 * scale);
    let mut canvas = Canvas::new(width, height, scale as f32)?;
    draw(&mut canvas, frame);
    Some((canvas.into_bgra(), width, height))
}

fn draw(c: &mut Canvas, frame: usize) {
    let phase = frame as f32 / SANTA_FRAME_COUNT as f32 * 2.0 * PI;

    // Дальние лошади темнее и выше: получается тройка в глубину.
    draw_horse(c, 22.0, 12.0, phase + 2.1, rgb(0.27, 0.17, 0.12), rgb(0.10, 0.07, 0.06));
    draw_horse(c, 30.0, 8.0, phase + 1.0, rgb(0.36, 0.23, 0.15), rgb(0.12, 0.08, 0.07));
    draw_horse(c, 14.0, 3.0, phase, rgb(0.50, 0.31, 0.19), rgb(0.14, 0.09, 0.07));

    // Дуга с колокольчиком над центральной лошадью.
    c.stroke(arc(52.0, 40.0, 15.0, 0.15, PI - 0.15), rgb(0.78, 0.16, 0.14), 3.2);
    c.fill_ellipse(48.5, 44.0 + phase.sin() * 1.2, 7.0, 7.0, GOLD);

    // Вожжи.
    c.stroke(quad((42.0, 39.0), (90.0, 26.0), (134.0, 37.0)), rgb(0.20, 0.13, 0.09), 1.0);

    draw_sleigh(c);
    draw_grandfather(c, (phase * 2.0).sin() * 0.7);
}

fn draw_horse(c: &mut Canvas, x: f32, y: f32, phase: f32, body: Rgb, mane: Rgb) {
    // Ноги: галоп по двум парам в противофазе.
    for (hip, offset) in [(9.0, 0.0), (15.0, 0.7), (37.0, PI), (43.0, PI + 0.7)] {
        let swing = (phase + offset).sin() * 7.0;
        let knee = (x + hip + swing * 0.7, y + 9.0);
        c.stroke(polyline(&[(x + hip, y + 20.0), knee, (knee.0 + swing * 0.5, y + 1.0 + swing.abs() * 0.25)]), body, 3.4);
    }

    c.fill_ellipse(x + 3.0, y + 15.0, 46.0, 18.0, body);

    // Шея и морда смотрят влево.
    c.fill(polygon(&[(x + 13.0, y + 31.0), (x + 3.0, y + 47.0), (x + 12.0, y + 50.0), (x + 23.0, y + 29.0)]), body);
    c.fill_ellipse(x - 9.0, y + 41.0, 20.0, 10.0, body);
    c.fill(polygon(&[(x + 5.0, y + 50.0), (x + 8.0, y + 57.0), (x + 10.0, y + 49.0)]), body);

    // Грива и хвост.
    c.stroke(quad((x + 11.0, y + 51.0), (x + 21.0, y + 44.0), (x + 23.0, y + 31.0)), mane, 3.0);
    c.stroke(quad((x + 48.0, y + 29.0), (x + 58.0, y + 29.0), (x + 59.0, y + 12.0 + phase.sin() * 3.0)), mane, 3.6);

    // Глаз.
    c.fill_ellipse(x - 1.0, y + 46.0, 2.2, 2.2, rgb(0.05, 0.04, 0.04));
}

fn draw_sleigh(c: &mut Canvas) {
    let runner = rgb(0.93, 0.73, 0.25);

    // Полоз с завитком впереди.
    let mut b = PathBuilder::new();
    b.move_to(196.0, 6.0);
    b.line_to(112.0, 6.0);
    b.cubic_to(94.0, 6.0, 88.0, 18.0, 96.0, 28.0);
    c.stroke(b.finish(), runner, 3.0);
    for x in [128.0, 178.0] {
        c.stroke(polyline(&[(x, 6.0), (x, 15.0)]), runner, 2.2);
    }

    // Короб.
    let mut b = PathBuilder::new();
    b.move_to(112.0, 14.0);
    b.line_to(192.0, 14.0);
    b.cubic_to(200.0, 22.0, 202.0, 36.0, 198.0, 46.0);
    b.line_to(180.0, 38.0);
    b.line_to(128.0, 36.0);
    b.cubic_to(116.0, 36.0, 108.0, 26.0, 112.0, 14.0);
    b.close();
    c.fill(b.finish(), rgb(0.70, 0.13, 0.13));

    c.stroke(polyline(&[(120.0, 20.0), (190.0, 20.0)]), GOLD, 1.8);
}

fn draw_grandfather(c: &mut Canvas, bob: f32) {
    // Мешок.
    c.fill_ellipse(164.0, 34.0, 34.0, 32.0, rgb(0.77, 0.18, 0.16));
    c.stroke(polyline(&[(174.0, 63.0), (188.0, 62.0)]), GOLD, 2.2);

    // Шуба.
    let mut b = PathBuilder::new();
    b.move_to(134.0, 34.0);
    b.cubic_to(136.0, 58.0 + bob, 161.0, 60.0 + bob, 165.0, 34.0 + bob);
    b.close();
    c.fill(b.finish(), FUR_BLUE);

    // Меховая оторочка и рука с вожжами.
    c.fill_rect(133.0, 33.0, 33.0, 4.0, rgb(0.95, 0.96, 0.98));
    c.stroke(polyline(&[(146.0, 47.0 + bob), (134.0, 39.0)]), FUR_BLUE, 5.5);

    // Лицо, борода, шапка.
    let head = 51.0 + bob;
    c.fill_ellipse(140.0, head, 16.0, 15.0, rgb(0.95, 0.76, 0.64));
    let mut b = PathBuilder::new();
    b.move_to(137.0, head + 7.0);
    b.cubic_to(134.0, head - 14.0, 156.0, head - 12.0, 157.0, head + 7.0);
    b.close();
    c.fill(b.finish(), WHITE);
    c.fill_ellipse(139.0, head + 9.0, 19.0, 13.0, FUR_BLUE);
    c.fill_rect(138.5, head + 9.0, 20.0, 3.6, WHITE);
    c.fill_ellipse(137.8, head + 5.5, 4.0, 4.0, rgb(0.84, 0.36, 0.34));

    // Посох с навершием.
    c.stroke(polyline(&[(160.0, 24.0), (169.0, 72.0)]), rgb(0.93, 0.76, 0.32), 2.2);
    c.fill_ellipse(166.0, 69.0, 7.0, 7.0, rgb(0.72, 0.87, 1.0));
}

/// Подарок для окна, которое принимает клик: квадрат со стороной `side` пикселей.
pub fn make_gift_image(side: u32) -> Option<Vec<u8>> {
    let mut c = Canvas::new(side, side, side as f32 / 44.0)?;
    c.fill_rounded(7.0, 4.0, 30.0, 25.0, 3.0, rgb(0.78, 0.15, 0.15));
    c.fill_rounded(4.0, 24.0, 36.0, 9.0, 2.5, rgb(0.89, 0.20, 0.19));
    let ribbon = rgb(0.96, 0.78, 0.28);
    c.fill_rect(19.0, 4.0, 6.0, 29.0, ribbon);
    c.fill_ellipse(10.0, 31.0, 12.0, 9.0, ribbon);
    c.fill_ellipse(22.0, 31.0, 12.0, 9.0, ribbon);
    Some(c.into_bgra())
}

/// Фон плашки с фразой. Размер в пикселях, радиус 14 pt.
pub fn make_phrase_panel(width: u32, height: u32, scale: f32) -> Option<Vec<u8>> {
    let mut c = Canvas::new(width, height, scale)?;
    c.fill_rounded(0.0, 0.0, width as f32 / scale, height as f32 / scale, 14.0, Rgb(0.09, 0.13, 0.27, 0.94));
    Some(c.into_bgra())
}

/// Цвет текста фразы, байты BGR.
const PHRASE_TEXT: [u8; 3] = [209, 242, 255];

/// Кладёт текст на плашку: `coverage` — яркость белых букв на чёрном, по байту на пиксель.
pub fn blend_phrase_text(panel: &mut [u8], coverage: &[u8]) {
    for (pixel, &a) in panel.chunks_exact_mut(4).zip(coverage) {
        let a = a as u32;
        for (channel, &source) in pixel.iter_mut().zip(&PHRASE_TEXT) {
            *channel = ((source as u32 * a + *channel as u32 * (255 - a)) / 255) as u8;
        }
        pixel[3] = ((255 * a + pixel[3] as u32 * (255 - a)) / 255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], width: u32, x: u32, y: u32) -> u8 {
        pixels[(y * width + x) as usize * 4 + 3]
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y * width + x) as usize * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn frames_have_mac_size_and_differ() {
        let frames: Vec<_> = (0..SANTA_FRAME_COUNT).filter_map(|f| make_santa_frame(f, 2)).collect();
        assert_eq!(frames.len(), 4);
        for (pixels, w, h) in &frames {
            assert_eq!((*w, *h), (420, 152));
            assert_eq!(pixels.len(), 420 * 152 * 4);
        }
        assert!(frames[0].0 != frames[1].0 && frames[1].0 != frames[2].0);
    }

    #[test]
    fn pixels_are_premultiplied() {
        let (pixels, _, _) = make_santa_frame(0, 2).unwrap_or_default();
        assert!(pixels.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn first_row_is_the_top_of_the_picture() {
        let (pixels, w, _) = make_santa_frame(0, 2).unwrap_or_default();
        // Навершие посоха вверху: центр (169.5, 72.5), y вверх, спрайт 76 pt.
        let top = pixel(&pixels, w, 339, (76.0f32 - 72.5) as u32 * 2);
        assert_eq!(top[3], 255);
        assert!(top[0] > top[2], "голубое навершие, BGRA: {top:?}");
        // В левом верхнем углу пусто.
        assert_eq!(alpha(&pixels, w, 2, 2), 0);
    }

    #[test]
    fn gift_is_drawn_inside_its_square() {
        let side = 88;
        let pixels = make_gift_image(side).unwrap_or_default();
        assert_eq!(pixels.len(), 88 * 88 * 4);
        assert_eq!(alpha(&pixels, side, 44, 60), 255);
        assert_eq!(alpha(&pixels, side, 1, 1), 0);
    }

    #[test]
    fn text_replaces_panel_where_covered() {
        let mut panel = vec![10, 20, 30, 240, 10, 20, 30, 240];
        blend_phrase_text(&mut panel, &[255, 0]);
        assert_eq!(panel[..4], [209, 242, 255, 255]);
        assert_eq!(panel[4..], [10, 20, 30, 240]);
    }
}
