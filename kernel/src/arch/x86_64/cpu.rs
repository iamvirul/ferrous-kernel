//! CPU Local Storage for x86-64.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

use ferrous_core::irq::IrqNesting;

/// Per-CPU local data structure.
///
/// In 64-bit mode, the `GS` segment register is typically used to point to
/// CPU-local storage. The `KernelGsBase` MSR holds the base address of this
/// structure for Ring 0, while the standard `GsBase` MSR is used for Ring 3.
/// The `SWAPGS` instruction swaps these two MSRs during privilege transitions.
#[repr(C)]
pub struct CpuLocal {
    /// Pointer to the top of the kernel stack for the currently executing task.
    /// The `SYSCALL` entry stub reads this to switch stacks securely.
    pub kernel_stack: u64,

    /// Scratch space used by the `SYSCALL` entry stub to temporarily save the
    /// user's stack pointer (`RSP`) before switching to the kernel stack.
    pub user_rsp: u64,

    /// Pointer to the current `TaskControlBlock`.
    /// Stored as a raw pointer to avoid locking overhead in the hot path.
    pub current_task: u64,
}

impl CpuLocal {
    /// Create an empty `CpuLocal` structure.
    pub const fn new() -> Self {
        Self {
            kernel_stack: 0,
            user_rsp: 0,
            current_task: 0,
        }
    }
}

/// The static CPU-local storage for the Bootstrap Processor (BSP).
/// (In Phase 2, we only support a single core).
pub static mut CPU_LOCAL: CpuLocal = CpuLocal::new();

/// `IA32_GS_BASE`: the active GS base.
const MSR_GS_BASE: u32 = 0xC000_0101;
/// `IA32_KERNEL_GS_BASE`: the value `swapgs` exchanges with `IA32_GS_BASE`.
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
/// User GS base for every task today: user code cannot set its own yet
/// (CR4.FSGSBASE is off), so it is always 0 (ADR-0009 §3a).
const USER_GS_BASE: u64 = 0;

/// Establish the ring-0 GS invariant (ADR-0009 §3a): `GS_BASE` = this
/// CPU's [`CpuLocal`], and `IA32_KERNEL_GS_BASE` = the user GS.
///
/// Every entry from ring 3 (SYSCALL, IRQs) then `swapgs`es into the kernel
/// GS, and every return to ring 3 `swapgs`es back, so ring-0 code always
/// sees the kernel GS and the context switch never has to touch it.
///
/// # Safety
///
/// - Once per CPU, at CPL 0, during early initialisation.
/// - **After** `gdt::init`: loading the GS selector there resets `GS_BASE`
///   to the descriptor base (0), which would undo this.
pub unsafe fn init() {
    let cpu_local_addr = core::ptr::addr_of!(CPU_LOCAL) as u64;
    wrmsr(MSR_GS_BASE, cpu_local_addr);
    wrmsr(MSR_KERNEL_GS_BASE, USER_GS_BASE);
}

/// `true` if the ring-0 GS invariant holds right now: `GS_BASE` is this
/// CPU's [`CpuLocal`] (ADR-0009 §3a). Maskable kernel entry points (IRQ
/// dispatch, SYSCALL) check it, so a missing or doubled `swapgs` fails at
/// the next entry instead of corrupting memory later.
///
/// Never assert this in an NMI, #MC or #DB handler: those can legitimately
/// arrive in the ring-0 windows around `swapgs` where the user GS is still
/// loaded.
pub fn kernel_gs_active() -> bool {
    let lo: u32;
    let hi: u32;
    // SAFETY: reading IA32_GS_BASE at CPL 0 has no side effects.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_GS_BASE,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((u64::from(hi) << 32) | u64::from(lo)) == core::ptr::addr_of!(CPU_LOCAL) as u64
}

