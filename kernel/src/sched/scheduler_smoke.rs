//! Boot self-test for the round-robin scheduler (Phase 2.2.2, ADR-0004).
//!
//! The boot path becomes `BOOT_TASK`. In each scenario it spawns workers
//! and then **blocks**, so the workers run under the real scheduler. When
//! they have all exited the run queue is empty, so the scheduler falls back
//! to the idle task. The test's idle body plays the part of the interrupt
//! that, in production, would wake a blocked task: it records that it ran
//! and wakes boot, which resumes and checks the results.
//!
//! Checks:
//! - 21.1 round-robin: three yielding tasks interleave in strict FIFO order.
//! - 21.2 time slice: two busy tasks driving `scheduler_tick` (standing in
//!   for the timer ISR, #75) are preempted after exactly
//!   `TIME_SLICE_TICKS` ticks each, and CPU time is charged per task.
//! - 21.3 idle: idle runs only when the queue is empty, once per scenario.
//! - 21.4 block / wake / exit, and a lone yield or idle safe point performs
//!   no context switch (`sys_yield` relies on this today).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::scheduler::{
    block_current, context_switch_count, cpu_ticks, current_task, exit_current, init,
    preempt_if_needed, runnable_count, scheduler_tick, spawn, task_state, wake, yield_now,
    KernelStack, BOOT_TASK, TIME_SLICE_TICKS,
};
use crate::task::{TaskId, TaskState};

/// Interior-mutable cell for test-only statics.
struct TestCell<T>(UnsafeCell<T>);

// SAFETY: single CPU; only the running task touches these, and tasks run
// strictly one after another (a switch is the only way control moves).
unsafe impl<T> Sync for TestCell<T> {}

/// A kernel stack that can be handed out exactly once (a one-shot cell).
struct StackSlot {
    taken: AtomicBool,
    stack: UnsafeCell<KernelStack>,
}

// SAFETY: the only access to `stack` is the `&mut` returned by `take`, and
// the `taken` flag guarantees `take` succeeds at most once, so no two
// references to the stack can ever exist.
unsafe impl Sync for StackSlot {}

impl StackSlot {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            stack: UnsafeCell::new(KernelStack::new()),
        }
    }

    /// Hand out the stack. Panics on a second call, so the returned `&mut`
    /// is always unique (which is why `mut_from_ref` is sound here).
    #[allow(clippy::mut_from_ref)]
    fn take(&'static self) -> &'static mut KernelStack {
        assert!(
            !self.taken.swap(true, Ordering::AcqRel),
            "kernel stack handed out twice"
        );
        // SAFETY: the flag above proves this is the first and only borrow.
        unsafe { &mut *self.stack.get() }
    }
}

static IDLE_STACK: StackSlot = StackSlot::new();
static WORKER_STACKS: [StackSlot; 5] = [const { StackSlot::new() }; 5];

/// Event log: one byte per scheduling-relevant step.
const LOG_CAPACITY: usize = 128;
static EVENTS: TestCell<[u8; LOG_CAPACITY]> = TestCell::new(UnsafeCell::new([0; LOG_CAPACITY]));
static EVENT_LEN: AtomicUsize = AtomicUsize::new(0);

impl<T> TestCell<T> {
    const fn new(cell: UnsafeCell<T>) -> Self {
        Self(cell)
    }
}

fn record(event: u8) {
    let i = EVENT_LEN.fetch_add(1, Ordering::Relaxed);
    assert!(i < LOG_CAPACITY, "event log overflow");
    // SAFETY: only the running task records (see TestCell).
    unsafe { (*EVENTS.0.get())[i] = event };
}

/// Take and clear the event log.
fn take_events(out: &mut [u8; LOG_CAPACITY]) -> usize {
    let len = EVENT_LEN.swap(0, Ordering::Relaxed);
    // SAFETY: only boot reads, while every worker is blocked or exited.
    *out = unsafe { *EVENTS.0.get() };
    len
}

