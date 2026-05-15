//! Frame-pointer stack walker for panic diagnostics (Phase 1.4.2).
//!
//! Walks the x86-64 call stack via the RBP frame-pointer chain and writes raw
//! return addresses to COM1.
//!
//! # Best-effort semantics
//!
//! Frame-pointer availability depends on how each function was compiled:
//!
//! - **Debug builds (`opt-level = 0`)**: most functions emit a standard
//!   `push rbp; mov rbp, rsp` prologue naturally, so the chain is usually
//!   intact for Ferrous code.
//! - **Release builds (`opt-level ≥ 2`)**: the compiler may omit frame
//!   pointers for leaf functions and short functions.  Forcing
//!   `force-frame-pointers=yes` would fix this but it increases stack depth
//!   across the entire binary (including rebuilt `core`/`alloc` via
//!   `build-std`), which in UEFI debug builds triggers a `#DE: Divide Error`
//!   exception in the frame allocator initialisation.  We therefore accept
//!   best-effort traces rather than mandating the flag.
//!
//! If no valid frames are found, the walker prints
//! `(no frames — frame pointers unavailable at this depth)` and returns
//! without aborting — the caller (panic handler or smoke test) continues
//! normally.
//!
//! # Output format
//!
//! ```text
//! [TRACE] Stack trace (RBP chain):
//!   #0  0x000000000001a2b3
//!   #1  0x000000000001789c
//!   #2  0x0000000000012345
//!   (end of trace)
//! ```
//!
//! Resolve raw addresses offline:
//!
//! ```bash
//! addr2line -e target/x86_64-unknown-uefi/debug/ferrous-boot.efi 0x1a2b3
//! ```
//!
//! # Frame-pointer walk mechanics
//!
//! On every non-leaf function call the x86-64 SysV ABI prologue executes:
//!
//! ```asm
//! push rbp          ; save caller's frame pointer
//! mov  rbp, rsp     ; RBP now points to [saved_rbp, return_addr, locals…]
//! ```
//!
//! This creates a singly-linked list of activation records:
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │ [rbp + 0]   saved RBP  (previous frame) │──► ...
//! │ [rbp + 8]   return address              │
//! │ [rbp + 16…] local variables             │
//! └─────────────────────────────────────────┘
//! ```
//!
//! The walk starts at the current RBP (read via inline asm), dereferences
//! `[rbp+8]` for the return address, then advances to `[rbp]` for the next
//! frame. It stops when RBP is null, misaligned, or [`MAX_FRAMES`] have been
//! printed.
//!
//! # Safety contract
//!
//! - `force-frame-pointers = true` must be set for all built crates. Ferrous's
//!   workspace `Cargo.toml` guarantees this.
//! - Every stack page must be mapped. In Phase 1 the entire first GiB is
//!   identity-mapped, and the kernel stack lives in BSS well within that range.
//! - The walk is not async-signal-safe. Call only from a single-threaded panic
//!   context (always true in Phase 1 — single-core, interrupts disabled).

/// Maximum frames to unwind before giving up.
///
/// Limits output length and prevents an infinite walk when the frame chain is
/// corrupt (e.g. in debug code that omits the prologue for a leaf function).
pub const MAX_FRAMES: usize = 16;

/// Walk the RBP frame-pointer chain and print return addresses to COM1.
///
/// Reads the current hardware RBP register, then follows the chain of saved
/// frame pointers until it encounters a null/misaligned RBP, a zero return
/// address (bottom of the bootstrap asm stub), or [`MAX_FRAMES`] frames.
///
/// # Safety
///
/// - Frame pointers must be enabled (workspace profile sets
///   `force-frame-pointers = true`).
/// - The entire kernel stack must be identity-mapped and readable. This is
///   guaranteed in Phase 1.
/// - Must be called from a single-threaded, non-reentrant context.
pub unsafe fn print_stack_trace() {
    crate::serial_write_str("[TRACE] Stack trace (RBP chain):\r\n");

    // Capture current RBP via inline assembly.
    //
    // SAFETY: reading RBP is always valid at ring-0; `nomem, nostack` prevents
    // the compiler from reordering or merging memory operations around this read.
    let mut rbp: u64;
    core::arch::asm!(
        "mov {out}, rbp",
        out = out(reg) rbp,
        options(nomem, nostack, preserves_flags),
    );

    let mut frame = 0usize;

    while frame < MAX_FRAMES {
        // Null RBP — reached the bottom of the call stack (the bootstrap asm
        // sets `xor rbp, rbp` before the first Rust frame).
        if rbp == 0 {
            break;
        }

        // Misaligned RBP — frame chain is corrupt; stop safely.
        if rbp % 8 != 0 {
            crate::serial_write_str("  (corrupt frame pointer — walk aborted)\r\n");
            break;
        }

        // Read return address at [rbp + 8].
        //
        // SAFETY: rbp is non-null, 8-byte aligned, within the identity-mapped
        // kernel stack.  [rbp+8] is always written by the `call` instruction's
        // implicit push of the next-instruction address.
        let ret_addr: u64 = unsafe { *((rbp as *const u64).add(1)) };

        // Zero return address means the call chain bottomed out (bootstrap
        // asm stub uses `xor rbp, rbp` so the prev-frame read would be 0).
        if ret_addr == 0 {
            break;
        }

        // Emit: `  #N  0x<16-hex-digit addr>\r\n`
        crate::serial_write_str("  #");
        crate::serial_write_usize(frame);
        crate::serial_write_str("  0x");
        crate::serial_write_usize_hex(ret_addr as usize);
        crate::serial_write_str("\r\n");

        // Advance: [rbp] holds the previous frame's RBP.
        //
        // SAFETY: rbp is valid (asserted above); the saved-RBP slot at [rbp]
        // is either a valid prior frame pointer or zero (bottom of stack).
        rbp = unsafe { *(rbp as *const u64) };
        frame += 1;
    }

    if frame == 0 {
        crate::serial_write_str("  (no frames — frame pointers unavailable at this depth)\r\n");
    } else {
        crate::serial_write_str("  (end of trace)\r\n");
    }
}