/// Write a 64-bit MSR.
///
/// # Safety
///
/// CPL 0; `msr` must exist and accept `value` (otherwise `wrmsr` #GPs).
unsafe fn wrmsr(msr: u32, value: u64) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") value as u32,
        in("edx") (value >> 32) as u32,
        options(nostack, preserves_flags),
    );
}

/// Disable interrupts and halt this CPU permanently.
///
/// Used when there is nothing left to run (e.g. `sys_exit` before the
/// scheduler exists). `hlt` is in a loop because NMIs can still wake it.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` at CPL 0 only stops this CPU. It touches no
        // memory and breaks no invariant of the surrounding code.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// RFLAGS.IF — interrupt enable flag (bit 9). Also the bit `switch_to`
/// uses in `RegisterState.rflags` for a task's interrupt intent.
pub const RFLAGS_IF: u64 = 1 << 9;

/// Set once by [`enable_interrupts`]: the system takes interrupts from then
/// on. Before it, every task runs with IF = 0 whatever its intent.
static INTERRUPTS_LIVE: AtomicBool = AtomicBool::new(false);

/// `true` once [`enable_interrupts`] has run.
pub fn interrupts_live() -> bool {
    INTERRUPTS_LIVE.load(Ordering::Acquire)
}

/// Turn interrupts on for the system: set the "live" flag and `sti`.
///
/// # Safety
///
/// - Nesting depth 0 (no `InterruptGuard` alive), checked.
/// - GDT/TSS, IDT gates for every unmasked source, the interrupt
///   controller and its sources, and the scheduler are all initialised.
/// - Called once.
pub unsafe fn enable_interrupts() {
    assert_eq!(
        irq_disable_depth(),
        0,
        "enable_interrupts inside an interrupt-disabled scope"
    );
    assert!(!interrupts_live(), "enable_interrupts called twice");
    // Publish "live" *before* `sti`: an IRQ can arrive as soon as `sti`
    // takes effect, and a switch it causes computes the incoming task's IF
    // as `intent AND live` (ADR-0009 §4, §5). Setting the flag first means
    // no task can be started with IF = 0 in that window.
    INTERRUPTS_LIVE.store(true, Ordering::Release);
    // SAFETY: the caller guarantees every enabled source has a handler.
    core::arch::asm!("sti", options(nostack));
}

/// Swap the per-CPU "interrupts were enabled before the outermost scope"
/// bit: install `incoming` and return the outgoing value. Used only by
/// `switch_to` to make that bit per task (ADR-0009 §4).
///
/// # Panics
///
/// If no interrupt-disabled scope is open (the switch guard must be).
pub(crate) fn exchange_outermost_enabled(incoming: bool) -> bool {
    let _scope = disable_interrupts();
    // SAFETY: interrupts are disabled (see `CpuIrqState`). The scope just
    // opened is nested *inside* the switch guard, so the outermost bit we
    // replace is the switch guard's, not this one's.
    unsafe { (*CPU_IRQ_STATE.0.get()).replace_outermost_enabled(incoming) }
}

/// This CPU's interrupt-disable nesting state.
///
/// Single CPU in Phase 2, so one instance. It moves into `CpuLocal` with SMP.
struct CpuIrqState(UnsafeCell<IrqNesting>);

// SAFETY: the inner `IrqNesting` is only read or written by
// `disable_interrupts`, `irq_disable_depth`, and `exit_interrupt_scope`, all
// with interrupts disabled on the only CPU. No IRQ handler can interleave. NMI
// handlers never touch it: by `IrqSpinLock`'s rules they take no locks,
// and nothing else uses this state.
unsafe impl Sync for CpuIrqState {}

static CPU_IRQ_STATE: CpuIrqState = CpuIrqState(UnsafeCell::new(IrqNesting::new()));

/// `true` if maskable interrupts are currently enabled (RFLAGS.IF set).
pub fn interrupts_enabled() -> bool {
    let rflags: u64;
    // SAFETY: `pushfq; pop` reads RFLAGS through a single stack slot that is
    // pushed and popped within the block. No memory beyond that is touched,
    // and the flags are not modified.
    unsafe {
        core::arch::asm!("pushfq", "pop {}", out(reg) rflags, options(preserves_flags));
    }
    rflags & RFLAGS_IF != 0
}

