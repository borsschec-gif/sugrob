//! Окна подарка и фразы. Оверлей клики пропускает, поэтому подарок живёт в своём окне.
//! Всё в главном потоке: сообщения разбирает кадровый цикл, в том числе на паузе.
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use sugrob_core::sim::{blend_phrase_text, make_gift_image, make_phrase_panel, phrase_origin, v2, Rect, GIFT_SIZE, PHRASE_PADDING, V2};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Log;

const GIFT_CLASS: PCWSTR = w!("SugrobGift");
const PHRASE_CLASS: PCWSTR = w!("SugrobPhrase");
const HOLD_TIMER: usize = 1;
const FADE_TIMER: usize = 2;
const HOLD_MS: u32 = 3500;
const FADE_MS: u32 = 16;
/// Шагов около 25 за 16 мс: те же 0,4 с, что на Mac.
const FADE_STEP: isize = 10;

static GIFT_CLICKED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn gift_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Подарок открывается с первого клика и не забирает фокус.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            GIFT_CLICKED.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn blend(alpha: u8) -> BLENDFUNCTION {
    BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 }
}

unsafe extern "system" fn phrase_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg != WM_TIMER {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if wparam.0 == HOLD_TIMER {
        let _ = KillTimer(Some(hwnd), HOLD_TIMER);
        if SetTimer(Some(hwnd), FADE_TIMER, FADE_MS, None) == 0 {
            let _ = DestroyWindow(hwnd);
        }
    } else if wparam.0 == FADE_TIMER {
        let alpha = GetWindowLongPtrW(hwnd, GWLP_USERDATA) - FADE_STEP;
        if alpha <= 0 {
            let _ = KillTimer(Some(hwnd), FADE_TIMER);
            let _ = DestroyWindow(hwnd);
        } else {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, alpha);
            let _ = UpdateLayeredWindow(hwnd, None, None, None, None, None, COLORREF(0), Some(&blend(alpha as u8)), ULW_ALPHA);
        }
    }
    LRESULT(0)
}

/// Заливает окно готовыми пикселями BGRA с премультиплицированной альфой.
unsafe fn present(hwnd: HWND, pixels: &[u8], width: i32, height: i32, position: POINT) -> Result<()> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return Err(windows::core::Error::from_win32());
    }
    let result = (|| {
        let (bitmap, bits) = make_dib(dc, width, height)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits, pixels.len().min((width * height * 4) as usize));
        let previous = SelectObject(dc, bitmap.into());
        let size = SIZE { cx: width, cy: height };
        let origin = POINT::default();
        let done = UpdateLayeredWindow(hwnd, None, Some(&position), Some(&size), Some(dc), Some(&origin), COLORREF(0), Some(&blend(255)), ULW_ALPHA);
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        done
    })();
    let _ = DeleteDC(dc);
    result
}

unsafe fn make_dib(dc: HDC, width: i32, height: i32) -> Result<(HBITMAP, *mut u8)> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    if bits.is_null() {
        let _ = DeleteObject(bitmap.into());
        return Err(windows::core::Error::new(E_FAIL, "пустой DIB"));
    }
    Ok((bitmap, bits as *mut u8))
}

/// Georgia Bold Italic 19 pt, как на Mac. Без шрифта GDI подставит похожий.
unsafe fn phrase_font(scale: f32) -> HFONT {
    CreateFontW(
        -(19.0 * scale).round() as i32,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
        1,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        (DEFAULT_PITCH.0 | FF_ROMAN.0) as u32,
        w!("Georgia"),
    )
}

/// Плашка с текстом в пикселях. Буквы рисует GDI белым по чёрному, яркость становится покрытием.
unsafe fn render_phrase(text: &[u16], scale: f32) -> Result<(Vec<u8>, i32, i32)> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return Err(windows::core::Error::from_win32());
    }
    let font = phrase_font(scale);
    let old_font = SelectObject(dc, font.into());
    let result = (|| {
        let mut extent = SIZE::default();
        if !GetTextExtentPoint32W(dc, text, &mut extent).as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        let padding = PHRASE_PADDING * scale;
        let width = extent.cx + (padding * 2.0).round() as i32;
        let height = extent.cy + padding.round() as i32;
        let mut panel = make_phrase_panel(width as u32, height as u32, scale).ok_or_else(|| windows::core::Error::new(E_FAIL, "плашка не нарисовалась"))?;

        let (bitmap, bits) = make_dib(dc, width, height)?;
        let previous = SelectObject(dc, bitmap.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let drawn = TextOutW(dc, padding.round() as i32, (padding / 2.0).round() as i32, text).as_bool();
        let _ = GdiFlush();
        let area = (width * height) as usize;
        let coverage: Vec<u8> = std::slice::from_raw_parts(bits as *const u8, area * 4).chunks_exact(4).map(|p| p[1]).collect();
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        if !drawn {
            return Err(windows::core::Error::new(E_FAIL, "текст не нарисовался"));
        }
        blend_phrase_text(&mut panel, &coverage);
        Ok((panel, width, height))
    })();
    SelectObject(dc, old_font);
    let _ = DeleteObject(font.into());
    let _ = DeleteDC(dc);
    result
}

