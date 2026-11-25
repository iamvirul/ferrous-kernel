//! Interrupt-disable nesting bookkeeping (pure; no hardware access).
//!
//! The kernel disables interrupts for every critical section (e.g. while an
//! `IrqSpinLock` is held), and critical sections nest. If each scope simply
//! restored the interrupt flag it saw on entry, releasing scopes out of
//! order would re-enable interrupts while an inner lock is still held:
//!
//! ```text
//! IF = 1
//! lock A   (saved: on)   IF = 0
//! lock B   (saved: off)
//! drop A   → restore "on" → IF = 1   ← B still held: an IRQ that takes B
//!                                       now spins forever on this CPU
//! drop B
//! ```
//!
//! [`IrqNesting`] instead counts active scopes and remembers only the
//! state from *before the outermost one*. Interrupts come back on only when
//! the last scope exits, whatever the release order. This is the xv6
//! `push_off` / `pop_off` scheme. The kernel keeps one instance per CPU and
//! touches it only while interrupts are disabled.

/// Per-CPU count of active interrupt-disabled scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IrqNesting {
    depth: usize,
    enabled_before_outermost: bool,
}

impl IrqNesting {
    /// No active scopes.
    pub const fn new() -> Self {
        Self {
            depth: 0,
            enabled_before_outermost: false,
        }
    }

    /// Record entry into a scope. `enabled_before` is the interrupt flag as
    /// it was just before this scope disabled interrupts.
    ///
    /// # Panics
    ///
    /// If the nesting depth would overflow `usize` (an unbalanced-enter bug).
    pub fn enter(&mut self, enabled_before: bool) {
        if self.depth == 0 {
            self.enabled_before_outermost = enabled_before;
        }
        self.depth = self
            .depth
            .checked_add(1)
            .expect("IrqNesting: depth overflow (unbalanced enter)");
    }

    /// Record exit from a scope. Returns `true` exactly when this was the
    /// last active scope *and* interrupts were enabled before the first one,
    /// i.e. when the caller must now re-enable interrupts.
    ///
    /// # Panics
    ///
    /// If there is no active scope (an unbalanced-exit bug).
    pub fn exit(&mut self) -> bool {
        assert!(self.depth > 0, "IrqNesting: exit without matching enter");
        self.depth -= 1;
        self.depth == 0 && self.enabled_before_outermost
    }

    /// Number of active scopes.
    pub const fn depth(&self) -> usize {
        self.depth
    }

    /// Whether interrupts were enabled before the outermost active scope.
    ///
    /// # Panics
    ///
    /// If no scope is active (the value is meaningless then).
    pub fn outermost_enabled(&self) -> bool {
        assert!(self.depth > 0, "IrqNesting: no active scope");
        self.enabled_before_outermost
    }

    /// Replace the outermost scope's "were interrupts enabled" bit and
    /// return the old one.
    ///
    /// Used by the context switch (ADR-0009 §4): the bit belongs to the
    /// *task* that opened the outermost scope, so on a switch the outgoing
    /// task's bit is saved and the incoming task's is installed. The depth
    /// is unchanged.
    ///
    /// # Panics
    ///
    /// If no scope is active.
    pub fn replace_outermost_enabled(&mut self, enabled: bool) -> bool {
        assert!(self.depth > 0, "IrqNesting: no active scope");
        core::mem::replace(&mut self.enabled_before_outermost, enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_no_scopes() {
        assert_eq!(IrqNesting::new().depth(), 0);
    }

    #[test]
    fn single_scope_restores_enabled() {
        let mut n = IrqNesting::new();
        n.enter(true);
        assert!(n.exit());
    }

    #[test]
    fn single_scope_keeps_disabled() {
        let mut n = IrqNesting::new();
        n.enter(false);
        assert!(!n.exit());
    }

    #[test]
    fn nested_in_order_reenables_only_at_outermost_exit() {
        let mut n = IrqNesting::new();
        n.enter(true); // A
        n.enter(false); // B: IF was already off
        assert!(!n.exit()); // B
        assert!(n.exit()); // A
    }

    #[test]
    fn out_of_order_release_never_reenables_while_a_scope_is_active() {
        // The CodeRabbit scenario: lock A, lock B, drop A, drop B.
        let mut n = IrqNesting::new();
        n.enter(true); // A saw IF = 1
        n.enter(false); // B saw IF = 0
        assert!(!n.exit(), "dropping A first must keep IRQs off: B is held");
        assert_eq!(n.depth(), 1);
        assert!(n.exit(), "the last exit restores the pre-A state");
    }

    #[test]
    fn inner_state_never_overrides_outermost() {
        // An inner scope entered with IF = 1 (impossible in practice, since
        // the outer scope cleared it) still must not decide the outcome.
        let mut n = IrqNesting::new();
        n.enter(false);
        n.enter(true);
        assert!(!n.exit());
        assert!(!n.exit());
    }

    #[test]
    fn state_resets_between_outermost_scopes() {
        let mut n = IrqNesting::new();
        n.enter(true);
        assert!(n.exit());
        n.enter(false);
        assert!(!n.exit(), "a new outermost scope records its own state");
    }

    #[test]
    fn replace_outermost_swaps_the_bit_and_keeps_depth() {
        let mut n = IrqNesting::new();
        n.enter(false); // outgoing task's switch scope: it had IF = 0
        assert!(!n.replace_outermost_enabled(true), "returns the old bit");
        assert!(n.outermost_enabled());
        assert_eq!(n.depth(), 1);
        assert!(n.exit(), "the incoming task's bit decides re-enabling");
    }

    #[test]
    fn replaced_bit_governs_the_final_exit_even_when_nested() {
        let mut n = IrqNesting::new();
        n.enter(true);
        n.enter(false);
        n.replace_outermost_enabled(false);
        assert!(!n.exit());
        assert!(!n.exit(), "outermost bit was replaced with 'disabled'");
    }

    #[test]
    #[should_panic(expected = "no active scope")]
    fn replace_without_a_scope_panics() {
        IrqNesting::new().replace_outermost_enabled(true);
    }

    #[test]
    #[should_panic(expected = "no active scope")]
    fn outermost_without_a_scope_panics() {
        let _ = IrqNesting::new().outermost_enabled();
    }

    #[test]
    #[should_panic(expected = "exit without matching enter")]
    fn unbalanced_exit_panics() {
        IrqNesting::new().exit();
    }
}
