//! Ferrous Kernel — library crate root.
//!
//! Exposes kernel subsystems as a library so the bootloader can call
//! smoke tests during early boot (Steps 14–15 of `kernel_main`).
//!
//! # Exposed modules
//!
//! | Module    | Contents                                               |
//! |-----------|--------------------------------------------------------|
//! | [`memory`]| Physical frame allocator, paging, address space mgmt  |
//! | [`task`]  | Task and process data structures                       |

#![no_std]

extern crate alloc;

pub mod memory;
pub mod task;
