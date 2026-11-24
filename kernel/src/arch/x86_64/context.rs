//! Context switch: save one task's callee-saved state, resume another's.
//!
//! Design: ADR-0008. This module is the only assembly in the switch path;
//! the TSS and CR3 work happens in Rust, before it, in `sched::switch`.
//!
//! # Saved state
//!
//! Exactly the System V AMD64 callee-saved registers (rbx, rbp, r12–r15)
//! plus `rsp` and a resume `rip`, stored in the task's
//! [`RegisterState`]. Caller-saved registers are dead across the call by the
//! ABI. `rflags` is not switched: every switch runs with interrupts disabled
//! and DF clear. FPU/SSE state is not saved (deferred to Phase 3), so kernel
//! code must not keep SSE state live across a switch.

use core::arch::global_asm;
use core::mem::{offset_of, size_of};

use crate::task::RegisterState;

// The asm addresses `RegisterState` fields by offset. The offsets are taken
// from the struct itself (`offset_of!` below), so a reorder updates the asm
// automatically. The size check catches an added or removed field, which
// would otherwise leave stale assumptions in this file.
const _: () = assert!(size_of::<RegisterState>() == 9 * 8);

global_asm!(
    // -----------------------------------------------------------------------
    // ferrous_switch_context(prev: *mut RegisterState /* rdi */,
    //                        next: *const RegisterState /* rsi */)
    //
    // Entered by `call` with interrupts disabled. Returns (via `ret` at
    // label 2) only when some later switch resumes `prev`.
    // -----------------------------------------------------------------------
    ".global ferrous_switch_context",
    "ferrous_switch_context:",
    // 1. Save prev's callee-saved registers.
    "mov [rdi + {rbx}], rbx",
    "mov [rdi + {rbp}], rbp",
    "mov [rdi + {r12}], r12",
    "mov [rdi + {r13}], r13",
    "mov [rdi + {r14}], r14",
    "mov [rdi + {r15}], r15",
    // 2. Save prev's rsp. It points at our return address into prev's
    //    caller, so prev's frame is exactly a normal call frame.
    "mov [rdi + {rsp}], rsp",
    // 3. prev resumes at label 2. RAX is caller-saved, so it is free.
    "lea rax, [rip + 2f]",
    "mov [rdi + {rip}], rax",
    // ---- prev is fully saved; nothing below may touch [rdi]. ----
    // 4. Load next's callee-saved registers.
    "mov rbx, [rsi + {rbx}]",
    "mov rbp, [rsi + {rbp}]",
    "mov r12, [rsi + {r12}]",
    "mov r13, [rsi + {r13}]",
    "mov r14, [rsi + {r14}]",
    "mov r15, [rsi + {r15}]",
    // 5. Switch stacks. From here on we run on next's kernel stack.
    "mov rsp, [rsi + {rsp}]",
    // 6. Resume next: label 2 of an earlier switch (a resumed task) or the
    //    entry trampoline (a new task). No Rust code runs between the stack
    //    switch above and this jump.
    "jmp qword ptr [rsi + {rip}]",
    // A resumed task lands here with its own rsp, which points at its saved
    // return address. Return into its caller of ferrous_switch_context.
    "2:",
    "ret",
    // -----------------------------------------------------------------------
    // ferrous_task_entry_trampoline: first instruction of every new task.
    //
    // Reached by the `jmp` above, with the registers that
    // `init_kernel_task_context` placed in the new RegisterState:
    //   r12 = the task's entry function (fn() -> !)
    //   rbp = 0 (terminates the RBP-chain stack walker)
    //   rsp = 16-byte-aligned stack top - 8, holding 0 as the fake return
    //         address. That is the state the ABI expects at function entry,
    //         so we jump (not call) straight into the Rust entry.
    // -----------------------------------------------------------------------
    ".global ferrous_task_entry_trampoline",
    "ferrous_task_entry_trampoline:",
    "mov rdi, r12",
    "jmp {task_entry}",
    rbx = const offset_of!(RegisterState, rbx),
    rbp = const offset_of!(RegisterState, rbp),
    r12 = const offset_of!(RegisterState, r12),
    r13 = const offset_of!(RegisterState, r13),
    r14 = const offset_of!(RegisterState, r14),
    r15 = const offset_of!(RegisterState, r15),
    rsp = const offset_of!(RegisterState, rsp),
    rip = const offset_of!(RegisterState, rip),
    task_entry = sym task_entry,
);

extern "sysv64" {
    fn ferrous_switch_context(prev: *mut RegisterState, next: *const RegisterState);
    fn ferrous_task_entry_trampoline();
}

