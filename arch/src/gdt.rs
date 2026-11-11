use spin::Once;
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Индекс IST для двойного отказа: у него должен быть свой стек.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const STACK_SIZE: usize = 4096 * 5;

#[repr(C, align(16))]
struct InterruptStack([u8; STACK_SIZE]);

static DOUBLE_FAULT_STACK: InterruptStack = InterruptStack([0; STACK_SIZE]);
static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    tss: SegmentSelector,
}

static mut TSS_PTR: *mut TaskStateSegment = core::ptr::null_mut();

pub fn user_cs() -> u16 {
    let tables = gdt(tss());
    tables.1.user_code.0
}

pub fn user_ss() -> u16 {
    let tables = gdt(tss());
    tables.1.user_data.0
}

/// Вершина стека ядра для смены привилегии (RSP0). Ноль — не трогать.
pub fn set_kernel_stack(top: u64) {
    unsafe {
        if !TSS_PTR.is_null() && top != 0 {
            (*TSS_PTR).privilege_stack_table[0] = VirtAddr::new(top);
        }
    }
}

pub fn init() {
    let tss = tss();
    unsafe {
        TSS_PTR = tss as *const TaskStateSegment as *mut TaskStateSegment;
    }
    let tables = gdt(tss);
    tables.0.load();
    unsafe {
        CS::set_reg(tables.1.code);
        SS::set_reg(tables.1.data);
        load_tss(tables.1.tss);
    }
}

fn tss() -> &'static TaskStateSegment {
    TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        let stack_bottom = core::ptr::addr_of!(DOUBLE_FAULT_STACK.0) as u64;
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(stack_bottom + STACK_SIZE as u64);
        tss
    })
}

fn gdt(tss: &'static TaskStateSegment) -> &'static (GlobalDescriptorTable, Selectors) {
    GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        // Порядок важен для iretq: данные пользователя сразу после данных ядра,
        // код пользователя следом. Селекторы получают RPL 3 из DPL дескриптора.
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(tss));
        (
            gdt,
            Selectors {
                code,
                data,
                user_data,
                user_code,
                tss: tss_selector,
            },
        )
    })
}
