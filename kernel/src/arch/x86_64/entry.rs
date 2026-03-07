//! x86-64 kernel entry point.
//!
//! This module will contain the `kernel_entry` function once the bootloader
//! is extended to load the kernel as a separate ELF binary (Phase 2+).
//!
//! For Phase 1, `kernel_entry` lives in the bootloader binary (since both
//! are compiled as a single UEFI application). The structure here documents
//! what `kernel_entry` must do when it moves to this module:
//!
//! 1. Receive `*const KernelBootInfo` in RDI (SysV AMD64 ABI).
//! 2. Validate the magic and version fields — halt on mismatch.
//! 3. Zero the BSS section using linker-provided `__bss_start`/`__bss_end`.
//! 4. Switch RSP to the kernel's primary stack (see [`stack`] module).
//! 5. Call `kernel_main(boot_info: &'static KernelBootInfo)`.
//!
//! # Stack switch (step 4)
//!
//! After validating `boot_info`, the entry point must switch from the
//! bootstrap stack (set up by the bootloader) to the kernel's own primary
//! stack before calling into Rust code that may have deep call chains.
//!
//! ```text
//! ; After BSS zero, before kernel_main:
//! mov rsp, [KERNEL_STACK_TOP]   ; switch to 64 KiB kernel stack
//! xor rbp, rbp                  ; clear frame pointer (no caller)
//! call kernel_main              ; RSP now points to kernel stack
//! ```
//!
//! # Safety requirements for `kernel_entry`
//!
//! - RSP must initially point to a valid bootstrap stack (16-byte aligned).
//! - Interrupts must be disabled (`cli` executed before the call).
//! - UEFI boot services must have already exited.
//! - The pointer in RDI must be non-null and point to a valid `KernelBootInfo`.
//!
//! # Future linker script symbols
//!
//! The following symbols must be exported by `kernel.ld`:
//! - `__bss_start`   — start of the `.bss` section
//! - `__bss_end`     — end of the `.bss` section
//! - `__stack_top`   — top of the kernel primary stack (RSP initial value)
//! - `__stack_bottom`— bottom of the kernel primary stack (guard page base)

// Placeholder: entry logic is currently in boot/src/main.rs.
// This module will be populated when the kernel becomes a separate binary.
