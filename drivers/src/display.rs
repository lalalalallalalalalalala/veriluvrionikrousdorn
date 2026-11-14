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
    draw_y: usize,
    columns: usize,
    rows: usize,
    column: usize,
    row: usize,
    fg: u32,
    bg: u32,
}

unsafe impl Send for Display {}

static DISPLAY: Mutex<Option<Display>> = Mutex::new(None);

/// `int 0x80` идёт через interrupt gate: обработчик сискалла крутится с
/// закрытыми прерываниями. Если стол уже держит консоль и был вытеснен,
/// обычный spin не отдаст процессор. При занятости открываем прерывания
/// и ждём тика, чтобы держатель успел отпустить замок.
fn lock_display() -> spin::MutexGuard<'static, Option<Display>> {
    loop {
        if let Some(guard) = DISPLAY.try_lock() {
            return guard;
        }
        vlr_arch::enable_interrupts();
        core::hint::spin_loop();
    }
}

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
        draw_y: 0,
        columns,
        rows,
        column: 0,
        row: 0,
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
    };
    display.clear();
    *lock_display() = Some(display);
    crate::devices::add("fb-display");
    if bga_present() {
        crate::devices::add("bga-display");
        crate::serial::write_str("bga-ok\n");
        if let Some((w, h)) = read_edid_mode() {
            NATIVE.store(((w as u32) << 16) | h as u32, Ordering::Relaxed);
            crate::serial::write_str("edid-ok\n");
        }
    }
}

pub fn is_ready() -> bool {
    lock_display().is_some()
}

pub fn set_color(rgb: u32) {
    if let Some(display) = lock_display().as_mut() {
        display.fg = rgb;
    }
}

pub fn clear() {
    if let Some(display) = lock_display().as_mut() {
        display.clear();
    }
}

pub fn write_str(text: &str) {
    if !TEXT_ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some(display) = lock_display().as_mut() {
        for byte in text.bytes() {
            display.write_byte(byte);
        }
    }
}

pub fn write_byte(byte: u8) {
    if let Some(display) = lock_display().as_mut() {
        display.write_byte(byte);
    }
}

/// Не блокироваться, если консоль уже занята (обработчик паники).
pub fn size() -> Option<(usize, usize)> {
    lock_display().as_ref().map(|display| (display.width, display.height))
}

pub fn fill_rect(x: usize, y: usize, w: usize, h: usize, rgb: u32) {
    if let Some(display) = lock_display().as_mut() {
        display.fill_rect(x, y, w, h, rgb);
    }
}

pub fn blit_rgb(x: usize, y: usize, w: usize, h: usize, rgb: &[u8]) {
    if let Some(display) = lock_display().as_mut() {
        display.blit_rgb(x, y, w, h, rgb);
    }
}

pub fn blit_solid(x: usize, y: usize, w: usize, h: usize, pixels: &[u32], scale: usize) {
    if let Some(display) = lock_display().as_mut() {
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

pub fn glyph_char(x: usize, y: usize, ch: char, fg: u32, bg: u32, scale: usize) {
    let bits = crate::cyr::bits(ch).unwrap_or_else(|| {
        if ch.is_ascii() {
            FONT8X8[ch as usize]
        } else {
            FONT8X8[b'?' as usize]
        }
    });
    if let Some(display) = lock_display().as_mut() {
        display.paint_bits(x, y, &bits, fg, bg, scale.max(1));
    }
}

pub fn glyph(x: usize, y: usize, ch: u8, fg: u32, bg: u32, scale: usize) {
    if let Some(display) = lock_display().as_mut() {
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

    fn paint_bits(&mut self, x: usize, y: usize, glyph: &[u8; 8], fg: u32, bg: u32, scale: usize) {
        for gy in 0..FONT_H {
            let bits = glyph[gy];
            for gx in 0..FONT_W {
                let on = bits & (1 << gx) != 0;
                let color = if on { fg } else { bg };
                let px = x + gx * scale;
                let py = y + gy * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        self.put_pixel(px + dx, py + dy, color);
                    }
                }
            }
        }
    }

    fn glyph(&mut self, x: usize, y: usize, ch: u8, fg: u32, bg: u32, scale: usize) {
        let glyph = FONT8X8[ch.min(127) as usize];
        self.paint_bits(x, y, &glyph, fg, bg, scale);
    }

    fn put_pixel(&mut self, x: usize, y: usize, rgb: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let y = y + self.draw_y;
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
            if self.bytes_per_pixel > 3 {
                pixel.add(3).write_volatile(0);
            }
        }
    }
}

