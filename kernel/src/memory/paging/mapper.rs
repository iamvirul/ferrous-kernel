//! Kernel page-table mapper.
//!
//! Two mappers are provided:
//!
//! | Type                | Purpose                                             |
//! |---------------------|-----------------------------------------------------|
//! | [`KernelPageTable`] | Phase-1 static bootstrap structure (PML4+PDPT+PD)  |
//! | [`ActivePageTable`] | Live page-table walker — map, unmap, translate      |
//!
//! # Phase 1 invariant — identity mapping (VA == PA)
//!
//! All page-table manipulation in Phase 1 relies on the identity mapping
//! established by [`KernelPageTable::init`]: every page-table frame is
//! accessible at its physical address (virtual == physical).
//!
//! This invariant is **documented but not enforced** here; it holds because:
//!
//! 1. UEFI loads the PE/COFF binary with identity mapping.
//! 2. [`KernelPageTable::init`] preserves identity for `[0, 1 GiB)`.
//! 3. All newly allocated frames come from the physical frame allocator,
//!    which operates within that range.
//!
//! Phase 2 will break this invariant (kernel will run at higher-half VAs),
//! requiring a virtual-to-physical translation table for page-walker access.

use super::{
    entry::{flags, PageTableEntry},
    table::PageTable,
    VirtualAddress,
};

// ---------------------------------------------------------------------------
// Allocation trait
// ---------------------------------------------------------------------------

/// A source of 4 KiB-aligned physical frames for page-table allocation.
///
/// Implementations wrap the global physical frame allocator (Phase 1) or a
/// per-process allocator (Phase 2+).
pub trait FrameAllocate {
    /// Allocate one 4 KiB physical frame, returning its base address.
    ///
    /// The returned frame is uninitialized; callers that use it as a page
    /// table must zero it before writing entries.
    ///
    /// Returns `None` if physical memory is exhausted.
    fn allocate_frame(&mut self) -> Option<u64>;
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by [`ActivePageTable::map_4k`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The physical frame allocator returned `None` while creating an
    /// intermediate page-table level.
    OutOfMemory,

    /// A 4 KiB mapping was requested for a virtual address that is already
    /// mapped.  The existing physical address is provided.
    AlreadyMapped(u64),

    /// A 1 GiB huge PDPT entry covers the target virtual address.  Splitting
    /// 1 GiB pages is not implemented in Phase 1.
    HugePageConflict,
}

/// Errors returned by [`ActivePageTable::unmap_4k`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmapError {
    /// The virtual address has no 4 KiB mapping (the walk terminated early
    /// due to an absent intermediate entry).
    NotMapped,

    /// The virtual address is covered by a huge page (2 MiB PD entry or
    /// 1 GiB PDPT entry).  Use the appropriate huge-page unmap operation
    /// (not yet implemented in Phase 1).
    HugePage,
}

// ---------------------------------------------------------------------------
// Internal walk error
// ---------------------------------------------------------------------------

enum WalkError {
    NotMapped,
    HugePage,
}

// ---------------------------------------------------------------------------
// Free functions — TLB management
// ---------------------------------------------------------------------------

/// Invalidate the TLB entry for a single virtual address.
///
/// Must be called after modifying a page-table entry that is (or was)
/// present, to ensure the CPU does not use a stale cached translation.
///
/// # Safety
///
/// - CPU must be in 64-bit paged mode.
/// - `virt` should be a canonical virtual address, though the CPU silently
///   ignores `INVLPG` on non-canonical addresses.
pub unsafe fn invlpg(virt: u64) {
    core::arch::asm!(
        "invlpg [{addr}]",
        addr = in(reg) virt,
        options(nostack, preserves_flags),
    );
}

