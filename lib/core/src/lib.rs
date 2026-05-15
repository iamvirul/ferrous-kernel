//! `ferrous-core` — shared kernel utilities for Ferrous (Phase 1.4.3+).
//!
//! A `no_std` library consumed by both the UEFI bootloader (`ferrous-boot`)
//! and the bare-metal kernel binary (`ferrous-kernel`).  It provides
//! primitives that would otherwise need to be duplicated across crates.
//!
//! # Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`macros`] | Kernel assertion and debug macros (`kassert!`, `kassert_eq!`, `kdebug_assert!`, `kunreachable!`, …) |
//!
//! # `no_std` guarantee
//!
//! This crate never depends on `std`, `libc`, or any OS-specific API.  All
//! items are safe to use in interrupt handlers, early boot code, and any
//! context where the heap may not be available.

#![no_std]
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod macros;
