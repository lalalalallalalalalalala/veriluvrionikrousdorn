//! Байты оболочки поверх общей очереди событий.

use core::sync::atomic::{AtomicBool, Ordering};

pub use crate::event::Event;

static SERIAL_READY: AtomicBool = AtomicBool::new(false);

pub fn note_serial_ready() {
    SERIAL_READY.store(true, Ordering::Release);
}

pub fn push(byte: u8) {
    crate::event::push_byte(byte);
}

pub fn pop() -> Option<u8> {
    crate::event::pop_byte()
}

pub fn read_byte() -> u8 {
    crate::event::read_byte()
}