/// Flush the entire TLB by reloading CR3.
///
/// More expensive than a targeted [`invlpg`], but necessary after bulk
/// changes such as splitting a 2 MiB huge page into 512 × 4 KiB entries,
/// where 512 individual `INVLPG` calls would be wasteful.
///
/// # Safety
///
/// - CPU must have valid page tables loaded.
/// - The identity mapping must cover the instruction stream so execution
///   continues correctly after the reload.
pub unsafe fn flush_tlb_all() {
    let cr3: u64;
    core::arch::asm!(
        "mov {cr3}, cr3",
        cr3 = out(reg) cr3,
        options(nomem, nostack),
    );
    core::arch::asm!(
        "mov cr3, {cr3}",
        cr3 = in(reg) cr3,
        options(nostack),
    );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Ensure `entry` points to a child [`PageTable`].
///
/// - If `entry` is already present (and not a huge page), returns its
///   physical address unchanged.
/// - If `entry` is absent, allocates a frame, zeroes it, and installs
///   `Present | Writable` into `entry`.
///
/// # Safety
///
/// Identity mapping (VA == PA) must hold for the newly allocated frame.
unsafe fn ensure_table<A: FrameAllocate>(
    entry: &mut PageTableEntry,
    alloc: &mut A,
) -> Result<u64, MapError> {
    if entry.is_present() {
        Ok(entry.phys_addr())
    } else {
        let frame = alloc.allocate_frame().ok_or(MapError::OutOfMemory)?;
        // Zero the frame: it may contain garbage from a previous allocation.
        // SAFETY: frame is a valid, exclusively-owned 4 KiB physical frame;
        // identity mapping makes the physical address directly addressable.
        let table = &mut *(frame as *mut PageTable);
        table.zero();
        *entry = PageTableEntry::new(frame, flags::PRESENT | flags::WRITABLE);
        Ok(frame)
    }
}

/// Split a 2 MiB PD huge-page entry into 512 × 4 KiB PT entries.
///
/// The new PT is populated with entries that reproduce the original 2 MiB
/// mapping at 4 KiB granularity, preserving all flags except `HUGE_PAGE`.
/// The PD entry is then updated to point to the new PT.
///
/// After installing the PT, the entire TLB is flushed (via [`flush_tlb_all`])
/// because the 2 MiB TLB entry is now stale — 512 individual `INVLPG` calls
/// would be correct but unnecessarily verbose for Phase 1.
///
/// # Safety
///
/// - `pde` must point to a present, huge (2 MiB) PD entry.
/// - Identity mapping must hold.
/// - `alloc` must return valid 4 KiB frames.
unsafe fn split_huge_pd<A: FrameAllocate>(
    pde: &mut PageTableEntry,
    alloc: &mut A,
) -> Result<(), MapError> {
    debug_assert!(
        pde.is_present() && pde.is_huge(),
        "split_huge_pd: entry is not a present 2 MiB huge page"
    );

    let huge_base = pde.phys_addr(); // 2 MiB-aligned PA of the original region.

    // Carry over existing flags (P, RW, U/S, PWT, PCD, G, XD) but strip
    // HUGE_PAGE (PS), which has no meaning in a PT entry.
    let base_flags = (pde.raw() & !flags::PHYS_ADDR_MASK & !flags::HUGE_PAGE) | flags::PRESENT;

    let pt_frame = alloc.allocate_frame().ok_or(MapError::OutOfMemory)?;
    // SAFETY: pt_frame is a fresh, exclusively-owned frame; VA == PA.
    let pt = &mut *(pt_frame as *mut PageTable);

    // Each of the 512 PT entries maps one 4 KiB subpage of the original
    // 2 MiB region.  Entry i covers physical address huge_base + i×4 KiB.
    for (i, entry) in pt.iter_mut().enumerate() {
        let phys = huge_base + (i as u64) * 4096;
        *entry = PageTableEntry::new(phys, base_flags);
    }

    // Replace the 2 MiB PD entry with a pointer to the new PT.
    *pde = PageTableEntry::new(pt_frame, flags::PRESENT | flags::WRITABLE);

    // Flush all TLB entries — the 2 MiB entry is stale after this change.
    flush_tlb_all();

    Ok(())
}

/// Walk the active page tables down to the PT entry for `virt`.
///
/// Returns a mutable reference to the PT entry, or a [`WalkError`] if any
/// intermediate level is absent or is a huge page that blocks the walk.
///
/// # Safety
///
/// Identity mapping must hold; CR3 must point to a valid PML4.
unsafe fn pt_entry_mut(virt: VirtualAddress) -> Result<&'static mut PageTableEntry, WalkError> {
    // Read the current CR3 to obtain the PML4 physical address.
    let cr3: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
    let pml4_phys = cr3 & flags::PHYS_ADDR_MASK;

    // SAFETY: pml4_phys is a valid 4 KiB-aligned frame; VA == PA.
    let pml4 = &mut *(pml4_phys as *mut PageTable);
    let pml4e = pml4.entry_mut(virt.pml4_index());
    if !pml4e.is_present() {
        return Err(WalkError::NotMapped);
    }

    let pdpt = &mut *(pml4e.phys_addr() as *mut PageTable);
    let pdpte = pdpt.entry_mut(virt.pdpt_index());
    if !pdpte.is_present() {
        return Err(WalkError::NotMapped);
    }
    if pdpte.is_huge() {
        return Err(WalkError::HugePage); // 1 GiB page — cannot walk further.
    }

    let pd = &mut *(pdpte.phys_addr() as *mut PageTable);
    let pde = pd.entry_mut(virt.pd_index());
    if !pde.is_present() {
        return Err(WalkError::NotMapped);
    }
    if pde.is_huge() {
        return Err(WalkError::HugePage); // 2 MiB page — cannot walk to PT level.
    }

    let pt = &mut *(pde.phys_addr() as *mut PageTable);
    Ok(pt.entry_mut(virt.pt_index()))
}

