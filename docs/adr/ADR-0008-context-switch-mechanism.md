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

### 1. The assembly: `switch_context(prev, next, prev_fpu, next_fpu)`

*(Amended: the two FPU-area arguments, in RDX and RCX, were added for §5.)*

System V: RDI = `prev`, RSI = `next`. The steps, in order:

1. Store rbx, rbp, r12–r15 into `prev` at their fixed offsets.
2. Store `rsp` into `prev.rsp`. At this point it points at `switch_context`'s return
   address, so the frame is exactly a normal call frame.
3. Store the address of the local resume label (`2:`) into `prev.rip`.
4. Load rbx, rbp, r12–r15 and `rsp` from `next`.
5. `jmp [next.rip]`.
6. At `2:`, which a resumed task reaches, `ret` back into its own caller of
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

`prev: *mut TaskControlBlock`, `next: *const TaskControlBlock` *(amended: raw pointers,
not references)*. While `prev` is switched out, other tasks access its TCB, so a `&mut` or
`&` held across the suspension would let the compiler assume the TCB is unchanged when
`prev` resumes, which is undefined behaviour. No reference to either TCB lives across
the call; the asm gets field pointers from `addr_of_mut!` / `addr_of!`.

**Contract** *(amended)*: the switch guard must be the **only** open interrupt scope
(depth 1, asserted in debug builds). Locks such as the run queue's must be released
before switching. A new task closes exactly one inherited scope (§3), so a second open
scope would start it with interrupts off and the lock still held, and it would deadlock.

Before the assembly runs, in this order:

1. **Both kernel-entry stacks ← `next`'s stack**, on every switch *(amended: the SYSCALL
   stack was missed originally)*:
   - `TSS.RSP0`, which the CPU uses for interrupts and exceptions taken in ring 3;
   - `CpuLocal.kernel_stack`, which the SYSCALL stub loads with `mov rsp, gs:[0]`
     (16-byte aligned, as that stub requires).
2. **CR3 ← `next.page_table_root()`** only if it differs from the current CR3. Two tasks of
   the same process share a root, so the TLB flush is skipped. Comparing against the live
   CR3 instead of owner PIDs makes the check correct even for kernel tasks, which have no
   process.
