//! Context switching between tasks (Phase 2.2.3, ADR-0008).
//!
//! [`switch_to`] is the scheduler's only way to change the running task. In
//! order, it:
//!
//! 1. points `TSS.RSP0` at the incoming task's kernel stack (every switch);
//! 2. loads the incoming task's CR3, but only if it differs from the
//!    current one (same-process switches skip the TLB flush);
//! 3. runs the `arch` assembly that saves and restores the integer and
//!    x87/SSE state and swaps stacks.
//!
//! Steps 1 and 2 run before the stack switch. That is safe because every
//! kernel stack, TCB, and this code are mapped identically under every page
//! table root (the `[0, 1 GiB)` identity map is shared by all of them).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::context;
use crate::arch::x86_64::cpu::{self, irq_disable_depth, InterruptGuard};
use crate::arch::x86_64::gdt;
use crate::memory::address_space::{active_pml4_phys, load_page_table_root};
use crate::task::{FpuState, RegisterState, TaskControlBlock};

/// Number of CR3 loads performed by [`switch_to`] (observability; also used
/// by the boot self-test to prove same-root switches skip the load).
static CR3_LOADS: AtomicU64 = AtomicU64::new(0);

/// Total CR3 loads performed by [`switch_to`] since boot.
pub fn cr3_load_count() -> u64 {
    CR3_LOADS.load(Ordering::Relaxed)
}

/// Prepare a fresh task so that the first [`switch_to`] into it starts
/// executing `entry` on its own kernel stack.
///
/// Sets `rip` to the entry trampoline, `r12` to `entry`, `rbp` to 0 (the end
/// of the frame chain), and `rsp` to its 16-byte-aligned stack top minus 8,
/// with 0 stored there as the fake return address. That is the exact state
/// the System V ABI expects at function entry.
///
/// # Safety
///
/// - `tcb`'s stack bounds must describe memory that is valid, writable, and
///   used by nothing else (the `TaskControlBlock::new` contract).
/// - The stack must have room for at least one `u64` below its 16-byte
///   aligned top.
pub unsafe fn init_kernel_task_context(tcb: &mut TaskControlBlock, entry: fn() -> !) {
    let aligned_top = (tcb.stack_top() as u64) & !0xF;
    let rsp = aligned_top - 8;
    assert!(
        rsp >= tcb.stack_bottom() as u64,
        "kernel stack too small for an initial frame"
    );
    // SAFETY: `rsp` lies within [stack_bottom, stack_top) (asserted above),
    // is 8-byte aligned, and the caller guarantees the stack is ours to use.
    (rsp as *mut u64).write(0);

    tcb.registers = RegisterState {
        rsp,
        rip: context::task_entry_trampoline_addr(),
        r12: entry as usize as u64,
        rbp: 0,
        ..RegisterState::default()
    };
    tcb.fpu = FpuState::initial();
}