// ---------------------------------------------------------------------------
// ActivePageTable
// ---------------------------------------------------------------------------

/// Live page-table walker for the currently-loaded CR3.
///
/// Provides [`map_4k`], [`unmap_4k`], and [`translate`] over the active
/// page tables.  Huge-page splits are performed automatically when
/// [`map_4k`] encounters a 2 MiB PD entry on the path to the target VA.
///
/// # Phase 1 invariant — identity mapping (VA == PA)
///
/// All page-table frames are accessed by casting their physical addresses
/// directly to raw pointers.  This is correct only while the identity
/// mapping is active (VA == PA for all page-table frames).  Phase 2 will
/// need a separate virtual window for page-walker access.
///
/// # Thread safety
///
/// Phase 1 is single-core with interrupts disabled.  There is no locking.
/// Phase 2 will wrap this in a spinlock-guarded `AddressSpace`.
///
/// [`map_4k`]: ActivePageTable::map_4k
/// [`unmap_4k`]: ActivePageTable::unmap_4k
/// [`translate`]: ActivePageTable::translate
pub struct ActivePageTable;

impl ActivePageTable {
    /// Borrow the currently-active page tables.
    ///
    /// # Safety
    ///
    /// - The CPU must be in 64-bit paged mode (CR0.PG = 1).
    /// - The identity-mapping invariant must hold: every page-table frame is
    ///   accessible at its physical address (VA == PA).
    /// - The caller must guarantee exclusive access (Phase 1: single-core,
    ///   interrupts disabled).
    pub unsafe fn current() -> Self {
        Self
    }

    /// Translate a virtual address to its physical address.
    ///
    /// Walks the PML4 → PDPT → PD → PT hierarchy, correctly handling:
    /// - 1 GiB huge pages (PDPT entry with PS=1)
    /// - 2 MiB huge pages (PD entry with PS=1)
    /// - 4 KiB pages (PT entry)
    ///
    /// Returns `None` if any level is absent or if the final PT entry has
    /// Present=0.
    ///
    /// # Safety
    ///
    /// Identity mapping must hold; CR3 must point to a valid PML4.
    pub unsafe fn translate(&self, virt: VirtualAddress) -> Option<u64> {
        let cr3: u64;
        core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
        let pml4 = &*((cr3 & flags::PHYS_ADDR_MASK) as *const PageTable);

        let pml4e = pml4.entry(virt.pml4_index());
        if !pml4e.is_present() {
            return None;
        }

        let pdpt = &*(pml4e.phys_addr() as *const PageTable);
        let pdpte = pdpt.entry(virt.pdpt_index());
        if !pdpte.is_present() {
            return None;
        }
        if pdpte.is_huge() {
            // 1 GiB huge page: PA = pdpte.phys_addr() | (virt & 0x3FFF_FFFF).
            return Some(pdpte.phys_addr() | (virt.as_u64() & 0x3FFF_FFFF));
        }

        let pd = &*(pdpte.phys_addr() as *const PageTable);
        let pde = pd.entry(virt.pd_index());
        if !pde.is_present() {
            return None;
        }
        if pde.is_huge() {
            // 2 MiB huge page: PA = pde.phys_addr() | (virt & 0x1F_FFFF).
            return Some(pde.phys_addr() | (virt.as_u64() & 0x1F_FFFF));
        }

        let pt = &*(pde.phys_addr() as *const PageTable);
        let pte = pt.entry(virt.pt_index());
        if !pte.is_present() {
            return None;
        }
        Some(pte.phys_addr() | virt.page_offset())
    }

