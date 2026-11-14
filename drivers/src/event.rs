//! Одна очередь событий ввода для `vlr-wm`.
//!
//! Клавиатура, мышь, планшет, перо, тачпад и последовательный порт кладут
//! сюда одно и то же перечисление.

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use vlr_arch::without_interrupts;

#[derive(Clone, Copy)]
pub enum Event {
    Key {
        down: bool,
        usage: u16,
        text: [u8; 4],
        text_len: u8,
    },
    PointerRel { dx: i16, dy: i16 },
    PointerAbs { x: i32, y: i32 },
    Touch { id: u8, x: i32, y: i32, down: bool },
    Pen {
        x: i32,
        y: i32,
        pressure: u16,
        tilt_x: i8,
        tilt_y: i8,
        down: bool,
    },
    Scroll { dz: i8 },
    Button { button: u8, down: bool },
}

const EMPTY: Event = Event::Key {
    down: false,
    usage: 0,
    text: [0; 4],
    text_len: 0,
};
const CAP: usize = 64;

struct Queue {
    buf: [Event; CAP],
    head: usize,
    tail: usize,
    extra: [u8; 4],
    extra_len: u8,
    extra_at: u8,
    last_cr: bool,
}

static mut QUEUE: Queue = Queue {
    buf: [EMPTY; CAP],
    head: 0,
    tail: 0,
    extra: [0; 4],
    extra_len: 0,
    extra_at: 0,
    last_cr: false,
};

static CUR_X: AtomicI32 = AtomicI32::new(64);
static CUR_Y: AtomicI32 = AtomicI32::new(64);
static CUR_LEFT: AtomicBool = AtomicBool::new(false);

pub fn cursor() -> (i32, i32, bool) {
    (
        CUR_X.load(Ordering::Relaxed),
        CUR_Y.load(Ordering::Relaxed),
        CUR_LEFT.load(Ordering::Relaxed),
    )
}

pub fn push(event: Event) {
    without_interrupts(|| unsafe {
        let queue = &mut *core::ptr::addr_of_mut!(QUEUE);
        let next = (queue.head + 1) % CAP;
        if next == queue.tail {
            return;
        }
        queue.buf[queue.head] = event;
        queue.head = next;
    });
}

pub fn pop_event() -> Option<Event> {
    without_interrupts(|| unsafe {
        let queue = &mut *core::ptr::addr_of_mut!(QUEUE);
        if queue.head == queue.tail {
            None
        } else {
            let event = queue.buf[queue.tail];
            queue.tail = (queue.tail + 1) % CAP;
            Some(event)
        }
    })
}

pub fn push_byte(byte: u8) {
    push(Event::Key {
        down: true,
        usage: 0,
        text: [byte, 0, 0, 0],
        text_len: 1,
    });
}

/// Байт для оболочки. Указатель и колесо здесь не отдаются.
pub fn pop_byte() -> Option<u8> {
    without_interrupts(|| unsafe {
        let queue = &mut *core::ptr::addr_of_mut!(QUEUE);
        if queue.extra_at < queue.extra_len {
            let byte = queue.extra[queue.extra_at as usize];
            queue.extra_at += 1;
            return Some(byte);
        }
        loop {
            if queue.head == queue.tail {
                return None;
            }
            let event = queue.buf[queue.tail];
            queue.tail = (queue.tail + 1) % CAP;
            let Event::Key {
                down: true,
                text,
                text_len,
                ..
            } = event
            else {
                continue;
            };
            if text_len == 0 {
                continue;
            }
            let first = match text[0] {
                b'\r' => {
                    queue.last_cr = true;
                    b'\n'
                }
                b'\n' if queue.last_cr => {
                    queue.last_cr = false;
                    continue;
                }
                other => {
                    queue.last_cr = false;
                    other
                }
            };
            if text_len > 1 {
                queue.extra = text;
                queue.extra[0] = first;
                queue.extra_len = text_len;
                queue.extra_at = 1;
            }
            return Some(first);
        }
    })
}

pub fn apply(event: &Event) {
    let speed = i32::from(super::settings::speed());
    match *event {
        Event::PointerRel { dx, dy } => {
            CUR_X.fetch_add(i32::from(dx) * speed, Ordering::Relaxed);
            CUR_Y.fetch_add(i32::from(dy) * speed, Ordering::Relaxed);
            clamp_cursor();
        }
        Event::PointerAbs { x, y } => move_abs(x, y),
        Event::Touch { x, y, down, .. } => {
            move_abs(x, y);
            CUR_LEFT.store(down, Ordering::Relaxed);
        }
        Event::Pen { x, y, down, .. } => {
            move_abs(x, y);
            CUR_LEFT.store(down, Ordering::Relaxed);
        }
        Event::Button { button: 0, down } => CUR_LEFT.store(down, Ordering::Relaxed),
        Event::Scroll { dz } => {
            super::serial::write_str(if dz < 0 { "scroll -1\n" } else { "scroll 1\n" });
        }
        _ => {}
    }
}

fn move_abs(x: i32, y: i32) {
    let prev_x = CUR_X.load(Ordering::Relaxed);
    let prev_y = CUR_Y.load(Ordering::Relaxed);
    CUR_X.store(x, Ordering::Relaxed);
    CUR_Y.store(y, Ordering::Relaxed);
    clamp_cursor();
    let (nx, ny, _) = cursor();
    if (nx - prev_x).abs() + (ny - prev_y).abs() < 2 {
        return;
    }
    super::serial::write_str("ptr ");
    write_i32(nx);
    super::serial::write_str(" ");
    write_i32(ny);
    super::serial::write_str("\n");
}

fn clamp_cursor() {
    let Some((w, h)) = super::display::size() else {
        return;
    };
    let mut x = CUR_X.load(Ordering::Relaxed);
    let mut y = CUR_Y.load(Ordering::Relaxed);
    x = x.clamp(0, (w as i32).saturating_sub(1));
    y = y.clamp(0, (h as i32).saturating_sub(1));
    CUR_X.store(x, Ordering::Relaxed);
    CUR_Y.store(y, Ordering::Relaxed);
}

fn write_i32(mut value: i32) {
    if value < 0 {
        super::serial::write_str("-");
        value = value.saturating_neg();
    }
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    let mut n = value as u32;
    if n == 0 {
        super::serial::write_str("0");
        return;
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    super::serial::write_str(core::str::from_utf8(&buf[i..]).unwrap_or("0"));
}

pub fn read_byte() -> u8 {
    loop {
        super::usb_hid::poll();
        if let Some(byte) = pop_byte() {
            return byte;
        }
        super::serial::poll();
        if let Some(byte) = pop_byte() {
            return byte;
        }
        vlr_arch::hlt();
    }
}