struct GiftWindow {
    hwnd: HWND,
    scale: f32,
    position: POINT,
}

impl Drop for GiftWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn to_pixels(point: V2, scale: f32) -> POINT {
    POINT { x: (point.x * scale).round() as i32, y: (point.y * scale).round() as i32 }
}

unsafe fn create_gift(position: POINT, scale: f32) -> Result<GiftWindow> {
    let side = (GIFT_SIZE * scale).round() as i32;
    let pixels = make_gift_image(side as u32).ok_or_else(|| windows::core::Error::new(E_FAIL, "подарок не нарисовался"))?;
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        GIFT_CLASS,
        w!("Sugrob"),
        WS_POPUP,
        position.x,
        position.y,
        side,
        side,
        None,
        None,
        Some(GetModuleHandleW(None)?.into()),
        None,
    )?;
    let window = GiftWindow { hwnd, scale, position };
    present(hwnd, &pixels, side, side, position)?;
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    Ok(window)
}

unsafe fn create_phrase(text: &str, center: V2, screen: Rect, scale: f32) -> Result<HWND> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let (pixels, width, height) = render_phrase(&wide, scale)?;
    let origin = phrase_origin(center, v2(width as f32 / scale, height as f32 / scale), screen);
    let position = to_pixels(origin, scale);
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        PHRASE_CLASS,
        w!("Sugrob"),
        WS_POPUP,
        position.x,
        position.y,
        width,
        height,
        None,
        None,
        Some(GetModuleHandleW(None)?.into()),
        None,
    )?;
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 255);
    // Без таймера окно осталось бы навсегда, поэтому лучше без фразы.
    if let Err(error) = present(hwnd, &pixels, width, height, position) {
        let _ = DestroyWindow(hwnd);
        return Err(error);
    }
    if SetTimer(Some(hwnd), HOLD_TIMER, HOLD_MS, None) == 0 {
        let _ = DestroyWindow(hwnd);
        return Err(windows::core::Error::new(E_FAIL, "таймер фразы не создан"));
    }
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    Ok(hwnd)
}

pub struct GiftLayer {
    gift: Option<GiftWindow>,
    /// Окна гаснут сами; список нужен, чтобы держать их над оверлеем.
    phrases: Vec<HWND>,
    failed: bool,
}

impl GiftLayer {
    pub fn new() -> Self {
        unsafe {
            if let Ok(instance) = GetModuleHandleW(None) {
                let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
                RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(gift_proc), hInstance: instance.into(), lpszClassName: GIFT_CLASS, hCursor: cursor, ..Default::default() });
                RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(phrase_proc), hInstance: instance.into(), lpszClassName: PHRASE_CLASS, hCursor: cursor, ..Default::default() });
            }
        }
        GiftLayer { gift: None, phrases: Vec::new(), failed: false }
    }

    /// Держит окно на месте подарка. Двигает только когда он правда сдвинулся.
    pub fn sync(&mut self, position: Option<V2>, scale: f32, log: &mut Log) {
        let Some(position) = position else {
            self.gift = None;
            self.failed = false;
            return;
        };
        let point = to_pixels(position, scale);
        if self.gift.as_ref().is_some_and(|g| g.scale != scale) {
            self.gift = None;
        }
        match &mut self.gift {
            Some(gift) => {
                if gift.position != point {
                    gift.position = point;
                    unsafe {
                        let _ = SetWindowPos(gift.hwnd, None, point.x, point.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
            }
            // Одна запись в лог и никаких новых попыток на каждом кадре.
            None if !self.failed => match unsafe { create_gift(point, scale) } {
                Ok(gift) => self.gift = Some(gift),
                Err(error) => {
                    self.failed = true;
                    log.write(&format!("santa: окно подарка: {error}"));
                }
            },
            None => {}
        }
    }

    pub fn take_click(&self) -> bool {
        GIFT_CLICKED.swap(false, Ordering::Relaxed)
    }

    pub fn keep_on_top(&mut self) {
        let raise = |hwnd: HWND| unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        };
        if let Some(gift) = &self.gift {
            raise(gift.hwnd);
        }
        self.phrases.retain(|&hwnd| unsafe { IsWindow(Some(hwnd)) }.as_bool());
        self.phrases.iter().copied().for_each(raise);
    }

    pub fn show_phrase(&mut self, text: &str, center: V2, screen: Rect, scale: f32, log: &mut Log) {
        match unsafe { create_phrase(text, center, screen, scale) } {
            Ok(hwnd) => self.phrases.push(hwnd),
            Err(error) => log.write(&format!("santa: фраза: {error}")),
        }
    }
}
