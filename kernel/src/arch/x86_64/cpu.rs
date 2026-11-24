//! CPU Local Storage for x86-64.

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

/// Interrupt-enable state captured by [`save_and_disable_interrupts`].
///
/// Pass it back to [`restore_interrupts`] to undo exactly that disable,
/// which makes nested critical sections compose: an inner restore never
/// re-enables interrupts that an outer section disabled.
#[must_use = "dropping the saved state leaves interrupts disabled"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedInterruptState {
    enabled: bool,
}

impl SavedInterruptState {
    /// `true` if interrupts were enabled when the state was saved.
    pub const fn were_enabled(self) -> bool {
        self.enabled
    }
}

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

/// Disable maskable interrupts and return the previous state.
///
/// Not `nomem`: the `cli` must also act as a compiler barrier, so memory
/// accesses of the critical section cannot be hoisted above it.
pub fn save_and_disable_interrupts() -> SavedInterruptState {
    let rflags: u64;
    // SAFETY: at CPL 0, `pushfq; pop; cli` reads RFLAGS (one transient stack
    // slot) and clears IF. Clearing IF cannot break any invariant: it only
    // defers maskable interrupts until `restore_interrupts`.
    unsafe {
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) rflags);
    }
    SavedInterruptState {
        enabled: rflags & RFLAGS_IF != 0,
    }
}

/// Undo a [`save_and_disable_interrupts`]: re-enable interrupts only if
/// they were enabled when `saved` was captured.
///
/// Not `nomem`, for the same compiler-barrier reason as the disable.
pub fn restore_interrupts(saved: SavedInterruptState) {
    if saved.enabled {
        // SAFETY: interrupts were enabled before the matching save, so
        // re-enabling them restores the caller's original state. The IDT is
        // installed long before any lock can be taken.
        unsafe { core::arch::asm!("sti", options(nostack)) };
    }
}
