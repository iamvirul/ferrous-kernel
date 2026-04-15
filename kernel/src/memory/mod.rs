//! Physical memory management subsystem.
//!
//! This module provides the kernel's authoritative view of physical memory,
//! parsed from the bootloader-provided [`KernelMemoryMap`] at startup.
//!
//! # Usage
//!
//! During early kernel initialisation (before the allocator runs), call
//! [`init`] exactly once with the memory map from [`KernelBootInfo`]:
//!
//! ```ignore
//! // SAFETY: called once, single-threaded, interrupts disabled.
//! let map = unsafe { memory::init(&boot_info.memory_map) }
//!     .expect("memory map parse failed");
//! ```
//!
//! Thereafter any kernel subsystem can call [`get`] to borrow the map:
//!
//! ```ignore
//! let map = memory::get().expect("memory not initialised");
//! for region in map.usable_regions() { ... }
//! ```
//!
//! # Re-exports
//!
//! The parsing types ([`MemoryMap`], [`MemoryRegionKind`], [`MemoryStats`],
//! [`ParseError`]) live in [`ferrous_boot_info`] so they can be tested on the
//! host without targeting `x86_64-unknown-none`. They are re-exported here
//! for ergonomic access within the kernel.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use ferrous_boot_info::KernelMemoryMap;

pub use ferrous_boot_info::{MemoryMap, MemoryRegionKind, MemoryStats, ParseError};

// ---------------------------------------------------------------------------
// Global memory map
// ---------------------------------------------------------------------------

/// Sentinel: set to `true` after [`init`] completes successfully.
///
/// Uses `Ordering::Release` on write and `Ordering::Acquire` on read so that
/// all writes to `MEMORY_MAP` are visible to any thread that observes
/// `INITIALIZED == true`.  In Phase 1 the kernel is single-core and
/// interrupts are disabled during init, so the atomic is purely defensive.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The global physical memory map.
///
/// # SAFETY invariant
///
/// This static is in one of two states:
///
/// 1. **Uninitialised** (`INITIALIZED == false`): only [`init`] may write it.
/// 2. **Initialised** (`INITIALIZED == true`): immutable from this point on;
///    safe to take shared references via [`get`].
///
/// No mutable reference is ever taken after [`init`] sets `INITIALIZED`.
// SAFETY: written once in `init()` before INITIALIZED is set to true.
#[allow(static_mut_refs)]
static mut MEMORY_MAP: MaybeUninit<MemoryMap> = MaybeUninit::uninit();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the global kernel memory map.
///
/// Parses `source`, stores the result in a `'static` slot, and returns a
/// reference to it.  This reference is valid for the lifetime of the kernel.
///
/// # Errors
///
/// Propagates any [`ParseError`] from [`MemoryMap::parse`].
///
/// # Safety
///
/// - Must be called **exactly once**.
/// - Must be called **before** any call to [`get`].
/// - Must be called from a **single-threaded context** with interrupts
///   disabled (the standard early-boot environment).
///
/// Violating any of these invariants is undefined behaviour.
pub unsafe fn init(source: &KernelMemoryMap) -> Result<&'static MemoryMap, ParseError> {
    debug_assert!(
        !INITIALIZED.load(Ordering::Relaxed),
        "memory::init() called more than once"
    );

    let map = MemoryMap::parse(source)?;

    // SAFETY: single-threaded, interrupts disabled, INITIALIZED is still
    // false so no concurrent reader exists.
    #[allow(static_mut_refs)]
    MEMORY_MAP.write(map);

    // Release store: all writes to MEMORY_MAP are visible after this.
    INITIALIZED.store(true, Ordering::Release);

    // SAFETY: MEMORY_MAP was fully written above.
    #[allow(static_mut_refs)]
    Ok(MEMORY_MAP.assume_init_ref())
}

/// Returns a shared reference to the global memory map.
///
/// Returns `None` if [`init`] has not been called yet.  After a successful
/// [`init`] call this function always returns `Some`.
pub fn get() -> Option<&'static MemoryMap> {
    if INITIALIZED.load(Ordering::Acquire) {
        // SAFETY: MEMORY_MAP is fully initialised when INITIALIZED is true
        // and is never written again after that point.
        #[allow(static_mut_refs)]
        Some(unsafe { MEMORY_MAP.assume_init_ref() })
    } else {
        None
    }
}
