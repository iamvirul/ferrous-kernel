//! Kernel stack definition for x86-64.
//!
//! This module provides the `KernelStack` type and the size constants used
//! for the kernel's primary execution stack. The stack grows **downward**
//! on x86-64: RSP starts at the highest valid address (`top()`) and moves
//! toward the lowest address (`bottom()`) as calls and pushes occur.
//!
//! # Layout (64 KiB example)
//!
//! ```text
//! High address ──────────────────────────────────────────── top()  ← RSP initial
//!              │                                          │
//!              │        usable stack space (60 KiB)      │
//!              │                                          │
//!              ├──────────────────────────────────────────┤
//!              │      soft guard region (4 KiB)           │  ← future: unmapped page
//! Low address  ──────────────────────────────────────────── bottom()
//! ```
//!
//! The bottom 4 KiB is reserved as a soft guard region. It cannot be
//! enforced without page-table control (Task 1.3.3), but is zeroed at
//! startup and documented here so the invariant is clear.
//!
//! # Phase notes
//!
//! During Phase 1 the `KERNEL_STACK` static lives in `boot/src/main.rs`
//! because the bootloader and kernel run in the same address space. When
//! the kernel becomes a separate ELF binary, that static moves here and
//! the linker script exports `__stack_top` / `__stack_bottom`.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Total size of the kernel primary stack in bytes (64 KiB).
///
/// Breakdown:
/// - 60 KiB usable stack depth
/// - 4 KiB soft guard region at the bottom (enforced by page tables in Phase 1.3)
pub const KERNEL_STACK_SIZE: usize = 64 * 1024;

/// Size of the soft guard region at the bottom of the stack (4 KiB = one page).
///
/// This region is left zeroed. Once page table management is in place
/// (Task 1.3.3) it will be mapped as non-present to catch stack overflows.
pub const KERNEL_STACK_GUARD_SIZE: usize = 4 * 1024;

/// Usable stack depth = total size minus the guard region.
pub const KERNEL_STACK_USABLE_SIZE: usize = KERNEL_STACK_SIZE - KERNEL_STACK_GUARD_SIZE;

// ---------------------------------------------------------------------------
// KernelStack type
// ---------------------------------------------------------------------------

/// A statically allocated, 16-byte-aligned kernel stack.
///
/// The size parameter `SIZE` is the total allocation in bytes, including the
/// soft guard region at the bottom.
///
/// # Usage
///
/// ```ignore
/// use kernel::arch::x86_64::stack::{KernelStack, KERNEL_STACK_SIZE};
///
/// static KERNEL_STACK: KernelStack<KERNEL_STACK_SIZE> = KernelStack::new();
///
/// // In entry asm, after validating boot info:
/// let rsp = KERNEL_STACK.top() as u64;
/// // ... set RSP via asm ...
/// ```
///
/// # Alignment
///
/// The struct carries `#[repr(C, align(16))]` so the compiler places it on a
/// 16-byte boundary. The `top()` pointer is therefore also 16-byte aligned,
/// satisfying the x86-64 ABI requirement before the first `call` instruction.
#[repr(C, align(16))]
pub struct KernelStack<const SIZE: usize> {
    data: [u8; SIZE],
}

impl<const SIZE: usize> KernelStack<SIZE> {
    /// Create a zeroed `KernelStack`. Usable as a `const` / `static` initialiser.
    pub const fn new() -> Self {
        Self { data: [0u8; SIZE] }
    }

    /// Return a pointer to the top of the stack (highest valid address).
    ///
    /// This is the value RSP should be set to before the kernel starts
    /// executing. On x86-64 the stack grows downward, so the first `push`
    /// or `call` will decrement RSP by 8 before writing, placing the value
    /// at `top - 8`.
    ///
    /// The returned pointer is one-past-the-end of the backing array, which
    /// is a valid (non-dereferenceable) address under Rust's pointer rules.
    pub fn top(&self) -> *const u8 {
        // SAFETY: `self.data.len() == SIZE`. Adding `SIZE` to the base gives
        // a pointer one past the end — always valid to form, never to
        // dereference. RSP is set to this value; the CPU decrements it before
        // each write, so no out-of-bounds write occurs at the top.
        unsafe { self.data.as_ptr().add(SIZE) }
    }

    /// Return a pointer to the bottom of the stack (lowest address).
    ///
    /// The bottom `KERNEL_STACK_GUARD_SIZE` bytes are the soft guard region.
    /// Once page-table management is implemented, this page will be marked
    /// non-present to catch stack overflows.
    pub fn bottom(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// Physical address of the stack top, suitable for loading into RSP.
    pub fn top_addr(&self) -> usize {
        self.top() as usize
    }

    /// Physical address of the stack bottom.
    pub fn bottom_addr(&self) -> usize {
        self.bottom() as usize
    }

    /// Total allocated size in bytes (includes guard region).
    pub const fn size() -> usize {
        SIZE
    }
}
