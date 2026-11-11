//! Рабочий стол: обои, полоска, окна и курсор.
//!
//! Текстовая консоль при этом уходит в окно терминала и в serial, чтобы
//! безголовые проверки по-прежнему видели приглашение `vch`.

#![no_std]

extern crate alloc;

mod logo;
mod term;
mod tile;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use alloc::string::String;
use alloc::vec::Vec;

static LAST_MARK: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_STAMP: AtomicU32 = AtomicU32::new(u32::MAX);
static NEED_WINDOWS: AtomicBool = AtomicBool::new(true);

const BAR_H: usize = 40;
const TITLE_H: usize = 28;
const BAR: u32 = 0x008E2450;
const CARD: u32 = 0x00FFF7F8;
const INK: u32 = 0x002A1420;
const TITLE: u32 = 0x00C4235A;

struct Win {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &'static str,
    kind: u8,
    alive: bool,
}

struct Desk {
    user: [u8; 24],
    user_len: usize,
    windows: [Win; 3],
    drag: Option<usize>,
    grab_x: i32,
    grab_y: i32,
    left: bool,
    apps: Vec<String>,
}

static mut DESK: Desk = Desk {
    user: [0; 24],
    user_len: 0,
    windows: [
        Win { x: 70, y: 70, w: 620, h: 400, title: "vch", kind: 1, alive: true },
        Win { x: 740, y: 70, w: 280, h: 340, title: "Apps", kind: 2, alive: false },
        Win { x: 0, y: 0, w: 0, h: 0, title: "", kind: 0, alive: false },
    ],
    drag: None,
    grab_x: 0,
    grab_y: 0,
    left: false,
    apps: Vec::new(),
};

pub fn mirror(text: &str) {
    term::push(text);
}

pub fn set_user(name: &str) {
    unsafe {
        let n = name.len().min(DESK.user.len());
        DESK.user[..n].copy_from_slice(&name.as_bytes()[..n]);
        DESK.user_len = n;
    }
}

pub fn ask(title: &str, label: &str, secret: bool) -> String {
    vlr_drivers::serial::write_str(label);
    vlr_drivers::serial::write_str(": ");
    let mut value = String::new();
    loop {
        draw_card(title, label, &value, secret);
        match vlr_drivers::input::read_byte() {
            b'\n' => {
                vlr_drivers::serial::write_str("\n");
                return value;
            }
            0x08 | 0x7F => {
                if value.pop().is_some() && !secret {
                    vlr_drivers::serial::write_str("\u{8} \u{8}");
                }
            }
            byte if (0x20..0x7F).contains(&byte) && value.len() < 32 => {
                value.push(byte as char);
                if secret {
                    vlr_drivers::serial::write_str("*");
                } else {
                    let text = [byte];
                    vlr_drivers::serial::write_str(core::str::from_utf8(&text).unwrap_or(""));
                }
            }
            _ => {}
        }
    }
}

pub fn desktop() -> ! {
    vlr_drivers::display::set_text_enabled(false);
    vlr_console::set_mirror(mirror);
    unsafe {
        if DESK.apps.is_empty() {
            if let Ok(names) = vlr_vfs::list_dir("/Apps") {
                DESK.apps = names;
            }
        }
    }
    paint_wallpaper_all();
    let mut cursor = (0i32, 0i32);
    loop {
        frame(&mut cursor);
        vlr_sched_sleep();
    }
}

fn vlr_sched_sleep() {
    // Планировщик не в зависимостях GUI: спим через таймер и hlt короткими шагами.
    let start = vlr_drivers::timer::ticks();
    while vlr_drivers::timer::ticks().saturating_sub(start) < 4 {
        vlr_arch::hlt();
    }
}

