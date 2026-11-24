//! `ferrous-sched`: scheduler data structures for Ferrous (Phase 2.2.1).
//!
//! Pure, `no_std`, `unsafe`-free, and host-testable. The kernel wraps these
//! types in an interrupt-safe lock (`kernel::sync::IrqSpinLock`). The types
//! themselves have no notion of concurrency.
//!
//! # Modules
//!
//! | Item | Purpose |
//! |------|---------|
//! | [`RunQueue`] | FIFO of runnable task handles, fixed capacity, no heap |

#![no_std]
#![warn(missing_docs)]

// `#[test]` needs the std harness on the host; the crate itself stays no_std.
#[cfg(test)]
extern crate std;

/// Errors returned by [`RunQueue::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunQueueError {
    /// The item is already queued. A task may be runnable at most once,
    /// which enforces the single-owner invariant: queued XOR running.
    AlreadyQueued,
    /// All `N` slots are occupied.
    Full,
}

/// A fixed-capacity FIFO ready queue of task handles.
///
/// Stores small `Copy` handles (the kernel uses `TaskId`), never the task
/// control blocks themselves. TCB ownership stays with the task table, so
/// the queue needs no `unsafe` and cannot dangle.
///
/// # Invariants
///
/// - Exactly the `len` slots at logical positions `head .. head + len`
///   (mod `N`) are `Some`; every other slot is `None`.
/// - No handle appears twice.
///
/// # Complexity
///
/// `dequeue` and `is_empty` are O(1). `enqueue` (duplicate check) and
/// `remove` are O(len), bounded by `N`, so every operation runs in bounded
/// time and is safe for interrupt context.
#[derive(Debug, Clone)]
pub struct RunQueue<T, const N: usize> {
    slots: [Option<T>; N],
    head: usize,
    len: usize,
}

impl<T: Copy + Eq, const N: usize> Default for RunQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Eq, const N: usize> RunQueue<T, N> {
    /// Create an empty queue. `const` so it can initialise a `static`.
    ///
    /// # Panics
    ///
    /// At compile time, if `N == 0`.
    pub const fn new() -> Self {
        const { assert!(N > 0, "RunQueue capacity must be non-zero") };
        Self {
            slots: [None; N],
            head: 0,
            len: 0,
        }
    }

    /// Number of queued handles.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// `true` if no task is runnable.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` if every slot is occupied.
    pub const fn is_full(&self) -> bool {
        self.len == N
    }

    /// Maximum number of handles the queue can hold.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Add `item` to the back of the queue (make the task runnable).
    ///
    /// # Errors
    ///
    /// - [`RunQueueError::AlreadyQueued`] if `item` is already queued
    ///   (checked first: it signals a scheduler bug, which is worth more
    ///   than "full").
    /// - [`RunQueueError::Full`] if all `N` slots are in use.
    ///
    /// On error the queue is unchanged.
    pub fn enqueue(&mut self, item: T) -> Result<(), RunQueueError> {
        if self.contains(item) {
            return Err(RunQueueError::AlreadyQueued);
        }
        if self.is_full() {
            return Err(RunQueueError::Full);
        }
        let tail = self.physical(self.len);
        self.slots[tail] = Some(item);
        self.len += 1;
        Ok(())
    }

    /// Remove and return the front of the queue (pick the next task).
    pub fn dequeue(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let item = self.slots[self.head].take();
        debug_assert!(
            item.is_some(),
            "RunQueue invariant: occupied slot was empty"
        );
        self.head = self.physical(1);
        self.len -= 1;
        item
    }

    /// Remove `item` wherever it is in the queue (the task blocked or exited).
    ///
    /// Returns `true` if it was queued. `false` is not an error: a task that
    /// is currently running is, by the single-owner invariant, not queued.
    /// Relative order of the remaining items is preserved.
    pub fn remove(&mut self, item: T) -> bool {
        let Some(offset) = self.position(item) else {
            return false;
        };
        // Close the gap: shift every later item one logical slot forward.
        for i in offset..self.len - 1 {
            let (to, from) = (self.physical(i), self.physical(i + 1));
            self.slots[to] = self.slots[from];
        }
        let last = self.physical(self.len - 1);
        self.slots[last] = None;
        self.len -= 1;
        true
    }

