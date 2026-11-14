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
        Win { x: 420, y: 430, w: 300, h: 200, title: "Settings", kind: 3, alive: false },
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

static CHECK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn set_checker(check: fn(&str) -> bool) {
    CHECK.store(check as usize, Ordering::Relaxed);
}

pub fn refresh_apps() {
    unsafe {
        if let Ok(names) = vlr_vfs::list_dir("/Apps") {
            DESK.apps = names;
        }
        DESK.windows[1].alive = true;
    }
    NEED_WINDOWS.store(true, Ordering::Relaxed);
}

pub fn desktop() -> ! {
    vlr_drivers::display::set_text_enabled(false);
    vlr_console::set_mirror(mirror);
    vlr_wm::set_active(true);
    vlr_drivers::display::enable_flip();
    load_settings();
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
    let Some((width, _height)) = vlr_drivers::display::size() else {
        return;
    };
    route_input();
    let (mx, my, left) = vlr_drivers::event::cursor();
    let mouse = vlr_drivers::mouse::Mouse { x: mx, y: my, left };
    wallpaper_rect(cursor.0, cursor.1, 16, 20);
    let mut extra = vlr_wm::focus_id();
    for info in vlr_wm::snapshot() {
        extra = extra.wrapping_add(info.text.len() as u64).wrapping_add(info.w as u64);
    }
    let mark = term::fingerprint() ^ extra;
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
        for info in vlr_wm::snapshot() {
            if !info.minimized {
                draw_client(&info);
            }
        }
        NEED_WINDOWS.store(false, Ordering::Relaxed);
    }
    draw_cursor(mouse.x, mouse.y);
    *cursor = (mouse.x, mouse.y);
    vlr_drivers::display::present();
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
    if hit_clients(x, y) {
        NEED_WINDOWS.store(true, Ordering::Relaxed);
        return;
    }
    unsafe {
        if y < BAR_H as i32 {
            if (168..240).contains(&x) {
                DESK.windows[1].alive = true;
                NEED_WINDOWS.store(true, Ordering::Relaxed);
            } else if (248..320).contains(&x) {
                lock_screen();
            } else if (328..384).contains(&x) {
                DESK.windows[2].alive = true;
                NEED_WINDOWS.store(true, Ordering::Relaxed);
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
            if win.kind == 3 {
                let row = (y - win.y - TITLE_H as i32 - 8) / 28;
                if row == 0 {
                    vlr_drivers::settings::cycle_speed();
                } else if row == 1 {
                    vlr_drivers::layout::set_russian(!vlr_drivers::layout::is_russian());
                } else if row == 2 {
                    let (w, h) = vlr_drivers::display::size().unwrap_or((1280, 720));
                    let modes = vlr_drivers::display::mode_list();
                    let pos = modes.iter().position(|mode| *mode == (w as u16, h as u16)).unwrap_or(0);
                    let next = modes[(pos + 1) % modes.len()];
                    if vlr_drivers::display::set_mode(next.0, next.1) {
                        paint_wallpaper_all();
                    }
                }
                save_settings();
                NEED_WINDOWS.store(true, Ordering::Relaxed);
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
    text(44, 14, "Vrlvrnkrsdrn", 0x00FFFFFF, BAR, 1);
    vlr_drivers::display::fill_rect(168, 8, 72, 24, 0x00C4235A);
    text(180, 14, "Apps", 0x00FFFFFF, 0x00C4235A, 1);
    vlr_drivers::display::fill_rect(248, 8, 72, 24, 0x00601830);
    text(260, 14, "Lock", 0x00FFFFFF, 0x00601830, 1);
    vlr_drivers::display::fill_rect(328, 8, 56, 24, 0x00601830);
    text(336, 14, "Set", 0x00FFFFFF, 0x00601830, 1);
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
    } else if win.kind == 3 {
        let speed = alloc::format!("speed {}", vlr_drivers::settings::speed());
        let layout = if vlr_drivers::layout::is_russian() { "layout ru" } else { "layout us" };
        let (w, h) = vlr_drivers::display::size().unwrap_or((0, 0));
        let mode = alloc::format!("mode {w}x{h}");
        text(x + 12, y + TITLE_H + 16, &speed, INK, CARD, 2);
        text(x + 12, y + TITLE_H + 48, layout, INK, CARD, 2);
        text(x + 12, y + TITLE_H + 80, &mode, INK, CARD, 2);
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
    for ch in message.chars() {
        if ch == '\n' {
            continue;
        }
        vlr_drivers::display::glyph_char(cursor, y, ch, fg, bg, scale);
        cursor += 8 * scale;
    }
}

fn route_input() {
    vlr_drivers::usb_hid::poll();
    while let Some(event) = vlr_drivers::event::pop_event() {
        vlr_drivers::event::apply(&event);
        let vlr_drivers::event::Event::Key { down: true, text, text_len, .. } = event else {
            continue;
        };
        for index in 0..text_len as usize {
            route_byte(text[index]);
        }
    }
}

fn route_byte(byte: u8) {
    if byte == vlr_wm::CTRL_L {
        lock_screen();
        NEED_WINDOWS.store(true, Ordering::Relaxed);
        paint_wallpaper_all();
        return;
    }
    if byte == vlr_wm::CTRL_N {
        let _ = vlr_wm::cycle();
        return;
    }
    vlr_wm::deliver_key(byte);
}

fn save_settings() {
    let _ = vlr_vfs::make_dir("/AppData/desktop");
    let layout = if vlr_drivers::layout::is_russian() { "ru" } else { "us" };
    let (w, h) = vlr_drivers::display::size().unwrap_or((0, 0));
    let text = alloc::format!(
        "speed={}\nlayout={layout}\nmode={w}x{h}\n",
        vlr_drivers::settings::speed()
    );
    let _ = vlr_vfs::write_file("/AppData/desktop/settings", text.as_bytes());
}

fn load_settings() {
    let Ok(bytes) = vlr_vfs::read_file("/AppData/desktop/settings") else {
        return;
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return;
    };
    vlr_drivers::settings::apply_text(text);
    for line in text.split('\n') {
        let Some(rest) = line.strip_prefix("mode=") else { continue };
        let mut parts = rest.split('x');
        let w = parts.next().unwrap_or("0").parse::<u16>().unwrap_or(0);
        let h = parts.next().unwrap_or("0").parse::<u16>().unwrap_or(0);
        if w > 0 && h > 0 {
            let _ = vlr_drivers::display::set_mode(w, h);
        }
    }
}

fn lock_screen() {
    vlr_drivers::serial::write_str("locked\n");
    loop {
        let password = ask("Locked", "password", true);
        let check = CHECK.load(Ordering::Relaxed);
        let ok = if check == 0 {
            password.is_empty()
        } else {
            let check: fn(&str) -> bool = unsafe { core::mem::transmute(check) };
            check(&password)
        };
        if ok {
            vlr_drivers::serial::write_str("unlocked\n");
            return;
        }
        vlr_drivers::serial::write_str("wrong password\n");
    }
}

fn draw_client(info: &vlr_wm::Info) {
    let x = info.x.max(0) as usize;
    let y = info.y.max(0) as usize;
    let w = info.w.max(80) as usize;
    let h = info.h.max(80) as usize;
    let title_bg = if info.focused { TITLE } else { 0x00885 };
    vlr_drivers::display::fill_rect(x, y, w, TITLE_H, title_bg);
    vlr_drivers::display::fill_rect(x, y + TITLE_H, w, h - TITLE_H, CARD);
    text(x + 8, y + 8, &info.title, 0x00FFFFFF, title_bg, 1);
    vlr_drivers::display::fill_rect(x + w - 44, y + 6, 16, 16, 0x00FFE4EE);
    text(x + w - 40, y + 8, "-", TITLE, 0x00FFE4EE, 1);
    vlr_drivers::display::fill_rect(x + w - 24, y + 6, 16, 16, 0x00FFFFFF);
    text(x + w - 20, y + 8, "x", TITLE, 0x00FFFFFF, 1);
    text(x + 8, y + TITLE_H + 8, &info.text, INK, CARD, 1);
    vlr_drivers::display::fill_rect(x + w - 14, y + h - 14, 12, 12, TITLE);
}

fn hit_clients(x: i32, y: i32) -> bool {
    let list = vlr_wm::snapshot();
    for info in list.iter().rev() {
        if info.minimized || x < info.x || y < info.y || x >= info.x + info.w || y >= info.y + info.h {
            continue;
        }
        if x >= info.x + info.w - 26 && y < info.y + TITLE_H as i32 {
            vlr_wm::close(info.id);
            wallpaper_rect(info.x, info.y, info.w, info.h);
            return true;
        }
        if x >= info.x + info.w - 48 && y < info.y + TITLE_H as i32 {
            vlr_wm::minimize(info.id);
            wallpaper_rect(info.x, info.y, info.w, info.h);
            return true;
        }
        if x >= info.x + info.w - 16 && y >= info.y + info.h - 16 {
            vlr_wm::set_size(info.id, x - info.x, y - info.y);
            return true;
        }
        vlr_wm::focus(info.id);
        return true;
    }
    false
}

fn user_name() -> String {
    unsafe {
        String::from(core::str::from_utf8(&DESK.user[..DESK.user_len]).unwrap_or("user"))
    }
}