const MODES: [(u16, u16); 5] = [(640, 480), (800, 600), (1024, 768), (1280, 720), (1280, 1024)];
static NATIVE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FLIP: AtomicBool = AtomicBool::new(false);

pub fn mode_list() -> &'static [(u16, u16)] { &MODES }

pub fn native_mode() -> Option<(u16, u16)> {
    let packed = NATIVE.load(Ordering::Relaxed);
    if packed == 0 { None } else { Some(((packed >> 16) as u16, packed as u16)) }
}

pub fn describe(buf: &mut [u8]) -> usize {
    let (w, h) = size().unwrap_or((0, 0));
    let mut text = [0u8; 160];
    let mut at = 0usize;
    write_raw(&mut text, &mut at, b"mode ");
    write_num(&mut text, &mut at, w as u32);
    write_raw(&mut text, &mut at, b"x");
    write_num(&mut text, &mut at, h as u32);
    write_raw(&mut text, &mut at, b"\nmodes");
    for (mw, mh) in MODES {
        write_raw(&mut text, &mut at, b" ");
        write_num(&mut text, &mut at, mw as u32);
        write_raw(&mut text, &mut at, b"x");
        write_num(&mut text, &mut at, mh as u32);
    }
    if let Some((nw, nh)) = native_mode() {
        write_raw(&mut text, &mut at, b"\nnative ");
        write_num(&mut text, &mut at, nw as u32);
        write_raw(&mut text, &mut at, b"x");
        write_num(&mut text, &mut at, nh as u32);
    }
    write_raw(&mut text, &mut at, b"\n");
    let n = at.min(buf.len());
    buf[..n].copy_from_slice(&text[..n]);
    n
}

pub fn set_mode(width: u16, height: u16) -> bool {
    if !MODES.contains(&(width, height)) || !bga_present() {
        crate::serial::write_str("mode unchanged\n");
        return false;
    }
    bga_write(4, 0);
    bga_write(1, width);
    bga_write(2, height);
    bga_write(3, 32);
    bga_write(6, width);
    bga_write(7, height);
    bga_write(9, 0);
    bga_write(4, 0x41);
    if let Some(display) = lock_display().as_mut() {
        display.width = width as usize;
        display.height = height as usize;
        display.stride = width as usize;
        display.bytes_per_pixel = 4;
        display.bgr = true;
        display.draw_y = 0;
        display.columns = display.width / CELL_W;
        display.rows = display.height / CELL_H;
        display.len = display.stride.saturating_mul(display.height).saturating_mul(4);
        display.clear();
    }
    FLIP.store(false, Ordering::Relaxed);
    let label = match (width, height) {
        (640, 480) => "640x480",
        (800, 600) => "800x600",
        (1024, 768) => "1024x768",
        (1280, 720) => "1280x720",
        _ => "1280x1024",
    };
    crate::serial::write_str("mode ");
    crate::serial::write_str(label);
    crate::serial::write_str("\n");
    true
}

/// Скопировать видимый кадр в задний буфер перед частичной перерисовкой.
pub fn sync_back() {
    if !FLIP.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = lock_display();
    let Some(display) = guard.as_mut() else { return };
    let screen = display.stride.saturating_mul(display.height).saturating_mul(display.bytes_per_pixel);
    if screen == 0 || display.len < screen.saturating_mul(2) {
        return;
    }
    let back = display.draw_y.saturating_mul(display.stride).saturating_mul(display.bytes_per_pixel);
    let front = if display.draw_y == 0 { screen } else { 0 };
    unsafe {
        core::ptr::copy_nonoverlapping(display.ptr.add(front), display.ptr.add(back), screen);
    }
}