fn frame(cursor: &mut (i32, i32)) {
    let Some((width, height)) = vlr_drivers::display::size() else {
        return;
    };
    vlr_drivers::mouse::clamp(width as i32, height as i32);
    let mouse = vlr_drivers::mouse::state();
    wallpaper_rect(cursor.0, cursor.1, 16, 20);
    let mark = term::fingerprint();
    let clock = vlr_drivers::rtc::read();
    let stamp = clock.hour as u32 * 3600 + clock.minute as u32 * 60 + clock.second as u32;
    let dragging = unsafe { DESK.drag.is_some() };
    let redraw_windows = mark != LAST_MARK.load(Ordering::Relaxed) || dragging || NEED_WINDOWS.load(Ordering::Relaxed);
    LAST_MARK.store(mark, Ordering::Relaxed);
    unsafe {
        let pressed = mouse.left && !DESK.left;
        DESK.left = mouse.left;
        if pressed {
            click(mouse.x, mouse.y);
        }
        if let Some(index) = DESK.drag {
            if mouse.left {
                let win = &mut DESK.windows[index];
                wallpaper_rect(win.x, win.y, win.w, win.h);
                win.x = (mouse.x - DESK.grab_x).max(0);
                win.y = (mouse.y - DESK.grab_y).max(BAR_H as i32);
            } else {
                DESK.drag = None;
            }
        }
    }
    if stamp != LAST_STAMP.load(Ordering::Relaxed) || redraw_windows {
        draw_bar(width);
        LAST_STAMP.store(stamp, Ordering::Relaxed);
    }
    if redraw_windows {
        unsafe {
            for index in 0..DESK.windows.len() {
            if DESK.windows[index].alive {
                draw_window(index);
            }
            }
        }
        NEED_WINDOWS.store(false, Ordering::Relaxed);
    }
    draw_cursor(mouse.x, mouse.y);
    *cursor = (mouse.x, mouse.y);
}

fn wallpaper_rect(x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some((width, height)) = vlr_drivers::display::size() else {
        return;
    };
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w).max(0) as usize;
    let y1 = (y + h).max(0) as usize;
    let tile = tile::TILE;
    for y in y0..y1.min(height) {
        for x in x0..x1.min(width) {
            let sx = x % tile;
            let sy = y % tile;
            let i = (sy * tile + sx) * 3;
            vlr_drivers::display::blit_rgb(x, y, 1, 1, &tile::TILE_RGB[i..i + 3]);
        }
    }
}

fn click(x: i32, y: i32) {
    unsafe {
        if y < BAR_H as i32 {
            if (110..190).contains(&x) {
                DESK.windows[1].alive = true;
            }
            return;
        }
        for index in (0..DESK.windows.len()).rev() {
            let win = &DESK.windows[index];
            if !win.alive || x < win.x || y < win.y || x >= win.x + win.w || y >= win.y + win.h {
                continue;
            }
            if x >= win.x + win.w - 26 && y < win.y + TITLE_H as i32 {
                wallpaper_rect(win.x, win.y, win.w, win.h);
                DESK.windows[index].alive = false;
                return;
            }
            if y < win.y + TITLE_H as i32 {
                DESK.drag = Some(index);
                DESK.grab_x = x - win.x;
                DESK.grab_y = y - win.y;
                return;
            }
            if win.kind == 2 {
                let row = (y - win.y - TITLE_H as i32 - 8) / 28;
                if row >= 0 {
                    if let Some(name) = DESK.apps.get(row as usize) {
                        let path = alloc::format!("/Apps/{name}/{name}.vlr");
                        let _ = vlr_loader::exec_path(&path);
                    }
                }
            }
            return;
        }
    }
}

fn draw_card(title: &str, label: &str, value: &str, secret: bool) {
    paint_wallpaper_all();
    let Some((width, height)) = vlr_drivers::display::size() else {
        return;
    };
    let w = 460.min(width.saturating_sub(40));
    let h = 280.min(height.saturating_sub(40));
    let x = (width - w) / 2;
    let y = (height - h) / 2;
    vlr_drivers::display::fill_rect(x, y, w, h, CARD);
    vlr_drivers::display::fill_rect(x, y, w, 8, TITLE);
    vlr_drivers::display::blit_solid(x + 24, y + 28, logo::LOGO_W, logo::LOGO_H, &logo::LOGO, 2);
    text(x + 100, y + 40, title, INK, CARD, 2);
    text(x + 36, y + 130, label, TITLE, CARD, 1);
    let shown = if secret {
        star_count(value.len())
    } else {
        String::from(value)
    };
    vlr_drivers::display::fill_rect(x + 36, y + 160, w - 72, 36, 0x00FFFFFF);
    text(x + 44, y + 168, &shown, INK, 0x00FFFFFF, 2);
    text(x + 36, y + 220, "Enter confirms", 0x00886677, CARD, 1);
    draw_bar(width);
}

fn star_count(n: usize) -> String {
    let mut s = String::new();
    for _ in 0..n {
        s.push('*');
    }
    s
}

