use x86_64::instructions::port::Port;

const MASTER_CMD: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_CMD: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;
const EOI: u8 = 0x20;

/// Переназначить векторы IRQ на 32..47 и замаскировать все линии.
pub fn init() {
    unsafe {
        let mut master_cmd = Port::<u8>::new(MASTER_CMD);
        let mut master_data = Port::<u8>::new(MASTER_DATA);
        let mut slave_cmd = Port::<u8>::new(SLAVE_CMD);
        let mut slave_data = Port::<u8>::new(SLAVE_DATA);

        master_cmd.write(0x11);
        io_wait();
        slave_cmd.write(0x11);
        io_wait();
        master_data.write(32);
        io_wait();
        slave_data.write(40);
        io_wait();
        master_data.write(4);
        io_wait();
        slave_data.write(2);
        io_wait();
        master_data.write(0x01);
        io_wait();
        slave_data.write(0x01);
        io_wait();
        master_data.write(0xFF);
        slave_data.write(0xFF);
    }
}

pub fn unmask(irq: u8) {
    let (port, shift) = if irq < 8 {
        (MASTER_DATA, irq)
    } else {
        (SLAVE_DATA, irq - 8)
    };
    unsafe {
        let mut data = Port::<u8>::new(port);
        let mask = data.read() & !(1 << shift);
        data.write(mask);
    }
}

pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            Port::<u8>::new(SLAVE_CMD).write(EOI);
        }
        Port::<u8>::new(MASTER_CMD).write(EOI);
    }
}

fn io_wait() {
    unsafe {
        Port::<u8>::new(0x80).write(0);
    }
}
