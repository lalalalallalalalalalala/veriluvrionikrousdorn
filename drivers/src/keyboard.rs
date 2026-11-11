//! Клавиатура PS/2, набор сканкодов 1, IRQ1.
//!
//! В очередь ввода попадают байты ASCII, `\n` для Enter и `0x08` для Backspace.

use core::sync::atomic::{AtomicBool, Ordering};

use x86_64::instructions::port::Port;

static SHIFT: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    vlr_arch::register_irq(1, on_irq);
    vlr_arch::unmask_irq(1);
    // Считать байт, который контроллер мог положить ещё до нашего обработчика.
    on_irq();
}

fn on_irq() {
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        let mut data = Port::<u8>::new(0x60);
        if status.read() & 0x01 == 0 {
            return;
        }
        handle_scancode(data.read());
    }
}

fn handle_scancode(scancode: u8) {
    if scancode == 0xE0 || scancode == 0xE1 {
        EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    if EXTENDED.swap(false, Ordering::Relaxed) {
        return;
    }
    match scancode {
        0x2A | 0x36 => SHIFT.store(true, Ordering::Relaxed),
        0xAA | 0xB6 => SHIFT.store(false, Ordering::Relaxed),
        released if released & 0x80 != 0 => {}
        pressed => {
            if let Some(byte) = decode(pressed, SHIFT.load(Ordering::Relaxed)) {
                crate::input::push(byte);
            }
        }
    }
}

fn decode(scancode: u8, shift: bool) -> Option<u8> {
    Some(match scancode {
        0x02 => shifted(shift, b'1', b'!'),
        0x03 => shifted(shift, b'2', b'@'),
        0x04 => shifted(shift, b'3', b'#'),
        0x05 => shifted(shift, b'4', b'$'),
        0x06 => shifted(shift, b'5', b'%'),
        0x07 => shifted(shift, b'6', b'^'),
        0x08 => shifted(shift, b'7', b'&'),
        0x09 => shifted(shift, b'8', b'*'),
        0x0A => shifted(shift, b'9', b'('),
        0x0B => shifted(shift, b'0', b')'),
        0x0C => shifted(shift, b'-', b'_'),
        0x0D => shifted(shift, b'=', b'+'),
        0x0E => 0x08,
        0x10 => letter(shift, b'q'),
        0x11 => letter(shift, b'w'),
        0x12 => letter(shift, b'e'),
        0x13 => letter(shift, b'r'),
        0x14 => letter(shift, b't'),
        0x15 => letter(shift, b'y'),
        0x16 => letter(shift, b'u'),
        0x17 => letter(shift, b'i'),
        0x18 => letter(shift, b'o'),
        0x19 => letter(shift, b'p'),
        0x1A => shifted(shift, b'[', b'{'),
        0x1B => shifted(shift, b']', b'}'),
        0x1C => b'\n',
        0x1E => letter(shift, b'a'),
        0x1F => letter(shift, b's'),
        0x20 => letter(shift, b'd'),
        0x21 => letter(shift, b'f'),
        0x22 => letter(shift, b'g'),
        0x23 => letter(shift, b'h'),
        0x24 => letter(shift, b'j'),
        0x25 => letter(shift, b'k'),
        0x26 => letter(shift, b'l'),
        0x27 => shifted(shift, b';', b':'),
        0x28 => shifted(shift, b'\'', b'"'),
        0x29 => shifted(shift, b'`', b'~'),
        0x2B => shifted(shift, b'\\', b'|'),
        0x2C => letter(shift, b'z'),
        0x2D => letter(shift, b'x'),
        0x2E => letter(shift, b'c'),
        0x2F => letter(shift, b'v'),
        0x30 => letter(shift, b'b'),
        0x31 => letter(shift, b'n'),
        0x32 => letter(shift, b'm'),
        0x33 => shifted(shift, b',', b'<'),
        0x34 => shifted(shift, b'.', b'>'),
        0x35 => shifted(shift, b'/', b'?'),
        0x39 => b' ',
        _ => return None,
    })
}

fn shifted(shift: bool, base: u8, with_shift: u8) -> u8 {
    if shift {
        with_shift
    } else {
        base
    }
}

fn letter(shift: bool, lower: u8) -> u8 {
    if shift {
        lower.to_ascii_uppercase()
    } else {
        lower
    }
}