pub fn present() {
    if !FLIP.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = lock_display();
    let Some(display) = guard.as_mut() else { return };
    let showing_back = display.draw_y == 0;
    let visible = if showing_back { 0 } else { display.height as u16 };
    bga_write(9, visible);
    display.draw_y = if showing_back { display.height } else { 0 };
}

pub fn enable_flip() {
    let mut guard = lock_display();
    let Some(display) = guard.as_mut() else { return };
    if !bga_present() {
        return;
    }
    let screen = display.stride * display.height * display.bytes_per_pixel;
    let pages = (screen + 4095) / 4096;
    let Some(phys0) = vlr_memory::translate(display.ptr as u64) else { return };
    let virt0 = display.ptr as u64;
    for page in 0..pages {
        let virt = virt0 + screen as u64 + page as u64 * 4096;
        let phys = phys0 + screen as u64 + page as u64 * 4096;
        if vlr_memory::map_4k(virt, phys, true).is_err() && vlr_memory::translate(virt).is_none() {
            return;
        }
    }
    let mem = bga_read(10) as usize * 64 * 1024;
    if mem < screen.saturating_mul(2) {
        return;
    }
    bga_write(7, (display.height as u16).saturating_mul(2));
    display.len = screen.saturating_mul(2);
    FLIP.store(true, Ordering::Relaxed);
    crate::devices::add("double-buffer");
}

fn bga_present() -> bool {
    let id = bga_read(0);
    (0xB0C0..=0xB0C5).contains(&id)
}

fn bga_write(index: u16, value: u16) {
    use x86_64::instructions::port::Port;
    unsafe {
        Port::<u16>::new(0x01CE).write(index);
        Port::<u16>::new(0x01CF).write(value);
    }
}

fn bga_read(index: u16) -> u16 {
    use x86_64::instructions::port::Port;
    unsafe {
        Port::<u16>::new(0x01CE).write(index);
        Port::<u16>::new(0x01CF).read()
    }
}

fn write_raw(buf: &mut [u8], at: &mut usize, bytes: &[u8]) {
    for byte in bytes {
        if *at >= buf.len() { return; }
        buf[*at] = *byte;
        *at += 1;
    }
}

fn write_num(buf: &mut [u8], at: &mut usize, mut n: u32) {
    let mut tmp = [0u8; 10];
    let mut i = tmp.len();
    if n == 0 { write_raw(buf, at, b"0"); return; }
    while n > 0 {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    write_raw(buf, at, &tmp[i..]);
}

fn read_edid_mode() -> Option<(u16, u16)> {
    use x86_64::instructions::port::Port;
    unsafe { Port::<u16>::new(0x510).write(0x19); }
    let mut countb = [0u8; 4];
    for byte in &mut countb {
        *byte = unsafe { Port::<u8>::new(0x511).read() };
    }
    let count = u32::from_be_bytes(countb);
    if count == 0 || count > 256 { return None; }
    let mut select = 0u16;
    for _ in 0..count {
        let mut entry = [0u8; 64];
        for byte in &mut entry {
            *byte = unsafe { Port::<u8>::new(0x511).read() };
        }
        if entry[8..].windows(8).any(|w| w == b"etc/edid") {
            select = u16::from_be_bytes([entry[4], entry[5]]);
            break;
        }
    }
    if select == 0 { return None; }
    unsafe { Port::<u16>::new(0x510).write(select); }
    let mut edid = [0u8; 128];
    for byte in &mut edid {
        *byte = unsafe { Port::<u8>::new(0x511).read() };
    }
    if edid[0] != 0 || edid[1] != 0xFF { return None; }
    let h = (edid[56] as u16) | (((edid[58] as u16) & 0xF0) << 4);
    let v = (edid[59] as u16) | (((edid[61] as u16) & 0xF0) << 4);
    if h < 640 || v < 480 { None } else { Some((h, v)) }
}
