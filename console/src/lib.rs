//! Консоль ядра: один поток текста на экран и в последовательный порт.
//!
//! Цвет относится только к framebuffer. Последовательный порт получает тот же
//! текст, а перевод строки дополняется `\r`, чтобы его понял терминал.

#![no_std]

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use vlr_drivers::{display, serial};

static MIRROR: AtomicUsize = AtomicUsize::new(0);

/// Куда дублировать текст, когда экран занят рабочим столом.
pub fn set_mirror(sink: fn(&str)) {
    MIRROR.store(sink as usize, Ordering::Relaxed);
}

fn paint(text: &str) {
    let sink = MIRROR.load(Ordering::Relaxed);
    if sink == 0 {
        display::write_str(text);
    } else {
        let sink: fn(&str) = unsafe { core::mem::transmute(sink) };
        sink(text);
    }
}

pub fn write_str(text: &str) {
    // Один процессор: пока пишем, тик не должен увести нас с занятой консолью.
    vlr_arch::without_interrupts(|| {
        paint(text);
        serial::write_str(text);
    });
}

pub fn clear() {
    vlr_arch::without_interrupts(|| {
        if MIRROR.load(Ordering::Relaxed) == 0 {
            display::clear();
        } else {
            paint("\u{0c}");
        }
    });
    serial::write_str("\u{1b}[2J\u{1b}[H");
}

pub fn backspace() {
    vlr_arch::without_interrupts(|| {
        if MIRROR.load(Ordering::Relaxed) == 0 {
            display::write_byte(0x08);
        } else {
            paint("\u{08}");
        }
    });
    serial::write_str("\u{8} \u{8}");
}

pub fn set_color(rgb: u32) {
    display::set_color(rgb);
}

pub fn _print(args: fmt::Arguments) {
    let _ = fmt::write(&mut Writer, args);
}

struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_str(text);
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}
