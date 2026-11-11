//! Таблицы страниц x86-64.
//!
//! Загрузчик уже отобразил физическую память по смещению `physical_memory_offset`.
//! Мы берём его таблицу четвертого уровня из CR3 и поверх неё отображаем и
//! снимаем отдельные страницы 4 КиБ. Полный доступ к физическим кадрам остаётся
//! через это смещение: `virt = phys + offset`.

use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::registers::model_specific::{Efer, EferFlags};
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    Translate,
};
use x86_64::{PhysAddr, VirtAddr};

use crate::frame::PAGE_SIZE;
use crate::MapError;

struct MapperSlot {
    inner: Option<OffsetPageTable<'static>>,
}

unsafe impl Send for MapperSlot {}

static MAPPER: Mutex<MapperSlot> = Mutex::new(MapperSlot { inner: None });
static PHYS_OFFSET: Mutex<Option<u64>> = Mutex::new(None);

pub unsafe fn init(offset: u64) {
    let virt_offset = VirtAddr::new(offset);
    let l4 = active_level_4_table(virt_offset);
    let mapper = unsafe { OffsetPageTable::new(l4, virt_offset) };
    let _ = Efer::update(|flags| {
        *flags |= EferFlags::NO_EXECUTE_ENABLE;
    });
    *PHYS_OFFSET.lock() = Some(offset);
    MAPPER.lock().inner = Some(mapper);
    crate::user::capture_kernel_cr3();
}

pub fn phys_offset() -> Option<u64> {
    *PHYS_OFFSET.lock()
}

pub fn phys_to_virt(phys: u64) -> Option<u64> {
    phys_offset().map(|offset| offset.wrapping_add(phys))
}

pub fn translate(virt: u64) -> Option<u64> {
    let mut slot = MAPPER.lock();
    let mapper = slot.inner.as_mut()?;
    mapper
        .translate_addr(VirtAddr::new(virt))
        .map(|phys| phys.as_u64())
}

pub fn map_4k(virt: u64, phys: u64, writable: bool) -> Result<(), MapError> {
    if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err(MapError::Misaligned);
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    let frame =
        PhysFrame::from_start_address(PhysAddr::new(phys)).map_err(|_| MapError::Misaligned)?;
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    let mut slot = MAPPER.lock();
    let mapper = slot.inner.as_mut().ok_or(MapError::NotInitialized)?;
    let mut frames_slot = crate::frames();
    let frames = frames_slot.as_mut().ok_or(MapError::NotInitialized)?;
    let flush = unsafe { mapper.map_to(page, frame, flags, frames) }.map_err(map_err)?;
    flush.flush();
    Ok(())
}

pub fn map_identity(phys: u64) -> Result<(), MapError> {
    map_4k(phys, phys, true)
}

pub fn unmap_4k(virt: u64) -> Result<u64, MapError> {
    if virt % PAGE_SIZE != 0 {
        return Err(MapError::Misaligned);
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    let mut slot = MAPPER.lock();
    let mapper = slot.inner.as_mut().ok_or(MapError::NotInitialized)?;
    let (frame, flush) = mapper.unmap(page).map_err(|_| MapError::NotMapped)?;
    flush.flush();
    Ok(frame.start_address().as_u64())
}

unsafe impl FrameAllocator<Size4KiB> for crate::frame::BitmapFrameAlloc {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate()
            .and_then(|phys| PhysFrame::from_start_address(PhysAddr::new(phys)).ok())
    }
}

fn map_err(err: x86_64::structures::paging::mapper::MapToError<Size4KiB>) -> MapError {
    use x86_64::structures::paging::mapper::MapToError;
    match err {
        MapToError::FrameAllocationFailed => MapError::OutOfFrames,
        MapToError::ParentEntryHugePage | MapToError::PageAlreadyMapped(_) => {
            MapError::AlreadyMapped
        }
    }
}

unsafe fn active_level_4_table(offset: VirtAddr) -> &'static mut PageTable {
    let (frame, _) = Cr3::read();
    let virt = offset + frame.start_address().as_u64();
    let table = virt.as_mut_ptr::<PageTable>();
    unsafe { &mut *table }
}
