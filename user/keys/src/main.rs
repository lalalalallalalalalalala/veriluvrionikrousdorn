#![no_std]
#![no_main]

use vlr_libc::{win_open, win_poll, write, yield_now, FD_STDOUT};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let id = win_open("keys", 300, 180);
    if id < 0 {
        let _ = write(FD_STDOUT, b"keys-fail\n");
        vlr_libc::exit(1);
    }
    let _ = write(FD_STDOUT, b"keys-open\n");
    let mut line = [0u8; 32];
    let mut len = 0usize;
    loop {
        let ev = win_poll(id as u64);
        if ev < 0 {
            yield_now();
            continue;
        }
        let byte = ev as u8;
        if byte == b'\n' {
            let _ = write(FD_STDOUT, b"keys:");
            let _ = write(FD_STDOUT, &line[..len]);
            let _ = write(FD_STDOUT, b"\n");
            len = 0;
        } else if byte >= 32 && len < line.len() {
            line[len] = byte;
            len += 1;
        }
    }
}
