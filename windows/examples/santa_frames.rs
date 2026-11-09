//! Сохраняет спрайты в PNG, чтобы посмотреть их без Windows.
//! cargo run --release --example santa_frames -- <папка>
use sugrob_core::sim::{make_gift_image, make_phrase_panel, make_santa_frame, SANTA_FRAME_COUNT};
use tiny_skia::{ColorU8, Pixmap, PremultipliedColorU8};

fn save(path: &str, bgra: &[u8], width: u32, height: u32, background: Option<[u8; 3]>) -> Result<(), String> {
    let mut pixmap = Pixmap::new(width, height).ok_or("пустой размер")?;
    for (out, p) in pixmap.pixels_mut().iter_mut().zip(bgra.chunks_exact(4)) {
        let (b, g, r, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
        let over = |top: u32, back: u8| (top + back as u32 * (255 - a) / 255).min(255) as u8;
        let color = match background {
            Some(bg) => ColorU8::from_rgba(over(r, bg[0]), over(g, bg[1]), over(b, bg[2]), 255).premultiply(),
            None => PremultipliedColorU8::from_rgba(r as u8, g as u8, b as u8, a as u8).ok_or("альфа не премультиплицирована")?,
        };
        *out = color;
    }
    pixmap.save_png(path).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let dir = std::env::args().nth(1).ok_or("нужна папка")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let night = Some([38, 52, 92]);
    for frame in 0..SANTA_FRAME_COUNT {
        let (pixels, w, h) = make_santa_frame(frame, 2).ok_or("кадр")?;
        save(&format!("{dir}/santa_{frame}.png"), &pixels, w, h, None)?;
        save(&format!("{dir}/santa_{frame}_night.png"), &pixels, w, h, night)?;
    }
    let side = 176;
    let gift = make_gift_image(side).ok_or("подарок")?;
    save(&format!("{dir}/gift.png"), &gift, side, side, None)?;
    save(&format!("{dir}/gift_night.png"), &gift, side, side, Some([200, 210, 225]))?;
    let (w, h) = (640, 76);
    let panel = make_phrase_panel(w, h, 2.0).ok_or("плашка")?;
    save(&format!("{dir}/phrase_panel.png"), &panel, w, h, Some([200, 210, 225]))?;
    Ok(())
}
