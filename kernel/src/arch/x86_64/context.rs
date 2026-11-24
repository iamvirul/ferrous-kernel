//! Context switch: save one task's state, resume another's.
//!
//! Design: ADR-0008. This module is the only assembly in the switch path;
//! the TSS and CR3 work happens in Rust, before it, in `sched::switch`.
//!
//! # Saved state
//!
//! - **Integer:** the System V AMD64 callee-saved registers (rbx, rbp,
//!   r12–r15) plus `rsp` and a resume `rip`, in the task's
//!   [`RegisterState`]. Caller-saved registers are dead across the call by
//!   the ABI.
//! - **x87 / MMX / SSE:** the full `FXSAVE` image (all XMM registers, MXCSR,
//!   the x87 stack and control word), in the task's [`FpuState`]. It is
//!   saved *eagerly*, on every switch, never lazily via CR0.TS, which avoids
//!   the LazyFP side channel (CVE-2018-3665) and needs no #NM handler.
//! - `rflags` is not switched: every switch runs with interrupts disabled
//!   and DF clear.

use core::arch::global_asm;
use core::mem::{offset_of, size_of};

use crate::arch::x86_64::cpu::end_inherited_interrupt_scope;
use crate::task::{FpuState, RegisterState};

// The asm addresses `RegisterState` fields by offset. The offsets are taken
// from the struct itself (`offset_of!` below), so a reorder updates the asm
// automatically. The size check catches an added or removed field, which
// would otherwise leave stale assumptions in this file.
const _: () = assert!(size_of::<RegisterState>() == 9 * 8);
// FXSAVE64 / FXRSTOR64 operate on exactly 512 bytes and fault (#GP) unless
// the area is 16-byte aligned.
const _: () = assert!(size_of::<FpuState>() == 512);
const _: () = assert!(core::mem::align_of::<FpuState>() == 16);

global_asm!(
    // -----------------------------------------------------------------------
    // ferrous_switch_context(prev:     *mut RegisterState   /* rdi */,
    //                        next:     *const RegisterState /* rsi */,
    //                        prev_fpu: *mut FpuState        /* rdx */,
    //                        next_fpu: *const FpuState      /* rcx */)
    //
    // Entered by `call` with interrupts disabled. Returns (via `ret` at
    // label 2) only when some later switch resumes `prev`.
    // -----------------------------------------------------------------------
    ".global ferrous_switch_context",
    "ferrous_switch_context:",
    // 0. Save prev's x87/SSE state first, before any instruction below can
    //    touch it. FXSAVE64 does not modify the registers it saves.
    "fxsave64 [rdx]",
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
    // ---- prev is fully saved; nothing below may touch [rdi] or [rdx]. ----
    // 4. Load next's callee-saved registers.
    "mov rbx, [rsi + {rbx}]",
    "mov rbp, [rsi + {rbp}]",
    "mov r12, [rsi + {r12}]",
    "mov r13, [rsi + {r13}]",
    "mov r14, [rsi + {r14}]",
    "mov r15, [rsi + {r15}]",
    // 5. Load next's x87/SSE state. Done here, inside the asm, so no
    //    compiler-generated SSE code can run after it and clobber it.
    //
    //    AMD CPUs do not restore the x87 last-instruction pointer, data
    //    pointer, and opcode (FIP/FDP/FOP) unless the image has an unmasked
    //    exception pending, so `next` would otherwise see `prev`'s values
    //    (an information leak across tasks; CVE-2006-1056 class). Overwrite
    //    them first with a harmless x87 load from a kernel constant: clear
    //    pending exceptions, empty the tag word so the load cannot overflow
    //    the stack, then load. FXRSTOR replaces everything else. This runs
    //    on every CPU: it costs a few cycles and needs no CPU detection.
    "fnclex",
    "emms",
    "fild dword ptr [rip + {fpu_scrub}]",
    "fxrstor64 [rcx]",
    // 6. Switch stacks. From here on we run on next's kernel stack.
    "mov rsp, [rsi + {rsp}]",
    // 7. Resume next: label 2 of an earlier switch (a resumed task) or the
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
    // Fail fast on a mis-built initial frame: at function entry the ABI
    // needs rsp ≡ 8 (mod 16). A violation would otherwise surface much later
    // as a #GP on some aligned SSE access in the task body.
    "lea rax, [rsp + 8]",
    "test al, 0xF",
    "jz 3f",
    "ud2",
    "3:",
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
    fpu_scrub = sym FPU_POINTER_SCRUB,
);

/// Operand of the x87 load that overwrites FIP/FDP/FOP before every
/// `fxrstor64` (see step 5 of `ferrous_switch_context`).
static FPU_POINTER_SCRUB: i32 = 0;

