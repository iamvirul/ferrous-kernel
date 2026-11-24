//! Round-robin scheduling policy (ADR-0004 §2).
//!
//! A pure state machine: it decides *which* task runs next and *when* the
//! running task has used up its slice. The kernel applies those decisions
//! (TCB states, `switch_to`). Keeping the policy free of hardware and TCBs
//! makes every scheduling rule unit-testable on the host.
//!
//! # Model
//!
//! - Exactly one task is `current` (running) at all times; it is never in
//!   the queue.
//! - The `idle` task is never queued. It runs only when the queue is empty.
//!   New work (`admit`, or a `tick` that finds the queue non-empty) only
//!   sets `need_resched`, a *request*. The policy never switches by itself:
//!   idle gives up the CPU when the kernel next calls `switch` at a safe
//!   point or yield.
//! - Every non-idle task runs for at most `slice_ticks` ticks before a
//!   preemption is requested.

use crate::{RunQueue, RunQueueError};

/// Why the running task is giving up the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    /// Voluntary yield: the task stays runnable and goes to the back.
    Yield,
    /// Slice expired (or idle displaced by work): back of the queue.
    Preempt,
    /// The task blocks: it leaves the queue until it is admitted again.
    Block,
    /// The task exits: it is never scheduled again.
    Exit,
}

/// A decided switch from `prev` to `next` (always distinct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch<T> {
    /// The task that was running.
    pub prev: T,
    /// The task that runs now.
    pub next: T,
}

/// Single-CPU, single-priority, strict FIFO round-robin with a fixed slice.
#[derive(Debug, Clone)]
pub struct RoundRobin<T, const N: usize> {
    queue: RunQueue<T, N>,
    current: T,
    idle: T,
    slice_ticks: u32,
    remaining_ticks: u32,
    need_resched: bool,
}

impl<T: Copy + Eq, const N: usize> RoundRobin<T, N> {
    /// Start with `running` on the CPU (typically the boot task) and `idle`
    /// as the fallback task.
    ///
    /// # Panics
    ///
    /// If `slice_ticks` is 0, or `running == idle`.
    pub fn new(running: T, idle: T, slice_ticks: u32) -> Self {
        assert!(slice_ticks > 0, "time slice must be at least one tick");
        assert!(running != idle, "the running task cannot be the idle task");
        Self {
            queue: RunQueue::new(),
            current: running,
            idle,
            slice_ticks,
            remaining_ticks: slice_ticks,
            need_resched: false,
        }
    }

    /// The task on the CPU.
    pub fn current(&self) -> T {
        self.current
    }

    /// `true` if the idle task is on the CPU.
    pub fn is_idle(&self) -> bool {
        self.current == self.idle
    }

    /// `true` if the running task should be switched out at the next safe
    /// point.
    pub fn need_resched(&self) -> bool {
        self.need_resched
    }

    /// Ticks left in the running task's slice.
    pub fn remaining_ticks(&self) -> u32 {
        self.remaining_ticks
    }

    /// Number of Ready tasks waiting.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// `true` if `task` is waiting in the queue.
    pub fn is_queued(&self, task: T) -> bool {
        self.queue.contains(task)
    }

    /// Make `task` runnable (newly created, or woken): back of the queue.
    /// If the CPU is idle, a reschedule is requested.
    ///
    /// # Errors
    ///
    /// - [`RunQueueError::AlreadyQueued`] if `task` is already queued, or is
    ///   the running task or the idle task (none of which may be admitted).
    /// - [`RunQueueError::Full`] if the queue is full.
    pub fn admit(&mut self, task: T) -> Result<(), RunQueueError> {
        if task == self.current || task == self.idle {
            return Err(RunQueueError::AlreadyQueued);
        }
        self.queue.enqueue(task)?;
        if self.is_idle() {
            self.need_resched = true;
        }
        Ok(())
    }

