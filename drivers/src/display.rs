//! Текстовый вывод в линейный framebuffer, который отдаёт загрузчик.
//!
//! Драйвер не зависит от `bootloader_api`: ядро передаёт уже разобранное
//! описание буфера. Символы рисуются шрифтом 8x8 с масштабом 2.

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use crate::font::FONT8X8;

static TEXT_ON: AtomicBool = AtomicBool::new(true);

/// Когда рабочий стол рисует сам, текстовая консоль не затирает кадр.
pub fn set_text_enabled(enabled: bool) {
    TEXT_ON.store(enabled, Ordering::Relaxed);
}

const SCALE: usize = 2;
const FONT_W: usize = 8;
const FONT_H: usize = 8;
const CELL_W: usize = FONT_W * SCALE;
const CELL_H: usize = FONT_H * SCALE;

const DEFAULT_FG: u32 = 0x00E7E2D6;
const DEFAULT_BG: u32 = 0x00111620;

pub struct FramebufferConfig {
    pub ptr: *mut u8,
    pub len: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub bgr: bool,
}

struct Display {
    ptr: *mut u8,
    len: usize,
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    bgr: bool,
    columns: usize,
    rows: usize,
    column: usize,
    row: usize,
    fg: u32,
    bg: u32,
}

unsafe impl Send for Display {}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);

pub fn init(config: FramebufferConfig) {
    if config.ptr.is_null() || config.bytes_per_pixel < 3 || config.width == 0 || config.height == 0
    {
        return;
    }
    let columns = config.width / CELL_W;
    let rows = config.height / CELL_H;
    if columns == 0 || rows == 0 {
        return;
    }
    let mut display = Display {
        ptr: config.ptr,
        len: config.len,
        width: config.width,
        height: config.height,
        stride: config.stride,
        bytes_per_pixel: config.bytes_per_pixel,
        bgr: config.bgr,
        columns,
        rows,
        column: 0,
        row: 0,
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
    };
    display.clear();
    *DISPLAY.lock() = Some(display);
}

pub fn is_ready() -> bool {
    DISPLAY.lock().is_some()
}

pub fn set_color(rgb: u32) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.fg = rgb;
    }
}

pub fn clear() {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.clear();
    }
}

pub fn write_str(text: &str) {
    if !TEXT_ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some(display) = DISPLAY.lock().as_mut() {
        for byte in text.bytes() {
            display.write_byte(byte);
        }
    }
}

pub fn write_byte(byte: u8) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.write_byte(byte);
    }
}

/// Не блокироваться, если консоль уже занята (обработчик паники).
pub fn size() -> Option<(usize, usize)> {
    DISPLAY.lock().as_ref().map(|display| (display.width, display.height))
}

pub fn fill_rect(x: usize, y: usize, w: usize, h: usize, rgb: u32) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.fill_rect(x, y, w, h, rgb);
    }
}

pub fn blit_rgb(x: usize, y: usize, w: usize, h: usize, rgb: &[u8]) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.blit_rgb(x, y, w, h, rgb);
    }
}

pub fn blit_solid(x: usize, y: usize, w: usize, h: usize, pixels: &[u32], scale: usize) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        let scale = scale.max(1);
        for row in 0..h {
            for col in 0..w {
                let color = pixels.get(row * w + col).copied().unwrap_or(0);
                if color == 0 {
                    continue;
                }
                display.fill_rect(x + col * scale, y + row * scale, scale, scale, color);
            }
        }
    }
}

pub fn glyph(x: usize, y: usize, ch: u8, fg: u32, bg: u32, scale: usize) {
    if let Some(display) = DISPLAY.lock().as_mut() {
        display.glyph(x, y, ch, fg, bg, scale.max(1));
    }
}

pub fn try_write_str(text: &str) -> bool {
    let Some(mut guard) = DISPLAY.try_lock() else {
        return false;
    };
    if let Some(display) = guard.as_mut() {
        for byte in text.bytes() {
            display.write_byte(byte);
        }
    }
    true
}

