//! Хост xHCI: кольца, сброс портов, адресация, control и bulk.
//!
//! События читаются опросом, без MSI. Буферы DMA — кадры `vlr-memory`,
//! уже отображённые загрузчиком. Прерывания на время передачи выключает
//! вызывающий `usb_msc`.

use core::sync::atomic::{fence, Ordering};

use spin::Mutex;

use super::hid::{self, Map};
use super::pci;

const TRBS: usize = 256;

struct Event {
    param: u64,
    status: u32,
    control: u32,
}

struct Page {
    virt: *mut u8,
    phys: u64,
}

struct Ring {
    trb: *mut Trb,
    phys: u64,
    enq: usize,
    cycle: u32,
}

#[repr(C)]
struct Trb {
    param: u64,
    status: u32,
    control: u32,
}

struct HidDev {
    live: bool,
    slot: u8,
    dci: u8,
    mps: u16,
    map: Map,
    ring: Ring,
    buf: Page,
    prev: [u16; 6],
    buttons: u8,
    tip: bool,
    last_x: i32,
    last_y: i32,
}

pub struct Host {
    cap: *mut u8,
    op: usize,
    db: usize,
    rt: usize,
    ports: u8,
    ctx: usize,
    slot: u8,
    msc_slot: u8,
    in_dci: u8,
    out_dci: u8,
    blocks: u64,
    block_size: u32,
    hid_n: u8,
    hids: [Option<HidDev>; 3],
    kept_out: [Option<Page>; 4],
    kept_ep0: [Option<Ring>; 4],
    kept_n: u8,
    cmd: Ring,
    ep0: Ring,
    bulk_in: Ring,
    bulk_out: Ring,
    event: *mut Trb,
    event_phys: u64,
    ev_deq: usize,
    ev_cycle: u32,
    dcbaa: Page,
    input: Page,
    output: Page,
    scratch: Page,
    bounce: Page,
    tag: u32,
}

unsafe impl Send for Host {}

pub struct Probe {
    pub host: Host,
    pub blocks: u64,
    pub block_size: u32,
}

static EXTRA: Mutex<Option<Host>> = Mutex::new(None);

pub fn poll_extra() {
    if let Some(host) = EXTRA.lock().as_mut() {
        host.poll_hid();
    }
}

pub fn probe_msc() -> Result<Probe, &'static str> {
    let bar = pci::find_xhci()?;
    let mut host = Host::start(bar.virt)?;
    crate::serial::write_str("usb-host\n");
    host.power_ports();
    for port in 1..=host.ports {
        if !host.port_connected(port) {
            continue;
        }
        match host.open_port(port) {
            Ok(true) => {
                if host.blocks == 0 {
                    if let Ok((blocks, block_size)) = host.capacity() {
                        host.blocks = blocks;
                        host.block_size = block_size;
                        crate::serial::write_str("usb-msc\n");
                    }
                }
                host.park()?;
            }
            Ok(false) => {
                host.park()?;
            }
            Err(err) => {
                crate::serial::write_str("usb-err ");
                crate::serial::write_str(err);
                crate::serial::write_str("\n");
            }
        }
    }
    host.arm_all();
    if host.blocks > 0 {
        let blocks = host.blocks;
        let block_size = host.block_size;
        return Ok(Probe {
            host,
            blocks,
            block_size,
        });
    }
    if host.hid_n > 0 {
        *EXTRA.lock() = Some(host);
    }
    crate::serial::write_str("usb-skip\n");
    Err("no msc")
}

