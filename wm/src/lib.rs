//! Окна, фокус и очередь ввода. Рисование остаётся в `vlr-gui`.
//!
//! Клавиатура не приклеена к `vch`: байты получает окно в фокусе.
//! Терминал `vch` читает отдельную очередь, только когда он в фокусе.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

pub const CTRL_L: u8 = 0x0c;
pub const CTRL_N: u8 = 0x0e;

const MAX: usize = 8;
const TEXT: usize = 512;
const EV: usize = 32;

struct Slot {
    used: bool,
    id: u64,
    title: [u8; 24],
    title_len: u8,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    z: u8,
    minimized: bool,
    text: [u8; TEXT],
    text_len: usize,
    ev: [u8; EV],
    head: usize,
    tail: usize,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            used: false,
            id: 0,
            title: [0; 24],
            title_len: 0,
            x: 80,
            y: 80,
            w: 280,
            h: 180,
            z: 0,
            minimized: false,
            text: [0; TEXT],
            text_len: 0,
            ev: [0; EV],
            head: 0,
            tail: 0,
        }
    }
}

struct Table {
    slots: [Slot; MAX],
    next_id: u64,
    focus: u64,
    next_z: u8,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    slots: [const { Slot::empty() }; MAX],
    next_id: 1,
    focus: 0,
    next_z: 1,
});

static ACTIVE: AtomicBool = AtomicBool::new(false);
static TTY_HEAD: AtomicU64 = AtomicU64::new(0);
static TTY_TAIL: AtomicU64 = AtomicU64::new(0);
static TTY: Mutex<[u8; 256]> = Mutex::new([0; 256]);

pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::Relaxed);
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Клавиша из общей очереди событий попадает в окно, которое в фокусе.
pub fn deliver_key(byte: u8) {
    if terminal_focused() {
        tty_push(byte);
    } else {
        push_key(focus_id(), byte);
    }
}

pub fn tty_push(byte: u8) {
    let mut buf = TTY.lock();
    let head = TTY_HEAD.load(Ordering::Relaxed) as usize % buf.len();
    let next = (head + 1) % buf.len();
    if next == TTY_TAIL.load(Ordering::Relaxed) as usize % buf.len() {
        return;
    }
    buf[head] = byte;
    TTY_HEAD.store(next as u64, Ordering::Relaxed);
}

pub fn tty_pop() -> Option<u8> {
    let tail = TTY_TAIL.load(Ordering::Relaxed) as usize % 256;
    let head = TTY_HEAD.load(Ordering::Relaxed) as usize % 256;
    if tail == head {
        return None;
    }
    let byte = TTY.lock()[tail];
    TTY_TAIL.store(((tail + 1) % 256) as u64, Ordering::Relaxed);
    Some(byte)
}

pub fn create(title: &str, w: i32, h: i32) -> Option<u64> {
    let mut table = TABLE.lock();
    let index = table.slots.iter().position(|slot| !slot.used)?;
    let id = table.next_id;
    table.next_id = table.next_id.saturating_add(1);
    let z = table.next_z;
    table.next_z = table.next_z.wrapping_add(1);
    let mut slot = Slot::empty();
    slot.used = true;
    slot.id = id;
    slot.w = w.clamp(160, 640);
    slot.h = h.clamp(120, 480);
    slot.x = 70 + (index as i32) * 36;
    slot.y = 488;
    slot.z = z;
    let n = title.len().min(slot.title.len());
    slot.title[..n].copy_from_slice(&title.as_bytes()[..n]);
    slot.title_len = n as u8;
    table.slots[index] = slot;
    table.focus = id;
    Some(id)
}

pub fn close(id: u64) -> bool {
    let mut table = TABLE.lock();
    if let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) {
        *slot = Slot::empty();
        if table.focus == id {
            table.focus = 0;
        }
        return true;
    }
    false
}

pub fn focus_id() -> u64 {
    TABLE.lock().focus
}

pub fn focus(id: u64) {
    let mut table = TABLE.lock();
    let found = id == 0 || table.slots.iter().any(|slot| slot.used && slot.id == id);
    if !found {
        return;
    }
    table.focus = id;
    let z = table.next_z;
    table.next_z = table.next_z.wrapping_add(1);
    if let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) {
        slot.z = z;
        slot.minimized = false;
    }
}

pub fn cycle() -> u64 {
    let mut table = TABLE.lock();
    let mut ids = Vec::new();
    ids.push(0);
    let mut windows: Vec<(u8, u64)> = table
        .slots
        .iter()
        .filter(|slot| slot.used && !slot.minimized)
        .map(|slot| (slot.z, slot.id))
        .collect();
    windows.sort_by_key(|item| item.0);
    for (_, id) in windows {
        ids.push(id);
    }
    if ids.is_empty() {
        table.focus = 0;
        return 0;
    }
    let pos = ids.iter().position(|id| *id == table.focus).unwrap_or(0);
    let next = ids[(pos + 1) % ids.len()];
    table.focus = next;
    next
}

pub fn push_key(id: u64, byte: u8) {
    let mut table = TABLE.lock();
    let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) else {
        return;
    };
    let next = (slot.head + 1) % EV;
    if next == slot.tail {
        return;
    }
    slot.ev[slot.head] = byte;
    slot.head = next;
}

pub fn poll(id: u64) -> Option<u8> {
    let mut table = TABLE.lock();
    let slot = table.slots.iter_mut().find(|slot| slot.used && slot.id == id)?;
    if slot.tail == slot.head {
        return None;
    }
    let byte = slot.ev[slot.tail];
    slot.tail = (slot.tail + 1) % EV;
    Some(byte)
}

pub fn append_text(id: u64, text: &str) {
    let mut table = TABLE.lock();
    let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) else {
        return;
    };
    for byte in text.bytes() {
        if slot.text_len >= TEXT {
            slot.text.copy_within(1.., 0);
            slot.text_len -= 1;
        }
        slot.text[slot.text_len] = byte;
        slot.text_len += 1;
    }
}

pub fn minimize(id: u64) {
    let mut table = TABLE.lock();
    if let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) {
        slot.minimized = true;
        if table.focus == id {
            table.focus = 0;
        }
    }
}

pub fn set_size(id: u64, w: i32, h: i32) {
    let mut table = TABLE.lock();
    if let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) {
        slot.w = w.clamp(160, 700);
        slot.h = h.clamp(120, 500);
    }
}

pub fn move_to(id: u64, x: i32, y: i32) {
    let mut table = TABLE.lock();
    if let Some(slot) = table.slots.iter_mut().find(|slot| slot.used && slot.id == id) {
        slot.x = x.max(0);
        slot.y = y.max(40);
    }
}

#[derive(Clone)]
pub struct Info {
    pub id: u64,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub z: u8,
    pub minimized: bool,
    pub text: String,
    pub focused: bool,
}

pub fn snapshot() -> Vec<Info> {
    let table = TABLE.lock();
    let mut list: Vec<Info> = table
        .slots
        .iter()
        .filter(|slot| slot.used)
        .map(|slot| Info {
            id: slot.id,
            title: core::str::from_utf8(&slot.title[..slot.title_len as usize])
                .unwrap_or("?")
                .into(),
            x: slot.x,
            y: slot.y,
            w: slot.w,
            h: slot.h,
            z: slot.z,
            minimized: slot.minimized,
            text: alloc::string::String::from_utf8_lossy(&slot.text[..slot.text_len]).into_owned(),
            focused: slot.id == table.focus,
        })
        .collect();
    list.sort_by_key(|info| info.z);
    list
}

pub fn terminal_focused() -> bool {
    TABLE.lock().focus == 0
}
