//! CPU Local Storage for x86-64.

use core::cell::UnsafeCell;
use core::marker::PhantomData;

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

/// Initialize the `KernelGsBase` MSR to point to the `CpuLocal` structure.
///
/// # Safety
///
/// Must be called once per CPU core during early initialization, at CPL=0.
pub unsafe fn init() {
    let cpu_local_addr = core::ptr::addr_of!(CPU_LOCAL) as u64;

    // IA32_KERNEL_GS_BASE MSR is 0xC0000102
    const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

    let low = (cpu_local_addr & 0xFFFF_FFFF) as u32;
    let high = (cpu_local_addr >> 32) as u32;

    core::arch::asm!(
        "wrmsr",
        in("ecx") MSR_KERNEL_GS_BASE,
        in("eax") low,
        in("edx") high,
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

/// RFLAGS.IF — interrupt enable flag (bit 9).
const RFLAGS_IF: u64 = 1 << 9;

/// This CPU's interrupt-disable nesting state.
///
/// Single CPU in Phase 2, so one instance. It moves into `CpuLocal` with SMP.
struct CpuIrqState(UnsafeCell<IrqNesting>);

// SAFETY: the inner `IrqNesting` is only read or written by
// `disable_interrupts` and `InterruptGuard::drop`, and both do so with
// interrupts disabled on the only CPU. No IRQ handler can interleave. NMI
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
        debug_assert!(
            !interrupts_enabled(),
            "IF was set while an InterruptGuard was alive"
        );
        // SAFETY: interrupts are still disabled: this scope is active, and
        // only the last exit re-enables them, below.
        let reenable = unsafe { (*CPU_IRQ_STATE.0.get()).exit() };
        if reenable {
            // SAFETY: this was the last active scope, and interrupts were
            // enabled before the first one, so this restores the original
            // state. Not `nomem`, so it acts as a compiler barrier.
            unsafe { core::arch::asm!("sti", options(nostack)) };
        }
    }
}