impl Host {
    fn start(virt: u64) -> Result<Self, &'static str> {
        let cap = virt as *mut u8;
        let caplen = unsafe { cap.read_volatile() } as usize;
        if !(0x20..=0x100).contains(&caplen) {
            return Err("caplen");
        }
        let hcs1 = rd32(cap, 0x04);
        let hcs2 = rd32(cap, 0x08);
        let hcc1 = rd32(cap, 0x10);
        let ports = ((hcs1 >> 24) & 0xFF) as u8;
        let max_slots = (hcs1 & 0xFF) as u8;
        if ports == 0 || max_slots == 0 {
            return Err("params");
        }
        let scratch_n = scratchpad_count(hcs2);
        if scratch_n > 4 {
            return Err("scratchpad");
        }
        let dcbaa = dma_page()?;
        let cmd_page = dma_page()?;
        let event_page = dma_page()?;
        let erst = dma_page()?;
        let input = dma_page()?;
        let output = dma_page()?;
        let ep0 = dma_page()?;
        let bulk_in = dma_page()?;
        let bulk_out = dma_page()?;
        let scratch = dma_page()?;
        let bounce = dma_page()?;
        if scratch_n > 0 {
            let array = dma_page()?;
            for index in 0..scratch_n as usize {
                let page = dma_page()?;
                unsafe {
                    (array.virt as *mut u64).add(index).write_volatile(page.phys);
                }
            }
            unsafe { (dcbaa.virt as *mut u64).write_volatile(array.phys) };
        }
        unsafe {
            (erst.virt as *mut u64).write_volatile(event_page.phys);
            (erst.virt.add(8) as *mut u32).write_volatile(TRBS as u32);
        }
        let ctx = if hcc1 & 4 != 0 { 64 } else { 32 };
        let host = Self {
            cap,
            op: caplen,
            db: rd32(cap, 0x14) as usize & !3,
            rt: rd32(cap, 0x18) as usize & !0x1F,
            ports,
            ctx,
            slot: 0,
            msc_slot: 0,
            in_dci: 0,
            out_dci: 0,
            blocks: 0,
            block_size: 0,
            hid_n: 0,
            hids: [None, None, None],
            kept_out: [None, None, None, None],
            kept_ep0: [None, None, None, None],
            kept_n: 0,
            cmd: Ring::new(cmd_page),
            ep0: Ring::new(ep0),
            bulk_in: Ring::new(bulk_in),
            bulk_out: Ring::new(bulk_out),
            event: event_page.virt as *mut Trb,
            event_phys: event_page.phys,
            ev_deq: 0,
            ev_cycle: 1,
            dcbaa,
            input,
            output,
            scratch,
            bounce,
            tag: 1,
        };
        host.handoff(hcc1);
        host.halt()?;
        host.reset()?;
        wr32(cap, host.op + 0x38, u32::from(max_slots.min(8)));
        wr64(cap, host.op + 0x30, host.dcbaa.phys);
        wr64(cap, host.op + 0x18, host.cmd.phys | 1);
        // Interrupter 0 начинается с +0x20, не с начала runtime-блока.
        wr32(cap, host.rt + 0x20, 2);
        wr32(cap, host.rt + 0x28, 1);
        wr64(cap, host.rt + 0x30, erst.phys);
        wr64(cap, host.rt + 0x38, host.event_phys);
        wr32(cap, host.op, rd32(cap, host.op) | 1 | 4);
        if !spin(|| rd32(cap, host.op + 4) & 1 == 0, 100_000) {
            return Err("run");
        }
        Ok(host)
    }

    fn power_ports(&self) {
        for port in 1..=self.ports {
            let off = self.portsc(port);
            let val = rd32(self.cap, off);
            wr32(self.cap, off, (val & !2) | (1 << 9));
        }
        let _ = spin(
            || (1..=self.ports).any(|port| self.port_connected(port)),
            80_000,
        );
    }

    fn port_connected(&self, port: u8) -> bool {
        rd32(self.cap, self.portsc(port)) & 1 != 0
    }

    fn open_port(&mut self, port: u8) -> Result<bool, &'static str> {
        let off = self.portsc(port);
        if rd32(self.cap, off) & 1 == 0 {
            return Err("empty");
        }
        self.reset_port(port)?;
        let status = rd32(self.cap, off);
        if status & 2 == 0 {
            return Err("ped");
        }
        let speed = ((status >> 10) & 0xF) as u8;
        if let Err(err) = self.enable_slot() {
            crate::serial::write_str("at-slot ");
            return Err(err);
        }
        if let Err(err) = self.address(port, speed) {
            crate::serial::write_str("at-addr ");
            return Err(err);
        }
        if self.configure_msc().is_ok() {
            self.msc_slot = self.slot;
            return Ok(true);
        }
        self.setup_hid()?;
        Ok(false)
    }

    fn reset_port(&mut self, port: u8) -> Result<(), &'static str> {
        let off = self.portsc(port);
        let mut val = rd32(self.cap, off);
        val &= !2;
        val |= (1 << 4) | (1 << 9);
        val |= (1 << 17) | (1 << 18) | (1 << 21) | (1 << 22) | (1 << 23);
        wr32(self.cap, off, val);
        if !spin(
            || {
                let now = rd32(self.cap, off);
                now & (1 << 4) == 0 && now & (1 << 21) != 0
            },
            40_000,
        ) {
            return Err("reset");
        }
        let now = rd32(self.cap, off);
        wr32(
            self.cap,
            off,
            (now & !2) | (1 << 17) | (1 << 18) | (1 << 21),
        );
        Ok(())
    }

    fn enable_slot(&mut self) -> Result<(), &'static str> {
        let ev = self.command(0, 9, 0)?;
        let slot = ((ev.control >> 24) & 0xFF) as u8;
        if slot == 0 {
            return Err("slot");
        }
        self.slot = slot;
        unsafe {
            (self.dcbaa.virt as *mut u64)
                .add(slot as usize)
                .write_volatile(self.output.phys);
        }
        Ok(())
    }

    fn address(&mut self, port: u8, speed: u8) -> Result<(), &'static str> {
        // Сначала 8 байт: полноскоростные HID так отвечают, даже если порт
        // xHCI помечен как high-speed. Иначе длинный дескриптор обрывается.
        let mps: u32 = if speed == 4 { 512 } else { 8 };
        zero(self.input.virt, 4096);
        zero(self.output.virt, 4096);
        wr32(self.input.virt, 4, 3);
        let slot = self.input.virt.wrapping_add(self.ctx);
        wr32(slot, 0, (1 << 27) | ((speed as u32) << 20));
        wr32(slot.wrapping_add(4), 0, u32::from(port) << 16);
        let ep0 = self.input.virt.wrapping_add(self.ctx * 2);
        wr32(ep0.wrapping_add(4), 0, (3 << 1) | (4 << 3) | (mps << 16));
        wr64(ep0.wrapping_add(8), 0, self.ep0.phys | 1);
        wr32(ep0.wrapping_add(16), 0, 8);
        self.command(self.input.phys, 11, u32::from(self.slot))?;
        Ok(())
    }

    fn configure_msc(&mut self) -> Result<(), &'static str> {
        let mut dev = [0u8; 18];
        self.control(0x80, 6, 0x0100, 0, &mut dev)?;
        let mut head = [0u8; 9];
        self.control(0x80, 6, 0x0200, 0, &mut head)?;
        let total = u16::from_le_bytes([head[2], head[3]]) as usize;
        if !(9..=512).contains(&total) {
            return Err("cfg");
        }
        let mut cfg = [0u8; 512];
        self.control(0x80, 6, 0x0200, 0, &mut cfg[..total])?;
        let config_value = cfg[5];
        let mut class = 0u8;
        let mut sub = 0u8;
        let mut proto = 0u8;
        let mut in_ep = None;
        let mut out_ep = None;
        let mut at = 0usize;
        while at + 2 <= total {
            let len = cfg[at] as usize;
            if len < 2 || at + len > total {
                break;
            }
            match cfg[at + 1] {
                4 if len >= 9 => {
                    class = cfg[at + 5];
                    sub = cfg[at + 6];
                    proto = cfg[at + 7];
                }
                5 if len >= 7 && class == 0x08 && sub == 0x06 && proto == 0x50 => {
                    let addr = cfg[at + 2];
                    let attr = cfg[at + 3] & 3;
                    let mps = u16::from_le_bytes([cfg[at + 4], cfg[at + 5]]);
                    if attr == 2 {
                        if addr & 0x80 != 0 {
                            in_ep = Some((addr, mps));
                        } else {
                            out_ep = Some((addr, mps));
                        }
                    }
                }
                _ => {}
            }
            at += len;
        }
        let (in_addr, in_mps) = in_ep.ok_or("bulk-in")?;
        let (out_addr, out_mps) = out_ep.ok_or("bulk-out")?;
        self.control(0x00, 9, u16::from(config_value), 0, &mut [])?;
        self.in_dci = dci(in_addr);
        self.out_dci = dci(out_addr);
        zero(self.input.virt, 4096);
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.output.virt,
                self.input.virt.wrapping_add(self.ctx),
                32,
            );
        }
        let entries = u32::from(self.in_dci.max(self.out_dci));
        let slot = self.input.virt.wrapping_add(self.ctx);
        let dword0 = rd32(slot, 0);
        wr32(slot, 0, (dword0 & !(0x1F << 27)) | (entries << 27));
        // A0: слот меняет Context Entries, иначе DCI выше EP0 — TRB Error.
        wr32(
            self.input.virt,
            4,
            1 | (1 << self.in_dci) | (1 << self.out_dci),
        );
        self.write_bulk(self.in_dci, true, in_mps, self.bulk_in.phys);
        self.write_bulk(self.out_dci, false, out_mps, self.bulk_out.phys);
        self.command(self.input.phys, 12, u32::from(self.slot))?;
        Ok(())
    }

    fn write_bulk(&self, dci: u8, is_in: bool, mps: u16, ring: u64) {
        let ep = self.input.virt.wrapping_add(self.ctx * (dci as usize + 1));
        let kind: u32 = if is_in { 6 } else { 2 };
        wr32(ep.wrapping_add(4), 0, (3 << 1) | (kind << 3) | (u32::from(mps) << 16));
        wr64(ep.wrapping_add(8), 0, ring | 1);
        wr32(ep.wrapping_add(16), 0, 1024);
    }

    fn capacity(&mut self) -> Result<(u64, u32), &'static str> {
        for _ in 0..4 {
            if self.bot(&[0; 6], None, None).is_ok() {
                break;
            }
            let mut sense = [0u8; 18];
            let _ = self.bot(&[0x03, 0, 0, 0, 18, 0], None, Some(&mut sense));
        }
        let mut data = [0u8; 8];
        self.bot(
            &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            None,
            Some(&mut data),
        )?;
        let last = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let size = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if last == 0 || size == 0 || last == u32::MAX {
            return Err("capacity");
        }
        Ok((last as u64 + 1, size))
    }

    pub fn read_at(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() > 4096 || buf.len() % 512 != 0 {
            return Err("len");
        }
        let blocks = (buf.len() / 512) as u16;
        let mut cdb = [0u8; 10];
        cdb[0] = 0x28;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());
        cdb[7..9].copy_from_slice(&blocks.to_be_bytes());
        self.bot(&cdb, None, Some(buf))
    }

    pub fn write_at(&mut self, lba: u32, buf: &[u8]) -> Result<(), &'static str> {
        if buf.len() > 4096 || buf.len() % 512 != 0 {
            return Err("len");
        }
        let blocks = (buf.len() / 512) as u16;
        let mut cdb = [0u8; 10];
        cdb[0] = 0x2A;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());
        cdb[7..9].copy_from_slice(&blocks.to_be_bytes());
        self.bot(&cdb, Some(buf), None)
    }

    pub fn sync_cache(&mut self) -> Result<(), &'static str> {
        self.bot(&[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0], None, None)
    }

    fn bot(
        &mut self,
        cdb: &[u8],
        data_out: Option<&[u8]>,
        mut data_in: Option<&mut [u8]>,
    ) -> Result<(), &'static str> {
        let flags: u8 = if data_in.is_some() { 0x80 } else { 0 };
        let length = data_in.as_ref().map(|buf| buf.len()).or_else(|| data_out.map(|buf| buf.len())).unwrap_or(0) as u32;
        let mut cbw = [0u8; 31];
        cbw[..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
        let tag = self.tag;
        self.tag = self.tag.wrapping_add(1);
        cbw[4..8].copy_from_slice(&tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&length.to_le_bytes());
        cbw[12] = flags;
        let n = cdb.len().min(16);
        cbw[14] = n as u8;
        cbw[15..15 + n].copy_from_slice(&cdb[..n]);
        self.stage_out(&cbw)?;
        if let Some(buf) = data_in.as_mut() {
            self.stage_in(buf)?;
        } else if let Some(buf) = data_out {
            self.stage_out(buf)?;
        }
        self.read_csw(tag)
    }

    fn stage_out(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > 4096 {
            return Err("stage");
        }
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), self.bounce.virt, data.len());
        }
        fence(Ordering::SeqCst);
        self.bulk_out
            .push(self.bounce.phys, data.len() as u32, (1 << 10) | (1 << 5));
        self.doorbell(self.bulk_slot(), self.out_dci);
        self.wait_transfer()
    }

    fn stage_in(&mut self, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() > 4096 {
            return Err("stage");
        }
        self.bulk_in
            .push(self.bounce.phys, buf.len() as u32, (1 << 10) | (1 << 5));
        self.doorbell(self.bulk_slot(), self.in_dci);
        self.wait_transfer()?;
        fence(Ordering::SeqCst);
        unsafe {
            core::ptr::copy_nonoverlapping(self.bounce.virt, buf.as_mut_ptr(), buf.len());
        }
        Ok(())
    }

    fn read_csw(&mut self, tag: u32) -> Result<(), &'static str> {
        let mut raw = [0u8; 13];
        self.stage_in(&mut raw)?;
        let sig = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let got = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        if sig != 0x5342_5355 || got != tag || raw[12] != 0 {
            return Err("csw");
        }
        Ok(())
    }

    fn control(
        &mut self,
        bm: u8,
        req: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
    ) -> Result<(), &'static str> {
        if buf.len() > 4096 {
            return Err("ctl");
        }
        let mut setup = [0u8; 8];
        setup[0] = bm;
        setup[1] = req;
        setup[2..4].copy_from_slice(&value.to_le_bytes());
        setup[4..6].copy_from_slice(&index.to_le_bytes());
        setup[6..8].copy_from_slice(&(buf.len() as u16).to_le_bytes());
        let trt: u32 = if buf.is_empty() {
            0
        } else if bm & 0x80 != 0 {
            3
        } else {
            2
        };
        self.ep0.push(
            u64::from_le_bytes(setup),
            8,
            (2 << 10) | (1 << 6) | (trt << 16) | (1 << 4),
        );
        if !buf.is_empty() {
            let dir_in = bm & 0x80 != 0;
            if !dir_in {
                unsafe {
                    core::ptr::copy_nonoverlapping(buf.as_ptr(), self.scratch.virt, buf.len());
                }
            }
            let dir = u32::from(dir_in);
            self.ep0.push(
                self.scratch.phys,
                buf.len() as u32,
                (3 << 10) | (dir << 16) | (1 << 4),
            );
        }
        let status_in = buf.is_empty() || bm & 0x80 == 0;
        let status_addr = self.ep0.push(
            0,
            0,
            (4 << 10) | (u32::from(status_in) << 16) | (1 << 5),
        );
        self.doorbell(self.slot, 1);
        self.wait_trb(status_addr)?;
        if bm & 0x80 != 0 && !buf.is_empty() {
            fence(Ordering::SeqCst);
            unsafe {
                core::ptr::copy_nonoverlapping(self.scratch.virt, buf.as_mut_ptr(), buf.len());
            }
        }
        Ok(())
    }

    fn command(&mut self, param: u64, kind: u32, slot: u32) -> Result<Event, &'static str> {
        self.cmd.push(param, 0, (kind << 10) | (slot << 24));
        self.doorbell(0, 0);
        self.wait_type(33)
    }

    fn wait_trb(&mut self, trb: u64) -> Result<(), &'static str> {
        for step in 0..4_000_000u32 {
            if step % 256 == 0 {
                let _ = rd32(self.cap, self.op + 4);
            }
            let Some(ev) = self.poll() else {
                core::hint::spin_loop();
                continue;
            };
            let ty = (ev.control >> 10) & 0x3F;
            if ty == 32 {
                let ep = ((ev.control >> 16) & 0x1F) as u8;
                let code = (ev.status >> 24) & 0xFF;
                if ep != 1 && ep != self.in_dci && ep != self.out_dci {
                    let slot = ((ev.control >> 24) & 0xFF) as u8;
                    self.finish_hid(slot, ep);
                }
                if ev.param == trb && (code == 1 || code == 13) {
                    return Ok(());
                }
                if ev.param == trb {
                    return Err("code");
                }
            }
        }
        Err("timeout")
    }

    fn bulk_slot(&self) -> u8 {
        if self.msc_slot == 0 { self.slot } else { self.msc_slot }
    }

    fn wait_transfer(&mut self) -> Result<(), &'static str> {
        for step in 0..4_000_000u32 {
            if step % 256 == 0 {
                let _ = rd32(self.cap, self.op + 4);
            }
            let Some(ev) = self.poll() else {
                core::hint::spin_loop();
                continue;
            };
            let ty = (ev.control >> 10) & 0x3F;
            if ty != 32 {
                continue;
            }
            let code = (ev.status >> 24) & 0xFF;
            if code != 1 && code != 13 {
                return Err("code");
            }
            let ep = ((ev.control >> 16) & 0x1F) as u8;
            if ep == 1 || ep == self.in_dci || ep == self.out_dci {
                return Ok(());
            }
            let slot = ((ev.control >> 24) & 0xFF) as u8;
            self.finish_hid(slot, ep);
        }
        Err("timeout")
    }

    fn wait_type(&mut self, want: u32) -> Result<Event, &'static str> {
        for step in 0..4_000_000u32 {
            if step % 256 == 0 {
                let _ = rd32(self.cap, self.op + 4);
            }
            if let Some(ev) = self.poll() {
                let ty = (ev.control >> 10) & 0x3F;
                if ty == 34 {
                    continue;
                }
                if ty == want {
                    let code = (ev.status >> 24) & 0xFF;
                    if code == 1 || code == 13 {
                        return Ok(ev);
                    }
                    crate::serial::write_str("usb-code ");
                    write_dec(code);
                    crate::serial::write_str("\n");
                    return Err("code");
                }
            } else {
                core::hint::spin_loop();
            }
        }
        Err("timeout")
    }

    fn poll(&mut self) -> Option<Event> {
        let trb = unsafe { self.event.add(self.ev_deq) };
        let control = unsafe { core::ptr::read_volatile(&(*trb).control) };
        if control & 1 != self.ev_cycle {
            return None;
        }
        fence(Ordering::SeqCst);
        let status = unsafe { core::ptr::read_volatile(&(*trb).status) };
        let param = unsafe { core::ptr::read_volatile(&(*trb).param) };
        self.ev_deq += 1;
        if self.ev_deq == TRBS {
            self.ev_deq = 0;
            self.ev_cycle ^= 1;
        }
        let next = self.event_phys + self.ev_deq as u64 * 16;
        wr64(self.cap, self.rt + 0x38, next | 8);
        Some(Event { param, status, control })
    }

    fn setup_hid(&mut self) -> Result<(), &'static str> {
        let mut head = [0u8; 9];
        self.control(0x80, 6, 0x0200, 0, &mut head)?;
        let total = u16::from_le_bytes([head[2], head[3]]) as usize;
        if !(9..=512).contains(&total) {
            return Err("cfg");
        }
        let mut cfg = [0u8; 512];
        self.control(0x80, 6, 0x0200, 0, &mut cfg[..total])?;
        let config_value = cfg[5];
        let mut class = 0u8;
        let mut proto = 0u8;
        let mut iface = 0u8;
        let mut report_len = 0usize;
        let mut endpoint = None;
        let mut at = 0usize;
        while at + 2 <= total {
            let len = cfg[at] as usize;
            if len < 2 || at + len > total {
                break;
            }
            match cfg[at + 1] {
                4 if len >= 9 => {
                    class = cfg[at + 5];
                    iface = cfg[at + 2];
                    proto = cfg[at + 7];
                }
                0x21 if len >= 9 && class == 3 => {
                    report_len = u16::from_le_bytes([cfg[at + 7], cfg[at + 8]]) as usize;
                }
                5 if len >= 7 && class == 3 => {
                    let addr = cfg[at + 2];
                    let attr = cfg[at + 3] & 3;
                    let mps = u16::from_le_bytes([cfg[at + 4], cfg[at + 5]]).max(1);
                    if attr == 3 && addr & 0x80 != 0 {
                        endpoint = Some((addr, mps));
                    }
                }
                _ => {}
            }
            at += len;
        }
        if class != 3 {
            return Err("not-hid");
        }
        let (addr, mps) = endpoint.ok_or("intr")?;
        let mut desc = [0u8; 8];
        let _ = self.control(0x81, 6, 0x2200, u16::from(iface), &mut desc);
        let _ = report_len;
        let mut map = hid::parse(&desc);
        let abs = map.fields[..map.n as usize].iter().any(|field| {
            field.page == 0x01 && field.usage == 0x30 && !field.relative && !field.constant
        });
        if map.fields[..map.n as usize].iter().any(|field| field.page == 0x07) {
            map.kind = 1;
        } else if abs {
            map.kind = 3;
        } else if map.n == 0 {
            map.kind = match proto {
                1 => 1,
                2 => 2,
                _ => 3,
            };
        }
        self.control(0x00, 9, u16::from(config_value), 0, &mut [])?;
        if self.hid_n as usize >= self.hids.len() {
            return Err("hids");
        }
        let dci = dci(addr);
        let ring = Ring::new(dma_page()?);
        let buf = dma_page()?;
        zero(self.input.virt, 4096);
        unsafe {
            core::ptr::copy_nonoverlapping(self.output.virt, self.input.virt.wrapping_add(self.ctx), 32);
        }
        let slot_ptr = self.input.virt.wrapping_add(self.ctx);
        let dword0 = rd32(slot_ptr, 0);
        wr32(slot_ptr, 0, (dword0 & !(0x1F << 27)) | (u32::from(dci) << 27));
        wr32(self.input.virt, 4, 1 | (1 << dci));
        let ep = self.input.virt.wrapping_add(self.ctx * (dci as usize + 1));
        wr32(ep, 0, 4 << 16);
        wr32(ep.wrapping_add(4), 0, (3 << 1) | (7 << 3) | (u32::from(mps) << 16));
        wr64(ep.wrapping_add(8), 0, ring.phys | 1);
        wr32(ep.wrapping_add(16), 0, u32::from(mps));
        self.command(self.input.phys, 12, u32::from(self.slot))?;
        let name = match map.kind {
            1 => "usb-keyboard",
            3 => "usb-tablet",
            4 => "usb-touchpad",
            5 => "usb-touchscreen",
            _ => "usb-mouse",
        };
        crate::devices::add(name);
        crate::serial::write_str(name);
        crate::serial::write_str("\n");
        let index = self.hid_n as usize;
        self.hids[index] = Some(HidDev {
            live: true,
            slot: self.slot,
            dci,
            mps,
            map,
            ring,
            buf,
            prev: [0; 6],
            buttons: 0,
            tip: false,
            last_x: 0,
            last_y: 0,
        });
        self.hid_n += 1;
        Ok(())
    }

    fn arm_all(&mut self) {
        let n = self.hid_n as usize;
        for index in 0..n {
            self.arm_hid(index);
        }
    }

    fn arm_hid(&mut self, index: usize) {
        let Some(dev) = self.hids[index].as_mut() else { return };
        let len = u32::from(dev.mps.min(64));
        let phys = dev.buf.phys;
        let slot = dev.slot;
        let dci = dev.dci;
        dev.ring.push(phys, len, (1 << 10) | (1 << 5));
        self.doorbell(slot, dci);
    }

    pub fn poll_hid(&mut self) {
        while let Some(ev) = self.poll() {
            let ty = (ev.control >> 10) & 0x3F;
            if ty != 32 {
                continue;
            }
            let code = (ev.status >> 24) & 0xFF;
            if code != 1 && code != 13 {
                continue;
            }
            let ep = ((ev.control >> 16) & 0x1F) as u8;
            if ep == 1 || ep == self.in_dci || ep == self.out_dci {
                continue;
            }
            let slot = ((ev.control >> 24) & 0xFF) as u8;
            self.finish_hid(slot, ep);
        }
    }

    fn finish_hid(&mut self, slot: u8, ep: u8) {
        let index = self.hids.iter().position(|dev| {
            dev.as_ref().is_some_and(|dev| dev.live && dev.slot == slot && dev.dci == ep)
        });
        let Some(index) = index else { return };
        let mut data = [0u8; 64];
        let n = {
            let Some(dev) = self.hids[index].as_ref() else { return };
            let n = usize::from(dev.mps.min(64));
            unsafe { core::ptr::copy_nonoverlapping(dev.buf.virt, data.as_mut_ptr(), n) };
            n
        };
        self.decode_hid(index, &data[..n]);
        self.arm_hid(index);
    }

    fn decode_hid(&mut self, index: usize, data: &[u8]) {
        let Some(dev) = self.hids[index].as_mut() else { return };
        let map = dev.map;
        let mut buttons = dev.buttons;
        let mut dx = 0i32;
        let mut dy = 0i32;
        let mut ax = None;
        let mut ay = None;
        let mut wheel = 0i32;
        let mut pressure = 0u32;
        let mut keys = [0u16; 6];
        let mut key_n = 0usize;
        let mut contacts = 0i32;
        for field in map.fields[..map.n as usize].iter() {
            if field.constant {
                continue;
            }
            for i in 0..field.count {
                let bit = field.bit + u16::from(i) * u16::from(field.size);
                let raw = hid::field_value(data, bit, field.size);
                let value = hid::signed_value(raw, field.size, field.min);
                let usage = if field.usage_max > field.usage { field.usage + u16::from(i) } else { field.usage };
                if field.page == 0x07 && field.array && raw != 0 && key_n < keys.len() {
                    keys[key_n] = raw as u16;
                    key_n += 1;
                } else if field.page == 0x07 && !field.array && field.usage >= 0xE0 {
                    crate::layout::hid_usage(usage, raw != 0);
                } else if field.page == 0x09 && !field.array {
                    let mask = 1u8 << i.min(7);
                    if raw != 0 { buttons |= mask; } else { buttons &= !mask; }
                } else if field.page == 0x01 && (usage == 0x30 || field.usage == 0x30) {
                    if field.relative { dx = dx.saturating_add(value); } else if ax.is_none() { ax = Some((value, field.max.max(1))); }
                    else { contacts += 1; }
                } else if field.page == 0x01 && (usage == 0x31 || field.usage == 0x31) {
                    if field.relative { dy = dy.saturating_add(value); } else if ay.is_none() { ay = Some((value, field.max.max(1))); }
                } else if field.page == 0x01 && usage == 0x38 {
                    wheel = wheel.saturating_add(value);
                } else if field.page == 0x0D && (usage == 0x30 || field.usage == 0x30) {
                    pressure = raw;
                } else if field.page == 0x0D && usage == 0x42 {
                    if raw != 0 { buttons |= 1; }
                }
            }
        }
        if map.n == 0 && map.kind == 1 && data.len() >= 8 {
            let modifiers = data[0];
            for (bit, usage) in [0xE0, 0xE1, 0xE2, 0xE3].into_iter().enumerate() {
                crate::layout::hid_usage(usage, modifiers & (1 << bit) != 0);
            }
            let prev = dev.prev;
            let mut keys = [0u16; 6];
            for i in 0..6 {
                keys[i] = data[2 + i] as u16;
            }
            for usage in prev {
                if usage != 0 && !keys.contains(&usage) {
                    crate::layout::hid_usage(usage, false);
                }
            }
            for usage in keys {
                if usage != 0 && !prev.contains(&usage) {
                    crate::layout::hid_usage(usage, true);
                }
            }
            dev.prev = keys;
            return;
        }
        if map.n == 0 && map.kind == 3 && data.len() >= 5 {
            let buttons = data[0];
            let x = u16::from_le_bytes([data[1], data[2]]) as i32;
            let y = u16::from_le_bytes([data[3], data[4]]) as i32;
            let (width, height) = crate::display::size().unwrap_or((1280, 720));
            let sx = (x as i64 * width as i64 / 32768) as i32;
            let sy = (y as i64 * height as i64 / 32768) as i32;
            crate::event::push(crate::event::Event::PointerAbs { x: sx, y: sy });
            crate::event::push(crate::event::Event::Pen {
                x: sx, y: sy, pressure: if buttons & 1 != 0 { 512 } else { 0 },
                tilt_x: 0, tilt_y: 0, down: buttons & 1 != 0,
            });
            if data.len() > 5 && data[5] != 0 {
                crate::event::push(crate::event::Event::Scroll { dz: data[5] as i8 });
            }
            if buttons != dev.buttons {
                crate::event::push(crate::event::Event::Button { button: 0, down: buttons & 1 != 0 });
                dev.buttons = buttons;
            }
            return;
        }
        if map.n == 0 && data.len() >= 3 {
            let buttons = data[0];
            let dx = data[1] as i8 as i16;
            let dy = data[2] as i8 as i16;
            if dx != 0 || dy != 0 {
                crate::event::push(crate::event::Event::PointerRel { dx, dy });
            }
            if data.len() > 3 && data[3] != 0 {
                crate::event::push(crate::event::Event::Scroll { dz: data[3] as i8 });
            }
            if buttons != dev.buttons {
                for bit in 0..5u8 {
                    if (buttons ^ dev.buttons) & (1 << bit) != 0 {
                        crate::event::push(crate::event::Event::Button { button: bit, down: buttons & (1 << bit) != 0 });
                    }
                }
                dev.buttons = buttons;
            }
            return;
        }
        if map.kind == 1 || map.fields[..map.n as usize].iter().any(|field| field.page == 0x07 && field.array) {
            let prev = dev.prev;
            for usage in prev {
                if usage != 0 && !keys[..key_n].contains(&usage) {
                    crate::layout::hid_usage(usage, false);
                }
            }
            for usage in keys[..key_n].iter().copied() {
                if usage != 0 && !prev.contains(&usage) {
                    crate::layout::hid_usage(usage, true);
                }
            }
            dev.prev = keys;
        }
        if buttons != dev.buttons {
            let changed = buttons ^ dev.buttons;
            for bit in 0..5u8 {
                if changed & (1 << bit) != 0 {
                    crate::event::push(crate::event::Event::Button { button: bit, down: buttons & (1 << bit) != 0 });
                }
            }
            dev.buttons = buttons;
        }
        if dx != 0 || dy != 0 {
            crate::event::push(crate::event::Event::PointerRel { dx: dx as i16, dy: dy as i16 });
        }
        if let (Some((x, xmax)), Some((y, ymax))) = (ax, ay) {
            let (width, height) = crate::display::size().unwrap_or((1280, 720));
            let sx = (x.max(0) as i64 * width as i64 / (xmax as i64 + 1)) as i32;
            let sy = (y.max(0) as i64 * height as i64 / (ymax as i64 + 1)) as i32;
            crate::event::push(crate::event::Event::PointerAbs { x: sx, y: sy });
            if pressure > 0 || map.kind == 3 {
                crate::event::push(crate::event::Event::Pen {
                    x: sx, y: sy,
                    pressure: pressure.min(u16::MAX as u32) as u16,
                    tilt_x: 0, tilt_y: 0,
                    down: pressure > 0 || buttons & 1 != 0,
                });
            }
            if map.kind == 5 {
                crate::event::push(crate::event::Event::Touch { id: 0, x: sx, y: sy, down: buttons & 1 != 0 });
            }
            if map.kind == 4 {
                let moved = (sx - dev.last_x).abs() + (sy - dev.last_y).abs();
                if contacts > 0 {
                    crate::event::push(crate::event::Event::Scroll { dz: ((sy - dev.last_y) / 8) as i8 });
                } else if dev.tip && buttons & 1 == 0 && moved < 12 {
                    crate::event::push(crate::event::Event::Button { button: 0, down: true });
                    crate::event::push(crate::event::Event::Button { button: 0, down: false });
                }
                dev.tip = buttons & 1 != 0;
                dev.last_x = sx;
                dev.last_y = sy;
            }
        }
        if wheel != 0 {
            crate::event::push(crate::event::Event::Scroll { dz: wheel as i8 });
        }
    }

    fn park(&mut self) -> Result<(), &'static str> {
        if self.kept_n as usize >= self.kept_out.len() {
            return Err("kept");
        }
        let new_out = dma_page()?;
        let new_ep0 = Ring::new(dma_page()?);
        let old_out = core::mem::replace(&mut self.output, new_out);
        let old_ep0 = core::mem::replace(&mut self.ep0, new_ep0);
        let index = self.kept_n as usize;
        self.kept_out[index] = Some(old_out);
        self.kept_ep0[index] = Some(old_ep0);
        self.kept_n += 1;
        Ok(())
    }

    fn doorbell(&self, slot: u8, target: u8) {
        fence(Ordering::SeqCst);
        wr32(self.cap, self.db + slot as usize * 4, u32::from(target));
    }

    fn portsc(&self, port: u8) -> usize {
        self.op + 0x400 + (port as usize - 1) * 0x10
    }

    fn handoff(&self, hcc1: u32) {
        let mut off = (((hcc1 >> 16) & 0xFFFF) as usize) * 4;
        if off == 0 {
            return;
        }
        for _ in 0..8 {
            let hdr = rd32(self.cap, off);
            let id = hdr & 0xFF;
            let next = (hdr >> 8) & 0xFF;
            if id == 1 {
                wr32(self.cap, off, rd32(self.cap, off) | (1 << 24));
                let _ = spin(|| rd32(self.cap, off) & (1 << 16) == 0, 20_000);
                return;
            }
            if next == 0 {
                break;
            }
            off += next as usize * 4;
        }
    }

    fn halt(&self) -> Result<(), &'static str> {
        wr32(self.cap, self.op, rd32(self.cap, self.op) & !1);
        if spin(|| rd32(self.cap, self.op + 4) & 1 != 0, 100_000) {
            Ok(())
        } else {
            Err("halt")
        }
    }

    fn reset(&self) -> Result<(), &'static str> {
        wr32(self.cap, self.op, rd32(self.cap, self.op) | 2);
        if !spin(|| rd32(self.cap, self.op) & 2 == 0, 100_000) {
            return Err("hcreset");
        }
        if spin(|| rd32(self.cap, self.op + 4) & (1 << 11) == 0, 100_000) {
            Ok(())
        } else {
            Err("cnr")
        }
    }
}

