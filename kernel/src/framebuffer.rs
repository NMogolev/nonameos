// =============================================================================
// NoNameOS — Framebuffer Graphics
// =============================================================================
//
// Линейный framebuffer, полученный от GRUB через Multiboot2.
// Формат пикселей: BGRA (32 bpp) — стандарт для VBE/VESA.
//
// API:
//   init(info)          — инициализация из Multiboot2 FramebufferInfo
//   put_pixel(x, y, c) — нарисовать пиксель
//   fill_rect(...)      — залить прямоугольник
//   draw_rect(...)      — рамка прямоугольника
//   draw_hline / vline  — горизонтальная / вертикальная линия
//   clear(color)        — залить весь экран
//   width() / height()  — размеры экрана
//
// Цвета задаются как u32 в формате 0x00RRGGBB.
// Внутри конвертируются в BGRA порядок для framebuffer.
//
// =============================================================================

use crate::multiboot2::FramebufferInfo;
use spin::Mutex;

// ---- Глобальное состояние ----

static FB: Mutex<Option<Fb>> = Mutex::new(None);

struct Fb {
    addr: *mut u8,
    pitch: u32,
    width: u32,
    height: u32,
    bpp: u8,
}

// SAFETY: Framebuffer — физическая память, доступ только через Mutex.
unsafe impl Send for Fb {}

// ---- Цвета (0x00RRGGBB) ----

pub const BLACK: u32       = 0x00000000;
pub const WHITE: u32       = 0x00FFFFFF;
pub const RED: u32         = 0x00FF0000;
pub const GREEN: u32       = 0x0000FF00;
pub const BLUE: u32        = 0x000000FF;
pub const CYAN: u32        = 0x0000FFFF;
pub const MAGENTA: u32     = 0x00FF00FF;
pub const YELLOW: u32      = 0x00FFFF00;
pub const DARK_GRAY: u32   = 0x00404040;
pub const LIGHT_GRAY: u32  = 0x00C0C0C0;
pub const DARK_BLUE: u32   = 0x00000080;
pub const DARK_CYAN: u32   = 0x00008080;
pub const ORANGE: u32      = 0x00FF8000;

// Тёмная тема
pub const BG_DARK: u32     = 0x001E1E2E;  // Catppuccin-like background
pub const FG_LIGHT: u32    = 0x00CDD6F4;  // Catppuccin text
pub const ACCENT: u32      = 0x0089B4FA;  // Catppuccin blue
pub const SURFACE: u32     = 0x00313244;  // Catppuccin surface
pub const OVERLAY: u32     = 0x006C7086;  // Catppuccin overlay

// ---- Инициализация ----

/// Инициализировать framebuffer из Multiboot2 информации.
pub fn init(info: &FramebufferInfo) {
    let fb = Fb {
        addr: info.addr as *mut u8,
        pitch: info.pitch,
        width: info.width,
        height: info.height,
        bpp: info.bpp,
    };

    *FB.lock() = Some(fb);
}

/// Проверить, инициализирован ли framebuffer.
pub fn is_available() -> bool {
    FB.lock().is_some()
}

/// Ширина экрана в пикселях.
pub fn width() -> u32 {
    FB.lock().as_ref().map(|fb| fb.width).unwrap_or(0)
}

/// Высота экрана в пикселях.
pub fn height() -> u32 {
    FB.lock().as_ref().map(|fb| fb.height).unwrap_or(0)
}

// ---- Примитивы рисования ----

/// Нарисовать один пиксель. Цвет: 0x00RRGGBB.
#[inline]
pub fn put_pixel(x: u32, y: u32, color: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        if x >= fb.width || y >= fb.height {
            return;
        }
        unsafe {
            let offset = (y * fb.pitch + x * (fb.bpp as u32 / 8)) as isize;
            let pixel = fb.addr.offset(offset);
            // BGRA format
            *pixel = (color & 0xFF) as u8;             // B
            *pixel.offset(1) = ((color >> 8) & 0xFF) as u8;  // G
            *pixel.offset(2) = ((color >> 16) & 0xFF) as u8; // R
            // Alpha byte (offset 3) не трогаем
        }
    }
}

/// Залить весь экран одним цветом.
pub fn clear(color: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let b = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let r = ((color >> 16) & 0xFF) as u8;
        let bytes_pp = fb.bpp as u32 / 8;

        for y in 0..fb.height {
            for x in 0..fb.width {
                unsafe {
                    let offset = (y * fb.pitch + x * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = b;
                    *pixel.offset(1) = g;
                    *pixel.offset(2) = r;
                }
            }
        }
    }
}

