//! Планировщик задач. Сейчас здесь только типы, на которые сядет многозадачность.
//!
//! Ядро однопоточное: [`Scheduler::spawn`] возвращает [`SchedError::NotImplemented`],
//! а [`Scheduler::schedule`] ничего не переключает.

#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedError {
    NotImplemented,
}

pub struct Scheduler;

impl Scheduler {
    pub const fn new() -> Self {
        Self
    }

    pub fn spawn(&mut self, _entry: fn() -> !) -> Result<TaskId, SchedError> {
        Err(SchedError::NotImplemented)
    }

    pub fn schedule(&mut self) {}
}

pub fn init() -> Scheduler {
    Scheduler::new()
}
