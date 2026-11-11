use spin::{Mutex, Once};
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::PrivilegeLevel;

use crate::gdt::DOUBLE_FAULT_IST_INDEX;

static IDT: Once<InterruptDescriptorTable> = Once::new();
static PAGE_FAULT_HOOK: Mutex<Option<fn(u64) -> !>> = Mutex::new(None);
static USER_FAULT_HOOK: Mutex<Option<fn(u64, &'static str) -> !>> = Mutex::new(None);

pub fn set_page_fault_hook(hook: fn(u64) -> !) {
    *PAGE_FAULT_HOOK.lock() = Some(hook);
}

pub fn set_user_fault_hook(hook: fn(u64, &'static str) -> !) {
    *USER_FAULT_HOOK.lock() = Some(hook);
}

fn from_user(frame: &InterruptStackFrame) -> bool {
    frame.code_segment.rpl() == PrivilegeLevel::Ring3
}

fn user_fault(addr: u64, kind: &'static str) -> ! {
    let hook = *USER_FAULT_HOOK.lock();
    if let Some(hook) = hook {
        hook(addr, kind);
    }
    crate::hlt_loop();
}

pub fn init() {
    idt().load();
}

fn idt() -> &'static InterruptDescriptorTable {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt[0x80]
                .set_handler_addr(crate::syscall_entry::handler_addr())
                .set_privilege_level(PrivilegeLevel::Ring3);
        }
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt[32].set_handler_fn(irq0);
        idt[33].set_handler_fn(irq1);
        idt[34].set_handler_fn(irq2);
        idt[35].set_handler_fn(irq3);
        idt[36].set_handler_fn(irq4);
        idt[37].set_handler_fn(irq5);
        idt[38].set_handler_fn(irq6);
        idt[39].set_handler_fn(irq7);
        idt[40].set_handler_fn(irq8);
        idt[41].set_handler_fn(irq9);
        idt[42].set_handler_fn(irq10);
        idt[43].set_handler_fn(irq11);
        idt[44].set_handler_fn(irq12);
        idt[45].set_handler_fn(irq13);
        idt[46].set_handler_fn(irq14);
        idt[47].set_handler_fn(irq15);
        idt
    })
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::log_exception(format_args!("exception: breakpoint\n{frame:?}\n"));
}

extern "x86-interrupt" fn page_fault_handler(
    frame: InterruptStackFrame,
    error: PageFaultErrorCode,
) {
    let address = Cr2::read_raw();
    if from_user(&frame) {
        user_fault(address, "page-fault");
    }
    crate::log_exception(format_args!(
        "exception: page fault at {address:#x} error={error:?}\n{frame:?}\n"
    ));
    let hook = *PAGE_FAULT_HOOK.lock();
    if let Some(hook) = hook {
        hook(address);
    }
    crate::hlt_loop();
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    if from_user(&frame) {
        user_fault(frame.instruction_pointer.as_u64(), "invalid-opcode");
    }
    crate::log_exception(format_args!("exception: invalid opcode\n{frame:?}\n"));
    crate::hlt_loop();
}

extern "x86-interrupt" fn general_protection_handler(frame: InterruptStackFrame, error: u64) {
    if from_user(&frame) {
        user_fault(frame.instruction_pointer.as_u64(), "general-protection");
    }
    crate::log_exception(format_args!(
        "exception: general protection error={error:#x}\n{frame:?}\n"
    ));
    crate::hlt_loop();
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error: u64) -> ! {
    crate::log_exception(format_args!(
        "exception: double fault error={error:#x}\n{frame:?}\n"
    ));
    crate::hlt_loop();
}

macro_rules! irq_handler {
    ($name:ident, $irq:literal) => {
        extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
            crate::dispatch_irq($irq);
        }
    };
}

irq_handler!(irq0, 0);
irq_handler!(irq1, 1);
irq_handler!(irq2, 2);
irq_handler!(irq3, 3);
irq_handler!(irq4, 4);
irq_handler!(irq5, 5);
irq_handler!(irq6, 6);
irq_handler!(irq7, 7);
irq_handler!(irq8, 8);
irq_handler!(irq9, 9);
irq_handler!(irq10, 10);
irq_handler!(irq11, 11);
irq_handler!(irq12, 12);
irq_handler!(irq13, 13);
irq_handler!(irq14, 14);
irq_handler!(irq15, 15);
