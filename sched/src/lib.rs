//! Потоки ядра: свой стек в куче, кооперативные yield/sleep/exit и
//! вытесняющий round-robin по тику PIT.
//!
//! Переключение — это `context_switch`: сохраняются callee-saved регистры и RSP.
//! Таймер шлёт EOI и только потом зовёт [`on_timer`], поэтому следующий тик
//! приходит уже новой задаче. Стек вышедшей задачи освобождается, когда она
//! точно не текущая.

#![no_std]

extern crate alloc;

use core::alloc::Layout;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use x86_64::instructions::interrupts;

pub const MAX_TASKS: usize = 32;
const STACK_SIZE: usize = 64 * 1024;
const NAME_CAP: usize = 16;

static READY: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Empty,
    Ready,
    Running,
    Sleeping,
    Dead,
}

#[derive(Clone, Copy)]
struct Fd {
    used: bool,
    inode: u64,
    pos: u64,
}

impl Fd {
    const fn empty() -> Self {
        Self {
            used: false,
            inode: 0,
            pos: 0,
        }
    }
}

struct Task {
    id: u64,
    name: [u8; NAME_CAP],
    name_len: u8,
    state: State,
    ticks: u64,
    sleep_until: u64,
    entry: Option<fn()>,
    stack: *mut u8,
    stack_layout: Option<Layout>,
    rsp: u64,
    kstack_top: u64,
    cr3: u64,
    user: bool,
    user_ip: u64,
    user_sp: u64,
    fds: [Fd; 8],
}

impl Task {
    const fn empty() -> Self {
        Self {
            id: 0,
            name: [0; NAME_CAP],
            name_len: 0,
            state: State::Empty,
            ticks: 0,
            sleep_until: 0,
            entry: None,
            stack: core::ptr::null_mut(),
            stack_layout: None,
            rsp: 0,
            kstack_top: 0,
            cr3: 0,
            user: false,
            user_ip: 0,
            user_sp: 0,
            fds: [const { Fd::empty() }; 8],
        }
    }
}

struct Table {
    tasks: [Task; MAX_TASKS],
    current: usize,
    next_id: u64,
}

static mut TABLE: Table = Table {
    tasks: [const { Task::empty() }; MAX_TASKS],
    current: 0,
    next_id: 1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedError {
    NoSlot,
    OutOfMemory,
    NotFound,
    Busy,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub struct TaskInfo {
    pub id: u64,
    pub name: [u8; NAME_CAP],
    pub name_len: u8,
    pub mode: &'static str,
    pub state: &'static str,
    pub ticks: u64,
}

impl TaskInfo {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            name: [0; NAME_CAP],
            name_len: 0,
            mode: "",
            state: "",
            ticks: 0,
        }
    }
}

core::arch::global_asm!(
    r#"
    .section .text
    .global context_switch
    context_switch:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov [rdi], rsp
        mov rsp, [rsi]
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp
        ret
    "#
);

unsafe extern "C" {
    fn context_switch(old_rsp: *mut u64, new_rsp: *const u64);
}

/// Текущий поток — тот, на чьём стеке мы уже работаем (поток загрузчика).
pub fn init() {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        table.tasks[0] = Task {
            id: 0,
            name: *b"idle\0\0\0\0\0\0\0\0\0\0\0\0",
            name_len: 4,
            state: State::Running,
            ticks: 0,
            sleep_until: 0,
            entry: None,
            stack: core::ptr::null_mut(),
            stack_layout: None,
            rsp: 0,
            kstack_top: 0,
            cr3: 0,
            user: false,
            user_ip: 0,
            user_sp: 0,
            fds: [const { Fd::empty() }; 8],
        };
        table.current = 0;
        table.next_id = 1;
        READY.store(true, Ordering::Release);
    });
    vlr_arch::set_preempt_hook(on_timer);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn spawn(name: &str, entry: fn()) -> Result<TaskId, SchedError> {
    interrupts::without_interrupts(|| unsafe { spawn_locked(name, entry) })
}

pub fn spawn_user(
    name: &str,
    entry: fn(),
    cr3: u64,
    user_ip: u64,
    user_sp: u64,
) -> Result<TaskId, SchedError> {
    interrupts::without_interrupts(|| unsafe {
        let id = spawn_locked(name, entry)?;
        let table = table();
        if let Some(task) = table.tasks.iter_mut().find(|task| task.id == id.0) {
            task.cr3 = cr3;
            task.user = true;
            task.user_ip = user_ip;
            task.user_sp = user_sp;
        }
        Ok(id)
    })
}

pub fn user_start() -> (u64, u64, u64) {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let task = &table.tasks[table.current];
        (task.user_ip, task.user_sp, task.kstack_top)
    })
}