extern "sysv64" {
    fn ferrous_switch_context(
        prev: *mut RegisterState,
        next: *const RegisterState,
        prev_fpu: *mut FpuState,
        next_fpu: *const FpuState,
    );
    fn ferrous_task_entry_trampoline();
}

/// Address of the first instruction every new task executes.
pub fn task_entry_trampoline_addr() -> u64 {
    ferrous_task_entry_trampoline as *const () as u64
}

/// Rust side of a new task's entry. It ends the interrupt scope the task
/// inherited from whoever switched to it, then runs its entry function.
/// Never returns.
///
/// `entry` arrives as a raw address (from r12) because `fn() -> !` is not
/// an FFI-safe parameter type.
extern "sysv64" fn task_entry(entry: usize) -> ! {
    // SAFETY: a new task is only ever entered through `switch_to`, whose
    // caller holds an `InterruptGuard`. That scope stays open across the
    // switch, and a new task has no guard of its own to close it, so this
    // closes it exactly once (ADR-0008 §3). Interrupts return to whatever
    // state preceded the outermost scope.
    unsafe { end_inherited_interrupt_scope() };
    // SAFETY: the only producer of r12 for a new task is
    // `sched::switch::init_kernel_task_context`, which stores the address of
    // a valid `fn() -> !`. Function pointers and `usize` have the same size
    // and representation on x86-64.
    let entry = unsafe { core::mem::transmute::<usize, fn() -> !>(entry) };
    entry()
}

/// Save the current context (integer and x87/SSE) into `prev` /
/// `prev_fpu`, then resume `next` / `next_fpu`.
///
/// Returns when some later switch resumes `prev`.
///
/// # Safety
///
/// - Exactly one interrupt-disabled scope must be open (an
///   `InterruptGuard` held by the caller, nesting depth 1), not merely
///   IF = 0. A new task closes one inherited scope in its trampoline, so a
///   bare `cli` would underflow the per-CPU nesting count, and extra open
///   scopes would leave the new task with interrupts off and locks held.
/// - All four pointers must be valid; `prev` and `next` must be distinct
///   tasks; none may move or be freed while their task is switched out (the
///   asm stores into `prev` before reading `next`).
/// - `next` / `next_fpu` must hold a context saved by this function, or one
///   built by `sched::switch::init_kernel_task_context` and
///   `FpuState::initial`.
/// - CR3 must map `next`'s kernel stack (and this code) identically to the
///   current mapping, which holds for all kernel stacks (ADR-0008).
/// - CR4.OSFXSR must be set (UEFI guarantees it for x86-64).
pub unsafe fn switch_context(
    prev: *mut RegisterState,
    next: *const RegisterState,
    prev_fpu: *mut FpuState,
    next_fpu: *const FpuState,
) {
    // SAFETY: forwarded verbatim; the caller upholds every condition above,
    // and the asm follows the sysv64 ABI declared on the extern.
    ferrous_switch_context(prev, next, prev_fpu, next_fpu);
}

// ---------------------------------------------------------------------------
// Boot self-test harnesses. Rust cannot name rbx/rbp as asm operands, and
// the vector test must control every XMM register, so these are assembly.
// ---------------------------------------------------------------------------

