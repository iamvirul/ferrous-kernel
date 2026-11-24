//! The kernel scheduler (Phase 2.2.2, ADR-0004).
//!
//! Single-CPU, single-priority, strict FIFO round-robin with a fixed time
//! slice. The *policy* is `ferrous_sched::RoundRobin` (pure, host-tested);
//! this module owns the task table and turns policy decisions into TCB state
//! transitions and `switch_to` calls.
//!
//! # Entry points
//!
//! | Function | Path |
//! |----------|------|
//! | [`yield_now`] | voluntary (also `sys_yield`) |
//! | [`block_current`] / [`wake`] | blocking wait / event wake-up |
//! | [`exit_current`] | task exit |
//! | [`scheduler_tick`] | timer ISR hook (#75): accounting only, never switches |
//! | [`preempt_if_needed`] | safe point: switches if a tick asked for it |
//!
//! # Switch protocol (ADR-0004 §3, ADR-0008 depth-1 contract)
//!
//! `schedule` opens the switch `InterruptGuard`, decides and updates TCB
//! states **under the scheduler lock**, releases the lock, and only then
//! calls `switch_to`. No lock is ever held across a switch.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use ferrous_sched::{RoundRobin, RunQueueError, SwitchReason};

use super::switch::{init_kernel_task_context, switch_to};
use super::RUN_QUEUE_CAPACITY;
use crate::arch::x86_64::cpu::{disable_interrupts, irq_disable_depth};
use crate::memory::address_space::active_pml4_phys;
use crate::sync::IrqSpinLock;
use crate::task::{ProcessId, TaskControlBlock, TaskId, TaskPriority, TaskState};

/// Ticks a task may run before it is preempted. The tick period belongs to
/// the timer (#75): at the planned 1 kHz this is 10 ms.
pub const TIME_SLICE_TICKS: u32 = 10;

/// Task table capacity. `TaskId` is the slot index. The policy's queue
/// (`RUN_QUEUE_CAPACITY`) must hold every non-idle task at once.
pub const MAX_TASKS: usize = 64;
const _: () = assert!(MAX_TASKS - 1 <= RUN_QUEUE_CAPACITY);

/// Size of a scheduler-managed kernel stack.
pub const KERNEL_TASK_STACK_SIZE: usize = 16 * 1024;

/// A kernel stack for [`spawn`] / [`init`]: 16-byte aligned, as the entry
/// trampoline and the SYSCALL stub require.
#[repr(C, align(16))]
pub struct KernelStack(pub [u8; KERNEL_TASK_STACK_SIZE]);

impl KernelStack {
    /// A zeroed stack, suitable for a `static`.
    pub const fn new() -> Self {
        Self([0; KERNEL_TASK_STACK_SIZE])
    }
}

impl Default for KernelStack {
    fn default() -> Self {
        Self::new()
    }
}

/// Task slot of the boot path, which becomes the first scheduled task.
pub const BOOT_TASK: TaskId = TaskId::new(0);
/// Task slot of the idle task.
pub const IDLE_TASK: TaskId = TaskId::new(1);

/// Errors from the scheduler API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedError {
    /// [`init`] has not run yet.
    NotInitialized,
    /// [`init`] was called a second time.
    AlreadyInitialized,
    /// All [`MAX_TASKS`] slots are in use.
    TableFull,
    /// The id does not name a created task.
    NoSuchTask,
    /// [`wake`] was called on a task that is not blocked.
    NotBlocked,
}

// ---------------------------------------------------------------------------
// Task table
// ---------------------------------------------------------------------------

/// Fixed storage for every TCB. Static, so TCBs never move (`switch_to`
/// holds raw pointers to them across suspensions).
struct TaskTable([UnsafeCell<Option<TaskControlBlock>>; MAX_TASKS]);

// SAFETY: a slot is written (None -> Some) exactly once, by `init`/`spawn`
// under the scheduler lock, before its id is published to the policy. It is
// never freed (no reaping yet). After that, shared access goes through the
// TCB's atomics (state, cpu_ticks) and short-lived `&` borrows, and the
// register/FPU save areas are written only by `switch_to` through raw
// pointers while no borrow is live (single CPU, interrupts disabled).
unsafe impl Sync for TaskTable {}

static TASKS: TaskTable = TaskTable([const { UnsafeCell::new(None) }; MAX_TASKS]);

