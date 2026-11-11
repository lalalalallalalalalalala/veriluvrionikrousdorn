//! Куча ядра. Стартовый кусок отображён сразу, дальше она растёт, пока
//! хватает кадров, до [`HEAP_MAX`].
//!
//! `KernelAllocator` ставится глобальным аллокатором в бинарнике ядра, а не здесь:
//! иначе тесты этого крейта на хосте подменили бы системный аллокатор.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::frame::PAGE_SIZE;
use crate::paging;
use crate::MapError;

/// Верхняя половина, далеко от смещения физической памяти, которое выбирает загрузчик.
pub const HEAP_START: u64 = 0xFFFF_8100_0000_0000;
pub const HEAP_INITIAL: usize = 256 * 1024;
pub const HEAP_CHUNK: usize = 256 * 1024;
pub const HEAP_MAX: usize = 16 * 1024 * 1024;

static HEAP: Mutex<Heap> = Mutex::new(Heap::empty());
static HEAP_MAPPED: AtomicUsize = AtomicUsize::new(0);
static HEAP_READY: AtomicUsize = AtomicUsize::new(0);

pub fn init() -> Result<(), MapError> {
    map_range(HEAP_START, HEAP_INITIAL)?;
    unsafe {
        core::ptr::write_bytes(HEAP_START as *mut u8, 0, HEAP_INITIAL);
        HEAP.lock().init(HEAP_START as *mut u8, HEAP_INITIAL);
    }
    HEAP_MAPPED.store(HEAP_INITIAL, Ordering::Release);
    HEAP_READY.store(1, Ordering::Release);
    Ok(())
}

pub fn total() -> usize {
    HEAP_MAPPED.load(Ordering::Relaxed)
}

pub fn used() -> usize {
    if HEAP_READY.load(Ordering::Acquire) == 0 {
        0
    } else {
        HEAP.lock().used()
    }
}

pub fn free() -> usize {
    if HEAP_READY.load(Ordering::Acquire) == 0 {
        0
    } else {
        HEAP.lock().free()
    }
}

fn map_range(start: u64, size: usize) -> Result<(), MapError> {
    let pages = size / PAGE_SIZE as usize;
    for index in 0..pages {
        let virt = start + index as u64 * PAGE_SIZE;
        let phys = crate::alloc_frame().ok_or(MapError::OutOfFrames)?;
        if let Err(err) = paging::map_4k(virt, phys, true) {
            let _ = crate::free_frame(phys);
            for done in 0..index {
                let _ = crate::unmap_4k(start + done as u64 * PAGE_SIZE);
            }
            return Err(err);
        }
    }
    unsafe {
        core::ptr::write_bytes(start as *mut u8, 0, size);
    }
    Ok(())
}

/// Вызывается с уже захваченной кучей. Добавляет [`HEAP_CHUNK`], если есть куда.
fn grow(heap: &mut Heap) -> bool {
    let mapped = HEAP_MAPPED.load(Ordering::Relaxed);
    if mapped >= HEAP_MAX || mapped + HEAP_CHUNK > HEAP_MAX {
        return false;
    }
    let start = HEAP_START + mapped as u64;
    if map_range(start, HEAP_CHUNK).is_err() {
        return false;
    }
    unsafe {
        heap.extend(HEAP_CHUNK);
    }
    HEAP_MAPPED.store(mapped + HEAP_CHUNK, Ordering::Relaxed);
    true
}

pub struct KernelAllocator;

impl KernelAllocator {
    pub const fn new() -> Self {
        Self
    }
}

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        interrupts::without_interrupts(|| {
            if HEAP_READY.load(Ordering::Acquire) == 0 || layout.size() == 0 {
                return core::ptr::null_mut();
            }
            let mut heap = HEAP.lock();
            loop {
                if let Ok(ptr) = heap.allocate_first_fit(layout) {
                    return ptr.as_ptr();
                }
                if !grow(&mut heap) {
                    return core::ptr::null_mut();
                }
            }
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }
        interrupts::without_interrupts(|| unsafe {
            HEAP.lock()
                .deallocate(core::ptr::NonNull::new_unchecked(ptr), layout);
        });
    }
}