/// Stop running `prev` and resume `next`. Returns when some later switch
/// resumes `prev`.
///
/// `_irq` is proof that interrupts are disabled. It must stay alive across
/// the call, which keeps interrupts off for the whole switch. That scope is
/// closed on the far side: by `next` dropping its own guard if it is being
/// resumed, or by the new-task entry trampoline if `next` is new
/// (ADR-0008 §3).
///
/// # Contract: the switch guard must be the only open interrupt scope
///
/// Release every lock (e.g. the run queue's `IrqSpinLock`) *before* calling
/// this. A new task closes exactly one inherited scope, so with two open it
/// would start with interrupts still disabled and the lock still held, and
/// deadlock on its first `lock()`. Debug builds assert the depth is 1.
///
/// # Why raw pointers
///
/// While `prev` is switched out, other tasks access its TCB (and `next`
/// saves into its own on the way back). A `&mut` / `&` held across the
/// suspension would let the compiler assume the TCB is unchanged when
/// `prev` resumes, which is undefined behaviour. So no reference to either
/// TCB is held across the switch.
///
/// # Safety
///
/// - `prev` and `next` must point to valid, distinct TCBs that do not move
///   or get freed while switched out.
/// - `prev` must be the task that is currently running.
/// - `next` must be a real task whose context was saved by a previous
///   `switch_to`, or initialised by [`init_kernel_task_context`].
/// - No other reference to either TCB may be live for the duration of the
///   call.
pub unsafe fn switch_to(
    prev: *mut TaskControlBlock,
    next: *const TaskControlBlock,
    _irq: &InterruptGuard,
) {
    assert!(
        !core::ptr::eq(prev, next),
        "switch_to: prev and next are the same task"
    );
    debug_assert_eq!(
        irq_disable_depth(),
        1,
        "switch_to: only the switch guard may be open (release locks first)"
    );

    // Everything we need from `next`, read through short-lived borrows that
    // end before the switch.
    let (next_stack_top, next_root) = {
        // SAFETY: valid per the caller contract; the borrow ends at the end
        // of this block, before any suspension.
        let next_ref = &*next;
        assert!(
            next_ref.registers.rip != 0,
            "switch_to: task {:?} has no resumable context",
            next_ref.id
        );
        debug_assert!(
            next_ref.registers.rsp >= next_ref.stack_bottom() as u64
                && next_ref.registers.rsp < next_ref.stack_top() as u64,
            "switch_to: task {:?} rsp {:#x} outside its kernel stack",
            next_ref.id,
            next_ref.registers.rsp
        );
        (next_ref.stack_top() as u64, next_ref.page_table_root())
    };

    // 1. Both kernel-entry stacks must be `next`'s own:
    //    - TSS.RSP0, used by the CPU for interrupts and exceptions taken in
    //      ring 3 (the CPU aligns it to 16 itself);
    //    - CpuLocal.kernel_stack, loaded by the SYSCALL stub (`gs:[0]`),
    //      which assumes a 16-byte-aligned stack.
    // SAFETY: both are only written here and during boot setup, with
    // interrupts disabled on the only CPU. Packed or `static mut` fields are
    // written by value through raw places, with no reference taken.
    unsafe {
        gdt::TSS.rsp0 = next_stack_top;
        core::ptr::addr_of_mut!(cpu::CPU_LOCAL.kernel_stack).write(next_stack_top & !0xF);
    }

    // 2. Address space, only on change.
    if next_root != active_pml4_phys() {
        debug_assert!(
            switch_memory_mapped_identically(prev, next, next_root),
            "switch_to: kernel memory used by the switch is not identity-mapped under {next_root:#x}"
        );
        // SAFETY: `next_root` is a valid PML4 that maps the kernel, this
        // code, and both kernel stacks identically to the current root
        // (TaskControlBlock::new contract; checked above in debug builds).
        load_page_table_root(next_root);
        CR3_LOADS.fetch_add(1, Ordering::Relaxed);
    }

    // 3. Registers (integer and x87/SSE) and stack. Field pointers come
    //    from `addr_of_mut!` / `addr_of!`, so no reference outlives this line.
    // SAFETY: interrupts are disabled (`_irq`); the pointers are valid and
    // distinct (contract, asserted above); `next` is resumable (contract and
    // the rip assertion), and its FPU image came from FXSAVE or
    // `FpuState::initial`; its memory is mapped (above). `FpuState` is
    // 16-byte aligned by type.
    context::switch_context(
        core::ptr::addr_of_mut!((*prev).registers),
        core::ptr::addr_of!((*next).registers),
        core::ptr::addr_of_mut!((*prev).fpu),
        core::ptr::addr_of!((*next).fpu),
    );
}

/// Debug check: everything the switch touches after CR3 changes must map
/// to the same physical memory under `next_root` as now (VA == PA, the
/// identity map). That is `next`'s stack, the stack we are running on,
/// both TCBs (the asm writes `prev`'s and reads `next`'s), and this code.
fn switch_memory_mapped_identically(
    prev: *const TaskControlBlock,
    next: *const TaskControlBlock,
    next_root: u64,
) -> bool {
    use crate::memory::address_space::translate_in;
    use crate::memory::paging::VirtualAddress;

    let current_rsp: u64;
    // SAFETY: reads RSP into a register; no memory or flags are touched.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack, preserves_flags));
    }
    // SAFETY: `next` is valid per the `switch_to` contract; the borrow is
    // confined to this expression.
    let next_stack_probe = unsafe { (*next).stack_top() as u64 - 8 };
    let probes = [
        next_stack_probe,
        current_rsp,
        prev as u64,
        next as u64,
        switch_memory_mapped_identically as *const () as u64,
    ];
    probes.iter().all(|&addr| {
        VirtualAddress::try_new(addr).is_some_and(|va| {
            // SAFETY: `next_root` is a valid PML4 (TaskControlBlock::new
            // contract), and page-table frames are identity-mapped.
            unsafe { translate_in(next_root, va) == Some(addr) }
        })
    })
}
