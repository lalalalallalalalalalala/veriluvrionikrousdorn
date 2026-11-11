//! UART 16550, COM1 (`0x3F8`), 115200 8N1.
//!
//! Приём идёт и по IRQ4, и опросом: опрос нужен, если байт пришёл до включения
//! прерываний.

use core::sync::atomic::{AtomicBool, Ordering};

use x86_64::instructions::port::Port;

const PORT: u16 = 0x3F8;

static READY: AtomicBool = AtomicBool::new(false);

pub fn init() {
    unsafe {
        let mut ier = Port::<u8>::new(PORT + 1);
        let mut fcr = Port::<u8>::new(PORT + 2);
        let mut lcr = Port::<u8>::new(PORT + 3);
        let mut mcr = Port::<u8>::new(PORT + 4);
        let mut data = Port::<u8>::new(PORT);

        ier.write(0x00);
        lcr.write(0x80);
        data.write(0x01);
        ier.write(0x00);
        lcr.write(0x03);
        fcr.write(0xC7);
        mcr.write(0x0B);
    }
    READY.store(true, Ordering::Release);
}

/// Включить прерывание по приёму и повесить его на IRQ4.
pub fn enable_rx_irq() {
    crate::input::note_serial_ready();
    vlr_arch::register_irq(4, on_irq);
    unsafe {
        Port::<u8>::new(PORT + 1).write(0x01);
    }
    vlr_arch::unmask_irq(4);
    poll();
}

pub fn write_byte(byte: u8) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let mut lsr = Port::<u8>::new(PORT + 5);
        let mut data = Port::<u8>::new(PORT);
        let mut spins = 0;
        while lsr.read() & 0x20 == 0 && spins < 100_000 {
            spins += 1;
            core::hint::spin_loop();
        }
        data.write(byte);
    }
}

pub fn write_str(text: &str) {
    for &byte in text.as_bytes() {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}

/// Забрать уже пришедшие байты, не дожидаясь IRQ.
pub fn poll() {
    vlr_arch::without_interrupts(poll_raw);
}

fn poll_raw() {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let mut lsr = Port::<u8>::new(PORT + 5);
        let mut data = Port::<u8>::new(PORT);
        for _ in 0..32 {
            if lsr.read() & 0x01 == 0 {
                break;
            }
            crate::input::push(data.read());
        }
    }
}

fn on_irq() {
    poll();
}
