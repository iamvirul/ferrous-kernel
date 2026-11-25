//! Hardware interrupt entry and dispatch (Phase 2.2.4, ADR-0009 §3).
//!
//! One assembly stub per PIC line (vectors 32–47) saves the caller-saved
//! registers and calls [`irq_dispatch`]. The timer handler charges a tick,
//! sends EOI, and then preempts at the ADR-0004 safe point.
//!
//! # GS (ADR-0009 §3a)
//!
//! In ring 0, `GS_BASE` always holds the kernel's `CpuLocal`. A stub that
//! interrupted ring 3 (`CS & 3` in the CPU frame) runs `swapgs` on entry
//! and again before `iretq`, followed by `lfence` on both paths (SWAPGS
//! speculation, CVE-2019-1125). The context switch therefore never has to
//! touch GS.
//!
//! # FPU
//!
//! The stubs do not save x87/SSE state: the kernel is soft-float, so no
//! compiled handler touches it. The guard below turns that assumption into
//! a build failure if it ever stops being true.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::cpu::disable_interrupts;
use super::pic;

// The stubs skip FXSAVE because kernel code is soft-float. If the kernel is
// ever built for its bare-metal targets with SSE enabled, compiled handlers
// could clobber a preempted task's vector registers: refuse to build.
#[cfg(all(any(target_os = "uefi", target_os = "none"), target_feature = "sse"))]
compile_error!(
    "kernel built with SSE enabled: IRQ stubs must save x87/SSE state (ADR-0009 §3) before this is allowed"
);

/// Timer interrupts since `enable_interrupts` (uptime in ticks).
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Context switches performed from the timer ISR (preemptions).
static TIMER_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// Spurious IRQ7 / IRQ15 deliveries (expected occasionally; not errors).
static SPURIOUS_IRQS: AtomicU64 = AtomicU64::new(0);
/// IRQs on lines that should be masked (a bug if non-zero).
static UNEXPECTED_IRQS: AtomicU64 = AtomicU64::new(0);
/// IRQs that interrupted ring 3 (took the `swapgs` entry/exit path).
static USER_MODE_IRQS: AtomicU64 = AtomicU64::new(0);

/// Timer ticks since interrupts were enabled. Read by any subsystem.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Preemptions triggered by the timer interrupt.
pub fn timer_preemptions() -> u64 {
    TIMER_PREEMPTIONS.load(Ordering::Relaxed)
}

/// Spurious IRQ7/IRQ15 deliveries seen.
pub fn spurious_irqs() -> u64 {
    SPURIOUS_IRQS.load(Ordering::Relaxed)
}

/// Interrupts on masked lines seen (should stay 0).
pub fn unexpected_irqs() -> u64 {
    UNEXPECTED_IRQS.load(Ordering::Relaxed)
}

/// IRQs that interrupted ring 3 (exercised the `swapgs` path).
pub fn user_mode_irqs() -> u64 {
    USER_MODE_IRQS.load(Ordering::Relaxed)
}

global_asm!(
    // irq_stub VECTOR: entered through an interrupt gate (IF = 0) with the
    // CPU frame [RIP, CS, RFLAGS, RSP, SS] at RSP. No error code for IRQs.
    ".macro irq_stub vector",
    ".global ferrous_irq_\\vector",
    "ferrous_irq_\\vector:",
    // Ring 3 -> kernel GS. [rsp + 8] is the interrupted CS.
    "test byte ptr [rsp + 8], 3",
    "jz 1f",
    "swapgs",
    "1:",
    "lfence",
    // Caller-saved GPRs: the Rust handler preserves the callee-saved ones.
    // The CPU aligned RSP to 16 before its 5-word frame (RSP ≡ 8 mod 16
    // here); 9 pushes make it ≡ 0 for the call.
    "push rax",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "cld",
    "mov edi, \\vector",
    // Second argument: did this interrupt arrive from ring 3? After the 9
    // pushes the CPU frame starts at [rsp + 72], so its CS is at +80.
    "xor esi, esi",
    "test byte ptr [rsp + 80], 3",
    "setnz sil",
    "call {dispatch}",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rax",
    // Back to user GS if returning to ring 3.
    "test byte ptr [rsp + 8], 3",
    "jz 2f",
    "swapgs",
    "2:",
    "lfence",
    "iretq",
    ".endm",
    "irq_stub 32",
    "irq_stub 33",
    "irq_stub 34",
    "irq_stub 35",
    "irq_stub 36",
    "irq_stub 37",
    "irq_stub 38",
    "irq_stub 39",
    "irq_stub 40",
    "irq_stub 41",
    "irq_stub 42",
    "irq_stub 43",
    "irq_stub 44",
    "irq_stub 45",
    "irq_stub 46",
    "irq_stub 47",
    // Address table, indexed by IRQ line, for installing the IDT gates.
    ".pushsection .rodata",
    ".balign 8",
    ".global ferrous_irq_stub_table",
    "ferrous_irq_stub_table:",
    ".quad ferrous_irq_32",
    ".quad ferrous_irq_33",
    ".quad ferrous_irq_34",
    ".quad ferrous_irq_35",
    ".quad ferrous_irq_36",
    ".quad ferrous_irq_37",
    ".quad ferrous_irq_38",
    ".quad ferrous_irq_39",
    ".quad ferrous_irq_40",
    ".quad ferrous_irq_41",
    ".quad ferrous_irq_42",
    ".quad ferrous_irq_43",
    ".quad ferrous_irq_44",
    ".quad ferrous_irq_45",
    ".quad ferrous_irq_46",
    ".quad ferrous_irq_47",
    ".popsection",
    dispatch = sym irq_dispatch,
);

