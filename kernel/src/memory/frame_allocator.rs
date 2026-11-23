//! Global physical frame allocator.
//!
//! Wraps [`BitmapFrameAllocator`] in a static storage instance guarded by an
//! `AtomicBool` initialisation sentinel.
//!
//! # Threading model (Phase 1)
//!
//! The kernel runs single-core with interrupts disabled during all allocator
//! calls. The `unsafe` functions below rely on the caller honouring this
//! contract. Phase 2 will replace the bare `static mut` access with a
//! spinlock-protected wrapper.
//!
//! # Usage
//!
//! ```ignore
//! // In kernel_main, after memory::init():
//! // SAFETY: called once, single-threaded, interrupts disabled.
//! unsafe { frame_allocator::init(memory_map) };
//!
//! // Anywhere after init():
//! let frame = unsafe { frame_allocator::allocate() }.expect("out of physical memory");
//! // ... use frame ...
//! unsafe { frame_allocator::deallocate(frame) };
//! ```

use core::sync::atomic::{AtomicBool, Ordering};

use ferrous_boot_info::{BitmapFrameAllocator, KERNEL_BITMAP_WORDS};

pub use ferrous_boot_info::{PhysFrame, PAGE_SIZE};

use super::MemoryMap;

// ---------------------------------------------------------------------------
// Global allocator instance
// ---------------------------------------------------------------------------

/// Sentinel: `true` after [`init`] completes successfully.
///
/// `Ordering::Release` on store / `Ordering::Acquire` on load ensures all
/// bitmap writes in `init` are visible to any reader that observes
/// `INITIALIZED == true`.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The kernel's physical frame allocator.
///
/// Stored as a `static mut` rather than `MaybeUninit` because
/// `BitmapFrameAllocator::new()` is `const fn` and produces an all-zero
/// value that matches BSS zero-initialisation. The 2 MiB bitmap therefore
/// lives in BSS and does not bloat the binary image.
///
/// # SAFETY invariant
///
/// - Before `INITIALIZED` is `true`: only [`init`] may mutate this.
/// - After `INITIALIZED` is `true`: only [`allocate`] and [`deallocate`]
///   may mutate this; callers guarantee single-threaded access.
#[allow(static_mut_refs)]
static mut ALLOCATOR: BitmapFrameAllocator<KERNEL_BITMAP_WORDS> = BitmapFrameAllocator::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` if the allocator has been initialised.
#[inline]
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// Initialise the global frame allocator from the parsed memory map.
///
/// Marks all immediately-usable (conventional) physical frames as free.
/// Reclaimable regions (bootloader code/data, ACPI tables) remain reserved
/// until a future reclamation pass (Phase 2+).
///
/// # Safety
///
/// - Must be called **exactly once**.
/// - Must be called **after** `memory::init()`.
/// - Must be called from a **single-threaded context** with interrupts disabled
///   (the standard early-boot environment).
///
/// Violating any of these invariants is undefined behaviour.
pub unsafe fn init(map: &MemoryMap) {
    debug_assert!(
        !INITIALIZED.load(Ordering::Relaxed),
        "frame_allocator::init() called more than once"
    );

    // SAFETY: single-threaded, interrupts disabled, INITIALIZED is still false
    // so no concurrent reader exists.
    #[allow(static_mut_refs)]
    ALLOCATOR.init_from_memory_map(map);

    // Release store: all writes to ALLOCATOR are visible after this.
    INITIALIZED.store(true, Ordering::Release);
}

/// Initialise the global frame allocator, but only mark frames **below**
/// `phys_limit` as free.
///
/// This is identical to [`init`] except that it never writes to the portion
/// of the 2 MiB BSS bitmap that corresponds to frames at or above
/// `phys_limit`.  In QEMU TCG mode, writing to the upper bitmap triggers
/// thousands of demand-page faults; this variant avoids that entirely for
/// smoke tests that only need low-memory frames.
///
/// # Safety
///
/// Same as [`init`]: called exactly once, single-threaded, interrupts disabled.
pub unsafe fn init_below(map: &MemoryMap, phys_limit: u64) {
    debug_assert!(
        !INITIALIZED.load(Ordering::Relaxed),
        "frame_allocator::init_below() called more than once"
    );

    #[allow(static_mut_refs)]
    ALLOCATOR.init_from_memory_map_below(map, phys_limit);

    INITIALIZED.store(true, Ordering::Release);
}

/// Allocates a single 4 KiB physical frame.
///
/// Returns `None` if all usable frames are exhausted (out-of-memory).
///
/// # Safety
///
/// - [`init`] must have been called before the first allocation.
/// - Must be called from a single-threaded context with interrupts disabled
///   (Phase 1 invariant; Phase 2 will add spinlock protection).
pub unsafe fn allocate() -> Option<PhysFrame> {
    debug_assert!(
        INITIALIZED.load(Ordering::Relaxed),
        "frame_allocator::allocate() called before init()"
    );
    // SAFETY: INITIALIZED guarantees init() ran. Caller guarantees single-
    // threaded access.
    #[allow(static_mut_refs)]
    ALLOCATOR.allocate()
}

/// Returns a physical frame to the free pool.
///
/// # Safety
///
/// - [`init`] must have been called.
/// - `frame` must have been obtained from a prior call to [`allocate`] and
///   must not have been deallocated already (no double-free).
/// - Must be called from a single-threaded context with interrupts disabled
///   (Phase 1 invariant).
pub unsafe fn deallocate(frame: PhysFrame) {
    debug_assert!(
        INITIALIZED.load(Ordering::Relaxed),
        "frame_allocator::deallocate() called before init()"
    );
    // SAFETY: see allocate().
    #[allow(static_mut_refs)]
    ALLOCATOR.deallocate(frame);
}

/// Marks a physical address range as reserved, removing any free frames in
/// that range from the allocation pool.
///
/// Safe to call after `init` to protect regions that were initially marked
/// usable but should not be allocated (e.g., the kernel image if it resides
/// in conventional memory).
///
/// # Safety
///
/// - [`init`] must have been called.
/// - Must be called from a single-threaded context with interrupts disabled.
pub unsafe fn mark_reserved(phys_start: u64, size_bytes: u64) {
    debug_assert!(
        INITIALIZED.load(Ordering::Relaxed),
        "frame_allocator::mark_reserved() called before init()"
    );
    #[allow(static_mut_refs)]
    ALLOCATOR.mark_reserved(phys_start, size_bytes);
}

/// Returns the number of free physical frames, or `None` if not initialised.
pub fn free_frames() -> Option<usize> {
    if INITIALIZED.load(Ordering::Acquire) {
        // SAFETY: INITIALIZED guarantees ALLOCATOR is fully initialised and
        // we only take a shared reference (read-only).
        #[allow(static_mut_refs)]
        Some(unsafe { ALLOCATOR.free_frames() })
    } else {
        None
    }
}

/// Returns the total number of frames the allocator tracks (`KERNEL_BITMAP_WORDS * 64`),
/// or `None` if not initialised.
pub fn total_frames() -> Option<usize> {
    if INITIALIZED.load(Ordering::Acquire) {
        #[allow(static_mut_refs)]
        Some(unsafe { ALLOCATOR.total_frames() })
    } else {
        None
    }
}

/// Returns the number of allocated frames, or `None` if not initialised.
pub fn allocated_frames() -> Option<usize> {
    if INITIALIZED.load(Ordering::Acquire) {
        #[allow(static_mut_refs)]
        Some(unsafe { ALLOCATOR.allocated_frames() })
    } else {
        None
    }
}
