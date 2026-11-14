//! Скорость указателя. Раскладка живёт в `layout`.

use core::sync::atomic::{AtomicU8, Ordering};

static SPEED: AtomicU8 = AtomicU8::new(1);

pub fn speed() -> u8 {
    SPEED.load(Ordering::Relaxed).clamp(1, 4)
}

pub fn set_speed(value: u8) {
    SPEED.store(value.clamp(1, 4), Ordering::Relaxed);
}

pub fn cycle_speed() -> u8 {
    let next = match speed() {
        1 => 2,
        2 => 3,
        3 => 4,
        _ => 1,
    };
    set_speed(next);
    next
}

pub fn apply_text(text: &str) {
    for line in text.split('\n') {
        if let Some(value) = line.strip_prefix("speed=") {
            if let Ok(n) = value.trim().parse::<u8>() {
                set_speed(n);
            }
        }
        if let Some(value) = line.strip_prefix("layout=") {
            crate::layout::set_russian(value.trim() == "ru");
        }
    }
}