    /// Account one timer tick to the running task. Returns
    /// [`need_resched`](Self::need_resched).
    ///
    /// The idle task has no slice: the first tick that finds work queued
    /// requests a reschedule (it does not switch; see [`switch`](Self::switch)).
    pub fn tick(&mut self) -> bool {
        if self.is_idle() {
            if !self.queue.is_empty() {
                self.need_resched = true;
            }
        } else {
            self.remaining_ticks = self.remaining_ticks.saturating_sub(1);
            if self.remaining_ticks == 0 {
                self.need_resched = true;
            }
        }
        self.need_resched
    }

    /// Decide the next task for `reason` and make it current.
    ///
    /// - `Yield` / `Preempt`: the running task goes to the back of the queue
    ///   (unless it is idle, which is never queued).
    /// - `Block` / `Exit`: the running task is dropped from scheduling.
    /// - The next task is the front of the queue, or idle if it is empty.
    ///
    /// Either way the slice is refilled and `need_resched` cleared. Returns
    /// `None` if the same task keeps the CPU (e.g. a yield with nothing else
    /// runnable), so no context switch is needed.
    ///
    /// # Panics
    ///
    /// - If the idle task tries to `Block` or `Exit`.
    /// - If re-queueing the running task finds the queue full. That breaks
    ///   the capacity invariant (`N` must be at least the number of non-idle
    ///   tasks) and is a kernel bug.
    pub fn switch(&mut self, reason: SwitchReason) -> Option<Switch<T>> {
        let prev = self.current;
        let prev_is_idle = prev == self.idle;
        match reason {
            SwitchReason::Yield | SwitchReason::Preempt => {
                if !prev_is_idle {
                    self.queue
                        .enqueue_known_absent(prev)
                        .expect("run queue full: capacity must cover every non-idle task");
                }
            }
            SwitchReason::Block | SwitchReason::Exit => {
                assert!(!prev_is_idle, "the idle task cannot block or exit");
            }
        }

        let next = self.queue.dequeue().unwrap_or(self.idle);
        self.remaining_ticks = self.slice_ticks;
        self.need_resched = false;

        if next == prev {
            return None;
        }
        self.current = next;
        Some(Switch { prev, next })
    }

