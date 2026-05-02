//! Kernel page-table mapper — Phase 1 implementation.
//!
//! [`KernelPageTable`] is the Phase-1 page table structure for Ferrous.
//! It owns three [`PageTable`]s inline (PML4 + PDPT + PD) and sets up:
//!
//! - A **1 GiB identity map** covering physical addresses `[0, 1 GiB)` using
//!   2 MiB huge pages, so the kernel continues executing at the same virtual
//!   addresses after `CR3` is loaded.
//! - A **higher-half alias** at `0xFFFF_8000_0000_0000` pointing to the same
//!   physical range, establishing the higher-half window for Phase 2.
//!
//! # Page table layout
//!
//! ```text
//! PML4[0]   ──→ PDPT   (identity: VA [0, 512 GiB))
//! PML4[256] ──→ PDPT   (higher-half alias: VA [0xFFFF_8000_..., ...))
//! PDPT[0]   ──→ PD     (covers VA [0, 1 GiB))
//! PD[i]          ──→ 2 MiB huge page at physical i × 2 MiB
//! ```
//!
//! With 512 PD entries × 2 MiB each = **1 GiB** is fully covered.
//!
//! # Phase scope
//!
//! This struct is intentionally minimal.  Phase 2 will replace it with a
//! proper mapper that:
//! - Allocates page-table frames from the physical frame allocator.
//! - Supports 4 KiB pages for fine-grained mapping.
//! - Tracks virtual-address-space regions.
//! - Enforces execute-disable and write-protect on kernel code/data pages.

use super::{
    entry::{flags, PageTableEntry},
    table::PageTable,
};

/// PML4 index for the higher-half window (`0xFFFF_8000_0000_0000`).
///
/// Bits [47:39] of `0xFFFF_8000_0000_0000` decode to `0x100` = 256.
const HIGHER_HALF_PML4_IDX: usize = 256;

/// Number of 2 MiB pages needed to cover 1 GiB of physical memory.
///
/// A single PD (512 entries) × 2 MiB per entry = 1 GiB exactly.
const HUGE_PAGES_1GIB: usize = 512;

/// Size of a 2 MiB huge page in bytes.
const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024; // 2 MiB

/// The kernel's Phase-1 page table structure.
///
/// Holds a PML4, one PDPT, and one PD inline.  The entire struct is
/// `4 KiB`-aligned so the physical address of each sub-table can be stored
/// directly in the parent entry.
///
/// # Size
///
/// 3 × 4 KiB = **12 KiB**, resident in BSS (zero-initialised, no image bloat).
///
/// # Usage
///
/// ```ignore
/// // Place in a static (BSS):
/// static mut KERNEL_PAGE_TABLE: KernelPageTable = KernelPageTable::new();
///
/// // In kernel_main (single-threaded, interrupts disabled):
/// // SAFETY: single-threaded, interrupts disabled, UEFI identity mapping active.
/// let pml4_phys = unsafe {
///     KERNEL_PAGE_TABLE.init();
///     KERNEL_PAGE_TABLE.phys_pml4_addr()
/// };
/// unsafe { core::arch::asm!("mov cr3, {p}", p = in(reg) pml4_phys); }
/// ```
#[repr(C, align(4096))]
pub struct KernelPageTable {
    pml4: PageTable,
    pdpt: PageTable,
    pd: PageTable,
}

impl KernelPageTable {
    /// Create a zeroed, uninitialised page table (all entries absent).
    ///
    /// Suitable for placement in a `static mut` — produces an all-zero BSS
    /// pattern.  Must be followed by [`init`] before loading into CR3.
    ///
    /// [`init`]: KernelPageTable::init
    pub const fn new() -> Self {
        Self {
            pml4: PageTable::new(),
            pdpt: PageTable::new(),
            pd: PageTable::new(),
        }
    }

    /// Populate the page tables for Phase 1.
    ///
    /// Sets up:
    ///
    /// 1. `PD[0..511]` — 512 × 2 MiB huge pages covering `[0, 1 GiB)`.
    /// 2. `PDPT[0]`    — points to the PD (covers VA `[0, 1 GiB)`).
    /// 3. `PML4[0]`    — points to the PDPT (identity: VA `0` = PA `0`).
    /// 4. `PML4[256]`  — points to the same PDPT (higher-half alias).
    ///
    /// # Safety
    ///
    /// - Must be called **exactly once** on this instance.
    /// - Must be called while the CPU has a valid **identity mapping** covering
    ///   the physical addresses of all three sub-tables.  Under UEFI this is
    ///   always true before `CR3` is switched.
    /// - `self` must reside in memory whose physical address equals its
    ///   virtual address (UEFI guarantees this for the PE/COFF binary).
    /// - Must be called **single-threaded** with interrupts disabled.
    pub unsafe fn init(&mut self) {
        // Zero all three tables to guarantee absent entries everywhere we
        // don't explicitly set a mapping.  BSS is already zero but this guard
        // makes the invariant explicit and protects against future callers
        // that reuse an existing KernelPageTable instance.
        self.pml4.zero();
        self.pdpt.zero();
        self.pd.zero();

        // Step 1: fill PD with 2 MiB huge page entries covering [0, 1 GiB).
        //
        // Entry i maps physical address i × 2 MiB.
        // Flags: Present | Writable | HugePage (PS=1).
        for i in 0..HUGE_PAGES_1GIB {
            let phys = (i as u64) * HUGE_PAGE_SIZE;
            *self.pd.entry_mut(i) =
                PageTableEntry::new(phys, flags::PRESENT | flags::WRITABLE | flags::HUGE_PAGE);
        }

        // Step 2: PDPT[0] → PD.
        //
        // Under UEFI identity mapping the virtual address of a static equals
        // its physical address.  We use `core::ptr::addr_of!` to obtain a
        // raw pointer without creating a mutable reference (which would be UB
        // while we hold `&mut self`).
        let pd_phys = core::ptr::addr_of!(self.pd) as u64;
        *self.pdpt.entry_mut(0) = PageTableEntry::new(pd_phys, flags::PRESENT | flags::WRITABLE);

        // Step 3: PML4[0] → PDPT  (identity window: VA 0..512 GiB).
        let pdpt_phys = core::ptr::addr_of!(self.pdpt) as u64;
        *self.pml4.entry_mut(0) = PageTableEntry::new(pdpt_phys, flags::PRESENT | flags::WRITABLE);

        // Step 4: PML4[256] → same PDPT  (higher-half alias).
        //
        // PML4 index 256 covers virtual addresses starting at
        // 0xFFFF_8000_0000_0000.  Reusing the same PDPT means
        // VA 0xFFFF_8000_xxxx_xxxx aliases PA 0x0000_0000_xxxx_xxxx.
        *self.pml4.entry_mut(HIGHER_HALF_PML4_IDX) =
            PageTableEntry::new(pdpt_phys, flags::PRESENT | flags::WRITABLE);
    }

    /// Return the physical address of the PML4 table.
    ///
    /// This value must be written to `CR3` to activate these page tables.
    /// Under UEFI identity mapping, the virtual address of a static equals
    /// its physical address.
    ///
    /// # Safety
    ///
    /// Must be called while the UEFI identity mapping (VA == PA) is still
    /// active, i.e., before the first `MOV CR3` that switches to our tables.
    pub unsafe fn phys_pml4_addr(&self) -> u64 {
        core::ptr::addr_of!(self.pml4) as u64
    }
}

impl Default for KernelPageTable {
    fn default() -> Self {
        Self::new()
    }
}
