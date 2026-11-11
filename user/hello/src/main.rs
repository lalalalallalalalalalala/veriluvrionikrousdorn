#![no_std]
#![no_main]

use vlr_libc::{exit, write, FD_STDOUT};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = write(FD_STDOUT, b"hello from ring 3\n");
    exit(0);
}
