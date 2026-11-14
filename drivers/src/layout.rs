//! Раскладки US и ЙЦУКЕН. Переключение — Alt+Shift.
//!
//! Ctrl+буква считается по латинской букве клавиши, поэтому Ctrl+L
//! запирает стол и в русской раскладке.

use super::event::{self, Event};

static mut ALT: bool = false;
static mut SHIFT: bool = false;
static mut CTRL: bool = false;
static mut ARMED: bool = false;
static mut RUSSIAN: bool = false;

pub fn is_russian() -> bool {
    unsafe { RUSSIAN }
}

pub fn set_russian(on: bool) {
    unsafe { RUSSIAN = on }
}

pub fn ps2(scancode: u8) {
    let up = scancode & 0x80 != 0;
    let code = scancode & 0x7F;
    match code {
        0x2A | 0x36 => unsafe { SHIFT = !up },
        0x1D => unsafe { CTRL = !up },
        0x38 => unsafe { ALT = !up },
        _ if !up => {
            if let Some(usage) = set1_usage(code) {
                hid_usage(usage, true);
            }
        }
        _ => {}
    }
    note_toggle();
}

pub fn hid_usage(usage: u16, down: bool) {
    match usage {
        0xE1 | 0xE5 => unsafe { SHIFT = down },
        0xE0 | 0xE4 => unsafe { CTRL = down },
        0xE2 | 0xE6 => unsafe { ALT = down },
        _ => {}
    }
    note_toggle();
    if !down {
        return;
    }
    if usage == 0x28 {
        emit(usage, &[b'\n']);
        return;
    }
    if usage == 0x2A {
        emit(usage, &[0x08]);
        return;
    }
    if unsafe { CTRL } {
        if let Some(byte) = latin_byte(usage, false, true) {
            emit(usage, &[byte]);
        }
        return;
    }
    if unsafe { RUSSIAN } {
        if let Some((bytes, n)) = cyrillic(usage, unsafe { SHIFT }) {
            emit(usage, &bytes[..n]);
        }
        return;
    }
    if let Some(byte) = latin_byte(usage, unsafe { SHIFT }, false) {
        emit(usage, &[byte]);
    }
}

fn note_toggle() {
    let both = unsafe { ALT && SHIFT };
    if both {
        unsafe { ARMED = true };
    } else if unsafe { ARMED } {
        unsafe {
            ARMED = false;
            RUSSIAN = !RUSSIAN;
        }
        crate::serial::write_str(if unsafe { RUSSIAN } {
            "layout ru\n"
        } else {
            "layout us\n"
        });
    }
}

fn emit(usage: u16, bytes: &[u8]) {
    let mut text = [0u8; 4];
    let n = bytes.len().min(4);
    text[..n].copy_from_slice(&bytes[..n]);
    event::push(Event::Key {
        down: true,
        usage,
        text,
        text_len: n as u8,
    });
    if usage != 0 {
        crate::serial::write_str("hid-key ");
        crate::serial::write_str(core::str::from_utf8(&text[..n]).unwrap_or("?"));
        crate::serial::write_str("\n");
    }
}

