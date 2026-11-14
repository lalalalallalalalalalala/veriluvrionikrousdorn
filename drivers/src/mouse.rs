//! Мышь PS/2, IRQ12. Три байта пакета: кнопки, dx, dy.

use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};

use x86_64::instructions::port::Port;

static X: AtomicI32 = AtomicI32::new(320);
static Y: AtomicI32 = AtomicI32::new(240);
static BUTTONS: AtomicU8 = AtomicU8::new(0);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PACKET: [AtomicU8; 4] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];
static PACKET_LEN: AtomicU8 = AtomicU8::new(3);

pub struct Mouse {
    pub x: i32,
    pub y: i32,
    pub left: bool,
}

pub fn init() {
    crate::devices::add("ps2-mouse");
    vlr_arch::unmask_irq(2);
    enable_device();
    if detect_wheel() {
        PACKET_LEN.store(4, Ordering::Relaxed);
        crate::devices::add("ps2-wheel");
    }
    if detect_touchpad() {
        crate::devices::add("ps2-touchpad");
    }
    let _ = mouse_write(0xF4);
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
    let len = PACKET_LEN.load(Ordering::Relaxed);
    if phase + 1 < len {
        PHASE.store(phase + 1, Ordering::Relaxed);
        return;
    }
    PHASE.store(0, Ordering::Relaxed);
    let flags = PACKET[0].load(Ordering::Relaxed);
    if flags & 0x08 == 0 {
        return;
    }
    let dx = PACKET[1].load(Ordering::Relaxed) as i8;
    let dy = PACKET[2].load(Ordering::Relaxed) as i8;
    crate::event::push(crate::event::Event::PointerRel {
        dx: i16::from(dx),
        dy: -i16::from(dy),
    });
    let left = flags & 1 != 0;
    if left != (BUTTONS.load(Ordering::Relaxed) & 1 != 0) {
        crate::event::push(crate::event::Event::Button { button: 0, down: left });
    }
    if flags & 0x02 != 0 && BUTTONS.load(Ordering::Relaxed) & 0x02 == 0 {
        crate::event::push(crate::event::Event::Button { button: 1, down: true });
    }
    BUTTONS.store(flags & 0x07, Ordering::Relaxed);
    X.fetch_add(i32::from(dx), Ordering::Relaxed);
    Y.fetch_sub(i32::from(dy), Ordering::Relaxed);
    if len >= 4 {
        let z = PACKET[3].load(Ordering::Relaxed) as i8;
        if z != 0 {
            crate::event::push(crate::event::Event::Scroll { dz: z });
        }
    }
}

fn detect_wheel() -> bool {
    if !(mouse_write(0xF3)
        && mouse_write(200)
        && mouse_write(0xF3)
        && mouse_write(100)
        && mouse_write(0xF3)
        && mouse_write(80)
        && mouse_write(0xF2))
    {
        return false;
    }
    try_read() == Some(3)
}

fn detect_touchpad() -> bool {
    if !mouse_write(0xE9) {
        return false;
    }
    let status = try_read();
    let _ = try_read();
    let _ = try_read();
    status == Some(0x47)
}

fn mouse_write(byte: u8) -> bool {
    wait_write();
    out_cmd(0xD4);
    wait_write();
    unsafe { Port::<u8>::new(0x60).write(byte) };
    for _ in 0..20_000 {
        if let Some(answer) = try_read() {
            return answer == 0xFA;
        }
    }
    false
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
