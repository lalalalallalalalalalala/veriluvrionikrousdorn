//! Клавиатура PS/2, набор сканкодов 1, IRQ1.
//!
//! Сканкоды идут в общую раскладку и становятся событиями `Key`.

use core::sync::atomic::{AtomicBool, Ordering};

use x86_64::instructions::port::Port;

static EXTENDED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    crate::devices::add("ps2-keyboard");
    vlr_arch::register_irq(1, on_irq);
    vlr_arch::unmask_irq(1);
    on_irq();
}

fn on_irq() {
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        let mut data = Port::<u8>::new(0x60);
        let status = status.read();
        if status & 0x01 == 0 || status & 0x20 != 0 {
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
    crate::layout::ps2(scancode);
}
