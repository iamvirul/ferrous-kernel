//! Kernel heap allocator (Phase 1.3.5).
//!
//! Provides [`KernelHeapAllocator`]: a `GlobalAlloc`-implementing wrapper
//! around `linked_list_allocator::LockedHeap`.  In Phase 1 the heap is
//! managed directly from the boot binary; this type is the canonical
//! implementation for the standalone kernel binary introduced in Phase 2.
//!
//! # Design
//!
//! - **Backing storage**: a BSS-resident `[u8; N]` buffer whose address is
//!   passed to [`KernelHeapAllocator::init`] at boot time.  BSS costs nothing
//!   in the binary image and is zero-initialised before the first instruction.
//! - **Allocator algorithm**: first-fit linked list (`linked_list_allocator`).
//!   Appropriate for Phase 1–2; replace with a slab allocator when SMP and
//!   high-frequency small allocations make contention measurable.
//! - **Thread safety**: `LockedHeap` uses a spin-lock.  Safe for Phase 1
//!   (single-core, interrupts disabled during boot).  In Phase 3+ the lock
//!   must not be held across interrupt handlers that may themselves allocate.
//!
//! # Initialisation
//!
//! ```ignore
//! // Declared in the kernel binary crate:
//! #[global_allocator]
//! static ALLOCATOR: KernelHeapAllocator = KernelHeapAllocator::empty();
//!
//! // Called once in kernel_main, before any heap use:
//! unsafe { ALLOCATOR.init(heap_start, heap_size); }
//! ```
//!
//! # Future work
//!
//! | Phase | Planned change |
//! |-------|----------------|
//! | 3     | Per-CPU slab caches for common sizes (8 B – 4 KiB) |
//! | 4     | NUMA-aware allocation; zone-based frame reservations |
//! | 5+    | Kernel-object allocator integrated with the capability system |

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

use linked_list_allocator::LockedHeap;

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// The kernel's global heap allocator.
///
/// Wraps [`linked_list_allocator::LockedHeap`] behind a safe public API.
/// The allocator starts uninitialised; call [`init`][Self::init] once during
/// early boot before the first allocation.
///
/// # Safety invariant
///
/// `ALLOCATOR.init(start, size)` must be called exactly once with a valid,
/// writable region before any call to `alloc` or `dealloc`.  Calling `alloc`
/// on an uninitialised `KernelHeapAllocator` will return a null pointer,
/// which Rust's allocator contract treats as an allocation failure and will
/// typically abort.
pub struct KernelHeapAllocator {
    inner: LockedHeap,
}

impl KernelHeapAllocator {
    /// Creates a new, uninitialised allocator.
    ///
    /// No memory is managed until [`init`][Self::init] is called.  This is a
    /// `const fn` so the allocator can be placed in a `static`.
    pub const fn empty() -> Self {
        Self {
            inner: LockedHeap::empty(),
        }
    }

    /// Initialise the allocator with a backing memory region.
    ///
    /// # Arguments
    ///
    /// * `heap_start` — pointer to the first byte of the heap region.
    /// * `heap_size`  — total number of bytes in the region.
    ///
    /// # Safety
    ///
    /// - `heap_start` must be valid for reads and writes of `heap_size` bytes.
    /// - The region `[heap_start, heap_start + heap_size)` must not overlap
    ///   any other live allocation or static.
    /// - Must be called exactly **once** before any heap allocation.
    /// - Must be called from a **single-threaded** context (interrupts
    ///   disabled), which is the normal Phase 1 early-boot environment.
    pub unsafe fn init(&self, heap_start: *mut u8, heap_size: usize) {
        // SAFETY: caller guarantees the region is valid, non-overlapping, and
        // that this is the first and only call.
        self.inner.lock().init(heap_start, heap_size);
    }

    /// Returns the number of free bytes currently available in the heap.
    ///
    /// Useful for smoke tests and diagnostic output.  Takes the spin-lock
    /// briefly; do not call from interrupt context.
    pub fn free_bytes(&self) -> usize {
        self.inner.lock().free()
    }

    /// Returns the total size of the heap region (free + allocated).
    pub fn total_bytes(&self) -> usize {
        self.inner.lock().size()
    }
}

// ---------------------------------------------------------------------------
// GlobalAlloc implementation
// ---------------------------------------------------------------------------

/// SAFETY: `LockedHeap` is `Send + Sync` (guarded by a spin-lock).  The
/// allocation and deallocation functions are logically thread-safe within the
/// single-core Phase 1 environment and will remain correct under SMP as long
/// as the spin-lock is not held across interrupt boundaries.
unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner
            .lock()
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was returned by `alloc` with the same `layout` and has
        // not been freed yet — guaranteed by the Rust allocator contract.
        self.inner
            .lock()
            .deallocate(NonNull::new_unchecked(ptr), layout);
    }
}
