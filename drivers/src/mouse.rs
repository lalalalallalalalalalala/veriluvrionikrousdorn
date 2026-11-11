//! Мышь PS/2, IRQ12. Три байта пакета: кнопки, dx, dy.

use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};

use x86_64::instructions::port::Port;

static X: AtomicI32 = AtomicI32::new(320);
static Y: AtomicI32 = AtomicI32::new(240);
static BUTTONS: AtomicU8 = AtomicU8::new(0);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PACKET: [AtomicU8; 3] = [AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)];

pub struct Mouse {
    pub x: i32,
    pub y: i32,
    pub left: bool,
}

pub fn init() {
    vlr_arch::unmask_irq(2);
    enable_device();
    vlr_arch::register_irq(12, on_irq);
    vlr_arch::unmask_irq(12);
}

pub fn state() -> Mouse {
    Mouse {
        x: X.load(Ordering::Relaxed),
        y: Y.load(Ordering::Relaxed),
        left: BUTTONS.load(Ordering::Relaxed) & 1 != 0,
    }
}

pub fn clamp(width: i32, height: i32) {
    let mut x = X.load(Ordering::Relaxed);
    let mut y = Y.load(Ordering::Relaxed);
    if x < 0 {
        x = 0;
    }
    if y < 0 {
        y = 0;
    }
    if width > 0 && x >= width {
        x = width - 1;
    }
    if height > 0 && y >= height {
        y = height - 1;
    }
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
}

fn on_irq() {
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        let mut data = Port::<u8>::new(0x60);
        let stat = status.read();
        if stat & 0x01 == 0 || stat & 0x20 == 0 {
            return;
        }
        push(data.read());
    }
}

fn push(byte: u8) {
    let phase = PHASE.load(Ordering::Relaxed);
    PACKET[phase as usize].store(byte, Ordering::Relaxed);
    if phase < 2 {
        PHASE.store(phase + 1, Ordering::Relaxed);
        return;
    }
    PHASE.store(0, Ordering::Relaxed);
    let flags = PACKET[0].load(Ordering::Relaxed);
    if flags & 0x08 == 0 {
        return;
    }
    let dx = PACKET[1].load(Ordering::Relaxed) as i8 as i32;
    let dy = PACKET[2].load(Ordering::Relaxed) as i8 as i32;
    X.fetch_add(dx, Ordering::Relaxed);
    Y.fetch_sub(dy, Ordering::Relaxed);
    BUTTONS.store(flags & 0x07, Ordering::Relaxed);
}

fn enable_device() {
    wait_write();
    out_cmd(0xA8);
    wait_write();
    out_cmd(0x20);
    wait_read();
    let config = unsafe { Port::<u8>::new(0x60).read() };
    wait_write();
    out_cmd(0x60);
    wait_write();
    unsafe { Port::<u8>::new(0x60).write(config | 0x02) };
    wait_write();
    out_cmd(0xD4);
    wait_write();
    unsafe { Port::<u8>::new(0x60).write(0xF4) };
    let _ = try_read();
}

fn out_cmd(value: u8) {
    unsafe { Port::<u8>::new(0x64).write(value) };
}

fn wait_write() {
    for _ in 0..100_000 {
        if unsafe { Port::<u8>::new(0x64).read() } & 0x02 == 0 {
            return;
        }
    }
}

fn wait_read() {
    for _ in 0..100_000 {
        if unsafe { Port::<u8>::new(0x64).read() } & 0x01 != 0 {
            return;
        }
    }
}

fn try_read() -> Option<u8> {
    if unsafe { Port::<u8>::new(0x64).read() } & 0x01 != 0 {
        Some(unsafe { Port::<u8>::new(0x60).read() })
    } else {
        None
    }
}
