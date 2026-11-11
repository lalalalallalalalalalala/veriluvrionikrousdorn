//! Общая очередь байтов от клавиатуры и последовательного порта.
//!
//! `\r` и `\r\n` с терминала превращаются в один `\n`, чтобы оболочка
//! не выполняла пустую строку следом за командой.

use core::sync::atomic::{AtomicBool, Ordering};

use vlr_arch::without_interrupts;

const CAPACITY: usize = 256;

struct Queue {
    buf: [u8; CAPACITY],
    head: usize,
    tail: usize,
    last_was_cr: bool,
}

static mut QUEUE: Queue = Queue {
    buf: [0; CAPACITY],
    head: 0,
    tail: 0,
    last_was_cr: false,
};

static SERIAL_READY: AtomicBool = AtomicBool::new(false);

pub fn note_serial_ready() {
    SERIAL_READY.store(true, Ordering::Release);
}

pub fn push(byte: u8) {
    without_interrupts(|| {
        // Очередь трогают только обработчики IRQ и этот путь с закрытыми прерываниями.
        let queue = unsafe { &mut *core::ptr::addr_of_mut!(QUEUE) };
        let normalized = match byte {
            b'\r' => {
                queue.last_was_cr = true;
                b'\n'
            }
            b'\n' if queue.last_was_cr => {
                queue.last_was_cr = false;
                return;
            }
            other => {
                queue.last_was_cr = false;
                other
            }
        };
        let next = (queue.head + 1) % CAPACITY;
        if next == queue.tail {
            return;
        }
        queue.buf[queue.head] = normalized;
        queue.head = next;
    });
}

pub fn pop() -> Option<u8> {
    without_interrupts(|| {
        let queue = unsafe { &mut *core::ptr::addr_of_mut!(QUEUE) };
        if queue.head == queue.tail {
            None
        } else {
            let byte = queue.buf[queue.tail];
            queue.tail = (queue.tail + 1) % CAPACITY;
            Some(byte)
        }
    })
}

/// Ждать байт. Между `hlt` опрашиваем UART: байт мог прийти до включения IRQ.
pub fn read_byte() -> u8 {
    loop {
        if let Some(byte) = pop() {
            return byte;
        }
        if SERIAL_READY.load(Ordering::Acquire) {
            crate::serial::poll();
            if let Some(byte) = pop() {
                return byte;
            }
        }
        vlr_arch::hlt();
    }
}
