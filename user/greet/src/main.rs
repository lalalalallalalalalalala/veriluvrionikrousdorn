#![no_std]
#![no_main]

use vlr_libc::{sleep, write, FD_STDOUT};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        let _ = write(FD_STDOUT, b"greet from ring 3\n");
        sleep(40);
    }
}