fn latin_byte(usage: u16, shift: bool, ctrl: bool) -> Option<u8> {
    let (base, shifted) = match usage {
        0x04 => (b'a', b'A'),
        0x05 => (b'b', b'B'),
        0x06 => (b'c', b'C'),
        0x07 => (b'd', b'D'),
        0x08 => (b'e', b'E'),
        0x09 => (b'f', b'F'),
        0x0A => (b'g', b'G'),
        0x0B => (b'h', b'H'),
        0x0C => (b'i', b'I'),
        0x0D => (b'j', b'J'),
        0x0E => (b'k', b'K'),
        0x0F => (b'l', b'L'),
        0x10 => (b'm', b'M'),
        0x11 => (b'n', b'N'),
        0x12 => (b'o', b'O'),
        0x13 => (b'p', b'P'),
        0x14 => (b'q', b'Q'),
        0x15 => (b'r', b'R'),
        0x16 => (b's', b'S'),
        0x17 => (b't', b'T'),
        0x18 => (b'u', b'U'),
        0x19 => (b'v', b'V'),
        0x1A => (b'w', b'W'),
        0x1B => (b'x', b'X'),
        0x1C => (b'y', b'Y'),
        0x1D => (b'z', b'Z'),
        0x1E => (b'1', b'!'),
        0x1F => (b'2', b'@'),
        0x20 => (b'3', b'#'),
        0x21 => (b'4', b'$'),
        0x22 => (b'5', b'%'),
        0x23 => (b'6', b'^'),
        0x24 => (b'7', b'&'),
        0x25 => (b'8', b'*'),
        0x26 => (b'9', b'('),
        0x27 => (b'0', b')'),
        0x2C => (b' ', b' '),
        0x2D => (b'-', b'_'),
        0x2E => (b'=', b'+'),
        0x2F => (b'[', b'{'),
        0x30 => (b']', b'}'),
        0x31 => (b'\\', b'|'),
        0x33 => (b';', b':'),
        0x34 => (b'\'', b'"'),
        0x36 => (b',', b'<'),
        0x37 => (b'.', b'>'),
        0x38 => (b'/', b'?'),
        _ => return None,
    };
    let byte = if shift { shifted } else { base };
    if ctrl && byte.is_ascii_alphabetic() {
        Some(byte.to_ascii_lowercase() & 0x1F)
    } else {
        Some(byte)
    }
}

fn cyrillic(usage: u16, shift: bool) -> Option<([u8; 4], usize)> {
    let letter = jcuken(usage)?;
    let letter = if shift {
        letter.to_uppercase().next().unwrap_or(letter)
    } else {
        letter
    };
    let mut buf = [0u8; 4];
    let n = letter.encode_utf8(&mut buf).len();
    Some((buf, n))
}

fn jcuken(usage: u16) -> Option<char> {
    Some(match usage {
        0x14 => 'й',
        0x1A => 'ц',
        0x08 => 'у',
        0x15 => 'к',
        0x17 => 'е',
        0x1C => 'н',
        0x18 => 'г',
        0x0C => 'ш',
        0x12 => 'щ',
        0x13 => 'з',
        0x2F => 'х',
        0x30 => 'ъ',
        0x04 => 'ф',
        0x16 => 'ы',
        0x07 => 'в',
        0x09 => 'а',
        0x0A => 'п',
        0x0B => 'р',
        0x0D => 'о',
        0x0E => 'л',
        0x0F => 'д',
        0x33 => 'ж',
        0x34 => 'э',
        0x1D => 'я',
        0x1B => 'ч',
        0x06 => 'с',
        0x19 => 'м',
        0x05 => 'и',
        0x11 => 'т',
        0x10 => 'ь',
        0x36 => 'б',
        0x37 => 'ю',
        0x2C => ' ',
        _ => return None,
    })
}

fn set1_usage(code: u8) -> Option<u16> {
    Some(match code {
        0x1E => 0x04,
        0x30 => 0x05,
        0x2E => 0x06,
        0x20 => 0x07,
        0x12 => 0x08,
        0x21 => 0x09,
        0x22 => 0x0A,
        0x23 => 0x0B,
        0x17 => 0x0C,
        0x24 => 0x0D,
        0x25 => 0x0E,
        0x26 => 0x0F,
        0x32 => 0x10,
        0x31 => 0x11,
        0x18 => 0x12,
        0x19 => 0x13,
        0x10 => 0x14,
        0x13 => 0x15,
        0x1F => 0x16,
        0x14 => 0x17,
        0x16 => 0x18,
        0x2F => 0x19,
        0x11 => 0x1A,
        0x2D => 0x1B,
        0x15 => 0x1C,
        0x2C => 0x1D,
        0x02 => 0x1E,
        0x03 => 0x1F,
        0x04 => 0x20,
        0x05 => 0x21,
        0x06 => 0x22,
        0x07 => 0x23,
        0x08 => 0x24,
        0x09 => 0x25,
        0x0A => 0x26,
        0x0B => 0x27,
        0x1C => 0x28,
        0x0E => 0x2A,
        0x39 => 0x2C,
        0x0C => 0x2D,
        0x0D => 0x2E,
        0x1A => 0x2F,
        0x1B => 0x30,
        0x2B => 0x31,
        0x27 => 0x33,
        0x28 => 0x34,
        0x33 => 0x36,
        0x34 => 0x37,
        0x35 => 0x38,
        _ => return None,
    })
}