/// Number of [`InterruptGuard`]s currently alive on this CPU.
pub fn irq_disable_depth() -> usize {
    let _scope = disable_interrupts();
    // SAFETY: read with interrupts disabled (see `CpuIrqState`). The guard
    // above accounts for itself, hence the `- 1`.
    unsafe { (*CPU_IRQ_STATE.0.get()).depth() - 1 }
}

/// Disable maskable interrupts until the returned guard and every other
/// live guard are dropped.
///
/// Guards may be dropped in any order. Interrupts are re-enabled only when
/// the *last* one goes, and only if they were enabled before the *first*
/// one (see `ferrous_core::irq`). An out-of-order release can therefore
/// never re-enable interrupts while another critical section is active.
///
/// Not `nomem`: the `cli` must also act as a compiler barrier, so memory
/// accesses of the critical section cannot be hoisted above it.
pub fn disable_interrupts() -> InterruptGuard {
    let enabled_before = interrupts_enabled();
    // SAFETY: at CPL 0, `cli` only defers maskable interrupts until the
    // matching guard drop. It cannot break any invariant.
    unsafe { core::arch::asm!("cli", options(nostack)) };
    // An IRQ between the read above and the `cli` runs to completion,
    // balanced, before we continue, so `enabled_before` is still accurate.
    // SAFETY: interrupts are now disabled (see `CpuIrqState`).
    unsafe { (*CPU_IRQ_STATE.0.get()).enter(enabled_before) };
    InterruptGuard {
        _not_send: PhantomData,
    }
}

/// Proof that maskable interrupts are disabled on this CPU (RAII).
///
/// Created by [`disable_interrupts`]. Dropping it ends its scope.
#[must_use = "dropping the guard immediately ends the interrupt-disabled scope"]
pub struct InterruptGuard {
    // Tied to this CPU's interrupt state, so it must not move to another
    // context (`*const ()` makes it !Send and !Sync).
    _not_send: PhantomData<*const ()>,
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        // SAFETY: this guard's own scope is active, so exiting one is
        // balanced.
        unsafe { exit_interrupt_scope() };
    }
}

/// Close the interrupt-disabled scope that a **new task** inherits from the
/// task that switched to it.
///
/// `switch_to` runs inside the caller's `InterruptGuard` scope, which stays
/// open across the switch. A resumed task closes one scope when it drops
/// its own guard after `switch_to` returns. A *new* task has no guard, so
/// its entry trampoline closes the inherited scope here, exactly once.
/// This keeps the per-CPU nesting count balanced across every switch, and
/// lets a new task run with the interrupt state that preceded the outermost
/// scope (ADR-0008 §3).
///
/// # Safety
///
/// Call only from the new-task entry path, exactly once per task, while
/// the inherited scope is still open (that is, before anything else).
pub(crate) unsafe fn end_inherited_interrupt_scope() {
    exit_interrupt_scope();
}

/// Leave one interrupt-disabled scope, re-enabling interrupts if it was the
/// last one and they were on before the first.
///
/// # Safety
///
/// A scope must be active (one more `enter` than `exit` so far).
unsafe fn exit_interrupt_scope() {
    debug_assert!(
        !interrupts_enabled(),
        "IF was set while an interrupt-disabled scope was active"
    );
    // SAFETY: interrupts are still disabled: this scope is active, and only
    // the last exit re-enables them, below.
    let reenable = (*CPU_IRQ_STATE.0.get()).exit();
    if reenable {
        // SAFETY: this was the last active scope, and interrupts were
        // enabled before the first one, so this restores the original state.
        // Not `nomem`, so it acts as a compiler barrier.
        core::arch::asm!("sti", options(nostack));
    }
}
