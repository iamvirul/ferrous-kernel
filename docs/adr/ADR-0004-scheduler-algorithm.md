# ADR-0004: Scheduler Algorithm and Run Queue

**Status:** Accepted  
**Date:** 2026-11-24  
**Deciders:** iamvirul  
**Tags:** phase-2, scheduler, run-queue, preemption

---

## Context

Phase 2.2 gives Ferrous its first scheduler. Three pieces already exist:

- **The run queue** (2.2.1, #71): `RunQueue<T, N>`, a fixed-capacity FIFO ring buffer of
  task handles, behind `IrqSpinLock`. When it was implemented we agreed to record its
  design here rather than in a separate ADR.
- **Context switching** (2.2.3, #72, ADR-0008): `switch_to(prev, next, &InterruptGuard)`.
  It must be called with the switch guard as the *only* open interrupt scope, so no lock
  may be held across it.
- **Interrupt nesting** (2.2.1): `InterruptGuard` plus a per-CPU depth count. New tasks
  close the inherited scope in their trampoline.

Phase 2.2.2 (#73) needs the policy and the `schedule()` entry points on top of these.
**Timer interrupts (#75) are not implemented yet**, and the kernel runs with interrupts
off. So this ADR fixes the scheduler's interface to the timer (a tick hook plus a
reschedule flag), and #75 will connect a real ISR to it. The real idle body (HLT with
interrupts enabled) is #74.

**Related GitHub Issues:**
- Implements: #73 (2.2.2 Round-Robin Scheduler)
- Records the design of: #71 (2.2.1 Run Queue)
- Depends on: #72 / ADR-0008 (context switch)
- Consumed by: #75 (2.2.4 Timer), #74 (2.2.5 Idle Task)

---

## Decision

**Decision:** single-CPU, single-priority, strict FIFO **round-robin** with a fixed time
slice. The policy is a pure, host-tested state machine (`ferrous_sched::RoundRobin`).
The kernel wraps it in an `IrqSpinLock` and switches only after releasing that lock.

### 1. Run queue (from 2.2.1)

- A ring buffer of `Option<T>` with capacity `N = 256`, holding task *handles*
  (`TaskId`), never TCBs. TCB ownership stays with the task table, so the queue needs no
  `unsafe` and its contents cannot dangle.
- `enqueue` rejects a duplicate handle and a full queue. `remove` keeps the order of the
  remaining tasks.
- The queue holds only *Ready* tasks. The running task is never in it (queued XOR
  running), and the policy enforces that half.
- **O(1) hot path:** the policy re-queues the outgoing task with `enqueue_known_absent`.
  It knows the task cannot already be queued, so it skips the O(n) duplicate scan, which
  is kept as a debug assertion. `dequeue` is O(1). Only `remove`, used when a *queued*
  task is blocked or killed, is O(n), bounded by 256.
- Why a ring buffer rather than an intrusive list: no `unsafe`, host-testable, no
  pinned TCBs. The cost is an O(n) `remove`, bounded and rare.

### 2. Policy: `RoundRobin<T, N>` (pure)

State: the run queue, `current: T`, the `idle: T` handle, `slice_ticks`,
`remaining_ticks`, and `need_resched`.

| Operation | Effect |
|---|---|
| `admit(t)` | A new or woken task becomes Ready: enqueue at the back. If the CPU is idle, set `need_resched`. |
| `tick()` | Called once per timer tick for the running task. Idle is preempted as soon as work exists. Any other task has its remaining slice decremented, and `need_resched` is set at 0. Returns `need_resched`. |
| `switch(reason)` | Picks the next task. With `Yield` or `Preempt` the outgoing task goes to the back (unless it is idle); with `Block` or `Exit` it is dropped. Next is the front of the queue, or idle if the queue is empty. Resets the slice and clears `need_resched`. Returns `None` if next == current (no switch). |
| `remove(t)` | Takes a *queued* task out (blocked or killed while Ready). |

- **Fixed slice:** `TIME_SLICE_TICKS = 10`. The tick period belongs to #75; at the
  planned 1 kHz this is 10 ms.
- **The idle task is never queued.** It runs only when the queue is empty, and is
  preempted on the first tick (or wake) after work arrives.

### 3. Kernel scheduler

- **Task table:** a static array of `MAX_TASKS = 256` TCB slots, with `TaskId` = slot
  index. Static storage means TCBs never move, which `switch_to` requires.
- **Stacks:** callers provide kernel stacks (`&'static mut` aligned buffers). Heap- or
  frame-backed stack allocation comes with process creation.
- **`schedule(reason)`** keeps the ADR-0008 depth-1 contract:
  1. open the switch `InterruptGuard` (depth 1);
  2. lock the scheduler (depth 2); let the policy decide; update both TCB states
     (Running → Ready / Blocked / Exiting; Ready → Running); take raw TCB pointers;
     **release the lock** (depth 1);
  3. `switch_to(prev, next, &guard)`;
  4. on resume, drop the guard.
- **Entry points:**
  - Voluntary: `yield_now()` (also used by `sys_yield`), `block_current()`, and
    `exit_current() -> !`.
  - Involuntary: `scheduler_tick()` is the **timer ISR hook for #75**. It charges one
    tick of CPU time to the running task and updates the policy, and it never switches.
    Switching happens at a safe point, `preempt_if_needed()`, which #75 will call on the
    interrupt-return path (or which kernel code calls at loop boundaries). This is the
    "reschedule flag" design from the issue. The ISR itself never switches while it holds
    locks.
  - `wake(id)`: a Blocked task becomes Ready again.
- **Exit:** the task becomes Exiting → Zombie and is never queued again. Its slot is not
  reused while it could still be running on its own stack. Reaping (freeing the slot and
  stack) belongs to process teardown later.
- **CPU time:** each TCB counts `cpu_ticks`, incremented by `scheduler_tick()` for the
  running task. It is readable for observability.

### 4. Rules for #75 (the timer ISR)

- The ISR must open an `InterruptGuard` before calling anything that locks. The CPU
  clears IF on interrupt entry, but the nesting count starts at zero.
- It calls `scheduler_tick()`, then `preempt_if_needed()` only after releasing its own
  guard, with no locks held, just before `iretq`. Its stub must preserve x87/SSE state
  (ADR-0008 risks).

---

## Rationale

**Key Factors:**
- **Correctness:** the policy is a pure state machine with exhaustive host tests. The
  kernel glue only maps decisions onto TCBs and `switch_to`.
- **Safety:** the lock is never held across a switch (the ADR-0008 depth-1 contract,
  asserted). TCBs live in static storage, so `switch_to`'s raw pointers stay valid.
- **Simplicity:** one queue, one priority, one fixed slice. That is enough for Phase 2's
  goal, and it is a baseline for later priority and energy-aware policies.

**Alignment with Charter Principles:** correctness and simplicity before features
(no priorities yet); fail visibly (every state transition is checked by the TCB state
machine and panics on violation).

---

## Alternatives Considered

### Alternative 1: Multi-level feedback queue / priorities now

**Pros:** Better interactivity; `TaskPriority` already exists in the TCB.

**Cons:** More policy surface to test, with no workload yet that needs it.

**Why Not Chosen:** Premature. It can replace `RoundRobin` behind the same kernel glue.

### Alternative 2: Switch directly inside the timer ISR

**Pros:** Lowest preemption latency.

**Cons:** Easy to switch while the ISR, or the code it interrupted, holds a lock, which
breaks the depth-1 contract and deadlocks.

**Why Not Chosen:** The tick hook plus a safe point gives the same latency once #75 calls
`preempt_if_needed()` on interrupt return, without the lock hazard.

### Alternative 3: Intrusive run-queue list in the TCB (from 2.2.1)

**Pros:** O(1) `remove`.

**Cons:** Raw pointers, pinned TCBs, no host tests.

**Why Not Chosen:** see §1.

---

## Consequences

### Positive
- #75 only has to call two functions. #74 only has to supply an idle body.
- Scheduling decisions are fully host-testable.

### Negative
- Kernel stacks are caller-provided for now.
- Zombies hold their slot until reaping exists.

### Risks
- **Lost wakeup in idle:** #74 must check the queue and execute `sti; hlt` atomically.
- **Starvation is impossible under strict RR**, but there is no fairness accounting
  beyond the tick count yet.

---

## Safety and Security Considerations

**Unsafe code:** the task table (a `Sync` wrapper over `UnsafeCell` slots, accessed only
under the scheduler lock or by the one running task for its own TCB), and the
`switch_to` call. All of it has safety comments.

**Security:** each task's kernel-entry stacks and FPU state are switched by ADR-0008. The
scheduler adds no shared mutable state outside its lock.

---

## Performance Considerations

`schedule()` is O(1): a dequeue, one `enqueue_known_absent`, two state stores and one
switch. `scheduler_tick()` is O(1). Only `remove` of a queued task is O(n), bounded by
256.

---

## Dependencies

**Depends on:** #71 (run queue), #72 / ADR-0008 (context switch).

**Blocks:** #75 (timer preemption wiring), #74 (idle body).

---

## Implementation Plan

1. `RoundRobin<T, N>` and `RunQueue::enqueue_known_absent` in `lib/sched`, with host
   tests.
2. Kernel `sched::scheduler`: the task table, `init`, `spawn`, `schedule`,
   `yield_now`, `scheduler_tick`, `preempt_if_needed`, `block_current`, `wake`,
   `exit_current`, `cpu_ticks`; `sys_yield` wired to `yield_now`.
3. QEMU self-test:
   - round-robin order across three tasks;
   - preemption at exactly `TIME_SLICE_TICKS`, driving the tick hook;
   - idle when the queue is empty;
   - block/wake and exit.

---

## References

- [ADR-0008](ADR-0008-context-switch-mechanism.md) (switch protocol, depth-1 contract)
- [CHARTER.md](../CHARTER.md)
- Issues: #71, #72, #73, #74, #75

---

## Status History

- 2026-11-24: Created (Status: Proposed); to be accepted before the Phase 2.2.2 implementation (#73) lands
- 2026-11-24: Accepted (PR #171). Implemented in `lib/sched/src/round_robin.rs` and `kernel/src/sched/scheduler.rs`. Validated in QEMU by boot checks 21.1–21.4 (round-robin order, exact time-slice preemption via the tick hook, idle only when the queue is empty, block/wake/exit), each also mutation-tested. Live timer preemption is wired in #75