    /// Map a 4 KiB virtual page to a physical frame.
    ///
    /// Creates intermediate page-table levels (PDPT, PD, PT) as needed by
    /// allocating frames from `alloc`.  If a 2 MiB PD huge-page entry
    /// covers the target VA, it is automatically split into 512 × 4 KiB
    /// entries before the new mapping is installed.
    ///
    /// # Errors
    ///
    /// - [`MapError::OutOfMemory`] — frame allocation failed.
    /// - [`MapError::AlreadyMapped`] — the PT entry is already present.
    /// - [`MapError::HugePageConflict`] — a 1 GiB PDPT huge page covers the
    ///   target VA; splitting 1 GiB pages is not supported in Phase 1.
    ///
    /// # Safety
    ///
    /// - Identity mapping must hold.
    /// - `phys` must be a 4 KiB-aligned physical address of a valid frame.
    /// - `flags` must include at least [`flags::PRESENT`].
    /// - The caller must guarantee that no other code is concurrently
    ///   modifying the page tables (Phase 1: single-core, interrupts off).
    pub unsafe fn map_4k<A: FrameAllocate>(
        &mut self,
        virt: VirtualAddress,
        phys: u64,
        entry_flags: u64,
        alloc: &mut A,
    ) -> Result<(), MapError> {
        let cr3: u64;
        core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
        let pml4 = &mut *((cr3 & flags::PHYS_ADDR_MASK) as *mut PageTable);

        // Level 1: ensure PDPT exists.
        let pml4e = pml4.entry_mut(virt.pml4_index());
        let pdpt_phys = ensure_table(pml4e, alloc)?;

        // Level 2: ensure PD exists (reject 1 GiB pages — not splittable yet).
        let pdpt = &mut *(pdpt_phys as *mut PageTable);
        let pdpte = pdpt.entry_mut(virt.pdpt_index());
        if pdpte.is_present() && pdpte.is_huge() {
            return Err(MapError::HugePageConflict);
        }
        let pd_phys = ensure_table(pdpte, alloc)?;

        // Level 3: handle 2 MiB huge page — split if necessary, then ensure PT.
        let pd = &mut *(pd_phys as *mut PageTable);
        let pde = pd.entry_mut(virt.pd_index());
        if pde.is_present() && pde.is_huge() {
            split_huge_pd(pde, alloc)?;
        }
        let pt_phys = ensure_table(pde, alloc)?;

        // Level 4: install the 4 KiB mapping.
        let pt = &mut *(pt_phys as *mut PageTable);
        let pte = pt.entry_mut(virt.pt_index());
        if pte.is_present() {
            return Err(MapError::AlreadyMapped(pte.phys_addr()));
        }
        *pte = PageTableEntry::new(phys, entry_flags);

        // Invalidate the TLB entry for this specific VA.
        invlpg(virt.as_u64());

        Ok(())
    }