    /// Take a *queued* task out of the queue (it blocked or was killed while
    /// Ready). Returns `true` if it was queued. The running task is not in
    /// the queue; use [`switch`](Self::switch) with `Block` / `Exit` for it.
    pub fn remove(&mut self, task: T) -> bool {
        self.queue.remove(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const IDLE: u32 = 0;
    const BOOT: u32 = 1;
    const SLICE: u32 = 3;

    fn policy() -> RoundRobin<u32, 8> {
        RoundRobin::new(BOOT, IDLE, SLICE)
    }

    /// Run `switch(reason)` and return the new current task.
    fn switch_to_next(p: &mut RoundRobin<u32, 8>, reason: SwitchReason) -> u32 {
        p.switch(reason);
        p.current()
    }

    // --- construction ---

    #[test]
    fn starts_with_the_running_task_and_a_full_slice() {
        let p = policy();
        assert_eq!(p.current(), BOOT);
        assert!(!p.is_idle());
        assert_eq!(p.remaining_ticks(), SLICE);
        assert!(!p.need_resched());
        assert_eq!(p.queued(), 0);
    }

    #[test]
    #[should_panic(expected = "at least one tick")]
    fn zero_slice_is_rejected() {
        RoundRobin::<u32, 8>::new(BOOT, IDLE, 0);
    }

    #[test]
    #[should_panic(expected = "cannot be the idle task")]
    fn running_task_cannot_be_idle() {
        RoundRobin::<u32, 8>::new(IDLE, IDLE, SLICE);
    }

    // --- admit ---

    #[test]
    fn admit_rejects_running_idle_and_duplicates() {
        let mut p = policy();
        assert_eq!(p.admit(BOOT), Err(RunQueueError::AlreadyQueued));
        assert_eq!(p.admit(IDLE), Err(RunQueueError::AlreadyQueued));
        p.admit(2).unwrap();
        assert_eq!(p.admit(2), Err(RunQueueError::AlreadyQueued));
        assert_eq!(p.queued(), 1);
    }

    #[test]
    fn admit_while_busy_does_not_request_resched() {
        let mut p = policy();
        p.admit(2).unwrap();
        assert!(!p.need_resched());
    }

    // --- round-robin order ---

    #[test]
    fn yield_rotates_strictly_fifo() {
        let mut p = policy();
        for t in [2, 3] {
            p.admit(t).unwrap();
        }
        let order: Vec<u32> = (0..6)
            .map(|_| switch_to_next(&mut p, SwitchReason::Yield))
            .collect();
        assert_eq!(order, [2, 3, BOOT, 2, 3, BOOT]);
    }

    #[test]
    fn yield_with_nothing_else_runnable_keeps_the_cpu() {
        let mut p = policy();
        p.tick();
        assert_eq!(p.switch(SwitchReason::Yield), None);
        assert_eq!(p.current(), BOOT);
        assert_eq!(p.remaining_ticks(), SLICE, "a yield refills the slice");
        assert_eq!(p.queued(), 0, "the running task is not left queued");
    }

    #[test]
    fn switch_reports_prev_and_next() {
        let mut p = policy();
        p.admit(2).unwrap();
        assert_eq!(
            p.switch(SwitchReason::Yield),
            Some(Switch {
                prev: BOOT,
                next: 2
            })
        );
    }

    // --- time slice / preemption ---

    #[test]
    fn need_resched_is_set_exactly_when_the_slice_expires() {
        let mut p = policy();
        p.admit(2).unwrap();
        for _ in 0..SLICE - 1 {
            assert!(!p.tick());
        }
        assert!(p.tick(), "the last tick of the slice requests a reschedule");
        assert_eq!(p.remaining_ticks(), 0);
    }

    #[test]
    fn preempt_moves_current_to_the_back_and_refills_the_slice() {
        let mut p = policy();
        for t in [2, 3] {
            p.admit(t).unwrap();
        }
        for _ in 0..SLICE {
            p.tick();
        }
        assert_eq!(switch_to_next(&mut p, SwitchReason::Preempt), 2);
        assert!(!p.need_resched());
        assert_eq!(p.remaining_ticks(), SLICE);
        assert_eq!(switch_to_next(&mut p, SwitchReason::Preempt), 3);
        assert_eq!(switch_to_next(&mut p, SwitchReason::Preempt), BOOT);
    }

    #[test]
    fn preempt_alone_keeps_running_with_a_fresh_slice() {
        let mut p = policy();
        for _ in 0..SLICE {
            p.tick();
        }
        assert_eq!(p.switch(SwitchReason::Preempt), None);
        assert_eq!(p.remaining_ticks(), SLICE);
        assert!(!p.need_resched());
    }

    #[test]
    fn ticks_past_expiry_do_not_underflow() {
        let mut p = policy();
        for _ in 0..SLICE + 5 {
            p.tick();
        }
        assert_eq!(p.remaining_ticks(), 0);
        assert!(p.need_resched());
    }

    // --- block / exit / idle ---

    #[test]
    fn block_with_empty_queue_falls_back_to_idle() {
        let mut p = policy();
        assert_eq!(
            p.switch(SwitchReason::Block),
            Some(Switch {
                prev: BOOT,
                next: IDLE
            })
        );
        assert!(p.is_idle());
        assert!(!p.is_queued(BOOT), "a blocked task is not queued");
    }

    #[test]
    fn exit_drops_the_task_for_good() {
        let mut p = policy();
        p.admit(2).unwrap();
        assert_eq!(switch_to_next(&mut p, SwitchReason::Exit), 2);
        // BOOT is gone: rotating 2 alone never brings BOOT back.
        assert_eq!(p.switch(SwitchReason::Yield), None);
        assert_eq!(p.current(), 2);
    }

    #[test]
    fn woken_task_is_admitted_and_displaces_idle() {
        let mut p = policy();
        p.switch(SwitchReason::Block); // BOOT blocks -> idle
        assert!(p.is_idle());
        p.admit(BOOT).unwrap(); // wake
        assert!(p.need_resched(), "work arrived while idle");
        assert_eq!(switch_to_next(&mut p, SwitchReason::Preempt), BOOT);
        assert!(!p.is_queued(IDLE), "idle is never queued");
    }

    #[test]
    fn idle_is_displaced_on_the_first_tick_with_work() {
        let mut p = policy();
        p.switch(SwitchReason::Block); // -> idle
        assert!(!p.tick(), "no work: idle keeps running");
        // Work queued without going through admit's resched (e.g. a race).
        p.admit(2).unwrap();
        p.need_resched = false;
        assert!(p.tick(), "first tick with work queued preempts idle");
    }

    #[test]
    fn idle_yield_with_no_work_stays_idle() {
        let mut p = policy();
        p.switch(SwitchReason::Block);
        assert_eq!(p.switch(SwitchReason::Yield), None);
        assert!(p.is_idle());
        assert_eq!(p.queued(), 0, "idle is never queued");
    }

    #[test]
    #[should_panic(expected = "idle task cannot block or exit")]
    fn idle_cannot_block() {
        let mut p = policy();
        p.switch(SwitchReason::Block);
        p.switch(SwitchReason::Block);
    }

    #[test]
    fn remove_takes_a_queued_task_out() {
        let mut p = policy();
        for t in [2, 3] {
            p.admit(t).unwrap();
        }
        assert!(p.remove(2));
        assert!(!p.remove(BOOT), "the running task is not queued");
        assert_eq!(switch_to_next(&mut p, SwitchReason::Yield), 3);
    }

    // --- invariant stress ---

    #[test]
    fn invariants_hold_over_random_operations() {
        // Deterministic LCG; tasks 1..=6, idle = 0.
        let mut p = RoundRobin::<u32, 8>::new(1, IDLE, SLICE);
        let mut alive = [true; 7]; // alive[t]: not exited
        let mut blocked = [false; 7];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for t in 2..=6 {
            p.admit(t).unwrap();
        }
        for _ in 0..20_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let cur = p.current();
            match (state >> 61) % 5 {
                0 => {
                    p.tick();
                }
                1 => {
                    p.switch(SwitchReason::Yield);
                }
                2 => {
                    if p.need_resched() {
                        p.switch(SwitchReason::Preempt);
                    }
                }
                3 if cur != IDLE => {
                    blocked[cur as usize] = true;
                    p.switch(SwitchReason::Block);
                }
                _ => {
                    // Wake a random blocked, live task.
                    let t = ((state >> 33) % 6 + 1) as u32;
                    if alive[t as usize] && blocked[t as usize] {
                        blocked[t as usize] = false;
                        p.admit(t).unwrap();
                    }
                }
            }
            let cur = p.current();
            // The running task is never queued; idle is never queued.
            assert!(!p.is_queued(cur));
            assert!(!p.is_queued(IDLE));
            // Blocked tasks are neither queued nor running.
            for t in 1..=6u32 {
                if blocked[t as usize] {
                    assert!(!p.is_queued(t) && cur != t, "blocked task {t} is runnable");
                }
            }
            // Idle runs only when nothing is queued, or a resched is pending.
            if p.is_idle() {
                assert!(p.queued() == 0 || p.need_resched());
            }
        }
        // Exit everything that can run; we must end on idle.
        alive.iter_mut().for_each(|a| *a = false);
        while !p.is_idle() {
            p.switch(SwitchReason::Exit);
        }
        assert_eq!(p.queued(), 0);
    }
}
