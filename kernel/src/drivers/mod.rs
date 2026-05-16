//! Kernel device drivers (Phase 1.4.4+).
//!
//! Each sub-module provides a safe abstraction over a hardware device or
//! firmware interface. All `unsafe` I/O is confined within the sub-module;
//! public APIs are safe to call from kernel code (given the invariants
//! documented on each type's constructor / initialiser).
//!
//! # Modules
//!
//! | Module | Device | Phase |
//! |--------|--------|-------|
//! | [`serial`] | 16550-compatible UART (COM1–COM4) | 1.1.3 / 1.4.4 |
//! | [`console`] | [`Console`](console::Console) trait + [`SerialConsole`](console::SerialConsole) | 1.4.4 |

pub mod console;
pub mod serial;
