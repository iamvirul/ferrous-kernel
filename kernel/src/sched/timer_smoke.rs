//! Boot self-test for timer interrupts and real preemption (Phase 2.2.4,
//! ADR-0009). Runs with interrupts already enabled.
//!
//! Checks:
//! - 22.1 PIC remapped and masked: only IRQ0 open.
//! - 22.2 the first ticks arrive within a TSC-bounded timeout, which does
//!   not depend on IRQ0, so a dead timer fails loudly instead of hanging.
//! - 22.3 once ticks are flowing, `hlt` is woken by the timer.
//! - 22.4 real preemption: two tasks that never yield and never call the
//!   tick hook are both preempted by the timer, and run with IF = 1.
//! - 22.5 interrupt intent survives boot: the idle task, last switched out
//!   before interrupts were live, now runs with them enabled.

use core::sync::atomic::{AtomicBool, Ordering};

use super::scheduler::{block_current, exit_current, spawn, KernelStack};
use super::scheduler_smoke::idle_saw_interrupts_enabled;
use crate::arch::x86_64::cpu::{interrupts_enabled, interrupts_live};
use crate::arch::x86_64::interrupts::{spurious_irqs, ticks, timer_preemptions, unexpected_irqs};
use crate::arch::x86_64::{pic, pit};

/// TSC cycles to wait for timer progress before declaring it dead.
/// Generous on purpose: QEMU TCG time is slow and uneven, and this only
/// bounds the failure case.
const TSC_TIMEOUT: u64 = 20_000_000_000;

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC reads the time-stamp counter; no memory or flags change.
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    }
    (u64::from(hi) << 32) | u64::from(lo)
}

/// Spin (without `hlt`, which a dead timer would never wake) until `done()`
/// or the TSC timeout. Returns whether `done()` became true.
fn spin_until(mut done: impl FnMut() -> bool) -> bool {
    let start = rdtsc();
    while !done() {
        if rdtsc().wrapping_sub(start) > TSC_TIMEOUT {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// A kernel stack handed out once (same one-shot pattern as the scheduler test).
struct StackSlot {
    taken: AtomicBool,
    stack: core::cell::UnsafeCell<KernelStack>,
}

// SAFETY: `take` hands out the only reference, at most once (flag).
unsafe impl Sync for StackSlot {}

impl StackSlot {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            stack: core::cell::UnsafeCell::new(KernelStack::new()),
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn take(&'static self) -> &'static mut KernelStack {
        assert!(
            !self.taken.swap(true, Ordering::AcqRel),
            "stack handed out twice"
        );
        // SAFETY: the flag proves this is the only borrow.
        unsafe { &mut *self.stack.get() }
    }
}

static SPINNER_STACKS: [StackSlot; 2] = [const { StackSlot::new() }; 2];

static A_RUNNING: AtomicBool = AtomicBool::new(false);
static B_RAN: AtomicBool = AtomicBool::new(false);
static A_DONE: AtomicBool = AtomicBool::new(false);

/// Never yields and never touches the scheduler: it can only lose the CPU
/// to the timer. It waits until B has run (which requires the timer to
/// preempt A), then exits.
fn spinner_a() -> ! {
    assert!(
        interrupts_enabled(),
        "new task started with IF = 0 after interrupts went live"
    );
    A_RUNNING.store(true, Ordering::Release);
    assert!(
        spin_until(|| B_RAN.load(Ordering::Acquire)),
        "timer never preempted spinner A"
    );
    A_DONE.store(true, Ordering::Release);
    exit_current()
}

/// Same, mirrored: it waits until A has finished, which requires the timer
/// to preempt B back to A.
fn spinner_b() -> ! {
    assert!(
        interrupts_enabled(),
        "new task started with IF = 0 after interrupts went live"
    );
    assert!(A_RUNNING.load(Ordering::Acquire), "B ran before A");
    B_RAN.store(true, Ordering::Release);
    assert!(
        spin_until(|| A_DONE.load(Ordering::Acquire)),
        "timer never preempted spinner B"
    );
    exit_current()
}

/// Run the Phase 2.2.4 timer self-test.
///
/// # Safety
///
/// Boot context; `enable_interrupts` has run; the scheduler is initialised
/// with the caller as the running task; called once.
///
/// # Panics
///
/// On any failed check, including a timer that never fires (bounded wait).
pub unsafe fn smoke_test() {
    log::info!("Timer interrupt smoke test (Phase 2.2.4)");
    assert!(
        interrupts_live() && interrupts_enabled(),
        "interrupts must be live"
    );

    // 22.1
    let (master, slave) = pic::masks();
    assert_eq!(
        (master, slave),
        (pic::MASTER_MASK, pic::SLAVE_MASK),
        "PIC masks"
    );
    log::info!(
        "[OK] 22.1) PIC remapped to vectors {}-{}; masks master={master:#04x} slave={slave:#04x} (only IRQ0 open)",
        pic::PIC1_VECTOR_BASE,
        pic::PIC2_VECTOR_BASE + 7
    );

    // 22.2: first ticks, bounded by the TSC (independent of IRQ0).
    let t0 = ticks();
    let tsc0 = rdtsc();
    assert!(
        spin_until(|| ticks() >= t0 + 3),
        "no timer interrupt within the TSC timeout: IRQ0 not delivered"
    );
    let cycles_per_tick = rdtsc().wrapping_sub(tsc0) / 3;
    log::info!(
        "[OK] 22.2) timer ticking at {} Hz (PIT divisor {}); ~{cycles_per_tick} TSC cycles/tick",
        pit::TIMER_HZ,
        pit::DIVISOR
    );

    // 22.3: now that ticks are known to flow, `hlt` must be woken by them.
    let t1 = ticks();
    while ticks() < t1 + 10 {
        // SAFETY: interrupts are enabled and the timer is proven to fire, so
        // `hlt` returns within one tick.
        core::arch::asm!("hlt", options(nomem, nostack));
    }
    log::info!("[OK] 22.3) hlt woken by the timer: 10 ticks elapsed while halted");

    // 22.4: real, involuntary preemption.
    let preempt_before = timer_preemptions();
    spawn(spinner_a, SPINNER_STACKS[0].take()).expect("spawn spinner A");
    spawn(spinner_b, SPINNER_STACKS[1].take()).expect("spawn spinner B");
    block_current(); // idle wakes us once both spinners have exited
    let preemptions = timer_preemptions() - preempt_before;
    assert!(A_DONE.load(Ordering::Acquire), "spinner A did not finish");
    assert!(
        preemptions >= 2,
        "expected the timer to preempt A and then B, saw {preemptions}"
    );
    log::info!(
        "[OK] 22.4) real preemption: 2 non-yielding tasks preempted by the timer ({preemptions} timer preemptions), IF=1 in both"
    );

    // 22.5
    assert!(
        idle_saw_interrupts_enabled(),
        "idle task (switched out before interrupts were live) ran with IF = 0"
    );
    log::info!("[OK] 22.5) per-task interrupt intent: idle, last switched out pre-enable, now runs with IF=1");

    assert_eq!(unexpected_irqs(), 0, "a masked IRQ line fired");
    log::info!(
        "Timer interrupt smoke test complete ({} ticks so far, {} spurious IRQs)",
        ticks(),
        spurious_irqs()
    );
}