/// Залить прямоугольник.
pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let b = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let r = ((color >> 16) & 0xFF) as u8;
        let bytes_pp = fb.bpp as u32 / 8;

        let x_end = (x + w).min(fb.width);
        let y_end = (y + h).min(fb.height);

        for py in y..y_end {
            for px in x..x_end {
                unsafe {
                    let offset = (py * fb.pitch + px * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = b;
                    *pixel.offset(1) = g;
                    *pixel.offset(2) = r;
                }
            }
        }
    }
}

/// Нарисовать рамку прямоугольника (не заливая внутри).
pub fn draw_rect(x: u32, y: u32, w: u32, h: u32, color: u32, thickness: u32) {
    if w == 0 || h == 0 { return; }
    let t = thickness;

    // Верхняя линия
    fill_rect(x, y, w, t, color);
    // Нижняя линия
    if h > t {
        fill_rect(x, y + h - t, w, t, color);
    }
    // Левая линия
    fill_rect(x, y + t, t, h.saturating_sub(t * 2), color);
    // Правая линия
    if w > t {
        fill_rect(x + w - t, y + t, t, h.saturating_sub(t * 2), color);
    }
}

/// Горизонтальная линия.
pub fn draw_hline(x: u32, y: u32, length: u32, color: u32) {
    fill_rect(x, y, length, 1, color);
}

/// Вертикальная линия.
pub fn draw_vline(x: u32, y: u32, length: u32, color: u32) {
    fill_rect(x, y, 1, length, color);
}

/// Горизонтальный градиент (от color_left до color_right).
pub fn fill_gradient_h(x: u32, y: u32, w: u32, h: u32, color_left: u32, color_right: u32) {
    if w == 0 { return; }

    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let bytes_pp = fb.bpp as u32 / 8;
        let y_end = (y + h).min(fb.height);
        let x_end = (x + w).min(fb.width);

        let r1 = ((color_left >> 16) & 0xFF) as i32;
        let g1 = ((color_left >> 8) & 0xFF) as i32;
        let b1 = (color_left & 0xFF) as i32;
        let r2 = ((color_right >> 16) & 0xFF) as i32;
        let g2 = ((color_right >> 8) & 0xFF) as i32;
        let b2 = (color_right & 0xFF) as i32;

        for py in y..y_end {
            for px in x..x_end {
                let t = (px - x) as i32;
                let total = w as i32;
                let r = (r1 + (r2 - r1) * t / total) as u8;
                let g = (g1 + (g2 - g1) * t / total) as u8;
                let b = (b1 + (b2 - b1) * t / total) as u8;

                unsafe {
                    let offset = (py * fb.pitch + px * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = b;
                    *pixel.offset(1) = g;
                    *pixel.offset(2) = r;
                }
            }
        }
    }
}

/// Вертикальный градиент (от color_top до color_bottom).
pub fn fill_gradient_v(x: u32, y: u32, w: u32, h: u32, color_top: u32, color_bottom: u32) {
    if h == 0 { return; }

    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let bytes_pp = fb.bpp as u32 / 8;
        let y_end = (y + h).min(fb.height);
        let x_end = (x + w).min(fb.width);

        let r1 = ((color_top >> 16) & 0xFF) as i32;
        let g1 = ((color_top >> 8) & 0xFF) as i32;
        let b1 = (color_top & 0xFF) as i32;
        let r2 = ((color_bottom >> 16) & 0xFF) as i32;
        let g2 = ((color_bottom >> 8) & 0xFF) as i32;
        let b2 = (color_bottom & 0xFF) as i32;

        for py in y..y_end {
            let t = (py - y) as i32;
            let total = h as i32;
            let r = (r1 + (r2 - r1) * t / total) as u8;
            let g = (g1 + (g2 - g1) * t / total) as u8;
            let b = (b1 + (b2 - b1) * t / total) as u8;

            for px in x..x_end {
                unsafe {
                    let offset = (py * fb.pitch + px * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = b;
                    *pixel.offset(1) = g;
                    *pixel.offset(2) = r;
                }
            }
        }
    }
}

// ---- Встроенный bitmap шрифт 8x16 (CP437 subset) ----

/// Встроенный шрифт 8x16 для ASCII 32-126.
/// Каждый символ — 16 байт (16 строк по 8 бит).
static FONT_8X16: &[u8] = include_bytes!("font8x16.raw");

/// Ширина символа в пикселях.
pub const CHAR_WIDTH: u32 = 8;