    /// `true` if `item` is queued.
    pub fn contains(&self, item: T) -> bool {
        self.position(item).is_some()
    }

    /// Queued handles from front to back.
    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        (0..self.len).filter_map(move |i| self.slots[self.physical(i)])
    }

    /// Logical offset (0 = front) of `item`, if queued.
    fn position(&self, item: T) -> Option<usize> {
        (0..self.len).find(|&i| self.slots[self.physical(i)] == Some(item))
    }

    /// Physical slot index of logical offset `offset` from the head.
    const fn physical(&self, offset: usize) -> usize {
        (self.head + offset) % N
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    type Queue = RunQueue<u64, 4>;

    fn contents(q: &Queue) -> Vec<u64> {
        q.iter().collect()
    }

    fn queue_of(items: &[u64]) -> Queue {
        let mut q = Queue::new();
        for &i in items {
            q.enqueue(i).unwrap();
        }
        q
    }

    // --- construction / is_empty ---

    #[test]
    fn new_queue_is_empty() {
        let q = Queue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.capacity(), 4);
        assert_eq!(q.dequeue_clone(), None);
    }

    #[test]
    fn is_empty_tracks_contents() {
        let mut q = Queue::new();
        q.enqueue(1).unwrap();
        assert!(!q.is_empty());
        q.dequeue();
        assert!(q.is_empty());
    }

    // --- enqueue / dequeue ---

    #[test]
    fn dequeue_returns_fifo_order() {
        let mut q = queue_of(&[1, 2, 3]);
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.dequeue(), Some(2));
        assert_eq!(q.dequeue(), Some(3));
        assert_eq!(q.dequeue(), None);
    }

    #[test]
    fn dequeue_on_empty_returns_none() {
        let mut q = Queue::new();
        assert_eq!(q.dequeue(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn enqueue_duplicate_is_rejected_and_queue_unchanged() {
        let mut q = queue_of(&[1, 2]);
        assert_eq!(q.enqueue(1), Err(RunQueueError::AlreadyQueued));
        assert_eq!(contents(&q), [1, 2]);
    }

    #[test]
    fn enqueue_when_full_is_rejected_and_queue_unchanged() {
        let mut q = queue_of(&[1, 2, 3, 4]);
        assert!(q.is_full());
        assert_eq!(q.enqueue(5), Err(RunQueueError::Full));
        assert_eq!(contents(&q), [1, 2, 3, 4]);
    }

    #[test]
    fn duplicate_is_reported_before_full() {
        let mut q = queue_of(&[1, 2, 3, 4]);
        assert_eq!(q.enqueue(3), Err(RunQueueError::AlreadyQueued));
    }

    #[test]
    fn dequeued_item_can_be_requeued() {
        // Round-robin pattern: run the front task, then put it at the back.
        let mut q = queue_of(&[1, 2]);
        let t = q.dequeue().unwrap();
        q.enqueue(t).unwrap();
        assert_eq!(contents(&q), [2, 1]);
    }

    #[test]
    fn wraps_around_the_ring() {
        let mut q = queue_of(&[1, 2, 3]);
        q.dequeue();
        q.dequeue();
        // head is now at physical slot 2; these writes wrap past the end.
        q.enqueue(4).unwrap();
        q.enqueue(5).unwrap();
        q.enqueue(6).unwrap();
        assert!(q.is_full());
        assert_eq!(contents(&q), [3, 4, 5, 6]);
        assert_eq!(q.dequeue(), Some(3));
        assert_eq!(q.dequeue(), Some(4));
    }

    // --- remove ---

    #[test]
    fn remove_from_middle_preserves_order() {
        let mut q = queue_of(&[1, 2, 3, 4]);
        assert!(q.remove(2));
        assert_eq!(contents(&q), [1, 3, 4]);
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn remove_front() {
        let mut q = queue_of(&[1, 2, 3]);
        assert!(q.remove(1));
        assert_eq!(contents(&q), [2, 3]);
    }

    #[test]
    fn remove_back() {
        let mut q = queue_of(&[1, 2, 3]);
        assert!(q.remove(3));
        assert_eq!(contents(&q), [1, 2]);
    }

    #[test]
    fn remove_only_item_empties_queue() {
        let mut q = queue_of(&[7]);
        assert!(q.remove(7));
        assert!(q.is_empty());
        assert_eq!(q.dequeue(), None);
    }

    #[test]
    fn remove_missing_returns_false_and_queue_unchanged() {
        let mut q = queue_of(&[1, 2]);
        assert!(!q.remove(9));
        assert_eq!(contents(&q), [1, 2]);
    }

    #[test]
    fn remove_from_empty_returns_false() {
        let mut q = Queue::new();
        assert!(!q.remove(1));
    }

    #[test]
    fn remove_from_middle_across_wraparound() {
        // Logical [3, 4, 5, 6] occupies physical slots 2, 3, 0, 1.
        let mut q = queue_of(&[1, 2, 3]);
        q.dequeue();
        q.dequeue();
        for i in [4, 5, 6] {
            q.enqueue(i).unwrap();
        }
        assert!(q.remove(4)); // the gap crosses the physical end of the ring
        assert_eq!(contents(&q), [3, 5, 6]);
        q.enqueue(7).unwrap();
        assert_eq!(contents(&q), [3, 5, 6, 7]);
        assert_eq!(q.dequeue(), Some(3));
    }

    #[test]
    fn removed_item_can_be_requeued() {
        let mut q = queue_of(&[1, 2]);
        q.remove(1);
        q.enqueue(1).unwrap();
        assert_eq!(contents(&q), [2, 1]);
    }

    #[test]
    fn freed_slot_is_reusable_after_remove_when_full() {
        let mut q = queue_of(&[1, 2, 3, 4]);
        q.remove(2);
        q.enqueue(5).unwrap();
        assert_eq!(contents(&q), [1, 3, 4, 5]);
    }

    // --- contains ---

    #[test]
    fn contains_reflects_membership() {
        let mut q = queue_of(&[1, 2]);
        assert!(q.contains(1));
        assert!(!q.contains(3));
        q.dequeue();
        assert!(!q.contains(1));
    }

    // --- invariant stress: model check against a Vec ---

    #[test]
    fn matches_vec_model_over_mixed_operations() {
        // Deterministic pseudo-random sequence (LCG): no external crates.
        let mut q = RunQueue::<u64, 8>::new();
        let mut model: Vec<u64> = Vec::new();
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..10_000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let id = (state >> 33) % 12;
            match (state >> 60) % 3 {
                0 => {
                    let expected = if model.contains(&id) {
                        Err(RunQueueError::AlreadyQueued)
                    } else if model.len() == 8 {
                        Err(RunQueueError::Full)
                    } else {
                        model.push(id);
                        Ok(())
                    };
                    assert_eq!(q.enqueue(id), expected);
                }
                1 => {
                    let expected = if model.is_empty() {
                        None
                    } else {
                        Some(model.remove(0))
                    };
                    assert_eq!(q.dequeue(), expected);
                }
                _ => {
                    let expected = model.iter().position(|&x| x == id).map(|p| model.remove(p));
                    assert_eq!(q.remove(id), expected.is_some());
                }
            }
            assert_eq!(q.iter().collect::<Vec<_>>(), model);
            assert_eq!(q.len(), model.len());
        }
    }

    impl Queue {
        /// Test helper: dequeue on a clone, leaving `self` untouched.
        fn dequeue_clone(&self) -> Option<u64> {
            self.clone().dequeue()
        }
    }
}