    /// Unmap a 4 KiB virtual page and invalidate its TLB entry.
    ///
    /// Clears the PT entry and issues an [`invlpg`] for the given VA.
    /// The physical frame that was mapped is **not** deallocated — the
    /// caller is responsible for returning it to the frame allocator if
    /// appropriate.
    ///
    /// # Errors
    ///
    /// - [`UnmapError::NotMapped`] — the VA has no present 4 KiB mapping.
    /// - [`UnmapError::HugePage`] — the VA is covered by a huge page.
    ///
    /// # Returns
    ///
    /// On success, the physical address of the frame that was unmapped.
    ///
    /// # Safety
    ///
    /// - Identity mapping must hold.
    /// - The caller must ensure no concurrent modification of the page tables.
    pub unsafe fn unmap_4k(&mut self, virt: VirtualAddress) -> Result<u64, UnmapError> {
        match pt_entry_mut(virt) {
            Ok(pte) => {
                if !pte.is_present() {
                    return Err(UnmapError::NotMapped);
                }
                let phys = pte.phys_addr();
                *pte = PageTableEntry::EMPTY;
                invlpg(virt.as_u64());
                Ok(phys)
            }
            Err(WalkError::NotMapped) => Err(UnmapError::NotMapped),
            Err(WalkError::HugePage) => Err(UnmapError::HugePage),
        }
    }
}

// ---------------------------------------------------------------------------
// KernelPageTable  (Phase-1 bootstrap structure — unchanged from 1.3.3)
// ---------------------------------------------------------------------------

/// PML4 index for the higher-half window (`0xFFFF_8000_0000_0000`).
///
/// Bits [47:39] of `0xFFFF_8000_0000_0000` decode to `0x100` = 256.
const HIGHER_HALF_PML4_IDX: usize = 256;

/// Number of 2 MiB pages needed to cover 1 GiB of physical memory.
const HUGE_PAGES_1GIB: usize = 512;

/// Size of a 2 MiB huge page in bytes.
const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;

/// The kernel's Phase-1 bootstrap page table structure.
///
/// Holds a PML4, one PDPT, and one PD inline (12 KiB BSS).  Call
/// [`init`] to populate the tables and then load the PML4 address into CR3.
///
/// After [`init`] completes, use [`ActivePageTable`] to perform all
/// subsequent map/unmap/translate operations.
///
/// # Layout after `init`
///
/// ```text
/// PML4[0]   ──→ PDPT   (identity: VA [0, 512 GiB) = PA [0, 512 GiB))
/// PML4[256] ──→ PDPT   (higher-half alias: 0xFFFF_8000_0000_0000)
/// PDPT[0]   ──→ PD     (covers VA [0, 1 GiB))
/// PD[0..511]──→ 2 MiB huge pages at PA 0, 2M, 4M, …
/// ```
///
/// [`init`]: KernelPageTable::init
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
    /// # Safety
    ///
    /// - Must be called **exactly once** on this instance.
    /// - Must be called while identity mapping (VA == PA) is in effect.
    /// - Must be called **single-threaded** with interrupts disabled.
    pub unsafe fn init(&mut self) {
        self.pml4.zero();
        self.pdpt.zero();
        self.pd.zero();

        for i in 0..HUGE_PAGES_1GIB {
            let phys = (i as u64) * HUGE_PAGE_SIZE;
            *self.pd.entry_mut(i) =
                PageTableEntry::new(phys, flags::PRESENT | flags::WRITABLE | flags::HUGE_PAGE);
        }

        let pd_phys = core::ptr::addr_of!(self.pd) as u64;
        *self.pdpt.entry_mut(0) = PageTableEntry::new(pd_phys, flags::PRESENT | flags::WRITABLE);

        let pdpt_phys = core::ptr::addr_of!(self.pdpt) as u64;
        *self.pml4.entry_mut(0) = PageTableEntry::new(pdpt_phys, flags::PRESENT | flags::WRITABLE);
        *self.pml4.entry_mut(HIGHER_HALF_PML4_IDX) =
            PageTableEntry::new(pdpt_phys, flags::PRESENT | flags::WRITABLE);
    }

    /// Return the physical address of the PML4 — write this to CR3.
    ///
    /// # Safety
    ///
    /// Must be called while identity mapping (VA == PA) is still active.
    pub unsafe fn phys_pml4_addr(&self) -> u64 {
        core::ptr::addr_of!(self.pml4) as u64
    }
}

impl Default for KernelPageTable {
    fn default() -> Self {
        Self::new()
    }
}
