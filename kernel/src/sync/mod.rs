//! Kernel synchronisation primitives.
//!
//! | Primitive | Use |
//! |-----------|-----|
//! | [`IrqSpinLock`] | Data shared with interrupt handlers (e.g. the run queue) |

mod irq_spin_lock;

pub use irq_spin_lock::{IrqSpinLock, IrqSpinLockGuard};