3. **`switch_context(prev.registers, next.registers, prev.fpu, next.fpu)`**, passed as
   raw field pointers.

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
- **New tasks** begin at an entry trampoline. It first calls
  `end_inherited_interrupt_scope()`, which closes the scope the task switching to them
  opened (a new task has no guard of its own to drop), and then calls the task's
  `fn() -> !`. The count therefore balances across *every* switch, and a new task runs
  with the interrupt state that preceded the outermost scope. *(Amended 2026-11-24:
  originally deferred to 2.2.4; implemented with #72.)* Whether that state is "enabled"
  depends on the system having interrupts on at all, which needs the PIC/APIC set up
  (2.2.4, #75). Today boot runs with them off, so tasks do too.
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
- **x87/SSE** *(amended)*: a harness loads sentinels into xmm0–15 and a distinct MXCSR,
  switches away and back, and checks all 17, in both directions. A new task must observe
  the power-on MXCSR, not its predecessor's.
- **Interrupt scopes** *(amended)*: every task body, and boot after every switch, must
  run at the test's base nesting depth.


### 5. x87 / MMX / SSE state *(amended 2026-11-24: originally deferred to Phase 3)*

Each TCB holds a 16-byte-aligned 512-byte `FpuState` in `FXSAVE` format. `switch_context`
saves it **eagerly** with `fxsave64 [prev_fpu]` as its first instruction, and restores it
with `fxrstor64 [next_fpu]` after loading `next`'s callee-saved registers and *before*
the stack switch. No Rust code, and so no compiler-generated SSE, can run between the
restore and the resume. New tasks start from `FpuState::initial()` (the `FNINIT` control
word and the power-on MXCSR), never from the previous task's state.

**Why eager rather than lazy (CR0.TS + #NM):** lazy switching leaks the previous owner's
registers through speculative execution (LazyFP, CVE-2018-3665), needs an #NM handler,
and saves little on modern CPUs. It also matters already: the kernel is compiled for the
UEFI target, which emits SSE, so vector state is live in kernel code too.

**Why FXSAVE rather than XSAVE:** it covers x87, MMX and SSE (xmm0–15, MXCSR), which is
every vector feature the kernel enables. AVX (YMM) state needs CR4.OSXSAVE, an XSAVE
area sized via CPUID, and `xsaveopt`/`xrstor`. That is deferred until AVX is enabled,
and is the only part of the original "Phase 3" item still outstanding. The boot
self-test asserts CR4.OSFXSR before using FXSAVE.

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
- Each TCB grows by 512 bytes (`FpuState`), and is 16-byte aligned.
- Every switch costs an `fxsave64` / `fxrstor64` pair, even for tasks that never touch
  vector registers. This is the price of eager switching, accepted for the security
  reasons in §5.

### Risks
- **AVX (YMM) state is not saved** (§5). Code must not enable or use AVX until the
  switch moves to XSAVE; x87/MMX/SSE are fully preserved.
- **The identity-map invariant is load-bearing.** If kernel stacks ever move out of
  memory that is mapped in every address space, loading CR3 before the stack switch
  becomes unsafe. Before CR3 is loaded, a debug assertion checks that `next`'s stack,
  the current stack, both TCBs and the switch code all translate to themselves under
  `next`'s root.
- **Interrupt and NMI handlers must preserve x87/SSE state** *(requirement for #75)*.
  The eager switch assumes vector state changes only in task code. A Rust ISR built with
  SSE that runs over a task would clobber that task's xmm registers or MXCSR, and the
  next `fxsave` would capture the damage. An NMI is not masked by IF, so it can even land
  in the few instructions between `fxrstor64` and the `jmp`. The IRQ and NMI entry stubs
  in #75 must therefore save and restore the FPU state themselves (or handlers must be
  built without SSE). Today this is latent, because interrupts are off and the boot NMI
  handler only reports and halts.

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
- `switch_to` encapsulates those conditions, apart from "`next` is a real task" and "the
  pointers are valid and not otherwise borrowed", which the scheduler owns. It asserts
  `prev != next` and that `next` has a resume `rip`.

**Security Implications:**
- Both kernel-entry stacks are updated on every switch: TSS.RSP0 (interrupts and
  exceptions) and `CpuLocal.kernel_stack` (SYSCALL). A user task therefore never enters
  the kernel on another task's stack.
- Callee-saved registers are fully restored, so no register state leaks between tasks.
  Caller-saved registers are dead across a call by the ABI.

---

## Performance Considerations

O(1): 8 stores, 8 loads, one `fxsave64` / `fxrstor64` pair (512 bytes each way), one
indirect `jmp`, one TSS store, plus a CR3 load only when the
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
- 2026-11-24: Amended (still Accepted, implemented with #72). x87/SSE state is now saved eagerly with FXSAVE/FXRSTOR (§5; previously deferred to Phase 3), and new tasks close their inherited interrupt scope in the entry trampoline (§3; previously deferred to 2.2.4). Validated by boot checks 20.4 and 20.5, each mutation-tested. Still outstanding: AVX/XSAVE, and system-wide interrupt enabling (#75)
- 2026-11-24: Amended after an independent second review (implemented with #72). `switch_to` takes raw pointers (no reference lives across the suspension); it also updates the SYSCALL stack in `CpuLocal`, not only TSS.RSP0; it asserts that only the switch guard is open; the identity-map debug check covers both stacks and both TCBs; the new-task trampoline checks rsp alignment; and ISR/NMI FPU preservation is recorded as a requirement for #75. The self-test now checks both XMM lanes, the x87 stack, and the SYSCALL stack