impl Ring {
    fn new(page: Page) -> Self {
        Self {
            trb: page.virt as *mut Trb,
            phys: page.phys,
            enq: 0,
            cycle: 1,
        }
    }

    fn push(&mut self, param: u64, status: u32, control: u32) -> u64 {
        if self.enq == TRBS - 1 {
            self.write(TRBS - 1, self.phys, 0, (6 << 10) | (1 << 1));
            self.cycle ^= 1;
            self.enq = 0;
        }
        let addr = self.phys + self.enq as u64 * 16;
        self.write(self.enq, param, status, control);
        self.enq += 1;
        addr
    }

    fn write(&mut self, index: usize, param: u64, status: u32, control: u32) {
        unsafe {
            let trb = self.trb.add(index);
            core::ptr::write_volatile(&mut (*trb).param, param);
            core::ptr::write_volatile(&mut (*trb).status, status);
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(&mut (*trb).control, control | self.cycle);
        }
    }
}

fn dma_page() -> Result<Page, &'static str> {
    let phys = vlr_memory::alloc_frame().ok_or("frame")?;
    let virt = vlr_memory::phys_to_virt(phys).ok_or("virt")? as *mut u8;
    zero(virt, 4096);
    Ok(Page { virt, phys })
}

fn zero(ptr: *mut u8, len: usize) {
    unsafe { core::ptr::write_bytes(ptr, 0, len) };
}

fn dci(addr: u8) -> u8 {
    let n = addr & 0x0F;
    if n == 0 {
        1
    } else if addr & 0x80 != 0 {
        n * 2 + 1
    } else {
        n * 2
    }
}

fn scratchpad_count(hcs2: u32) -> u32 {
    let lo = (hcs2 >> 21) & 0x1F;
    let hi = (hcs2 >> 27) & 0x1F;
    (hi << 5) | lo
}

fn write_dec(mut value: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    if value == 0 {
        crate::serial::write_str("0");
        return;
    }
    while value > 0 {
        i -= 1;
        buf[i] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    crate::serial::write_str(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}

fn spin(mut pred: impl FnMut() -> bool, spins: u32) -> bool {
    for _ in 0..spins {
        if pred() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn rd32(base: *mut u8, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(off) as *const u32) }
}

fn wr32(base: *mut u8, off: usize, value: u32) {
    unsafe { core::ptr::write_volatile(base.add(off) as *mut u32, value) };
}

fn wr64(base: *mut u8, off: usize, value: u64) {
    unsafe { core::ptr::write_volatile(base.add(off) as *mut u64, value) };
}
