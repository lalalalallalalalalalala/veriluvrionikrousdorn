//! Конфигурационное пространство PCI, порты `0xCF8`/`0xCFC`.
//!
//! Ищем только класс xHCI: `0x0C` / `0x03` / `0x30`. BAR отображается
//! как MMIO через линейное смещение загрузчика.

use x86_64::instructions::port::Port;

use vlr_memory::MapError;

pub struct XhciBar {
    pub phys: u64,
    pub virt: u64,
    pub len: u64,
}

pub fn find_xhci() -> Result<XhciBar, &'static str> {
    for bus in 0..8u8 {
        for dev in 0..32u8 {
            let header = read32(bus, dev, 0, 0x0C);
            let multi = (header >> 16) & 0x80 != 0;
            let funcs = if multi { 8 } else { 1 };
            for func in 0..funcs {
                let vendor = read32(bus, dev, func, 0x00);
                if vendor == 0 || vendor == 0xFFFF_FFFF {
                    continue;
                }
                let class = read32(bus, dev, func, 0x08);
                let base = (class >> 24) & 0xFF;
                let sub = (class >> 16) & 0xFF;
                let prog = (class >> 8) & 0xFF;
                if base != 0x0C || sub != 0x03 || prog != 0x30 {
                    continue;
                }
                return map_bar0(bus, dev, func);
            }
        }
    }
    Err("no xhci")
}

fn map_bar0(bus: u8, dev: u8, func: u8) -> Result<XhciBar, &'static str> {
    let bar0 = read32(bus, dev, func, 0x10);
    if bar0 & 1 != 0 {
        return Err("io bar");
    }
    let wide = ((bar0 >> 1) & 0x3) == 0x2;
    let bar1 = if wide { read32(bus, dev, func, 0x14) } else { 0 };
    let phys = (bar0 as u64 & 0xFFFF_FFF0) | ((bar1 as u64) << 32);
    if phys == 0 {
        return Err("bar0");
    }
    write32(bus, dev, func, 0x10, 0xFFFF_FFFF);
    if wide {
        write32(bus, dev, func, 0x14, 0xFFFF_FFFF);
    }
    let mask0 = read32(bus, dev, func, 0x10) & 0xFFFF_FFF0;
    let mask1 = if wide { read32(bus, dev, func, 0x14) } else { 0 };
    write32(bus, dev, func, 0x10, bar0);
    if wide {
        write32(bus, dev, func, 0x14, bar1);
    }
    let mask = (mask0 as u64) | ((mask1 as u64) << 32);
    let size = (!mask).wrapping_add(1).max(0x1000).min(128 * 1024);
    let cmd = read32(bus, dev, func, 0x04);
    write32(bus, dev, func, 0x04, cmd | 0x6);
    let virt = vlr_memory::map_mmio(phys, size).map_err(map_err)?;
    Ok(XhciBar { phys, virt, len: size })
}

fn map_err(err: MapError) -> &'static str {
    match err {
        MapError::OutOfFrames => "mmio frames",
        _ => "mmio",
    }
}

fn addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000 | ((bus as u32) << 16) | ((dev as u32) << 11) | ((func as u32) << 8) | (offset as u32 & 0xFC)
}

fn read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        Port::<u32>::new(0xCF8).write(addr(bus, dev, func, offset));
        Port::<u32>::new(0xCFC).read()
    }
}

fn write32(bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
    unsafe {
        Port::<u32>::new(0xCF8).write(addr(bus, dev, func, offset));
        Port::<u32>::new(0xCFC).write(value);
    }
}
