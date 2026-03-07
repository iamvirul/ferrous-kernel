//! Kernel device drivers.
//!
//! Each sub-module provides a safe abstraction over a hardware device or
//! firmware interface. All unsafe I/O is confined within the sub-module;
//! public APIs are safe to call from kernel code (given the invariants
//! documented on each type's constructor / initialiser).

pub mod serial;
