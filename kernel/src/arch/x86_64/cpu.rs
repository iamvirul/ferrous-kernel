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