/// Address of the first instruction every new task executes.
pub fn task_entry_trampoline_addr() -> u64 {
    ferrous_task_entry_trampoline as *const () as u64
}

/// Rust side of a new task's entry: run its entry function. Never returns.
///
/// `entry` arrives as a raw address (from r12) because `fn() -> !` is not
/// an FFI-safe parameter type.
extern "sysv64" fn task_entry(entry: usize) -> ! {
    // SAFETY: the only producer of r12 for a new task is
    // `sched::switch::init_kernel_task_context`, which stores the address of
    // a valid `fn() -> !`. Function pointers and `usize` have the same size
    // and representation on x86-64.
    let entry = unsafe { core::mem::transmute::<usize, fn() -> !>(entry) };
    entry()
}

/// Save the current callee-saved context into `prev`, then resume `next`.
///
/// Returns when some later switch resumes `prev`.
///
/// # Safety
///
/// - Interrupts must be disabled, and stay so until this returns.
/// - `prev` and `next` must be valid, distinct, and must not move or be
///   freed while their task is switched out (the asm stores into `prev`
///   before reading `next`).
/// - `next` must hold a context saved by this function, or one built by
///   `sched::switch::init_kernel_task_context`.
/// - CR3 must map `next`'s kernel stack (and this code) identically to the
///   current mapping, which holds for all kernel stacks (ADR-0008).
pub unsafe fn switch_context(prev: *mut RegisterState, next: *const RegisterState) {
    ferrous_switch_context(prev, next);
}

// ---------------------------------------------------------------------------
// Boot self-test harness: proves every callee-saved register survives a
// switch away and back. Rust cannot name rbx/rbp as asm operands, so this
// must be assembly.
// ---------------------------------------------------------------------------

global_asm!(
    // ferrous_switch_with_sentinels(prev /* rdi */, next /* rsi */,
    //                               base /* rdx */) -> u64 /* rax */
    //
    // Loads base+1 .. base+6 into rbx, rbp, r12–r15, switches to `next`, and
    // (once resumed) returns a bit mask of registers that did not survive:
    // bit 0 = rbx, 1 = rbp, 2 = r12, …, 5 = r15. 0 means all were restored.
    ".global ferrous_switch_with_sentinels",
    "ferrous_switch_with_sentinels:",
    // Preserve the caller's callee-saved registers (we overwrite them).
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    // rdx is caller-saved, so the switch does not restore it: keep `base` on
    // the stack. Entry rsp ≡ 8 (mod 16); 7 pushes make it ≡ 0 for the call.
    "push rdx",
    "lea rbx, [rdx + 1]",
    "lea rbp, [rdx + 2]",
    "lea r12, [rdx + 3]",
    "lea r13, [rdx + 4]",
    "lea r14, [rdx + 5]",
    "lea r15, [rdx + 6]",
    "call ferrous_switch_context",
    "pop rdx",
    "xor eax, eax",
    "lea rcx, [rdx + 1]",
    "cmp rbx, rcx",
    "setne cl",
    "or al, cl",
    "lea rcx, [rdx + 2]",
    "cmp rbp, rcx",
    "setne cl",
    "shl cl, 1",
    "or al, cl",
    "lea rcx, [rdx + 3]",
    "cmp r12, rcx",
    "setne cl",
    "shl cl, 2",
    "or al, cl",
    "lea rcx, [rdx + 4]",
    "cmp r13, rcx",
    "setne cl",
    "shl cl, 3",
    "or al, cl",
    "lea rcx, [rdx + 5]",
    "cmp r14, rcx",
    "setne cl",
    "shl cl, 4",
    "or al, cl",
    "lea rcx, [rdx + 6]",
    "cmp r15, rcx",
    "setne cl",
    "shl cl, 5",
    "or al, cl",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
);

extern "sysv64" {
    fn ferrous_switch_with_sentinels(
        prev: *mut RegisterState,
        next: *const RegisterState,
        base: u64,
    ) -> u64;
}

/// Self-test variant of [`switch_context`]: fills every callee-saved register
/// with `base + 1 ..= base + 6` before switching. Once `prev` is resumed, it
/// returns a mask of the registers that came back changed (0 = all intact).
///
/// # Safety
///
/// Same as [`switch_context`].
pub unsafe fn switch_context_checking_callee_saved(
    prev: *mut RegisterState,
    next: *const RegisterState,
    base: u64,
) -> u64 {
    ferrous_switch_with_sentinels(prev, next, base)
}