impl Display {
    fn clear(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.put_pixel(x, y, self.bg);
            }
        }
        self.column = 0;
        self.row = 0;
    }

    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.column = 0,
            0x08 => self.backspace(),
            byte => {
                if self.column >= self.columns {
                    self.newline();
                }
                let glyph = if byte < 128 { byte } else { b'?' };
                self.draw_glyph(self.column, self.row, glyph);
                self.column += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.column = 0;
        if self.row + 1 >= self.rows {
            self.scroll();
        } else {
            self.row += 1;
        }
    }

    fn backspace(&mut self) {
        if self.column == 0 {
            if self.row == 0 {
                return;
            }
            self.row -= 1;
            self.column = self.columns;
        }
        self.column -= 1;
        self.draw_glyph(self.column, self.row, b' ');
    }

    fn scroll(&mut self) {
        let row_bytes = CELL_H
            .saturating_mul(self.stride)
            .saturating_mul(self.bytes_per_pixel);
        let visible_rows_bytes = self.rows.saturating_mul(row_bytes);
        if row_bytes == 0 || visible_rows_bytes > self.len || row_bytes >= self.len {
            self.row = 0;
            self.clear();
            return;
        }
        unsafe {
            core::ptr::copy(
                self.ptr.add(row_bytes),
                self.ptr,
                visible_rows_bytes - row_bytes,
            );
        }
        let y0 = (self.rows - 1) * CELL_H;
        for y in y0..self.height.min(y0 + CELL_H) {
            for x in 0..self.width {
                self.put_pixel(x, y, self.bg);
            }
        }
    }

    fn draw_glyph(&mut self, column: usize, row: usize, ch: u8) {
        let glyph = FONT8X8[ch as usize];
        let origin_x = column * CELL_W;
        let origin_y = row * CELL_H;
        for gy in 0..FONT_H {
            let bits = glyph[gy];
            for gx in 0..FONT_W {
                // Младший бит — левый пиксель (договорённость font8x8).
                let on = bits & (1 << gx) != 0;
                let color = if on { self.fg } else { self.bg };
                let px = origin_x + gx * SCALE;
                let py = origin_y + gy * SCALE;
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        self.put_pixel(px + dx, py + dy, color);
                    }
                }
            }
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: u32) {
        for row in y..y.saturating_add(h).min(self.height) {
            for col in x..x.saturating_add(w).min(self.width) {
                self.put_pixel(col, row, rgb);
            }
        }
    }

    fn blit_rgb(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: &[u8]) {
        for row in 0..h {
            for col in 0..w {
                let i = (row * w + col) * 3;
                if i + 2 >= rgb.len() {
                    return;
                }
                let color = ((rgb[i] as u32) << 16) | ((rgb[i + 1] as u32) << 8) | rgb[i + 2] as u32;
                self.put_pixel(x + col, y + row, color);
            }
        }
    }

    fn glyph(&mut self, x: usize, y: usize, ch: u8, fg: u32, bg: u32, scale: usize) {
        let glyph = FONT8X8[ch.min(127) as usize];
        for gy in 0..FONT_H {
            let bits = glyph[gy];
            for gx in 0..FONT_W {
                let on = bits & (1 << gx) != 0;
                let color = if on { fg } else { bg };
                let px = x + gx * scale;
                let py = y + gy * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        if color != 0 || bg != 0 {
                            self.put_pixel(px + dx, py + dy, color);
                        }
                    }
                }
            }
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, rgb: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.stride + x) * self.bytes_per_pixel;
        if offset + self.bytes_per_pixel > self.len {
            return;
        }
        let red = (rgb >> 16) as u8;
        let green = (rgb >> 8) as u8;
        let blue = rgb as u8;
        let (p0, p1, p2) = if self.bgr {
            (blue, green, red)
        } else {
            (red, green, blue)
        };
        unsafe {
            let pixel = self.ptr.add(offset);
            pixel.write_volatile(p0);
            pixel.add(1).write_volatile(p1);
            pixel.add(2).write_volatile(p2);
        }
    }
}