/// Raw pointer to the TCB of `id`, if that slot is populated.
fn tcb_ptr(id: TaskId) -> Option<*mut TaskControlBlock> {
    let slot = TASKS.0.get(id.as_u64() as usize)?;
    // SAFETY: populated slots are never written again (see `TaskTable`), so
    // reading the discriminant cannot race a write.
    unsafe { (*slot.get()).as_mut().map(|t| t as *mut TaskControlBlock) }
}

/// Run `f` on a short-lived shared borrow of `id`'s TCB, with interrupts
/// masked for the whole lookup and borrow.
///
/// The masking matters once the timer can preempt (#75): an interrupt that
/// reached `switch_to` while this borrow was live would write the borrowed
/// TCB's save areas, violating the `TaskTable` invariant. Nesting is fine:
/// internal callers that already hold the scheduler lock just add a level.
fn with_tcb<R>(id: TaskId, f: impl FnOnce(&TaskControlBlock) -> R) -> Option<R> {
    let _irq = disable_interrupts();
    // SAFETY: the pointer is valid (static slot). The borrow ends before
    // `_irq` is dropped, so no switch can happen while it is live.
    tcb_ptr(id).map(|p| f(unsafe { &*p }))
}

/// Move `id` from `from` to `to`, panicking on an illegal transition. The
/// TCB state machine is the second line of defence behind the policy.
fn transition(id: TaskId, from: TaskState, to: TaskState) {
    with_tcb(id, |t| t.try_transition(from, to))
        .expect("scheduled task exists")
        .unwrap_or_else(|e| panic!("scheduler: task {id:?}: {e:?}"));
}

// ---------------------------------------------------------------------------
// Scheduler state
// ---------------------------------------------------------------------------

struct Scheduler {
    policy: RoundRobin<TaskId, RUN_QUEUE_CAPACITY>,
    next_free_slot: usize,
    /// The kernel's permanent page-table root, captured at `init`. Every
    /// kernel task runs under it, so a task can never cache a process root
    /// that might later be destroyed (`TaskControlBlock::new` contract).
    kernel_root: u64,
}

static SCHEDULER: IrqSpinLock<Option<Scheduler>> = IrqSpinLock::new(None);

/// Context switches performed (observability).
static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Install `stack` as task `slot`'s kernel stack, running `entry` (or, for
/// the boot task, already running on it: `entry = None`), under the
/// permanent page-table root `root`.
///
/// # Safety
///
/// `slot` is empty and will not be published before this returns; the
/// stack is valid, 16-byte aligned, and exclusively this task's; `root`
/// outlives the task.
unsafe fn install(slot: usize, bottom: *mut u8, top: *mut u8, root: u64, entry: Option<fn() -> !>) {
    let mut tcb = TaskControlBlock::new(
        TaskId::new(slot as u64),
        ProcessId::new(0),
        top,
        bottom,
        TaskPriority::Normal,
        root,
    );
    if let Some(entry) = entry {
        init_kernel_task_context(&mut tcb, entry);
    }
    *TASKS.0[slot].get() = Some(tcb);
}

/// Start scheduling, with the caller (the boot path, running on
/// `boot_stack_bottom .. boot_stack_top`) as [`BOOT_TASK`] and `idle_entry`
/// as [`IDLE_TASK`], the task that runs whenever nothing else is runnable.
///
/// # Errors
///
/// [`SchedError::AlreadyInitialized`] on a second call.
///
/// # Safety
///
/// - `boot_stack_bottom .. boot_stack_top` must describe the caller's own
///   kernel stack: valid, exclusively owned, with the caller's RSP inside
///   it, and `boot_stack_top` 16-byte aligned (the switch and SYSCALL paths
///   require it). The stack and its mappings must stay valid for as long as
///   `BOOT_TASK` can run, because the scheduler retains these bounds.
/// - The kernel's permanent page tables must be active: their root is
///   captured here and used for every kernel task.
/// - Single CPU; no interrupt scope or lock held.
pub unsafe fn init(
    boot_stack_bottom: *mut u8,
    boot_stack_top: *mut u8,
    idle_entry: fn() -> !,
    idle_stack: &'static mut KernelStack,
) -> Result<(), SchedError> {
    let mut sched = SCHEDULER.lock();
    if sched.is_some() {
        return Err(SchedError::AlreadyInitialized);
    }
    assert!(
        (boot_stack_top as usize).is_multiple_of(16),
        "scheduler init: boot stack top must be 16-byte aligned"
    );
    let kernel_root = active_pml4_phys();
    // SAFETY: slots 0 and 1 are empty (first init); the boot stack is the
    // caller's own (contract above); the idle stack is a unique
    // `&'static mut`; `kernel_root` is the permanent kernel root.
    install(
        BOOT_TASK.as_u64() as usize,
        boot_stack_bottom,
        boot_stack_top,
        kernel_root,
        None,
    );
    let idle = idle_stack.0.as_mut_ptr_range();
    install(
        IDLE_TASK.as_u64() as usize,
        idle.start,
        idle.end,
        kernel_root,
        Some(idle_entry),
    );
    transition(BOOT_TASK, TaskState::Ready, TaskState::Running);
    *sched = Some(Scheduler {
        policy: RoundRobin::new(BOOT_TASK, IDLE_TASK, TIME_SLICE_TICKS),
        next_free_slot: 2,
        kernel_root,
    });
    Ok(())
}

