//! Программируемый интервальный таймер (PIT), канал 0, IRQ0.

use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::instructions::port::Port;

const PIT_HZ: u32 = 1_193_182;
const TARGET_HZ: u32 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (PIT_HZ / TARGET_HZ) as u16;
    unsafe {
        let mut command = Port::<u8>::new(0x43);
        let mut channel0 = Port::<u8>::new(0x40);
        // Канал 0, младший/старший байт, режим 3 (меандр).
        command.write(0x36);
        channel0.write((divisor & 0xFF) as u8);
        channel0.write((divisor >> 8) as u8);
    }
    vlr_arch::register_irq(0, on_irq);
    vlr_arch::unmask_irq(0);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

fn on_irq() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}
