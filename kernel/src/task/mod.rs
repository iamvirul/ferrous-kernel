//! Task and process abstractions (Phase 2.1.1).
//!
//! This module defines the core data structures that represent concurrent
//! execution in Ferrous:
//!
//! - [`TaskId`] / [`ProcessId`] — opaque, unforgeable numeric identifiers.
//! - [`TaskState`] / [`ProcessState`] — state-machine types with validated
//!   transitions.
//! - [`TaskControlBlock`] — per-task execution context (saved registers,
//!   kernel stack, scheduling metadata).
//! - [`Process`] — address-space owner that groups one or more tasks.
//!
//! # Design constraints
//!
//! - All types are `no_std` compatible; no `std` is required or used.
//! - `TaskControlBlock` and `Process` are designed for slab allocation
//!   (fixed size, no internal self-references); heap allocation is used in
//!   Phase 2.1.1 tests while a slab allocator is introduced in Phase 2.2.
//! - State transitions are validated at runtime via atomic compare-and-swap;
//!   invalid transitions return [`TaskStateError`] / [`ProcessStateError`].
//! - `repr(C)` on [`RegisterState`] and [`TaskControlBlock`] keeps field
//!   offsets stable for the context-switch assembly added in Phase 2.2.3.

pub mod process;
pub mod task;

pub use process::{Process, ProcessId, ProcessState, ProcessStateError};
pub use task::{RegisterState, TaskControlBlock, TaskId, TaskPriority, TaskState, TaskStateError};

// ---------------------------------------------------------------------------
// Smoke test (called from boot/src/main.rs Step 14)
// ---------------------------------------------------------------------------

/// Run Phase 2.1.1 smoke tests, printing results to the log.
///
/// Tests:
/// 1. `TaskId` / `ProcessId` newtype round-trips.
/// 2. `TaskState` valid and invalid transitions.
/// 3. `ProcessState` valid and invalid transitions.
/// 4. `TaskControlBlock` construction and atomic state access.
/// 5. `Process` construction, task registration, and exit-code storage.
pub fn smoke_test() {
    use crate::task::process::MAX_TASKS_PER_PROCESS;

    log::info!("Task/process data structure smoke test (Phase 2.1.1)");

    // 14.1 — ID newtype round-trips
    let tid = TaskId::new(42);
    let pid = ProcessId::new(7);
    assert_eq!(tid.as_u64(), 42, "TaskId round-trip");
    assert_eq!(pid.as_u64(), 7, "ProcessId round-trip");
    log::info!(
        "[OK] 14.1) TaskId/ProcessId newtype: tid={} pid={}",
        tid.as_u64(),
        pid.as_u64()
    );

    // 14.2 — TaskState valid transitions
    assert!(TaskState::Ready.can_transition_to(TaskState::Running));
    assert!(TaskState::Running.can_transition_to(TaskState::Blocked));
    assert!(TaskState::Blocked.can_transition_to(TaskState::Ready));
    assert!(TaskState::Running.can_transition_to(TaskState::Exiting));
    assert!(TaskState::Exiting.can_transition_to(TaskState::Zombie));
    log::info!("[OK] 14.2) TaskState: valid transitions accepted");

    // 14.3 — TaskState invalid transitions rejected
    assert!(!TaskState::Ready.can_transition_to(TaskState::Blocked));
    assert!(!TaskState::Zombie.can_transition_to(TaskState::Ready));
    assert!(!TaskState::Blocked.can_transition_to(TaskState::Running));
    log::info!("[OK] 14.3) TaskState: invalid transitions rejected");

    // 14.4 — ProcessState transitions
    assert!(ProcessState::Active.can_transition_to(ProcessState::Exiting));
    assert!(ProcessState::Exiting.can_transition_to(ProcessState::Zombie));
    assert!(!ProcessState::Zombie.can_transition_to(ProcessState::Active));
    log::info!("[OK] 14.4) ProcessState: transitions enforced");

    // 14.5 — TaskControlBlock construction and state CAS
    // SAFETY: stack bounds are fabricated for the smoke test only; no context
    // switch is performed and the pointers are never dereferenced.
    #[repr(align(8))]
    struct AlignedStack([u8; 256]);
    let mut stack = AlignedStack([0u8; 256]);
    let stack_bottom = stack.0.as_mut_ptr();
    let stack_top = unsafe { stack_bottom.add(256) };

    let tcb = unsafe {
        TaskControlBlock::new(
            TaskId::new(1),
            ProcessId::new(1),
            stack_top,
            stack_bottom,
            TaskPriority::Normal,
        )
    };

    assert_eq!(tcb.state(), TaskState::Ready);
    tcb.try_transition(TaskState::Ready, TaskState::Running)
        .expect("Ready -> Running should succeed");
    assert_eq!(tcb.state(), TaskState::Running);
    assert!(
        tcb.try_transition(TaskState::Ready, TaskState::Running)
            .is_err(),
        "CAS must fail when current state != expected"
    );
    log::info!("[OK] 14.5) TaskControlBlock: construction and atomic CAS");

    // 14.6 — Process construction and task registration
    let mut proc = Process::new(ProcessId::new(1));
    assert_eq!(proc.state(), ProcessState::Active);
    assert_eq!(proc.task_count(), 0);

    proc.add_task(TaskId::new(1)).expect("first task add");
    proc.add_task(TaskId::new(2)).expect("second task add");
    assert_eq!(proc.task_count(), 2);
    assert!(proc.has_task(TaskId::new(1)));
    assert!(!proc.has_task(TaskId::new(99)));
    log::info!(
        "[OK] 14.6) Process: construction and task registration (count={})",
        proc.task_count()
    );

    // 14.7 — Process task list capacity enforced
    let mut full_proc = Process::new(ProcessId::new(2));
    for i in 0..MAX_TASKS_PER_PROCESS {
        full_proc
            .add_task(TaskId::new(i as u64))
            .expect("fill task list");
    }
    assert!(
        full_proc.add_task(TaskId::new(99)).is_err(),
        "overflow must be rejected"
    );
    log::info!(
        "[OK] 14.7) Process: task list capacity enforced (max={})",
        MAX_TASKS_PER_PROCESS
    );

    // 14.8 — Process exit code
    proc.try_transition(ProcessState::Active, ProcessState::Exiting)
        .expect("Active -> Exiting");
    proc.set_exit_code(0).expect("set_exit_code in Exiting state");
    assert_eq!(proc.exit_code(), Some(0));
    log::info!("[OK] 14.8) Process: exit code stored on Exiting state");

    log::info!("Task/process data structure smoke test complete");
}