/// `true` once [`init`] has run.
pub fn is_initialized() -> bool {
    SCHEDULER.lock().is_some()
}

/// Create a kernel task running `entry` on `stack` and make it runnable.
///
/// The task runs under the kernel's permanent page-table root (captured at
/// [`init`]), whatever address space is active when `spawn` is called. It
/// therefore never caches a process root that could be destroyed while
/// the task is switched out.
///
/// # Errors
///
/// [`SchedError::NotInitialized`] or [`SchedError::TableFull`].
pub fn spawn(entry: fn() -> !, stack: &'static mut KernelStack) -> Result<TaskId, SchedError> {
    let mut guard = SCHEDULER.lock();
    let sched = guard.as_mut().ok_or(SchedError::NotInitialized)?;
    let slot = sched.next_free_slot;
    if slot >= MAX_TASKS {
        return Err(SchedError::TableFull);
    }
    let range = stack.0.as_mut_ptr_range();
    // SAFETY: `slot` was never used (monotonic allocator, under the lock);
    // the stack is a unique `&'static mut` and 16-byte aligned by type; the
    // kernel root is permanent.
    unsafe { install(slot, range.start, range.end, sched.kernel_root, Some(entry)) };
    sched.next_free_slot += 1;
    let id = TaskId::new(slot as u64);
    // Capacity is checked at compile time (MAX_TASKS - 1 <= queue size), and
    // a fresh id cannot be queued yet, so admitting cannot fail.
    sched
        .policy
        .admit(id)
        .unwrap_or_else(|e| panic!("scheduler: admit new task {id:?}: {e:?}"));
    Ok(id)
}

/// The running task, if the scheduler is initialised.
pub fn current_task() -> Option<TaskId> {
    SCHEDULER.lock().as_ref().map(|s| s.policy.current())
}

/// Number of Ready tasks waiting for the CPU.
pub fn runnable_count() -> usize {
    SCHEDULER.lock().as_ref().map_or(0, |s| s.policy.queued())
}

/// State of task `id`, if it exists.
pub fn task_state(id: TaskId) -> Option<TaskState> {
    with_tcb(id, TaskControlBlock::state)
}

/// Timer ticks task `id` has spent running, if it exists.
pub fn cpu_ticks(id: TaskId) -> Option<u64> {
    with_tcb(id, TaskControlBlock::cpu_ticks)
}

