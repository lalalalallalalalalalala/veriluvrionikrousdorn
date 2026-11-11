//! Консоль ядра: один поток текста на экран и в последовательный порт.
//!
//! Цвет относится только к framebuffer. Последовательный порт получает тот же
//! текст, а перевод строки дополняется `\r`, чтобы его понял терминал.

#![no_std]

use core::fmt;

use vlr_drivers::{display, serial};

pub fn write_str(text: &str) {
    // Один процессор: пока пишем, тик не должен увести нас с занятой консолью.
    vlr_arch::without_interrupts(|| {
        display::write_str(text);
        serial::write_str(text);
    });
}

pub fn clear() {
    display::clear();
    serial::write_str("\u{1b}[2J\u{1b}[H");
}

pub fn backspace() {
    display::write_byte(0x08);
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
