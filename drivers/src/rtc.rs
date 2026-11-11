//! Часы CMOS. Нужны полоске рабочего стола, не планировщику.

use x86_64::instructions::port::Port;

pub struct Clock {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn read() -> Clock {
    while cmos(0x0A) & 0x80 != 0 {}
    let status_b = cmos(0x0B);
    let binary = status_b & 0x04 != 0;
    let hour24 = status_b & 0x02 != 0;
    let mut second = cmos(0x00);
    let mut minute = cmos(0x02);
    let mut hour = cmos(0x04);
    if !binary {
        second = bcd(second);
        minute = bcd(minute);
        hour = bcd(hour & 0x7F) | (hour & 0x80);
    }
    if !hour24 && hour & 0x80 != 0 {
        hour = (hour & 0x7F) + 12;
    }
    Clock {
        hour: hour % 24,
        minute: minute % 60,
        second: second % 60,
    }
}

fn cmos(index: u8) -> u8 {
    unsafe {
        Port::<u8>::new(0x70).write(index);
        Port::<u8>::new(0x71).read()
    }
}

fn bcd(value: u8) -> u8 {
    (value & 0x0F) + ((value >> 4) & 0x0F) * 10
}