global_asm!(
    // ferrous_switch_with_sentinels(prev /* rdi */, next /* rsi */,
    //                               prev_fpu /* rdx */, next_fpu /* rcx */,
    //                               base /* r8 */) -> u64 /* rax */
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
    // r8 is caller-saved, so the switch does not restore it: keep `base` on
    // the stack. Entry rsp ≡ 8 (mod 16); 7 pushes make it ≡ 0 for the call.
    "push r8",
    "lea rbx, [r8 + 1]",
    "lea rbp, [r8 + 2]",
    "lea r12, [r8 + 3]",
    "lea r13, [r8 + 4]",
    "lea r14, [r8 + 5]",
    "lea r15, [r8 + 6]",
    "call ferrous_switch_context",
    "pop r8",
    "xor eax, eax",
    "lea rcx, [r8 + 1]",
    "cmp rbx, rcx",
    "setne cl",
    "or al, cl",
    "lea rcx, [r8 + 2]",
    "cmp rbp, rcx",
    "setne cl",
    "shl cl, 1",
    "or al, cl",
    "lea rcx, [r8 + 3]",
    "cmp r12, rcx",
    "setne cl",
    "shl cl, 2",
    "or al, cl",
    "lea rcx, [r8 + 4]",
    "cmp r13, rcx",
    "setne cl",
    "shl cl, 3",
    "or al, cl",
    "lea rcx, [r8 + 5]",
    "cmp r14, rcx",
    "setne cl",
    "shl cl, 4",
    "or al, cl",
    "lea rcx, [r8 + 6]",
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

global_asm!(
    // ferrous_switch_with_vector_sentinels(prev /* rdi */, next /* rsi */,
    //                                      prev_fpu /* rdx */,
    //                                      next_fpu /* rcx */,
    //                                      base /* r8 */,
    //                                      mxcsr /* r9 */) -> u64 /* rax */
    //
    // Before switching, loads:
    //   - into xmm{i}: low lane base+i+1, high lane base+0x100+i+1, distinct
    //     per lane so a lane swap or partial restore is detected;
    //   - `mxcsr` into MXCSR;
    //   - `base` onto the x87 stack (`fild`), distinct per side.
    // Once resumed, returns a mask of what did not survive:
    //   bit i (0–15) = xmm{i}, either lane; bit 16 = MXCSR; bit 17 = x87.
    // 0 means the whole x87/SSE state was restored. MXCSR is put back to
    // its original value and the x87 stack is left empty before returning.
    //
    // Stack frame after `sub rsp, 40` (entry rsp ≡ 8 mod 16, so ≡ 0 here):
    //   [rsp +  0 .. 16]  128-bit scratch for movdqu / fstp
    //   [rsp + 16]        4-byte MXCSR scratch
    //   [rsp + 20]        4-byte original (caller's) MXCSR
    //   [rsp + 24]        expected MXCSR (u64)
    //   [rsp + 32]        base
    ".global ferrous_switch_with_vector_sentinels",
    "ferrous_switch_with_vector_sentinels:",
    "sub rsp, 40",
    "mov [rsp + 32], r8",
    "mov [rsp + 24], r9",
    "stmxcsr [rsp + 20]",
    "mov [rsp + 16], r9d",
    "ldmxcsr [rsp + 16]",
    "lea rax, [r8 + 1]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x101]",
    "mov [rsp + 8], rax",
    "movdqu xmm0, [rsp]",
    "lea rax, [r8 + 2]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x102]",
    "mov [rsp + 8], rax",
    "movdqu xmm1, [rsp]",
    "lea rax, [r8 + 3]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x103]",
    "mov [rsp + 8], rax",
    "movdqu xmm2, [rsp]",
    "lea rax, [r8 + 4]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x104]",
    "mov [rsp + 8], rax",
    "movdqu xmm3, [rsp]",
    "lea rax, [r8 + 5]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x105]",
    "mov [rsp + 8], rax",
    "movdqu xmm4, [rsp]",
    "lea rax, [r8 + 6]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x106]",
    "mov [rsp + 8], rax",
    "movdqu xmm5, [rsp]",
    "lea rax, [r8 + 7]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x107]",
    "mov [rsp + 8], rax",
    "movdqu xmm6, [rsp]",
    "lea rax, [r8 + 8]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x108]",
    "mov [rsp + 8], rax",
    "movdqu xmm7, [rsp]",
    "lea rax, [r8 + 9]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x109]",
    "mov [rsp + 8], rax",
    "movdqu xmm8, [rsp]",
    "lea rax, [r8 + 10]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10a]",
    "mov [rsp + 8], rax",
    "movdqu xmm9, [rsp]",
    "lea rax, [r8 + 11]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10b]",
    "mov [rsp + 8], rax",
    "movdqu xmm10, [rsp]",
    "lea rax, [r8 + 12]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10c]",
    "mov [rsp + 8], rax",
    "movdqu xmm11, [rsp]",
    "lea rax, [r8 + 13]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10d]",
    "mov [rsp + 8], rax",
    "movdqu xmm12, [rsp]",
    "lea rax, [r8 + 14]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10e]",
    "mov [rsp + 8], rax",
    "movdqu xmm13, [rsp]",
    "lea rax, [r8 + 15]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x10f]",
    "mov [rsp + 8], rax",
    "movdqu xmm14, [rsp]",
    "lea rax, [r8 + 16]",
    "mov [rsp], rax",
    "lea rax, [r8 + 0x110]",
    "mov [rsp + 8], rax",
    "movdqu xmm15, [rsp]",
    "fild qword ptr [rsp + 32]",
    "call ferrous_switch_context",
    "mov r8, [rsp + 32]",
    "xor edx, edx",
    "movdqu [rsp], xmm0",
    "lea rcx, [r8 + 1]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x101]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 0",
    "or edx, eax",
    "movdqu [rsp], xmm1",
    "lea rcx, [r8 + 2]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x102]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 1",
    "or edx, eax",
    "movdqu [rsp], xmm2",
    "lea rcx, [r8 + 3]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x103]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 2",
    "or edx, eax",
    "movdqu [rsp], xmm3",
    "lea rcx, [r8 + 4]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x104]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 3",
    "or edx, eax",
    "movdqu [rsp], xmm4",
    "lea rcx, [r8 + 5]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x105]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 4",
    "or edx, eax",
    "movdqu [rsp], xmm5",
    "lea rcx, [r8 + 6]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x106]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 5",
    "or edx, eax",
    "movdqu [rsp], xmm6",
    "lea rcx, [r8 + 7]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x107]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 6",
    "or edx, eax",
    "movdqu [rsp], xmm7",
    "lea rcx, [r8 + 8]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x108]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 7",
    "or edx, eax",
    "movdqu [rsp], xmm8",
    "lea rcx, [r8 + 9]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x109]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 8",
    "or edx, eax",
    "movdqu [rsp], xmm9",
    "lea rcx, [r8 + 10]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10a]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 9",
    "or edx, eax",
    "movdqu [rsp], xmm10",
    "lea rcx, [r8 + 11]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10b]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 10",
    "or edx, eax",
    "movdqu [rsp], xmm11",
    "lea rcx, [r8 + 12]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10c]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 11",
    "or edx, eax",
    "movdqu [rsp], xmm12",
    "lea rcx, [r8 + 13]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10d]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 12",
    "or edx, eax",
    "movdqu [rsp], xmm13",
    "lea rcx, [r8 + 14]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10e]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 13",
    "or edx, eax",
    "movdqu [rsp], xmm14",
    "lea rcx, [r8 + 15]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x10f]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 14",
    "or edx, eax",
    "movdqu [rsp], xmm15",
    "lea rcx, [r8 + 16]",
    "cmp rcx, [rsp]",
    "setne al",
    "lea rcx, [r8 + 0x110]",
    "cmp rcx, [rsp + 8]",
    "setne cl",
    "or al, cl",
    "movzx eax, al",
    "shl eax, 15",
    "or edx, eax",
    // MXCSR (bit 16)
    "stmxcsr [rsp + 16]",
    "mov eax, [rsp + 16]",
    "cmp rax, [rsp + 24]",
    "setne al",
    "movzx eax, al",
    "shl eax, 16",
    "or edx, eax",
    // x87 (bit 17): pop the top of stack as an integer; it must still be
    // our own `base`. If the x87 state was not restored, it is the other
    // side's value, or (on an empty stack) the integer-indefinite pattern.
    "fistp qword ptr [rsp]",
    "cmp r8, [rsp]",
    "setne al",
    "movzx eax, al",
    "shl eax, 17",
    "or edx, eax",
    // Restore the caller's MXCSR (MXCSR is callee-saved).
    "ldmxcsr [rsp + 20]",
    "mov eax, edx",
    "add rsp, 40",
    "ret",
);

extern "sysv64" {
    fn ferrous_switch_with_sentinels(
        prev: *mut RegisterState,
        next: *const RegisterState,
        prev_fpu: *mut FpuState,
        next_fpu: *const FpuState,
        base: u64,
    ) -> u64;
    fn ferrous_switch_with_vector_sentinels(
        prev: *mut RegisterState,
        next: *const RegisterState,
        prev_fpu: *mut FpuState,
        next_fpu: *const FpuState,
        base: u64,
        mxcsr: u64,
    ) -> u64;
}

/// Self-test variant of [`switch_context`]: fills every callee-saved register
/// with `base + 1 ..= base + 6` before switching. Once `prev` is resumed, it
/// returns a mask of the registers that came back changed (0 = all intact).
///
/// # Safety
///
/// Same as [`switch_context`], including the depth-1 interrupt-scope
/// requirement.
pub unsafe fn switch_context_checking_callee_saved(
    prev: *mut RegisterState,
    next: *const RegisterState,
    prev_fpu: *mut FpuState,
    next_fpu: *const FpuState,
    base: u64,
) -> u64 {
    ferrous_switch_with_sentinels(prev, next, prev_fpu, next_fpu, base)
}

/// Self-test variant of [`switch_context`]: fills xmm0–xmm15 with distinct
/// per-lane sentinels (low `base + i + 1`, high `base + 0x100 + i + 1`), MXCSR with `mxcsr`, and pushes `base` on
/// the x87 stack before switching. Once `prev` is resumed, it returns a mask of
/// what came back changed (bit i = xmm{i}, bit 16 = MXCSR, bit 17 = x87;
/// 0 = all intact). It restores the caller's MXCSR and pops the x87 stack.
///
/// # Safety
///
/// Same as [`switch_context`], including the depth-1 interrupt-scope
/// requirement, and `mxcsr` must have no reserved bits set (otherwise
/// `ldmxcsr` faults).
pub unsafe fn switch_context_checking_vector_state(
    prev: *mut RegisterState,
    next: *const RegisterState,
    prev_fpu: *mut FpuState,
    next_fpu: *const FpuState,
    base: u64,
    mxcsr: u32,
) -> u64 {
    ferrous_switch_with_vector_sentinels(prev, next, prev_fpu, next_fpu, base, u64::from(mxcsr))
}
