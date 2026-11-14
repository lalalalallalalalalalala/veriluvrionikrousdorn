//! Список найденных устройств ввода и дисплея.

const MAX: usize = 12;
const NAME: usize = 24;

struct Table {
    names: [[u8; NAME]; MAX],
    lens: [u8; MAX],
    n: usize,
}

static mut TABLE: Table = Table {
    names: [[0; NAME]; MAX],
    lens: [0; MAX],
    n: 0,
};

pub fn add(name: &str) {
    unsafe {
        if TABLE.n >= MAX || name.is_empty() {
            return;
        }
        for i in 0..TABLE.n {
            let len = TABLE.lens[i] as usize;
            if core::str::from_utf8(&TABLE.names[i][..len]).ok() == Some(name) {
                return;
            }
        }
        let n = name.len().min(NAME);
        TABLE.names[TABLE.n][..n].copy_from_slice(&name.as_bytes()[..n]);
        TABLE.lens[TABLE.n] = n as u8;
        TABLE.n += 1;
    }
}

pub fn list_into(buf: &mut [u8]) -> usize {
    unsafe {
        let mut at = 0usize;
        for i in 0..TABLE.n {
            let name = &TABLE.names[i][..TABLE.lens[i] as usize];
            if at + name.len() + 1 > buf.len() {
                break;
            }
            buf[at..at + name.len()].copy_from_slice(name);
            at += name.len();
            buf[at] = b'\n';
            at += 1;
        }
        at
    }
}
