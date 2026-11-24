//! Context switching between tasks (Phase 2.2.3, ADR-0008).
//!
//! [`switch_to`] is the scheduler's only way to change the running task. In
//! order, it:
//!
//! 1. points `TSS.RSP0` at the incoming task's kernel stack (every switch);
//! 2. loads the incoming task's CR3, but only if it differs from the
//!    current one (same-process switches skip the TLB flush);
//! 3. runs the `arch` assembly that swaps callee-saved registers and stacks.
//!
//! Steps 1 and 2 run before the stack switch. That is safe because every
//! kernel stack, TCB, and this code are mapped identically under every page
//! table root (the `[0, 1 GiB)` identity map is shared by all of them).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::context;
use crate::arch::x86_64::cpu::InterruptGuard;
use crate::arch::x86_64::gdt;
use crate::memory::address_space::{active_pml4_phys, load_page_table_root};
use crate::task::TaskControlBlock;

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

    let regs = &mut tcb.registers;
    *regs = Default::default();
    regs.rsp = rsp;
    regs.rip = context::task_entry_trampoline_addr();
    regs.r12 = entry as usize as u64;
    regs.rbp = 0;
}

/// Stop running `prev` and resume `next`. Returns when some later switch
/// resumes `prev`.
///
/// `_irq` is proof that interrupts are disabled. It must stay alive across
/// the call, which keeps interrupts off for the whole switch (ADR-0008 §3).
///
/// # Safety
///
/// - `next` must be a real task whose context was saved by a previous
///   `switch_to`, or initialised by [`init_kernel_task_context`].
/// - `prev` must be the task that is currently running.
/// - Neither TCB may move or be freed while switched out.
///
/// The `&mut` / `&` borrows already rule out `prev` and `next` aliasing.
pub unsafe fn switch_to(
    prev: &mut TaskControlBlock,
    next: &TaskControlBlock,
    _irq: &InterruptGuard,
) {
    assert!(
        next.registers.rip != 0,
        "switch_to: task {:?} has no resumable context",
        next.id
    );
    debug_assert!(
        next.registers.rsp >= next.stack_bottom() as u64
            && next.registers.rsp < next.stack_top() as u64,
        "switch_to: task {:?} rsp {:#x} outside its kernel stack",
        next.id,
        next.registers.rsp
    );

    // 1. Ring 3 -> ring 0 transitions of `next` must land on its own stack.
    // SAFETY: `TSS` is only written here and during boot GDT setup, both
    // with interrupts disabled on the only CPU. A packed field is written by
    // value, with no reference taken.
    unsafe {
        gdt::TSS.rsp0 = next.stack_top() as u64;
    }

    // 2. Address space, only on change.
    let next_root = next.page_table_root();
    if next_root != active_pml4_phys() {
        debug_assert!(
            stack_mapped_identically(next),
            "switch_to: task {:?} kernel stack is not identity-mapped under its root",
            next.id
        );
        // SAFETY: `next_root` is a valid PML4 that maps the kernel, this
        // code, and both kernel stacks identically to the current root
        // (TaskControlBlock::new contract; checked above in debug builds).
        load_page_table_root(next_root);
        CR3_LOADS.fetch_add(1, Ordering::Relaxed);
    }

    // 3. Registers and stack.
    // SAFETY: interrupts are disabled (`_irq`); the borrows make the
    // pointers valid and distinct; `next` is resumable (caller contract and
    // the rip assertion); its stack is mapped (above).
    context::switch_context(&mut prev.registers, &next.registers);
}

/// Debug check: `next`'s kernel stack must map to the same physical memory
/// under `next`'s root as it does now (VA == PA, the identity map).
fn stack_mapped_identically(next: &TaskControlBlock) -> bool {
    use crate::memory::address_space::translate_in;
    use crate::memory::paging::VirtualAddress;

    let probe = next.stack_top() as u64 - 8;
    let Some(va) = VirtualAddress::try_new(probe) else {
        return false;
    };
    // SAFETY: `next_root` is a valid PML4 (TaskControlBlock::new contract),
    // and page-table frames are identity-mapped.
    unsafe { translate_in(next.page_table_root(), va) == Some(probe) }
}
