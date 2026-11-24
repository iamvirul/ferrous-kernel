//! Scheduler (Phase 2.2).
//!
//! Phase 2.2.1 provides the ready queue only. The round-robin policy
//! (2.2.2) and the context switch (2.2.3) build on it.
//!
//! # Design
//!
//! - [`RunQueue`] is the pure, host-tested `ferrous_sched::RunQueue`, sized
//!   to [`RUN_QUEUE_CAPACITY`] and holding `TaskId` handles. It never holds
//!   TCBs: ownership stays with the task table, so the queue needs no
//!   `unsafe` and its contents cannot dangle.
//! - [`SharedRunQueue`] wraps it in an [`IrqSpinLock`], so every access is
//!   both mutually exclusive and safe against this CPU's interrupt handlers
//!   (the timer will enqueue and dequeue from IRQ context in 2.2.4).
//! - Single-owner invariant: a task is either queued or running, never
//!   both. `enqueue` rejects a task that is already queued, and the
//!   scheduler removes a task with `dequeue` before running it.

use crate::sync::IrqSpinLock;
use crate::task::TaskId;

pub use ferrous_sched::RunQueueError;

/// Maximum number of simultaneously runnable tasks (single CPU, Phase 2).
pub const RUN_QUEUE_CAPACITY: usize = 256;

/// The kernel's ready queue of runnable tasks.
pub type RunQueue = ferrous_sched::RunQueue<TaskId, RUN_QUEUE_CAPACITY>;

/// A [`RunQueue`] behind an interrupt-safe lock: the only way the kernel
/// shares a run queue.
pub type SharedRunQueue = IrqSpinLock<RunQueue>;

/// Exercise the run queue through its interrupt-safe lock (Phase 2.2.1).
///
/// Uses a dedicated `static` (the shape the scheduler will use) rather
/// than any shared kernel state.
///
/// # Panics
///
/// On any failed check: this is a boot-time self-test.
pub fn smoke_test() {
    use crate::arch::x86_64::cpu::{interrupts_enabled, irq_disable_depth};

    static QUEUE: SharedRunQueue = IrqSpinLock::new(RunQueue::new());
    static OTHER: IrqSpinLock<u8> = IrqSpinLock::new(0);

    log::info!("Run queue smoke test (Phase 2.2.1)");

    // 19.1: the lock masks interrupts while held and restores the prior state.
    let if_before = interrupts_enabled();
    {
        let guard = QUEUE.lock();
        assert!(
            !interrupts_enabled(),
            "IRQs must be off while the lock is held"
        );
        assert!(QUEUE.is_locked());
        assert!(QUEUE.try_lock().is_none(), "lock must be exclusive");
        assert!(guard.is_empty());
    }
    assert_eq!(interrupts_enabled(), if_before, "lock must restore IF");
    assert!(!QUEUE.is_locked());

    // Out-of-order release of two locks: dropping the outer guard first must
    // not end the interrupt-disabled scope while the inner lock is held.
    let depth_before = irq_disable_depth();
    let outer = QUEUE.lock();
    let inner = OTHER.lock();
    assert_eq!(irq_disable_depth(), depth_before + 2);
    drop(outer);
    assert_eq!(irq_disable_depth(), depth_before + 1);
    assert!(
        !interrupts_enabled(),
        "IRQs must stay off: inner lock still held"
    );
    drop(inner);
    assert_eq!(irq_disable_depth(), depth_before);
    assert_eq!(interrupts_enabled(), if_before, "last release restores IF");
    log::info!("[OK] 19.1) IrqSpinLock: exclusive, IRQs masked while held, IF restored");
    log::info!("[OK] 19.1) IrqSpinLock: out-of-order release keeps IRQs masked until last guard");

    let mut queue = QUEUE.lock();
    let (a, b, c) = (TaskId::new(1), TaskId::new(2), TaskId::new(3));

    // 19.2: FIFO order.
    for t in [a, b, c] {
        queue.enqueue(t).expect("enqueue into empty queue");
    }
    assert_eq!(queue.dequeue(), Some(a));
    assert_eq!(queue.dequeue(), Some(b));
    assert_eq!(queue.dequeue(), Some(c));
    assert_eq!(queue.dequeue(), None);
    log::info!("[OK] 19.2) enqueue/dequeue: FIFO order preserved");

    // 19.3: remove from the middle keeps the remaining order.
    for t in [a, b, c] {
        queue.enqueue(t).expect("enqueue");
    }
    assert!(queue.remove(b), "b was queued");
    assert!(!queue.remove(b), "b is gone");
    assert_eq!(queue.dequeue(), Some(a));
    assert_eq!(queue.dequeue(), Some(c));
    log::info!("[OK] 19.3) remove: middle task removed, order of others kept");

    // 19.4: single-owner and capacity limits.
    queue.enqueue(a).expect("enqueue");
    assert_eq!(queue.enqueue(a), Err(RunQueueError::AlreadyQueued));
    queue.dequeue();
    for id in 0..RUN_QUEUE_CAPACITY as u64 {
        queue.enqueue(TaskId::new(id)).expect("fill to capacity");
    }
    assert!(queue.is_full());
    assert_eq!(
        queue.enqueue(TaskId::new(u64::MAX)),
        Err(RunQueueError::Full)
    );
    log::info!(
        "[OK] 19.4) duplicate rejected; full at {} tasks rejected",
        RUN_QUEUE_CAPACITY
    );

    // 19.5: draining leaves the queue empty.
    while queue.dequeue().is_some() {}
    assert!(queue.is_empty());
    log::info!("[OK] 19.5) is_empty after drain");

    log::info!("Run queue smoke test complete");
}
