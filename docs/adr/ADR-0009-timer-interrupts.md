# ADR-0009: Timer Interrupts and Preemption (PIC + PIT)

**Status:** Accepted  
**Date:** 2026-11-24  
**Deciders:** iamvirul  
**Tags:** phase-2, interrupts, timer, pic, pit, preemption

---

## Context

The round-robin scheduler (ADR-0004, #73) is finished, but nothing drives it yet: the
kernel runs with interrupts off. Phase 2.2.4 (#75) adds the first hardware interrupt
source, a periodic timer, and turns interrupts on for the first time.

The design has to fit facts about the current system, several of them measured during
this ADR's investigation:

1. **Firmware leaves interrupts quiet.** After `ExitBootServices`, both 8259 PICs are
   fully masked (IMR `0xff`/`0xff`), and the LAPIC is enabled (`IA32_APIC_BASE =
   0xfee00900`) but its timer is not firing. A spike that turned IF on with nothing
   configured received no interrupts.
2. **The kernel is soft-float.** `x86_64-unknown-uefi` is `-mmx,-sse,+soft-float` with
   the red zone disabled, so compiled kernel code never touches x87/SSE state, and an
   interrupt taken in ring 0 cannot clobber a red zone.
3. **The boot IDT points every vector 32–255 at a halting stub,** and the kernel GDT/TSS
   is loaded late (boot Step 17), after the scheduler self-tests.
4. **ADR-0008 §3 keeps "interrupts were enabled before the outermost scope" per-CPU.**
   That is correct only while every switch starts from task code. Once a switch can start
   inside an ISR (where IF = 0), the flag would carry the ISR's IF = 0 into whichever task
   runs next.

**Related GitHub Issues:**
- Implements: #75 (2.2.4 Timer Interrupts)
- Completes: #73's live-preemption criterion (ADR-0004 §4)
- Precedes: #74 (2.2.5 Idle Task: a real HLT idle needs interrupts)

---

## Decision

**Decision:** use the legacy **8259 PIC pair remapped to vectors 32–47**, driven by the
**8254 PIT channel 0 at 100 Hz**. One assembly IRQ stub per PIC line calls a Rust
dispatcher. The timer handler charges a tick, sends EOI, and *then* preempts at the
ADR-0004 safe point. The interrupt-enable state of a task's outermost scope becomes
**per-task**, saved and restored by `switch_to`.

### 1. PIC (8259A pair)

- Standard ICW1–ICW4 initialisation: edge-triggered, cascade on IRQ2, 8086 mode. The
  master gets vector base **32** and the slave **40**, clear of CPU exceptions 0–31.
- **Masks: only IRQ0 (timer) is unmasked**; the slave is fully masked. IRQ2 (the
  cascade) stays masked until a slave device is needed.
- **EOI:** the dispatcher sends EOI for every *real* IRQ, before any switch, so a
  preempted task cannot leave the PIC blocking IRQ0.
- **Spurious IRQs:** a masked line can still raise vector 39 (IRQ7) or 47 (IRQ15). The
  dispatcher reads the in-service register (OCW3). A spurious IRQ7 gets no EOI; a
  spurious IRQ15 gets an EOI to the master only. Both are counted, not treated as
  errors.
- The LAPIC is left as the firmware configured it (virtual-wire delivery of the PIC
  through LINT0). Its timer is not started, which was verified by the spike. Moving to
  the LAPIC timer and the IOAPIC is the Phase 3 / SMP target.

### 2. PIT (8254 channel 0)

- Mode 2 (rate generator), lobyte/hibyte, `divisor = round(1_193_182 / TIMER_HZ)`.
- **`TIMER_HZ = 100`** (a 10 ms tick, as #75 specifies) is a kernel constant, validated
  at compile time: the divisor must fit 16 bits and be at least 2.
- The time slice stays `TIME_SLICE_TICKS = 10` (ADR-0004), which is **100 ms at
  100 Hz**. Raising `TIMER_HZ` to 1000 gives the 10 ms slice ADR-0004 assumed. 100 Hz
  is kept as #75's baseline because every tick costs work under QEMU TCG.
- A global `TICKS: AtomicU64` (uptime in ticks) is readable by any subsystem.

### 3. IRQ entry path

- **Stub (asm, one per vector 32–47):**
  1. if the interrupted context was ring 3 (`CS & 3` in the CPU frame), `swapgs`
     followed by `lfence` (§3a);
  2. push the nine caller-saved GPRs; after the CPU's 5-word frame RSP is 16-byte
     aligned;
  3. `cld`;
  4. `mov edi, vector`;
  5. `call irq_dispatch`;
  6. pop the registers; if returning to ring 3, `swapgs`; then `iretq`.

  There is **no `fxsave`**: the kernel is soft-float (Context 2), which a compile-time
  `cfg(target_feature = "sse")` guard enforces.

### 3a. GS invariant *(amended after review: originally "no `swapgs`")*

**Rule: whenever the CPU runs in ring 0, `GS_BASE` holds the kernel's `CpuLocal`, and
`IA32_KERNEL_GS_BASE` holds the user's GS.** This is the Linux convention.

- `cpu::init` sets `GS_BASE = &CpuLocal` and `KERNEL_GS_BASE = 0` (user GS). Previously
  the two were swapped: in kernel mode `GS_BASE` was 0 and `CpuLocal` lived in
  `KERNEL_GS_BASE`.
- **Every entry from ring 3** executes `swapgs`: SYSCALL (unchanged), and IRQs and
  exceptions whose frame shows CPL 3. **Every return to ring 3** does the same: SYSRET
  (unchanged), IRQ `iretq`, and the first entry in `jump_to_user_mode`, which now
  executes `swapgs` before its `iretq`. Kernel-to-kernel entries and returns of
  **maskable** sources never `swapgs` (for non-maskable entries, see below).
- Because ring-0 GS is the same for every task, **`switch_to` never touches GS**. This
  closes the review's failure path, where a timer preempted task A in ring 3 and resumed
  task B in the middle of a syscall with user GS still loaded. With the invariant, B
  resumes with kernel GS like everything else in ring 0.
- **User GS is 0 for every task today:** user code cannot change GS (no `wrgsbase`; CR4
  FSGSBASE is off). Once user tasks can set their own GS base, `switch_to` must save and
  restore `IA32_KERNEL_GS_BASE` per task.
- The conditional `swapgs` is followed by `lfence` on both the swap and no-swap paths.
  That is the SWAPGS speculation mitigation (CVE-2019-1125).

**Non-maskable entries: NMI, and later #MC / #DB** *(amended after review)*. The CS test
above is only sound for entries that IF masks. There are short windows where the CPU is
in ring 0 but GS is still the *user* value:
- an IRQ stub between the CPU's delivery and its entry `swapgs`;
- an IRQ stub between its return `swapgs` and `iretq`;
- the SYSCALL stub between its `swapgs` and `sysretq`;
- `jump_to_user_mode` between its `swapgs` and `iretq`.

IF is 0 in all of them, so no IRQ can land there, but an NMI can. Its frame shows CPL 0,
so a CS test would skip `swapgs`, and the handler would run with the user GS base. The
rule is:
- **Today, non-maskable handlers must not use GS.** The NMI handler (boot `__isr_2` →
  `exception_handler`) reports the fault and halts. It never reads `gs:`, and it runs on
  its own IST stack (ADR-0002), so the window is harmless. The other CPU exception stubs
  likewise never `swapgs` and never use GS: they also report and halt.
- **The debug GS assertion** (`cpu::kernel_gs_active`) is checked only on the maskable
  paths (IRQ dispatch and SYSCALL entry), never in an NMI handler, which can legitimately
  observe the user GS in these windows.
- **When a non-maskable handler first needs per-CPU data**, or exceptions become
  recoverable, it must use the *paranoid* entry instead of the CS test. That means
  reading `IA32_GS_BASE` with `rdmsr`: `swapgs` on entry only if it does not equal
  `&CpuLocal`, and on exit `swapgs` back only if it swapped on entry. This is the Linux
  `paranoid_entry` scheme, and the CS-based rule above stays for maskable IRQs.
- **Dispatcher (Rust, `extern "sysv64"`):**
  - **Timer (vector 32):** `TICKS += 1`; then, inside an `InterruptGuard`,
    `scheduler_tick()`; then the EOI; then, with no scope or lock held,
    `preempt_if_needed()`.
  - **Spurious (vectors 39 and 47):** handled as in §1.
  - **Any other vector:** a masked line fired, which is a bug. Log it, send the EOI, and
    count it.
- **Preemption from the ISR** switches tasks while still on the interrupted task's
  kernel stack, as ADR-0004 §4 requires. The preempted task later resumes inside its
  own ISR frame and `iretq`s back to where it was interrupted. Each task's kernel stack
  holds its own ISR frame, so this composes. Interrupts that arrive in ring 3 enter on
  `TSS.RSP0`, which `switch_to` keeps pointing at the running task (ADR-0008).
- **Budget:** the handler is O(1) and never blocks. A tick with no preemption does one
  atomic increment, one lock/unlock, and three port writes. #75's "< 1 µs" target cannot
  be measured meaningfully under QEMU TCG, so it is checked by inspection.

### 4. Per-task interrupt state (amends ADR-0008 §3)

The nesting count stays per-CPU: at a switch its depth is always 1 (asserted). But the
**outermost "were interrupts enabled before"** bit now belongs to the *task*:

- The saved bit records the task's **intent** *(amended after review)*: "should this
  task run with interrupts enabled at depth 0?" It does not record the boot-time global
  mask.
- `switch_to` stores `outer_enabled OR NOT live` in `prev.registers.rflags` (the IF bit
  only; that field was reserved until now). Before interrupts are live, every normal task
  intends to have them, so its bit is saved as 1. Saving a raw 0 there would leave the
  task with IF = 0 forever after `enable_interrupts()`; the idle task, which yields during
  the boot self-tests, would hit exactly that.
- `switch_to` then installs `next`'s saved bit **AND "interrupts are live"** as the
  per-CPU outer state.
- New tasks are created with IF set in that field, so they start with interrupts
  enabled once the system has them, and disabled before then.
- A task preempted from an ISR saves the ISR's IF = 0. When it is resumed, it returns
  into its ISR frame with interrupts still off, and `iretq` restores its real IF. A task
  that yielded or blocked from normal code saves IF = 1 and gets it back.

Without this, a task switched to from an ISR would inherit IF = 0 and never be preempted
again. Tracing that path while designing this ADR is what exposed the flaw.

### 5. Enabling interrupts

`cpu::enable_interrupts()` first sets a global **"interrupts live"** flag (Release
store), and **then** executes `sti` *(amended after review: order made explicit)*. The
order matters: an IRQ can be delivered as soon as `sti` takes effect, and a switch it
causes computes the incoming task's IF as `intent AND live` (§4). With the flag already
set, every deliverable IRQ sees it, so no task can be started with IF = 0 in that window.
It must be called at nesting depth 0, and only once the GDT/TSS, IDT stubs, PIC, PIT and
scheduler are all initialised. Boot therefore:

1. **Moves GDT/TSS loading earlier** (out of Step 17), so every IRQ runs on the kernel's
   own segments. `ltr` must happen exactly once.
2. Installs the IRQ stubs, initialises the PIC (remap and mask) and the PIT, then calls
   `enable_interrupts()`.
3. Runs the timer self-test, then continues to Steps 17–18 with interrupts on. The ring-3
   program now runs while the timer preempts it, which is a stability check.

---

## Rationale

- **Correctness:** EOI happens before any switch; spurious IRQs are handled; the IF
  state follows the task; the soft-float assumption is enforced by the compiler.
- **Simplicity:** the PIC and PIT are fully specified by four port sequences, need no
  MMIO mapping, and are what #75 asks for.
- **Safety:** all port I/O sits behind one `arch::x86_64::port` module with safety
  comments. The ISR never switches while holding a lock (ADR-0004 §4).

---

## Alternatives Considered

### Alternative 1: LAPIC timer and IOAPIC now

**Pros:** The Phase 3 / SMP-ready target; no PIC quirks.

**Cons:** Needs MMIO mappings (the LAPIC at 0xFEE00000 lies outside the 1 GiB identity
map), timer calibration against the PIT or TSC, and ACPI MADT parsing for the IOAPIC.

**Why Not Chosen:** #75 explicitly scopes it to Phase 3.

### Alternative 2: Disable the LAPIC globally (`IA32_APIC_BASE` bit 11)

**Pros:** Guarantees the PIC reaches the CPU directly.

**Cons:** On some CPUs the LAPIC cannot be re-enabled without a reset, which would block
the Phase 3 migration.

**Why Not Chosen:** Virtual-wire delivery works as the firmware configured it. The
self-test proves the PIC IRQ arrives, and that is the fallback signal if it ever does not.

### Alternative 3: Keep the IF flag per-CPU and enable interrupts in the trampoline

**Pros:** No `switch_to` change.

**Cons:** It fixes only new tasks. A *resumed* task whose last switch happened inside an
ISR would still inherit the wrong IF.

**Why Not Chosen:** The state is genuinely per task (§4).

---

## Consequences

### Positive
- Preemption is live, and #73's last acceptance criterion is met.
- Uptime ticks are available to every subsystem.

### Negative
- `RegisterState.rflags` now carries meaning (the saved IF), and `switch_to` reads and
  writes it.
- GDT/TSS initialisation moves earlier in boot.

### Risks
- **PIC delivery depends on the firmware's LINT0 setup.** The timer self-test fails
  loudly if no tick arrives.
- **A 100 ms slice** is coarse for interactive work. `TIMER_HZ` is a constant.
- **Interrupts are on from boot Step 22 onward.** Any later boot code that needs atomic
  sections must use `InterruptGuard`.

---

## Safety and Security Considerations

- **Port I/O** (`in`/`out`) requires CPL 0 and correct ports. It is wrapped once, and
  every call site carries a safety comment.
- **Ring-3 interrupts** enter on `TSS.RSP0` (the running task's kernel stack); the stub
  touches nothing user-controlled.
- **Unexpected or spurious IRQs** cannot wedge the PIC: the EOI rules above apply.

---

## Performance Considerations

At 100 Hz the timer costs one IRQ every 10 ms. Each one is O(1): an atomic increment,
one lock round-trip, the EOI, and a switch only when the slice expires.

---

## Dependencies

**Depends on:** ADR-0004 (scheduler hooks), ADR-0008 (switch, TSS.RSP0, nesting),
ADR-0002 (GDT/TSS).

**Blocks:** #74 (HLT idle).

---

## Implementation Plan

1. `arch::x86_64::port`; drivers for `pic` (remap, mask, EOI, ISR read) and `pit`
   (program `TIMER_HZ`).
2. `arch::x86_64::interrupts`:
   - the IRQ stubs for vectors 32–47 and `irq_dispatch`;
   - `TICKS`;
   - a soft-float `compile_error!` guard.
3. `cpu`: `enable_interrupts()` and the "interrupts live" flag. `IrqNesting` gains
   get/replace for the outermost bit (with host tests). `switch_to` saves and restores
   the per-task IF bit; new tasks start with IF set.
4. Boot:
   - load the GDT/TSS once, early;
   - install the stubs;
   - initialise the PIC and PIT;
   - enable interrupts;
   - run the timer self-test:
     - the PIC masks read back;
     - the first ticks arrive within a **bounded timeout measured with the TSC**, which
       does not depend on IRQ0. Without the timeout, a missing IRQ0 would hang the test
       instead of failing it loudly;
     - only after that is confirmed, ticks keep growing while the CPU sits in `hlt`;
     - two tasks that never yield are both preempted by the real timer;
     - the system stays stable through Steps 17–18 with the timer on.

---

## References

- Intel 8259A and 8254 datasheets; OSDev "8259 PIC" (spurious IRQs) and "PIT"
- [ADR-0004](ADR-0004-scheduler-algorithm.md), [ADR-0008](ADR-0008-context-switch-mechanism.md)
- Issues: #73, #74, #75

---

## Status History

- 2026-11-24: Created (Status: Proposed); to be accepted before the Phase 2.2.4 implementation (#75) lands
- 2026-11-24: Amended after CodeRabbit review (still Proposed). Added the GS invariant (§3a: kernel GS in ring 0, `swapgs` on every ring-3 entry and exit, `switch_to` never touches GS). The per-task IF bit now saves intent (`outer OR NOT live`), so tasks switched out before enablement stay preemptible. The self-test's first-tick wait uses a TSC timeout that does not depend on IRQ0
- 2026-11-25: Amended after the second CodeRabbit review (still Proposed). §3a now defines non-maskable entries: today NMI and exception handlers never use GS (they report and halt on an IST or the current stack); the GS assertion runs only on maskable paths; and a future GS-using NMI, #MC or #DB handler must use the paranoid `rdmsr IA32_GS_BASE` entry instead of the CS test. §5 states that "interrupts live" is published before `sti`, which is what `cpu::enable_interrupts` does
- 2026-11-25: Accepted (PR #173). Implemented in `kernel/src/arch/x86_64/{port,pic,pit,interrupts}.rs`, `cpu.rs` (GS invariant, interrupts-live flag, outermost IF exchange), and `sched/switch.rs` (per-task IF intent). Validated in QEMU by boot checks 22.0–22.5 and by timer IRQs taken in ring 3 (a non-zero count is required by `verify-boot.sh`). Each mechanism was mutation-tested