pub fn current_cr3() -> u64 {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        table.tasks[table.current].cr3
    })
}

pub fn alive(id: u64) -> bool {
    interrupts::without_interrupts(|| unsafe {
        table().tasks.iter().any(|task| {
            task.id == id && task.state != State::Empty && task.state != State::Dead
        })
    })
}

pub fn fd_open(inode: u64) -> Option<u64> {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let task = &mut table.tasks[table.current];
        for (index, slot) in task.fds.iter_mut().enumerate().skip(2) {
            if !slot.used {
                *slot = Fd {
                    used: true,
                    inode,
                    pos: 0,
                };
                return Some(index as u64);
            }
        }
        None
    })
}

pub fn fd_read(fd: u64) -> Option<(u64, u64)> {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let task = &table.tasks[table.current];
        let slot = task.fds.get(fd as usize)?;
        if slot.used { Some((slot.inode, slot.pos)) } else { None }
    })
}

pub fn fd_advance(fd: u64, pos: u64) {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let task = &mut table.tasks[table.current];
        if let Some(slot) = task.fds.get_mut(fd as usize) {
            if slot.used {
                slot.pos = pos;
            }
        }
    });
}

pub fn fd_close(fd: u64) -> bool {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let task = &mut table.tasks[table.current];
        if let Some(slot) = task.fds.get_mut(fd as usize) {
            if slot.used && fd >= 2 {
                *slot = Fd::empty();
                return true;
            }
        }
        false
    })
}

pub fn yield_now() {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let current = table.current;
        if table.tasks[current].state == State::Running {
            table.tasks[current].state = State::Ready;
        }
        schedule(table);
    });
}

pub fn sleep(delay: u64) {
    interrupts::without_interrupts(|| unsafe {
        let now = TICKS.load(Ordering::Relaxed);
        let table = table();
        let current = table.current;
        table.tasks[current].sleep_until = now.saturating_add(delay.max(1));
        table.tasks[current].state = State::Sleeping;
        schedule(table);
    });
}

pub fn exit() -> ! {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let current = table.current;
        table.tasks[current].state = State::Dead;
        schedule(table);
    });
    loop {
        x86_64::instructions::hlt();
    }
}

pub fn kill(id: u64) -> Result<(), SchedError> {
    interrupts::without_interrupts(|| unsafe {
        if id == 0 {
            return Err(SchedError::Busy);
        }
        let table = table();
        let Some(index) = find(table, id) else {
            return Err(SchedError::NotFound);
        };
        if index == table.current {
            table.tasks[index].state = State::Dead;
            schedule(table);
            return Ok(());
        }
        table.tasks[index].state = State::Dead;
        reap(table);
        Ok(())
    })
}

pub fn exists(name: &str) -> bool {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        table.tasks.iter().any(|task| {
            task.state != State::Empty && task.state != State::Dead && task_name(task) == name
        })
    })
}

pub fn current_id() -> u64 {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        table.tasks[table.current].id
    })
}

pub fn kernel_stack_top() -> u64 {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        table.tasks[table.current].kstack_top
    })
}

pub fn become_user(cr3: u64) {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let current = table.current;
        table.tasks[current].cr3 = cr3;
        table.tasks[current].user = true;
    });
}

pub fn snapshot(out: &mut [TaskInfo]) -> usize {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let mut count = 0;
        for task in &table.tasks {
            if task.state == State::Empty || task.state == State::Dead {
                continue;
            }
            if count >= out.len() {
                break;
            }
            out[count] = TaskInfo {
                id: task.id,
                name: task.name,
                name_len: task.name_len,
                mode: if task.user { "user" } else { "kernel" },
                state: state_name(task.state),
                ticks: task.ticks,
            };
            count += 1;
        }
        count
    })
}

/// Простой цикл, когда больше нечего делать на стеке загрузчика.
pub fn idle() -> ! {
    loop {
        interrupts::enable();
        if others_ready() {
            yield_now();
        } else {
            x86_64::instructions::hlt();
        }
    }
}

/// Хук таймера. Прерывания уже закрыты, EOI уже отправлен.
fn on_timer() {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let table = table();
        let current = table.current;
        table.tasks[current].ticks = table.tasks[current].ticks.saturating_add(1);
        wake(table);
        schedule(table);
    }
}

fn others_ready() -> bool {
    interrupts::without_interrupts(|| unsafe {
        let table = table();
        let current = table.current;
        table
            .tasks
            .iter()
            .enumerate()
            .any(|(index, task)| index != current && task.state == State::Ready)
    })
}

unsafe fn table() -> &'static mut Table {
    unsafe { &mut *core::ptr::addr_of_mut!(TABLE) }
}