fn draw_bar(width: usize) {
    vlr_drivers::display::fill_rect(0, 0, width, BAR_H, BAR);
    vlr_drivers::display::blit_solid(8, 4, logo::LOGO_W, logo::LOGO_H, &logo::LOGO, 1);
    text(44, 12, "VLR", 0x00FFFFFF, BAR, 2);
    vlr_drivers::display::fill_rect(110, 8, 72, 24, 0x00C4235A);
    text(122, 14, "Apps", 0x00FFFFFF, 0x00C4235A, 1);
    let clock = vlr_drivers::rtc::read();
    let stamp = alloc::format!("{:02}:{:02}:{:02}", clock.hour, clock.minute, clock.second);
    text(width.saturating_sub(360).max(220), 12, &stamp, 0x00FFE4EE, BAR, 2);
    let name = user_name();
    if !name.is_empty() {
        text(width.saturating_sub(160).max(420), 12, &name, 0x00FFFFFF, BAR, 2);
    }
}

fn draw_window(index: usize) {
    let win = unsafe { &DESK.windows[index] };
    let x = win.x.max(0) as usize;
    let y = win.y.max(0) as usize;
    let w = win.w.max(80) as usize;
    let h = win.h.max(80) as usize;
    vlr_drivers::display::fill_rect(x, y, w, TITLE_H, TITLE);
    vlr_drivers::display::fill_rect(x, y + TITLE_H, w, h - TITLE_H, CARD);
    text(x + 10, y + 8, win.title, 0x00FFFFFF, TITLE, 1);
    vlr_drivers::display::fill_rect(x + w - 24, y + 6, 16, 16, 0x00FFFFFF);
    text(x + w - 20, y + 8, "x", TITLE, 0x00FFFFFF, 1);
    if win.kind == 1 {
        draw_terminal(x + 8, y + TITLE_H + 8, (w - 16) / 8, (h - TITLE_H - 16) / 8);
    } else if win.kind == 2 {
        unsafe {
            for (row, name) in DESK.apps.iter().enumerate().take(10) {
                text(x + 12, y + TITLE_H + 12 + row * 28, name, INK, CARD, 2);
            }
        }
    }
}

fn draw_terminal(x: usize, y: usize, cols: usize, rows: usize) {
    if cols == 0 || rows == 0 {
        return;
    }
    let text_buf = term::snapshot();
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text_buf.chars() {
        if ch == '\n' {
            lines.push(core::mem::take(&mut current));
        } else if ch == '\r' || ch == '\u{c}' {
            continue;
        } else if current.len() >= cols {
            lines.push(core::mem::take(&mut current));
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    lines.push(current);
    let start = lines.len().saturating_sub(rows);
    for (row, line) in lines.iter().skip(start).take(rows).enumerate() {
        text(x, y + row * 8, line, INK, CARD, 1);
    }
}

fn draw_cursor(x: i32, y: i32) {
    let x = x.max(0) as usize;
    let y = y.max(0) as usize;
    for row in 0..14 {
        for col in 0..=row.min(8) {
            let color = if col == 0 || row == 13 || col == row.min(8) {
                0x00201418
            } else {
                0x00FFFFFF
            };
            vlr_drivers::display::fill_rect(x + col, y + row, 1, 1, color);
        }
    }
}

fn paint_wallpaper_all() {
    let Some((width, height)) = vlr_drivers::display::size() else {
        return;
    };
    let tile = tile::TILE;
    for y in (0..height).step_by(tile) {
        for x in (0..width).step_by(tile) {
            let w = tile.min(width - x);
            let h = tile.min(height - y);
            if w == tile && h == tile {
                vlr_drivers::display::blit_rgb(x, y, tile, tile, &tile::TILE_RGB);
            } else {
                for row in 0..h {
                    let start = row * tile * 3;
                    let end = start + w * 3;
                    vlr_drivers::display::blit_rgb(x, y + row, w, 1, &tile::TILE_RGB[start..end]);
                }
            }
        }
    }
}

fn text(x: usize, y: usize, message: &str, fg: u32, bg: u32, scale: usize) {
    let mut cursor = x;
    for byte in message.bytes() {
        if byte == b'\n' {
            continue;
        }
        vlr_drivers::display::glyph(cursor, y, byte, fg, bg, scale);
        cursor += 8 * scale;
    }
}

fn user_name() -> String {
    unsafe {
        String::from(core::str::from_utf8(&DESK.user[..DESK.user_len]).unwrap_or("user"))
    }
}
