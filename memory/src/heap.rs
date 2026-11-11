//! Куча ядра. Страницы заранее отображены, блоки раздаёт linked list allocator.
//!
//! `KernelAllocator` ставится глобальным аллокатором в бинарнике ядра, а не здесь:
//! иначе тесты этого крейта на хосте подменили бы системный аллокатор.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;

use crate::frame::PAGE_SIZE;
use crate::paging;
use crate::MapError;

/// Верхняя половина, далеко от смещения физической памяти, которое выбирает загрузчик.
pub const HEAP_START: u64 = 0xFFFF_8100_0000_0000;
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;

static HEAP: Mutex<Heap> = Mutex::new(Heap::empty());
static HEAP_USED: AtomicUsize = AtomicUsize::new(0);
static HEAP_READY: AtomicUsize = AtomicUsize::new(0);

pub fn init() -> Result<(), MapError> {
    let pages = HEAP_SIZE / PAGE_SIZE as usize;
    for index in 0..pages {
        let virt = HEAP_START + index as u64 * PAGE_SIZE;
        let phys = crate::alloc_frame().ok_or(MapError::OutOfFrames)?;
        paging::map_4k(virt, phys, true)?;
    }
    unsafe {
        core::ptr::write_bytes(HEAP_START as *mut u8, 0, HEAP_SIZE);
        HEAP.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }
    HEAP_READY.store(1, Ordering::Release);
    Ok(())
}

pub fn total() -> usize {
    if HEAP_READY.load(Ordering::Acquire) == 0 {
        0
    } else {
        HEAP_SIZE
    }
}

pub fn used() -> usize {
    HEAP_USED.load(Ordering::Relaxed)
}

pub fn free() -> usize {
    total().saturating_sub(used())
}

pub struct KernelAllocator;

impl KernelAllocator {
    pub const fn new() -> Self {
        Self
    }
}

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if HEAP_READY.load(Ordering::Acquire) == 0 {
            return core::ptr::null_mut();
        }
        match HEAP.lock().allocate_first_fit(layout) {
            Ok(ptr) => {
                HEAP_USED.fetch_add(layout.size(), Ordering::Relaxed);
                ptr.as_ptr()
            }
            Err(()) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            HEAP.lock()
                .deallocate(core::ptr::NonNull::new_unchecked(ptr), layout);
        }
        HEAP_USED.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}
