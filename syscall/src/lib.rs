//! Диспетчер `int 0x80`. Номера те же, что в `vlr-libc`.
//!
//! Указатели из кольца 3 принимаются только если каждый байт лежит в окне
//! пользователя и страница помечена USER. Иначе вызов возвращает `-1`, а ядро
//! не читает чужую память.

#![no_std]

use vlr_arch::syscall_entry::SyscallFrame;
use vlr_libc::{
    FD_STDIN, FD_STDOUT, SYS_EXIT, SYS_GETPID, SYS_GETTICKS, SYS_READ, SYS_SLEEP, SYS_WRITE,
    SYS_YIELD,
};

const FAULT: u64 = (-1i64) as u64;

pub fn init() {
    vlr_arch::set_syscall_handler(handle);
}

unsafe extern "C" fn handle(frame: *mut SyscallFrame) {
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *frame };
    let result = dispatch(frame.rax, frame.rdi, frame.rsi, frame.rdx);
    frame.rax = result;
}

fn dispatch(number: u64, a: u64, b: u64, c: u64) -> u64 {
    match number {
        SYS_READ => sys_read(a, b, c),
        SYS_WRITE => sys_write(a, b, c),
        SYS_EXIT => {
            let _ = a;
            vlr_sched::exit();
        }
        SYS_YIELD => {
            vlr_sched::yield_now();
            0
        }
        SYS_SLEEP => {
            vlr_sched::sleep(a);
            0
        }
        SYS_GETPID => vlr_sched::current_id(),
        SYS_GETTICKS => vlr_drivers::timer::ticks(),
        _ => FAULT,
    }
}

fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != FD_STDOUT {
        return FAULT;
    }
    let len = len.min(4096) as usize;
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut buf = [0u8; 256];
    let mut done = 0usize;
    while done < len {
        let chunk = (len - done).min(buf.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                (ptr + done as u64) as *const u8,
                buf.as_mut_ptr(),
                chunk,
            );
        }
        let text = core::str::from_utf8(&buf[..chunk]).unwrap_or("?");
        vlr_console::write_str(text);
        done += chunk;
    }
    len as u64
}

fn sys_read(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != FD_STDIN {
        return FAULT;
    }
    let len = len.min(4096) as usize;
    if len == 0 {
        return 0;
    }
    if !vlr_memory::is_user_range(ptr, len) {
        return FAULT;
    }
    let mut done = 0usize;
    while done < len {
        if let Some(byte) = vlr_drivers::input::pop() {
            unsafe {
                ((ptr + done as u64) as *mut u8).write(byte);
            }
            done += 1;
            if byte == b'\n' {
                break;
            }
        } else if done > 0 {
            break;
        } else {
            vlr_sched::sleep(1);
        }
    }
    done as u64
}
