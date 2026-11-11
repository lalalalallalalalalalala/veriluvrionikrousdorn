//! Архитектура x86-64: GDT, IDT, PIC и обработчики исключений процессора.
//!
//! Драйверы регистрируют обработчики IRQ и не знают, как устроена таблица
//! прерываний. Вывод текста сюда не входит: ядро подставляет функцию журнала.

#![no_std]
#![feature(abi_x86_interrupt)]

mod gdt;
mod idt;
mod pic;
pub mod syscall_entry;

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

static PREEMPT: AtomicUsize = AtomicUsize::new(0);

/// Вызывается после EOI таймера, уже с закрытыми прерываниями.
/// Планировщик переключает задачу здесь, чтобы следующий тик не ждал iret.
pub fn set_preempt_hook(hook: fn()) {
    PREEMPT.store(hook as usize, Ordering::Release);
}

pub(crate) fn dispatch_irq(irq: u8) {
    let ptr = IRQ_HANDLERS[irq as usize].load(Ordering::Acquire);
    if ptr != 0 {
        let handler: fn() = unsafe { core::mem::transmute(ptr) };
        handler();
    }
    pic::eoi(irq);
    if irq == 0 {
        let hook = PREEMPT.load(Ordering::Acquire);
        if hook != 0 {
            let hook: fn() = unsafe { core::mem::transmute(hook) };
            hook();
        }
    }
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

/// Сбой задачи кольца 3: ядро живо, задачу нужно снять.
pub fn set_user_fault_hook(hook: fn(u64, &'static str) -> !) {
    idt::set_user_fault_hook(hook);
}

pub fn set_syscall_handler(handler: unsafe extern "C" fn(*mut syscall_entry::SyscallFrame)) {
    syscall_entry::set_handler(handler);
}

pub fn user_cs() -> u16 {
    gdt::user_cs()
}

pub fn user_ss() -> u16 {
    gdt::user_ss()
}

pub fn set_kernel_stack(top: u64) {
    gdt::set_kernel_stack(top);
}

pub unsafe fn enter_user(rip: u64, user_rsp: u64, kernel_rsp: u64) -> ! {
    let cs = user_cs() as u64;
    let ss = user_ss() as u64;
    let rflags = 0x202u64;
    unsafe {
        core::arch::asm!(
            "mov rsp, {kernel_rsp}",
            "push {ss}",
            "push {user_rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            kernel_rsp = in(reg) kernel_rsp,
            ss = in(reg) ss,
            user_rsp = in(reg) user_rsp,
            rflags = in(reg) rflags,
            cs = in(reg) cs,
            rip = in(reg) rip,
            options(noreturn),
        );
    }
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