extern "C" {
    static ferrous_irq_stub_table: [u64; pic::IRQ_LINES as usize];
}

/// Address of the entry stub for IDT vector `vector` (32..=47).
///
/// # Panics
///
/// If `vector` is not a PIC vector.
pub fn irq_stub_address(vector: u8) -> u64 {
    let base = pic::PIC1_VECTOR_BASE;
    assert!(
        (base..base + pic::IRQ_LINES).contains(&vector),
        "vector {vector} is not a PIC IRQ vector"
    );
    // SAFETY: the table is an immutable array emitted by the asm above,
    // fully initialised at link time, with one entry per PIC line.
    unsafe { ferrous_irq_stub_table[usize::from(vector - base)] }
}

/// Rust side of every PIC IRQ. Runs with IF = 0 (interrupt gate) and
/// nesting depth 0. `from_user` is 1 if ring 3 was interrupted.
extern "sysv64" fn irq_dispatch(vector: u64, from_user: u64) {
    debug_assert!(
        super::cpu::kernel_gs_active(),
        "IRQ entered ring 0 without the kernel GS (swapgs imbalance)"
    );
    if from_user != 0 {
        USER_MODE_IRQS.fetch_add(1, Ordering::Relaxed);
    }
    let irq = (vector - u64::from(pic::PIC1_VECTOR_BASE)) as u8;
    match irq {
        pic::IRQ_TIMER => timer_interrupt(),
        pic::IRQ_SPURIOUS_MASTER | pic::IRQ_SPURIOUS_SLAVE if pic::is_spurious(irq) => {
            SPURIOUS_IRQS.fetch_add(1, Ordering::Relaxed);
            if irq == pic::IRQ_SPURIOUS_SLAVE {
                // The master really did deliver the cascade: acknowledge it.
                // SAFETY: servicing a spurious slave interrupt.
                unsafe { pic::send_eoi_master_only() };
            }
        }
        _ => {
            // A masked line fired: count it (no logging here: the logger's
            // lock could be held by the interrupted code) and acknowledge it
            // so the PIC is not left blocked.
            UNEXPECTED_IRQS.fetch_add(1, Ordering::Relaxed);
            // SAFETY: `irq` is the line being serviced.
            unsafe { pic::send_eoi(irq) };
        }
    }
}

/// IRQ0: account a tick, acknowledge, then preempt at the safe point.
fn timer_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    {
        // `scheduler_tick` takes the scheduler lock; open a scope first so
        // the nesting count reflects that we are in a critical section
        // (ADR-0004 §4).
        let _irq = disable_interrupts();
        crate::sched::scheduler::scheduler_tick();
    }
    // EOI before any switch: otherwise a preempted task would leave IRQ0
    // blocked at the PIC while other tasks run.
    // SAFETY: IRQ0 is the line being serviced.
    unsafe { pic::send_eoi(pic::IRQ_TIMER) };
    // Safe point: no scope and no lock held (preempt_if_needed asserts it).
    let before = crate::sched::scheduler::context_switch_count();
    crate::sched::scheduler::preempt_if_needed();
    if crate::sched::scheduler::context_switch_count() != before {
        TIMER_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
    }
}
