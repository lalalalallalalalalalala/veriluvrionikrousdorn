//! Вход `int 0x80`. Процессор уже переключил стек на RSP0 и положил кадр iret.

use core::sync::atomic::{AtomicUsize, Ordering};

use x86_64::VirtAddr;

#[repr(C)]
pub struct SyscallFrame {
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
}

static HANDLER: AtomicUsize = AtomicUsize::new(0);

pub fn set_handler(handler: unsafe extern "C" fn(*mut SyscallFrame)) {
    HANDLER.store(handler as usize, Ordering::Release);
}

pub fn handler_addr() -> VirtAddr {
    VirtAddr::new(syscall_entry as *const () as u64)
}

core::arch::global_asm!(
    r#"
    .section .text
    .global syscall_entry
    syscall_entry:
        push rax
        push rcx
        push rdx
        push rsi
        push rdi
        push r8
        push r9
        push r10
        push r11
        mov rdi, rsp
        call syscall_rust
        pop r11
        pop r10
        pop r9
        pop r8
        pop rdi
        pop rsi
        pop rdx
        pop rcx
        pop rax
        iretq
    "#
);

unsafe extern "C" {
    fn syscall_entry();
}

#[no_mangle]
pub unsafe extern "C" fn syscall_rust(frame: *mut SyscallFrame) {
    let handler = HANDLER.load(Ordering::Acquire);
    if handler == 0 || frame.is_null() {
        return;
    }
    let handler: unsafe extern "C" fn(*mut SyscallFrame) = unsafe { core::mem::transmute(handler) };
    unsafe { handler(frame) };
}
