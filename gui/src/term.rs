//! Кольцо текста терминала. Пишется из `console` с закрытыми прерываниями.

const CAP: usize = 4096;
static mut BUF: [u8; CAP] = [0; CAP];
static mut HEAD: usize = 0;
static mut TAIL: usize = 0;

pub fn push(text: &str) {
    unsafe {
        for byte in text.bytes() {
            let next = (HEAD + 1) % CAP;
            if next == TAIL {
                TAIL = (TAIL + 1) % CAP;
            }
            BUF[HEAD] = byte;
            HEAD = next;
        }
    }
}

pub fn fingerprint() -> u64 {
    vlr_arch::without_interrupts(|| unsafe {
        let mut hash = HEAD as u64 ^ ((TAIL as u64) << 16);
        let mut tail = TAIL;
        let mut n = 0u64;
        while tail != HEAD && n < 64 {
            hash = hash.wrapping_mul(131).wrapping_add(BUF[tail] as u64);
            tail = (tail + 1) % CAP;
            n += 1;
        }
        hash ^ (n << 32)
    })
}

pub fn snapshot() -> alloc::string::String {
    vlr_arch::without_interrupts(|| unsafe {
        let mut raw = alloc::vec::Vec::new();
        let mut tail = TAIL;
        while tail != HEAD {
            let byte = BUF[tail];
            tail = (tail + 1) % CAP;
            if byte == 0x0c {
                raw.clear();
            } else if byte == 0x08 {
                pop_utf8(&mut raw);
            } else {
                raw.push(byte);
            }
        }
        alloc::string::String::from_utf8_lossy(&raw).into_owned()
    })
}

fn pop_utf8(raw: &mut alloc::vec::Vec<u8>) {
    loop {
        let Some(byte) = raw.pop() else {
            return;
        };
        if byte & 0xC0 != 0x80 {
            return;
        }
    }
}
