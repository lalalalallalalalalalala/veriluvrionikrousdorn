//! Адресное пространство задачи кольца 3.
//!
//! Новая таблица L4 копирует записи ядра, поэтому код ядра, куча и смещение
//! физической памяти остаются доступны после `mov cr3`. Индекс L4 под пользователя
//! выбирается пустой, чтобы не делить его поддерево с ядром. Страницы пользователя
//! помечены USER; чужой указатель в системный вызов не проходит.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

use crate::frame::PAGE_SIZE;
use crate::paging::phys_offset;
use crate::MapError;

const USER_WINDOW: u64 = 16 * 1024 * 1024;

static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
static USER_L4_INDEX: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn capture_kernel_cr3() {
    let (frame, _) = Cr3::read();
    KERNEL_CR3.store(frame.start_address().as_u64(), Ordering::Release);
    if USER_L4_INDEX.load(Ordering::Acquire) == usize::MAX {
        USER_L4_INDEX.store(choose_user_index(), Ordering::Release);
    }
}

pub fn kernel_cr3() -> u64 {
    KERNEL_CR3.load(Ordering::Acquire)
}

pub fn user_base() -> u64 {
    (user_index() as u64) << 39
}

pub fn user_code() -> u64 {
    user_base() + 0x40_0000
}

pub fn user_stack_page() -> u64 {
    user_base() + 0x60_0000
}

pub fn user_stack_top() -> u64 {
    user_stack_page() + PAGE_SIZE
}

pub fn create_user_space() -> Result<u64, MapError> {
    let frame = crate::alloc_frame().ok_or(MapError::OutOfFrames)?;
    let virt = crate::phys_to_virt(frame).ok_or(MapError::NotInitialized)?;
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE as usize);
        let kernel = kernel_l4();
        let user = &mut *(virt as *mut PageTable);
        for index in 0..512 {
            user[index] = kernel[index].clone();
        }
        let slot = user_index();
        user[slot].set_unused();
    }
    Ok(frame)
}

pub fn map_user_page(
    l4_phys: u64,
    virt: u64,
    phys: u64,
    writable: bool,
    executable: bool,
) -> Result<(), MapError> {
    if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err(MapError::Misaligned);
    }
    let l4_virt = crate::phys_to_virt(l4_phys).ok_or(MapError::NotInitialized)?;
    let offset = VirtAddr::new(phys_offset().ok_or(MapError::NotInitialized)?);
    let l4 = unsafe { &mut *(l4_virt as *mut PageTable) };
    let mut mapper = unsafe { OffsetPageTable::new(l4, offset) };
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    let frame =
        PhysFrame::from_start_address(PhysAddr::new(phys)).map_err(|_| MapError::Misaligned)?;
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    let mut frames = crate::frames();
    let frames = frames.as_mut().ok_or(MapError::NotInitialized)?;
    let flush = unsafe { mapper.map_to(page, frame, flags, frames) }
        .map_err(|_| MapError::AlreadyMapped)?;
    flush.flush();
    Ok(())
}

pub fn activate_cr3(phys: u64) {
    let phys = if phys == 0 { kernel_cr3() } else { phys };
    if phys == 0 {
        return;
    }
    let (current, _) = Cr3::read();
    if current.start_address().as_u64() == phys {
        return;
    }
    let frame = PhysFrame::containing_address(PhysAddr::new(phys));
    unsafe { Cr3::write(frame, Cr3Flags::empty()) };
}

pub fn is_user_range(addr: u64, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let Some(end) = addr.checked_add(len as u64) else {
        return false;
    };
    let base = user_base();
    if addr < base || end > base + USER_WINDOW {
        return false;
    }
    let mut page = addr & !0xfff;
    while page < end {
        if !page_is_user(page) {
            return false;
        }
        page = page.saturating_add(PAGE_SIZE);
    }
    true
}

pub fn free_user_space(l4_phys: u64) {
    if l4_phys == 0 || l4_phys == kernel_cr3() {
        return;
    }
    let Some(virt) = crate::phys_to_virt(l4_phys) else {
        return;
    };
    unsafe {
        let l4 = &*(virt as *const PageTable);
        free_tree(l4[user_index()].clone(), 3);
    }
    let _ = crate::free_frame(l4_phys);
}

fn user_index() -> usize {
    let index = USER_L4_INDEX.load(Ordering::Acquire);
    if index == usize::MAX {
        0
    } else {
        index
    }
}

fn choose_user_index() -> usize {
    // Программы `.vlr` собраны по адресу `0x400000` (индекс L4 0).
    // Слот в таблице пользователя свой: код ядра лежит в другом индексе.
    0
}

fn kernel_l4() -> &'static PageTable {
    let (frame, _) = Cr3::read();
    let virt = crate::phys_to_virt(frame.start_address().as_u64()).expect("offset");
    unsafe { &*(virt as *const PageTable) }
}

fn page_is_user(virt: u64) -> bool {
    let (l4_frame, _) = Cr3::read();
    let Some(l4_virt) = crate::phys_to_virt(l4_frame.start_address().as_u64()) else {
        return false;
    };
    unsafe {
        let l4 = &*(l4_virt as *const PageTable);
        let i4 = ((virt >> 39) & 0x1ff) as usize;
        let e4 = &l4[i4];
        if !present_4k(e4) {
            return false;
        }
        let pdpt = table(e4.addr().as_u64());
        let i3 = ((virt >> 30) & 0x1ff) as usize;
        let e3 = &pdpt[i3];
        if !present_4k(e3) {
            return false;
        }
        let pd = table(e3.addr().as_u64());
        let i2 = ((virt >> 21) & 0x1ff) as usize;
        let e2 = &pd[i2];
        if !present_4k(e2) {
            return false;
        }
        let pt = table(e2.addr().as_u64());
        let i1 = ((virt >> 12) & 0x1ff) as usize;
        let e1 = &pt[i1];
        e1.flags().contains(PageTableFlags::PRESENT)
            && e1.flags().contains(PageTableFlags::USER_ACCESSIBLE)
    }
}

fn present_4k(entry: &x86_64::structures::paging::page_table::PageTableEntry) -> bool {
    let flags = entry.flags();
    flags.contains(PageTableFlags::PRESENT) && !flags.contains(PageTableFlags::HUGE_PAGE)
}

unsafe fn table(phys: u64) -> &'static PageTable {
    let virt = crate::phys_to_virt(phys).unwrap_or(0);
    unsafe { &*(virt as *const PageTable) }
}

fn free_tree(entry: x86_64::structures::paging::page_table::PageTableEntry, level: u8) {
    if !entry.flags().contains(PageTableFlags::PRESENT) {
        return;
    }
    let phys = entry.addr().as_u64();
    if level > 0 && !entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        let table = unsafe { &*(crate::phys_to_virt(phys).unwrap_or(0) as *const PageTable) };
        for index in 0..512 {
            free_tree(table[index].clone(), level - 1);
        }
    }
    let _ = crate::free_frame(phys);
}