/// Высота символа в пикселях.
pub const CHAR_HEIGHT: u32 = 16;

/// Нарисовать один символ ASCII.
pub fn draw_char(x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        // Индекс в font data (ASCII 32..127)
        let idx = if ch >= 32 && ch < 128 {
            (ch - 32) as usize
        } else {
            0 // пробел для неизвестных
        };

        let glyph_offset = idx * CHAR_HEIGHT as usize;
        if glyph_offset + CHAR_HEIGHT as usize > FONT_8X16.len() {
            return;
        }

        let bytes_pp = fb.bpp as u32 / 8;
        let fg_b = (fg & 0xFF) as u8;
        let fg_g = ((fg >> 8) & 0xFF) as u8;
        let fg_r = ((fg >> 16) & 0xFF) as u8;
        let bg_b = (bg & 0xFF) as u8;
        let bg_g = ((bg >> 8) & 0xFF) as u8;
        let bg_r = ((bg >> 16) & 0xFF) as u8;

        for row in 0..CHAR_HEIGHT {
            let py = y + row;
            if py >= fb.height { break; }

            let bits = FONT_8X16[glyph_offset + row as usize];

            for col in 0..CHAR_WIDTH {
                let px = x + col;
                if px >= fb.width { break; }

                let is_fg = (bits >> (7 - col)) & 1 != 0;
                let (r, g, b) = if is_fg {
                    (fg_r, fg_g, fg_b)
                } else {
                    (bg_r, bg_g, bg_b)
                };

                unsafe {
                    let offset = (py * fb.pitch + px * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = b;
                    *pixel.offset(1) = g;
                    *pixel.offset(2) = r;
                }
            }
        }
    }
}

/// Нарисовать символ с прозрачным фоном (только foreground пиксели).
pub fn draw_char_transparent(x: u32, y: u32, ch: u8, fg: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let idx = if ch >= 32 && ch < 128 {
            (ch - 32) as usize
        } else {
            0
        };

        let glyph_offset = idx * CHAR_HEIGHT as usize;
        if glyph_offset + CHAR_HEIGHT as usize > FONT_8X16.len() {
            return;
        }

        let bytes_pp = fb.bpp as u32 / 8;
        let fg_b = (fg & 0xFF) as u8;
        let fg_g = ((fg >> 8) & 0xFF) as u8;
        let fg_r = ((fg >> 16) & 0xFF) as u8;

        for row in 0..CHAR_HEIGHT {
            let py = y + row;
            if py >= fb.height { break; }

            let bits = FONT_8X16[glyph_offset + row as usize];

            for col in 0..CHAR_WIDTH {
                if (bits >> (7 - col)) & 1 == 0 { continue; }

                let px = x + col;
                if px >= fb.width { break; }

                unsafe {
                    let offset = (py * fb.pitch + px * bytes_pp) as isize;
                    let pixel = fb.addr.offset(offset);
                    *pixel = fg_b;
                    *pixel.offset(1) = fg_g;
                    *pixel.offset(2) = fg_r;
                }
            }
        }
    }
}

/// Нарисовать строку (ASCII).
pub fn draw_string(x: u32, y: u32, s: &str, fg: u32, bg: u32) {
    let mut cx = x;
    for byte in s.bytes() {
        if byte == b'\n' {
            // Переводы строк не обрабатываем тут — на уровне консоли
            continue;
        }
        draw_char(cx, y, byte, fg, bg);
        cx += CHAR_WIDTH;
    }
}

/// Нарисовать строку с прозрачным фоном.
pub fn draw_string_transparent(x: u32, y: u32, s: &str, fg: u32) {
    let mut cx = x;
    for byte in s.bytes() {
        if byte == b'\n' { continue; }
        draw_char_transparent(cx, y, byte, fg);
        cx += CHAR_WIDTH;
    }
}

// ---- Утилиты ----

/// Смешать два цвета (alpha blend). alpha: 0..255 (0 = c1, 255 = c2).
pub fn blend(c1: u32, c2: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv = 255 - a;

    let r = (((c1 >> 16) & 0xFF) * inv + ((c2 >> 16) & 0xFF) * a) / 255;
    let g = (((c1 >> 8) & 0xFF) * inv + ((c2 >> 8) & 0xFF) * a) / 255;
    let b = ((c1 & 0xFF) * inv + (c2 & 0xFF) * a) / 255;

    (r << 16) | (g << 8) | b
}

/// Создать цвет из RGB компонентов.
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
