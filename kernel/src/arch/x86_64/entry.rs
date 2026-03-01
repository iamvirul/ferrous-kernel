//! x86-64 kernel entry point.
//!
//! This module will contain the `kernel_entry` function once the bootloader
//! is extended to load the kernel as a separate ELF binary (Phase 1+).
//!
//! For Phase 1.1.2, `kernel_entry` lives in the bootloader binary (since
//! both are compiled together as a single UEFI application). The structure
//! here documents what `kernel_entry` must do when it moves to this module:
//!
//! 1. Receive `*const KernelBootInfo` in RDI (SysV AMD64 ABI).
//! 2. Validate the magic and version fields — halt on mismatch.
//! 3. Zero the BSS section using linker-provided `__bss_start`/`__bss_end`.
//! 4. Call `kernel_main(boot_info: &'static KernelBootInfo)`.
//!
//! # Safety requirements for `kernel_entry`
//!
//! - RSP must point to a valid stack (bootstrap stack, 16-byte aligned).
//! - Interrupts must be disabled (`cli` executed before the call).
//! - UEFI boot services must have already exited.
//! - The pointer in RDI must be non-null and point to a valid `KernelBootInfo`.
//!
//! # Future linker script symbols
//!
//! The following symbols must be exported by `kernel.ld`:
//! - `__bss_start` — start of the `.bss` section
//! - `__bss_end`   — end of the `.bss` section
//! - `__stack_top` — top of the bootstrap stack (RSP initial value)

// Placeholder: entry logic is currently in boot/src/main.rs.
// This module will be populated when the kernel becomes a separate binary.
