//! Память ядра: карта загрузчика, кадры, страницы и куча.
//!
//! Физическая память доступна по смещению, которое построил загрузчик.
//! Новые отображения (куча, явные `map_4k` / `unmap_4k`, identity) делаются
//! в его таблице страниц. Глобальный аллокатор объявляет бинарник ядра.

#![no_std]

mod frame;
mod heap;
mod paging;
mod user;

use core::cell::UnsafeCell;

use spin::Mutex;

pub use frame::{align_down, align_up, BitmapFrameAlloc, FreeError, PAGE_SIZE};
pub use heap::KernelAllocator;
pub use user::{
    activate_cr3, create_user_space, free_user_space, is_user_range, map_user_page, user_base,
    user_code, user_stack_page, user_stack_top,
};

const MAX_PHYS: u64 = 1 << 32;
const BITMAP_BYTES: usize = (MAX_PHYS / PAGE_SIZE / 8) as usize;
const MAX_REGIONS: usize = 64;

struct BitmapStore(UnsafeCell<[u8; BITMAP_BYTES]>);
unsafe impl Sync for BitmapStore {}

static BITMAP: BitmapStore = BitmapStore(UnsafeCell::new([0xFF; BITMAP_BYTES]));
static FRAMES: Mutex<Option<BitmapFrameAlloc>> = Mutex::new(None);
static STATE: spin::Once<MemoryState> = spin::Once::new();

#[derive(Clone, Copy, Debug)]
pub struct RegionRecord {
    pub start: u64,
    pub end: u64,
    pub usable: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryStats {
    pub frames_total: usize,
    pub frames_used: usize,
    pub frames_free: usize,
    pub heap_total: usize,
    pub heap_used: usize,
    pub heap_free: usize,
    pub phys_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    NotInitialized,
    OutOfFrames,
    AlreadyMapped,
    NotMapped,
    Misaligned,
}

pub struct MemoryState {
    regions: [RegionRecord; MAX_REGIONS],
    region_len: usize,
    truncated: bool,
    usable_bytes: u64,
    physical_memory_offset: Option<u64>,
}

pub fn init(regions: &[RegionRecord], physical_memory_offset: Option<u64>) -> &'static MemoryState {
    let state = STATE.call_once(|| {
        let mut built = MemoryState {
            regions: [RegionRecord {
                start: 0,
                end: 0,
                usable: false,
            }; MAX_REGIONS],
            region_len: 0,
            truncated: false,
            usable_bytes: 0,
            physical_memory_offset,
        };
        for region in regions {
            let size = region.end.saturating_sub(region.start);
            if region.usable {
                built.usable_bytes = built.usable_bytes.saturating_add(size);
            }
            if built.region_len < MAX_REGIONS {
                built.regions[built.region_len] = *region;
                built.region_len += 1;
            } else {
                built.truncated = true;
            }
        }
        built
    });
    let frames_ready = FRAMES.lock().is_some();
    if !frames_ready {
        init_frames(state.regions());
    }
    state
}

pub unsafe fn init_paging(offset: u64) {
    unsafe { paging::init(offset) }
}

pub fn init_heap() -> Result<(), MapError> {
    heap::init()
}

pub fn alloc_frame() -> Option<u64> {
    FRAMES.lock().as_mut()?.allocate()
}

pub fn free_frame(phys: u64) -> Result<(), FreeError> {
    FRAMES
        .lock()
        .as_mut()
        .ok_or(FreeError::NotAllocated)?
        .deallocate(phys)
}

pub fn translate(virt: u64) -> Option<u64> {
    paging::translate(virt)
}

pub fn phys_offset() -> Option<u64> {
    paging::phys_offset()
}

pub fn phys_to_virt(phys: u64) -> Option<u64> {
    paging::phys_to_virt(phys)
}

pub fn map_4k(virt: u64, phys: u64, writable: bool) -> Result<(), MapError> {
    paging::map_4k(virt, phys, writable)
}

pub fn map_identity(phys: u64) -> Result<(), MapError> {
    paging::map_identity(phys)
}

pub fn map_mmio(phys: u64, len: u64) -> Result<u64, MapError> {
    paging::map_mmio(phys, len)
}

pub fn unmap_4k(virt: u64) -> Result<u64, MapError> {
    let phys = paging::unmap_4k(virt)?;
    // Кадр данных возвращается в пул. Кадры самих таблиц страниц остаются.
    let _ = free_frame(phys);
    Ok(phys)
}

pub fn stats() -> MemoryStats {
    let (frames_total, frames_used, frames_free) = match FRAMES.lock().as_ref() {
        Some(frames) => (frames.total(), frames.used(), frames.free()),
        None => (0, 0, 0),
    };
    MemoryStats {
        frames_total,
        frames_used,
        frames_free,
        heap_total: heap::total(),
        heap_used: heap::used(),
        heap_free: heap::free(),
        phys_offset: paging::phys_offset(),
    }
}

pub(crate) fn frames() -> spin::MutexGuard<'static, Option<BitmapFrameAlloc>> {
    FRAMES.lock()
}

fn init_frames(regions: &[RegionRecord]) {
    let bitmap = unsafe { &mut *BITMAP.0.get() };
    let alloc = BitmapFrameAlloc::from_regions(bitmap, regions);
    *FRAMES.lock() = Some(alloc);
}

impl MemoryState {
    pub fn usable_bytes(&self) -> u64 {
        self.usable_bytes
    }

    pub fn region_count(&self) -> usize {
        self.region_len
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn physical_memory_offset(&self) -> Option<u64> {
        self.physical_memory_offset
    }

    pub fn regions(&self) -> &[RegionRecord] {
        &self.regions[..self.region_len]
    }
}
