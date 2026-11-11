//! Архитектура x86-64: GDT, IDT, PIC и обработчики исключений процессора.
//!
//! Драйверы регистрируют обработчики IRQ и не знают, как устроена таблица
//! прерываний. Вывод текста сюда не входит: ядро подставляет функцию журнала.

#![no_std]
#![feature(abi_x86_interrupt)]

mod gdt;
mod idt;
mod pic;

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;
use x86_64::instructions::interrupts;

/// Сколько линий IRQ обслуживает пара PIC 8259.
const IRQ_COUNT: usize = 16;

static IRQ_HANDLERS: [AtomicUsize; IRQ_COUNT] = [const { AtomicUsize::new(0) }; IRQ_COUNT];
static LOGGER: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// Подготовить GDT, IDT и переназначить PIC. Прерывания остаются закрытыми,
/// линии IRQ замаскированы, пока драйверы не вызовут [`unmask_irq`].
pub fn init() {
    disable_interrupts();
    gdt::init();
    idt::init();
    pic::init();
}

/// Запомнить обработчик линии IRQ (`0..16`). Вызывать до [`enable_interrupts`].
pub fn register_irq(irq: u8, handler: fn()) {
    assert!((irq as usize) < IRQ_COUNT, "irq out of range");
    IRQ_HANDLERS[irq as usize].store(handler as usize, Ordering::Release);
}

pub(crate) fn dispatch_irq(irq: u8) {
    let ptr = IRQ_HANDLERS[irq as usize].load(Ordering::Acquire);
    if ptr != 0 {
        let handler: fn() = unsafe { core::mem::transmute(ptr) };
        handler();
    }
    pic::eoi(irq);
}

/// Разрешить доставку указанной линии IRQ.
pub fn unmask_irq(irq: u8) {
    pic::unmask(irq);
}

/// Куда писать сообщения исключений. Строка действительна только на время вызова.
pub fn set_exception_logger(logger: fn(&str)) {
    *LOGGER.lock() = Some(logger);
}

/// После сообщения о page fault вызвать этот хук вместо остановки.
/// Нужен загрузочному тесту: обработчик печатает адрес и завершает QEMU.
pub fn set_page_fault_hook(hook: fn(u64) -> !) {
    idt::set_page_fault_hook(hook);
}

pub(crate) fn log_exception(args: fmt::Arguments) {
    let mut raw = [0u8; 384];
    let used = {
        let mut writer = SliceWriter {
            buf: &mut raw,
            used: 0,
        };
        let _ = fmt::write(&mut writer, args);
        writer.used
    };
    let message = core::str::from_utf8(&raw[..used]).unwrap_or("exception");
    if let Some(logger) = *LOGGER.lock() {
        logger(message);
    }
}

pub fn enable_interrupts() {
    interrupts::enable();
}

pub fn disable_interrupts() {
    interrupts::disable();
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    interrupts::without_interrupts(f)
}

pub fn hlt() {
    x86_64::instructions::hlt();
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

/// Сообщить QEMU (`isa-debug-exit` на порту `0xf4`) об успехе или ошибке и остановиться.
/// Успех — код выхода QEMU 33 (`0x10 << 1 | 1`).
pub fn debug_exit(success: bool) -> ! {
    let code: u32 = if success { 0x10 } else { 0x11 };
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(0xf4);
        port.write(code);
    }
    hlt_loop();
}

struct SliceWriter<'a> {
    buf: &'a mut [u8],
    used: usize,
}

impl fmt::Write for SliceWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let bytes = text.as_bytes();
        let room = self.buf.len().saturating_sub(self.used);
        let mut count = bytes.len().min(room);
        while count > 0 && (bytes[count - 1] & 0b1100_0000) == 0b1000_0000 {
            count -= 1;
        }
        self.buf[self.used..self.used + count].copy_from_slice(&bytes[..count]);
        self.used += count;
        Ok(())
    }
}
