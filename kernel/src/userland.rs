//! Встроенные программы кольца 3. Это не `.vlr`: байты лежат в образе ядра,
//! пока нет диска и загрузчика. ABI тот же, что у `vlr-libc` (`int 0x80`).

use vlr_memory::PAGE_SIZE;

// hello: write(1, "hello from ring 3\n"); exit(0)
static HELLO: &[u8] = &[
    0xb8, 0x02, 0x00, 0x00, 0x00, 0xbf, 0x01, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x35, 0x10, 0x00, 0x00,
    0x00, 0xba, 0x12, 0x00, 0x00, 0x00, 0xcd, 0x80, 0xb8, 0x03, 0x00, 0x00, 0x00, 0x31, 0xff, 0xcd,
    0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x66, 0x72, 0x6f, 0x6d, 0x20, 0x72, 0x69, 0x6e, 0x67,
    0x20, 0x33, 0x0a,
];

// greet: write the line, then sleep(50) forever so `ps` still shows it.
#[cfg(feature = "shell")]
static GREET: &[u8] = &[
    0xb8, 0x02, 0x00, 0x00, 0x00, 0xbf, 0x01, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x35, 0x15, 0x00, 0x00,
    0x00, 0xba, 0x12, 0x00, 0x00, 0x00, 0xcd, 0x80, 0xb8, 0x05, 0x00, 0x00, 0x00, 0xbf, 0x32, 0x00,
    0x00, 0x00, 0xcd, 0x80, 0xeb, 0xf2, 0x67, 0x72, 0x65, 0x65, 0x74, 0x20, 0x66, 0x72, 0x6f, 0x6d,
    0x20, 0x72, 0x69, 0x6e, 0x67, 0x20, 0x33, 0x0a,
];

// bad: read address 0. The page is not mapped, the task is killed.
static BAD: &[u8] = &[0x31, 0xc0, 0x48, 0x8b, 0x00, 0x0f, 0x0b];

pub fn launch_hello() {
    enter(HELLO);
}

#[cfg(feature = "shell")]
pub fn launch_greet() {
    enter(GREET);
}

pub fn launch_bad() {
    enter(BAD);
}

#[cfg(feature = "shell")]
pub fn launch_named(name: &str) -> Result<u64, &'static str> {
    let entry = match name {
        "hello" => launch_hello,
        "greet" => launch_greet,
        "bad" => launch_bad,
        _ => return Err("unknown program"),
    };
    vlr_sched::spawn(name, entry)
        .map(|id| id.0)
        .map_err(|_| "spawn failed")
}

fn enter(code: &[u8]) {
    if code.len() > PAGE_SIZE as usize {
        vlr_sched::exit();
    }
    let Ok(cr3) = vlr_memory::create_user_space() else {
        vlr_sched::exit();
    };
    let Some(code_frame) = vlr_memory::alloc_frame() else {
        vlr_sched::exit();
    };
    let Some(stack_frame) = vlr_memory::alloc_frame() else {
        let _ = vlr_memory::free_frame(code_frame);
        vlr_sched::exit();
    };
    unsafe {
        let code_dst = vlr_memory::phys_to_virt(code_frame).unwrap_or(0) as *mut u8;
        let stack_dst = vlr_memory::phys_to_virt(stack_frame).unwrap_or(0) as *mut u8;
        if code_dst.is_null() || stack_dst.is_null() {
            vlr_sched::exit();
        }
        core::ptr::write_bytes(code_dst, 0, PAGE_SIZE as usize);
        core::ptr::write_bytes(stack_dst, 0, PAGE_SIZE as usize);
        core::ptr::copy_nonoverlapping(code.as_ptr(), code_dst, code.len());
    }
    if vlr_memory::map_user_page(cr3, vlr_memory::user_code(), code_frame, false, true).is_err()
        || vlr_memory::map_user_page(cr3, vlr_memory::user_stack_page(), stack_frame, true, false)
            .is_err()
    {
        vlr_sched::exit();
    }
    vlr_arch::disable_interrupts();
    vlr_sched::become_user(cr3);
    vlr_memory::activate_cr3(cr3);
    let kernel_stack = vlr_sched::kernel_stack_top();
    vlr_arch::set_kernel_stack(kernel_stack);
    unsafe {
        vlr_arch::enter_user(
            vlr_memory::user_code(),
            vlr_memory::user_stack_top(),
            kernel_stack,
        );
    }
}