unsafe fn spawn_locked(name: &str, entry: fn()) -> Result<TaskId, SchedError> {
    let table = unsafe { table() };
    let Some(index) = table
        .tasks
        .iter()
        .position(|task| task.state == State::Empty)
    else {
        return Err(SchedError::NoSlot);
    };
    let layout = Layout::from_size_align(STACK_SIZE, 16).map_err(|_| SchedError::OutOfMemory)?;
    let stack = unsafe { alloc::alloc::alloc(layout) };
    if stack.is_null() {
        return Err(SchedError::OutOfMemory);
    }
    unsafe {
        core::ptr::write_bytes(stack, 0, STACK_SIZE);
    }
    let id = table.next_id;
    table.next_id = table.next_id.saturating_add(1);
    let mut task = Task::empty();
    task.id = id;
    task.name_len = copy_name(&mut task.name, name);
    task.state = State::Ready;
    task.entry = Some(entry);
    task.stack = stack;
    task.stack_layout = Some(layout);
    task.rsp = prepare_stack(stack, STACK_SIZE);
    let limit = stack as usize + STACK_SIZE;
    task.kstack_top = (limit & !15) as u64;
    table.tasks[index] = task;
    Ok(TaskId(id))
}

fn prepare_stack(bottom: *mut u8, size: usize) -> u64 {
    let limit = bottom as usize + size;
    let ret_slot = ((limit - 8) & !15) as *mut u64;
    unsafe {
        ret_slot.write(task_trampoline as *const () as usize as u64);
        let mut sp = ret_slot;
        for _ in 0..6 {
            sp = sp.sub(1);
            sp.write(0);
        }
        sp as u64
    }
}

extern "C" fn task_trampoline() -> ! {
    let entry = interrupts::without_interrupts(|| unsafe {
        let table = table();
        let current = table.current;
        table.tasks[current].entry.take()
    });
    interrupts::enable();
    if let Some(entry) = entry {
        entry();
    }
    exit();
}

unsafe fn schedule(table: &mut Table) {
    wake(table);
    reap(table);
    let current = table.current;
    if table.tasks[current].state == State::Running {
        table.tasks[current].state = State::Ready;
    }
    let next = pick_next(table, current);
    table.tasks[next].state = State::Running;
    if next == current {
        return;
    }
    table.current = next;
    let cr3 = table.tasks[next].cr3;
    let kstack = table.tasks[next].kstack_top;
    let old_rsp = core::ptr::addr_of_mut!(table.tasks[current].rsp);
    let new_rsp = core::ptr::addr_of!(table.tasks[next].rsp);
    vlr_memory::activate_cr3(cr3);
    vlr_arch::set_kernel_stack(kstack);
    unsafe {
        context_switch(old_rsp, new_rsp);
    }
}

fn pick_next(table: &Table, current: usize) -> usize {
    for step in 1..MAX_TASKS {
        let index = (current + step) % MAX_TASKS;
        if table.tasks[index].state == State::Ready {
            return index;
        }
    }
    current
}

fn wake(table: &mut Table) {
    let now = TICKS.load(Ordering::Relaxed);
    for task in &mut table.tasks {
        if task.state == State::Sleeping && now >= task.sleep_until {
            task.state = State::Ready;
        }
    }
}

fn reap(table: &mut Table) {
    let current = table.current;
    for index in 0..MAX_TASKS {
        if index == current || table.tasks[index].state != State::Dead {
            continue;
        }
        let stack = table.tasks[index].stack;
        let layout = table.tasks[index].stack_layout;
        let cr3 = table.tasks[index].cr3;
        table.tasks[index] = Task::empty();
        if cr3 != 0 {
            vlr_memory::free_user_space(cr3);
        }
        if let Some(layout) = layout {
            if !stack.is_null() {
                unsafe { alloc::alloc::dealloc(stack, layout) };
            }
        }
    }
}

fn find(table: &Table, id: u64) -> Option<usize> {
    table
        .tasks
        .iter()
        .position(|task| task.state != State::Empty && task.state != State::Dead && task.id == id)
}

fn task_name(task: &Task) -> &str {
    core::str::from_utf8(&task.name[..task.name_len as usize]).unwrap_or("?")
}

fn copy_name(dst: &mut [u8; NAME_CAP], name: &str) -> u8 {
    let bytes = name.as_bytes();
    let len = bytes.len().min(NAME_CAP);
    dst[..len].copy_from_slice(&bytes[..len]);
    len as u8
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Running => "running",
        State::Ready => "ready",
        State::Sleeping => "sleeping",
        State::Dead => "dead",
        State::Empty => "empty",
    }
}