/// Run-length encode `events` as (byte, run) pairs into `out`.
fn runs(events: &[u8], out: &mut [(u8, usize); 16]) -> usize {
    let mut n = 0;
    for &e in events {
        if n > 0 && out[n - 1].0 == e {
            out[n - 1].1 += 1;
        } else {
            assert!(n < out.len(), "too many runs");
            out[n] = (e, 1);
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Task bodies
// ---------------------------------------------------------------------------

/// Whether interrupts were enabled the last time the test idle task ran.
/// Idle last switched out *before* interrupts went live, so this proves its
/// saved intent was not frozen at the boot-time IF = 0 (ADR-0009 §4).
static IDLE_SAW_INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(false);

/// See [`IDLE_SAW_INTERRUPTS_ENABLED`].
pub(super) fn idle_saw_interrupts_enabled() -> bool {
    IDLE_SAW_INTERRUPTS_ENABLED.load(Ordering::Relaxed)
}

/// Idle (test variant): runs only when the queue is empty. It records 'I',
/// wakes boot (as an interrupt would in production), then yields to it.
fn test_idle() -> ! {
    loop {
        IDLE_SAW_INTERRUPTS_ENABLED.store(
            crate::arch::x86_64::cpu::interrupts_enabled(),
            Ordering::Relaxed,
        );
        assert_eq!(runnable_count(), 0, "idle ran while tasks were runnable");
        record(b'I');
        wake(BOOT_TASK).expect("boot is blocked while workers run");
        yield_now();
    }
}

const RR_ROUNDS: usize = 3;
/// Context switches in 21.1: boot->first worker, one per yield
/// (3 tasks x RR_ROUNDS), one per exit (3), and idle->boot.
const RR_EXPECTED_SWITCHES: u64 = 1 + 3 * RR_ROUNDS as u64 + 3 + 1;
/// Context switches in 21.2 (derivation at the assertion).
const SLICE_EXPECTED_SWITCHES: u64 = 8;

fn rr_worker(tag: u8) -> ! {
    for _ in 0..RR_ROUNDS {
        record(tag);
        yield_now();
    }
    exit_current()
}

fn rr_a() -> ! {
    rr_worker(b'A')
}
fn rr_b() -> ! {
    rr_worker(b'B')
}
fn rr_c() -> ! {
    rr_worker(b'C')
}

/// Ticks each busy task runs in total (two full slices).
const BUSY_TICKS: u64 = 2 * TIME_SLICE_TICKS as u64;

fn busy_worker(tag: u8) -> ! {
    let me = current_task().expect("scheduler running");
    loop {
        if cpu_ticks(me).expect("task exists") >= BUSY_TICKS {
            exit_current();
        }
        record(tag);
        // Stand-in for the timer ISR (#75): charge a tick, then the
        // interrupt-return safe point.
        scheduler_tick();
        preempt_if_needed();
    }
}

fn busy_p() -> ! {
    busy_worker(b'P')
}
fn busy_q() -> ! {
    busy_worker(b'Q')
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the Phase 2.2.2 scheduler self-test. Leaves the scheduler running
/// with only the boot task runnable (so later `sys_yield`s return at once).
///
/// # Safety
///
/// - Boot context, single CPU, kernel page tables active, interrupts off.
/// - `boot_stack_bottom .. boot_stack_top` is the stack the caller runs on.
/// - Must be called at most once (it initialises the scheduler).
///
/// # Panics
///
/// On any failed check.
pub unsafe fn smoke_test(boot_stack_bottom: *mut u8, boot_stack_top: *mut u8) {
    log::info!("Scheduler smoke test (Phase 2.2.2)");
    init(
        boot_stack_bottom,
        boot_stack_top,
        test_idle,
        IDLE_STACK.take(),
    )
    .expect("scheduler init");
    assert_eq!(current_task(), Some(BOOT_TASK));
    let mut log = [0u8; LOG_CAPACITY];
    let mut run = [(0u8, 0usize); 16];

    // 21.1: round-robin across three yielding tasks.
    let ids: [TaskId; 3] = [
        spawn(rr_a, WORKER_STACKS[0].take()).expect("spawn A"),
        spawn(rr_b, WORKER_STACKS[1].take()).expect("spawn B"),
        spawn(rr_c, WORKER_STACKS[2].take()).expect("spawn C"),
    ];
    let switches_before = context_switch_count();
    block_current(); // resumes when idle wakes us
                     // boot->A, 9 yields, 3 exits, idle->boot. An extra or missing switch
                     // (e.g. a yield that did not rotate) changes this count.
    assert_eq!(
        context_switch_count() - switches_before,
        RR_EXPECTED_SWITCHES,
        "round-robin made the wrong number of context switches"
    );
    let len = take_events(&mut log);
    assert_eq!(&log[..len], b"ABCABCABCI", "round-robin order");
    for id in ids {
        assert_eq!(
            task_state(id),
            Some(TaskState::Exiting),
            "worker {id:?} exited"
        );
    }
    log::info!(
        "[OK] 21.1) round-robin: 3 tasks x {RR_ROUNDS} yields in strict FIFO order (ABCABCABC)"
    );

    // 21.2: time-slice preemption driven by the tick hook.
    let p = spawn(busy_p, WORKER_STACKS[3].take()).expect("spawn P");
    let q = spawn(busy_q, WORKER_STACKS[4].take()).expect("spawn Q");
    let switches_before = context_switch_count();
    block_current();
    // boot->P, P->Q, Q->P, P->Q (four slice ends), Q->P (Q's second slice
    // end), P exits->Q, Q exits->idle, idle->boot. If Q missed its final
    // preemption it would simply exit (cpu_ticks already 20), giving the
    // same event runs and tick totals but only 7 switches, so this count is
    // what proves every slice end was enforced.
    assert_eq!(
        context_switch_count() - switches_before,
        SLICE_EXPECTED_SWITCHES,
        "time-slice scenario made the wrong number of context switches"
    );
    let len = take_events(&mut log);
    let n = runs(&log[..len], &mut run);
    let slice = TIME_SLICE_TICKS as usize;
    assert_eq!(
        &run[..n],
        &[
            (b'P', slice),
            (b'Q', slice),
            (b'P', slice),
            (b'Q', slice),
            (b'I', 1)
        ],
        "each task must run exactly one slice before preemption"
    );
    assert_eq!(cpu_ticks(p), Some(BUSY_TICKS));
    assert_eq!(cpu_ticks(q), Some(BUSY_TICKS));
    log::info!(
        "[OK] 21.2) time slice: preempted after exactly {TIME_SLICE_TICKS} ticks; {BUSY_TICKS} ticks charged per task"
    );

    // 21.3: idle ran exactly once per scenario, only with an empty queue
    // (test_idle asserts that), and the run queue is empty now.
    assert_eq!(runnable_count(), 0);
    log::info!("[OK] 21.3) idle: ran only when the queue was empty (once per scenario)");

    // 21.4: block/wake brought boot back; a lone yield or idle safe point is
    // a no-op (no switch), which `sys_yield` relies on today.
    assert_eq!(current_task(), Some(BOOT_TASK));
    assert_eq!(task_state(BOOT_TASK), Some(TaskState::Running));
    let switches = context_switch_count();
    yield_now();
    preempt_if_needed();
    assert_eq!(
        context_switch_count(),
        switches,
        "a lone task must not switch"
    );
    log::info!("[OK] 21.4) block/wake/exit verified; a lone yield performs no context switch");

    log::info!("Scheduler smoke test complete");
}