/// Context switches performed since boot.
pub fn context_switch_count() -> u64 {
    CONTEXT_SWITCHES.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Give up the CPU; the caller goes to the back of the queue. Returns
/// immediately if nothing else is runnable (or the scheduler is not
/// initialised).
///
/// # Panics
///
/// If called with an interrupt scope (`InterruptGuard`) or a lock held.
/// The same rule applies to [`preempt_if_needed`], [`block_current`], and
/// [`exit_current`].
pub fn yield_now() {
    schedule(SwitchReason::Yield);
}

/// **Timer ISR hook (#75).** Charge one tick to the running task and
/// advance its time slice. Never switches, so it is safe to call from an
/// interrupt handler, including one that holds an interrupt scope. Returns `true` if a reschedule is now pending: call
/// [`preempt_if_needed`] at the next safe point (with no locks held, e.g. on
/// the interrupt-return path).
pub fn scheduler_tick() -> bool {
    let mut guard = SCHEDULER.lock();
    let Some(sched) = guard.as_mut() else {
        return false;
    };
    let current = sched.policy.current();
    with_tcb(current, TaskControlBlock::charge_tick);
    sched.policy.tick()
}

/// Safe point: preempt the running task if a tick (or a wake while idle)
/// requested it.
///
/// Must be called with **no interrupt scope and no lock** held (it panics
/// otherwise). The timer ISR (#75) must therefore end its own
/// `InterruptGuard` before calling this on the interrupt-return path.
pub fn preempt_if_needed() {
    let pending = SCHEDULER
        .lock()
        .as_ref()
        .is_some_and(|s| s.policy.need_resched());
    if pending {
        schedule(SwitchReason::Preempt);
    }
}

/// Block the running task until some other context calls [`wake`] on it.
///
/// # Panics
///
/// If called by the idle task, or before [`init`].
pub fn block_current() {
    assert!(is_initialized(), "block_current before scheduler init");
    schedule(SwitchReason::Block);
}

/// Make a blocked task runnable again (back of the queue).
///
/// # Errors
///
/// [`SchedError::NotInitialized`], [`SchedError::NoSuchTask`], or
/// [`SchedError::NotBlocked`] if the task is not blocked.
pub fn wake(id: TaskId) -> Result<(), SchedError> {
    let mut guard = SCHEDULER.lock();
    let sched = guard.as_mut().ok_or(SchedError::NotInitialized)?;
    let woken = with_tcb(id, |t| {
        t.try_transition(TaskState::Blocked, TaskState::Ready)
    })
    .ok_or(SchedError::NoSuchTask)?;
    woken.map_err(|_| SchedError::NotBlocked)?;
    // A blocked task is neither running nor queued, so this cannot collide.
    sched
        .policy
        .admit(id)
        .map_err(|e: RunQueueError| -> SchedError { panic!("scheduler: wake {id:?}: {e:?}") })
}

/// Terminate the running task. It is never scheduled again; its TCB slot
/// stays (as `Exiting`) until reaping exists.
///
/// # Panics
///
/// If called by the idle task, or before [`init`].
pub fn exit_current() -> ! {
    assert!(is_initialized(), "exit_current before scheduler init");
    schedule(SwitchReason::Exit);
    unreachable!("scheduler: an exited task was resumed");
}

/// Ask the policy for the next task and switch to it (ADR-0004 §3).
///
/// Callers must hold **no interrupt scope and no lock**: this opens the
/// switch guard itself, and `switch_to` requires that guard to be the only
/// open scope (ADR-0008). With an outer scope, a new task would inherit two
/// scopes, close only one, and start with interrupts stuck off. That is
/// asserted in every build, because the release-mode failure is silent.
fn schedule(reason: SwitchReason) {
    assert_eq!(
        irq_disable_depth(),
        0,
        "schedule: called with an interrupt scope or lock held"
    );
    // The switch guard: the only interrupt scope open at `switch_to`.
    let irq = disable_interrupts();

    // Decide and update TCB states under the lock; take raw pointers only.
    let (prev, next) = {
        let mut guard = SCHEDULER.lock();
        let Some(sched) = guard.as_mut() else {
            return;
        };
        let Some(decision) = sched.policy.switch(reason) else {
            return; // the same task keeps the CPU
        };
        let prev_state = match reason {
            SwitchReason::Yield | SwitchReason::Preempt => TaskState::Ready,
            SwitchReason::Block => TaskState::Blocked,
            SwitchReason::Exit => TaskState::Exiting,
        };
        transition(decision.prev, TaskState::Running, prev_state);
        transition(decision.next, TaskState::Ready, TaskState::Running);
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        (
            tcb_ptr(decision.prev).expect("prev exists"),
            tcb_ptr(decision.next).expect("next exists"),
        )
    }; // lock released: only the switch guard is open now

    // SAFETY: both TCBs live in the static table (never move or freed) and
    // are distinct (the policy never returns prev == next); no reference to
    // either is live; `prev` is the running task (the policy's current);
    // `next` was initialised by `init_kernel_task_context` or saved by an
    // earlier switch; `irq` is the only open scope.
    unsafe { switch_to(prev, next, &irq) };
    // Resumed: dropping our guard closes the scope our switcher opened.
    drop(irq);
}
