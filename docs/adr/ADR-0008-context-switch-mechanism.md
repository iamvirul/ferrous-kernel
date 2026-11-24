# ADR-0008: Context Switch Mechanism

**Status:** Accepted  
**Date:** 2026-11-24  
**Deciders:** iamvirul  
**Tags:** phase-2, scheduler, context-switch, assembly, security

---

## Context

Phase 2.2.3 (#72) needs the routine that stops one task and resumes another: it saves
the outgoing task's execution state, switches kernel stack and (when needed) address
space, and restores the incoming task's state. The scheduler (2.2.2, #73) will call it,
as will the timer-driven preemption (2.2.4, #75) later.

This is the most safety-critical code in the kernel. One wrong offset or missed register
corrupts a kernel stack silently, and the corruption only shows up much later. The issue
therefore requires an ADR and a second review.

What already exists and constrains the design:

1. **`RegisterState` is `repr(C)` with fixed offsets.** It holds rbx, rbp, r12–r15, rsp,
   rip and rflags at `0x00`–`0x40` (Phase 2.1.1), laid out for exactly this routine.
2. **Every kernel stack and TCB lives in the `[0, 1 GiB)` identity map.** PML4[0] is
   copied into each user address space (ADR-0002, ADR-0007), so kernel memory has the
   same mapping under every CR3.
3. **Interrupt masking is a per-CPU nesting count** (`InterruptGuard`, Phase 2.2.1).
   Interrupts come back on only when the last guard drops.
4. **There is no task → address-space link yet.** `Process` holds only a placeholder
   for its address space, so the switcher cannot look up a task's CR3.

**Related GitHub Issues:**
- Implements: #72 (Phase 2.2.3: Context Switching)
- Consumed by: #73 (2.2.2 Round-Robin Scheduler), #75 (2.2.4 Timer Interrupts), #74 (2.2.5 Idle Task)

---

## Decision

**Decision:** A `global_asm!` routine, `switch_context(prev, next)`, saves and restores
the callee-saved registers plus `rsp` and `rip` directly in the two tasks' `RegisterState`.
It then resumes `next` with `jmp`, not `ret`. A Rust wrapper, `switch_to`, does the
address-space and TSS work *before* the assembly runs, and requires proof that interrupts
are disabled. It is an `unsafe fn`: its one remaining precondition, that `next` holds a
resumable context, cannot be expressed in the type system.

### 1. The assembly: `switch_context(prev: *mut RegisterState, next: *const RegisterState)`

System V: RDI = `prev`, RSI = `next`. The steps, in order:

1. Store rbx, rbp, r12–r15 into `prev` at their fixed offsets.
2. Store `rsp` into `prev.rsp`. At this point it points at `switch_context`'s return
   address, so the frame is exactly a normal call frame.
3. Store the address of the local resume label (`1:`) into `prev.rip`.
4. Load rbx, rbp, r12–r15 and `rsp` from `next`.
5. `jmp [next.rip]`.
6. At `1:`, which a resumed task reaches, `ret` back into its own caller of
   `switch_context`.

Why `jmp [rip]` rather than the push-everything-and-`ret` style:
- **One code path** serves resumed tasks (`rip` = the resume label) and brand-new tasks
  (`rip` = an entry trampoline). Nobody has to hand-build a fake frame on a new task's
  stack to make `ret` land in the right place.
- **It uses the layout Phase 2.1.1 fixed.** `RegisterState` *is* the saved context, so a
  debugger or panic dump can read it directly.

Offsets are written as `const` operands computed with `core::mem::offset_of!`, not
literals. A compile-time assertion pins `size_of::<RegisterState>()`. A field reorder
therefore either updates the assembly automatically or fails the build.

**`rflags` is not switched.** Every switch happens with interrupts disabled (see §3), and
the other flags carry no state across a call boundary in the System V ABI (DF must already
be clear). The `rflags` field stays for the future IRETQ return-to-user frame (ADR-0002),
not for this routine.

### 2. The wrapper: `switch_to(prev, next, irq: &InterruptGuard)`

`prev: &mut TaskControlBlock`, `next: &TaskControlBlock`. Before the assembly runs, in
this order:

1. **TSS.RSP0 ← `next.kernel_stack_top()`** on every switch, so a ring-3 → ring-0
   transition of `next` lands on `next`'s own kernel stack.
2. **CR3 ← `next.page_table_root()`** only if it differs from the current CR3. Two tasks of
   the same process share a root, so the TLB flush is skipped. Comparing against the live
   CR3 instead of owner PIDs makes the check correct even for kernel tasks, which have no
   process.
3. **`switch_context(&mut prev.registers, &next.registers)`**.

Loading CR3 *before* the stack switch is safe because of invariant (2): the code being
executed, both kernel stacks and both TCBs are mapped identically under every root. No
Rust code runs between the stack switch and the register restore; both happen inside the
single asm routine, as the issue requires.

**TCB change:** the TCB gains `page_table_root: u64`, the physical address of the task's
PML4, fixed at creation. The process that creates the task caches it from its
`AddressSpace`. Kernel-only tasks use the kernel root. This avoids a process lookup on the
hot path. When `Process` gains a real address-space handle, the creator fills this field
from it.

### 3. Interrupts, and new tasks

- **The `&InterruptGuard` argument is the proof** that interrupts are disabled for the
  entire switch. You cannot call `switch_to` without one, which enforces at compile time
  the issue's rule that interrupts stay disabled throughout.
- **Nesting across a switch.** The per-CPU interrupt nesting count is not per task. A task
  calls `switch_to` while holding its guard. When that task is later resumed, it returns
  from `switch_to` and drops its own guard, so every task's push and pop stay balanced on
  its own stack.
- **New tasks** begin at an entry trampoline that calls the task's `fn() -> !`. They start
  with interrupts disabled, inside the scope that the task switching to them opened.
  Phase 2.2.4 (timer) adds the step where a new task ends that inherited scope and enables
  interrupts. Until then all switching is cooperative, with interrupts off.
- **Initial stack:** the trampoline is entered by `jmp`, not `call`, so the new task's
  `rsp` is set to `stack_top - 8`, 16-byte aligned minus 8. That is exactly what the System
  V ABI expects at function entry. The 8-byte slot holds 0 as a terminating return address
  for the RBP-chain stack walker.

### 4. Validation

- **Two-task ping-pong in QEMU:** two kernel tasks with their own stacks switch back and
  forth N times. A shared log of the interleaving must come out exactly A, B, A, B, ….
- **Callee-saved registers:** an asm harness loads distinct sentinels into rbx, rbp and
  r12–r15, switches away and back, then checks all six.
- **CR3:** two `AddressSpace`s map the same user VA to different frames holding different
  values. A task in each reads the VA after being switched in and must see its own value.
  A switch between tasks with the same root must leave CR3 untouched.
- **TSS:** after every switch, `TSS.rsp0 == next.kernel_stack_top()`.

---

## Rationale

**Key Factors:**
- **Correctness:** offsets come from `offset_of!` and the layout is asserted at compile
  time. One asm routine with one resume path leaves few ways to get the frame wrong.
- **Safety:** the interrupt-disable requirement is a type, not a comment. CR3 and TSS work
  happens in Rust, where it can be reviewed and asserted; only the irreducible
  register/stack swap is in assembly.
- **Simplicity:** no per-switch heap, lookups or locks. The switch costs 8 stores, 8 loads,
  one indirect jump and an optional CR3 write.

**Alignment with Charter Principles:**
- "Unsafe Rust is explicit, reviewed, and isolated": all of it sits in one `global_asm!`
  block and one `unsafe` call, both with safety comments.
- "Fail fast, fail visibly": the layout asserts fail at compile time, and debug builds
  assert that `next.rsp` lies within `next`'s stack before switching.

---

## Alternatives Considered

### Alternative 1: Push callee-saved registers on the stack, save only RSP (Linux `__switch_to_asm` style)

**Description:** push rbx…r15, save rsp, load the next rsp, pop rbx…r15, `ret`.

**Pros:** Smallest TCB footprint; a well-known pattern.

**Cons:** A new task needs a hand-forged stack frame (fake pushed registers plus a return
address). Leaves the `RegisterState` layout fixed in 2.1.1 unused. Saved state is hidden
on stacks, where diagnostics can't easily see it.

**Why Not Chosen:** Forging frames is exactly the kind of silent-corruption risk this ADR
is trying to remove.

### Alternative 2: `#[naked]` Rust function

**Description:** write `switch_context` as `#[unsafe(naked)] extern "sysv64" fn` with
`naked_asm!`.

**Pros:** Lives next to the Rust code; it's a normal function symbol.

**Cons:** Equivalent machine code. The project's existing asm (SYSCALL entry, ISRs) is all
`global_asm!`, and the CI-pinned nightly is the only reference point for naked-function
syntax.

**Why Not Chosen:** Consistency with the existing assembly. This is easy to revisit; the
ABI is identical.

### Alternative 3: Switch CR3 inside the assembly, after the stack switch

**Description:** pass the next root to the asm and load it there.

**Pros:** A single place for "everything that changes".

**Cons:** More assembly, and the issue forbids Rust between the stack switch and the
restore anyway. Nothing requires CR3 to change after the stack, because kernel memory is
mapped identically under every root.

**Why Not Chosen:** It puts logic in assembly that can be written, reviewed and asserted
in Rust.

### Alternative 4: Look up the address space by `owner_pid` on each switch

**Pros:** No new TCB field.

**Cons:** It needs a process table that doesn't exist yet, plus a lookup and possibly a
lock on the hot path.

**Why Not Chosen:** Caching the root in the TCB is O(1) and lock-free.

---

## Consequences

### Positive
- The scheduler (2.2.2) gets a single primitive, `switch_to`, that is hard to misuse: it
  needs a guard, does the TSS/CR3 work itself, and asserts (in every build) that `next`
  has a resume `rip`. Debug builds also check that `next.rsp` is within `next`'s stack.
- New tasks need no forged frames.

### Negative
- `TaskControlBlock::new` gains a `page_table_root` parameter, which is a public API change.
- New tasks start with interrupts disabled until 2.2.4 adds the inherited-scope hand-off.

### Risks
- **Lazy FPU/SSE state is not saved** (deferred to Phase 3, per the issue). Kernel code
  that uses SSE across a switch would corrupt another task's vector registers. Kernel
  code must not use SSE/AVX state across `switch_to` until Phase 3.
- **The identity-map invariant is load-bearing.** If kernel stacks ever move out of
  memory that is mapped in every address space, loading CR3 before the stack switch
  becomes unsafe. A debug assertion checks that `next`'s stack is readable under
  `next`'s root before CR3 is loaded.

### Implementation Notes
- `kernel/src/arch/x86_64/context.rs`: the `switch_context` asm and the entry trampoline.
- `kernel/src/sched/switch.rs`: `switch_to` and the new-task constructor.
- `kernel/src/task/task.rs`: the `page_table_root` field, and the `RegisterState` offset
  constants exported for the asm.

---

## Safety and Security Considerations

**Unsafe Code:**
- `switch_context`. Callers must guarantee:
  - `prev` and `next` are valid and distinct;
  - `next` holds a context that was saved by `switch_context`, or built by the new-task
    constructor;
  - interrupts are disabled.
- `switch_to` encapsulates those conditions, apart from "`next` is a real task", which
  the scheduler owns. Its `&mut prev` / `&next` borrows rule out aliasing.

**Security Implications:**
- TSS.RSP0 is updated on every switch, so a user task can never enter the kernel on
  another task's stack.
- Callee-saved registers are fully restored, so no register state leaks between tasks.
  Caller-saved registers are dead across a call by the ABI.

---

## Performance Considerations

O(1): 8 stores, 8 loads, one indirect `jmp`, one TSS store, plus a CR3 load only when the
root changes. Same-process switches skip the TLB flush. There is no heap use, and no
locks inside the switch itself. Latency targets are measured with the scheduler in a
later phase.

---

## Dependencies

**Depends on:** Phase 2.1.1 (TCB layout), ADR-0002 (TSS, CPL transitions), Phase 2.2.1
(`InterruptGuard`).

**Blocks:** #73 (2.2.2 scheduler), #75 (2.2.4 timer preemption), #74 (2.2.5 idle task).

---

## Implementation Plan

1. `RegisterState` offset constants and the size assertion. The `switch_context` asm,
   with per-instruction safety comments.
2. The TCB `page_table_root` field; the `switch_to` wrapper (TSS, conditional CR3, guard
   parameter); the new-task constructor and entry trampoline.
3. Boot smoke tests: ping-pong ordering, callee-saved sentinels, CR3 between two address
   spaces, skipping CR3 on the same root, and TSS.RSP0 after every switch.

---

## References

**Project Documents:**
- [ADR-0002](ADR-0002-kernel-user-mode-transition.md) (TSS, RSP0)
- [ADR-0007](ADR-0007-system-call-abi-and-dispatch.md) (identity-map invariant)
- [CHARTER.md](../CHARTER.md)

**GitHub Issues:** #72, #73, #74, #75

**External References:**
- System V AMD64 ABI §3.2 (callee-saved registers, stack alignment, DF)
- Intel SDM Vol. 3A §7.7 (TSS in 64-bit mode), §4.10.4 (CR3 loads and TLB invalidation)
- xv6-riscv `swtch.S` (callee-saved-only switch, resume via return address)

---

## Status History

- 2026-11-24: Created (Status: Proposed); to be accepted before the Phase 2.2.3 implementation (#72) lands
- 2026-11-24: Accepted (PR #169). Implemented in `kernel/src/arch/x86_64/context.rs` and `kernel/src/sched/switch.rs`; validated in QEMU by boot checks 20.1–20.3 (ping-pong, callee-saved sentinels, CR3 between two address spaces), each also mutation-tested
