//! Per-process virtual address space management (Phase 2.1.2).
//!
//! Each [`AddressSpace`] owns an isolated PML4 page table and a fixed-capacity
//! list of [`VirtualRegion`]s that describe the user-mode mappings it
//! contains.  The kernel higher-half window is **shared** across all address
//! spaces: the upper 256 PML4 entries (indices 256–511, covering
//! `0xFFFF_8000_0000_0000` and above) are copied from the bootstrap PML4 at
//! construction time and never modified per-process.
//!
//! # Key invariants
//!
//! 1. **Identity mapping (VA == PA) holds for all page-table frames.**
//!    Phase 1 establishes this; all page-table frame accesses in this module
//!    cast physical addresses directly to pointers.  Phase 3 will introduce
//!    a dedicated virtual window for page-walker access.
//!
//! 2. **User regions live below `0x0000_8000_0000_0000`.**
//!    [`map_region`] rejects addresses in or above the higher-half window.
//!
//! 3. **Regions within one address space must not overlap.**
//!    [`map_region`] checks for overlap before installing any mappings.
//!
//! # Usage
//!
//! ```ignore
//! // SAFETY: called after frame_allocator::init(), single-threaded.
//! let mut aspace = unsafe { AddressSpace::new() }
//!     .expect("out of physical memory");
//!
//! let region = VirtualRegion {
//!     base: VirtualAddress::try_new(0x1000_0000).unwrap(),
//!     size: 4096,
//!     kind: RegionKind::Data,
//! };
//! unsafe { aspace.map_region(region).expect("mapping failed") };
//!
//! // Switch to this address space (flushes TLB).
//! unsafe { aspace.switch_to() };
//!
//! // Tear down all user mappings and free all frames.
//! unsafe { aspace.destroy() };
//! ```

use super::{
    frame_allocator,
    paging::{
        entry::{flags, PageTableEntry},
        table::PageTable,
        FrameAllocate, MapError, VirtualAddress,
    },
};
use crate::memory::frame_allocator::PhysFrame;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of [`VirtualRegion`]s an [`AddressSpace`] can hold.
///
/// Fixed-size to avoid heap allocation during early boot.  A dynamic list
/// will replace this once the heap is always available (Phase 2.2+).
pub const MAX_REGIONS_PER_SPACE: usize = 32;

/// Page size in bytes.
const PAGE_SIZE: usize = 4096;

/// First PML4 index belonging to the kernel higher-half window
/// (`0xFFFF_8000_0000_0000`, PML4 index 256).
const KERNEL_PML4_START: usize = 256;

/// Upper bound (exclusive) for user-space virtual addresses.
///
/// Addresses at or above this value are reserved for the kernel higher-half.
/// `0x0000_8000_0000_0000` is the first non-canonical (hole) address in
/// 4-level paging; the user half is `[0, USER_SPACE_END)`.
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

// ---------------------------------------------------------------------------
// RegionKind
// ---------------------------------------------------------------------------

/// The semantic type of a [`VirtualRegion`].
///
/// Each kind determines the page-table flags used when mapping pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Executable code — read-only in current phase (no NX support yet).
    Code,
    /// Read-write data (global variables, BSS, mmapped files).
    Data,
    /// Read-write stack, grows downward from `base + size`.
    Stack,
    /// Read-write heap, grows upward from `base`.
    Heap,
}

impl RegionKind {
    /// Return the x86-64 page-table flags for this region kind.
    ///
    /// All user regions include `PRESENT | USER_ACCESSIBLE`.  Write permission is
    /// granted to everything except `Code` pages.  NX / XD support will
    /// be added in Phase 3.
    pub const fn page_flags(self) -> u64 {
        match self {
            RegionKind::Code => flags::PRESENT | flags::USER_ACCESSIBLE,
            RegionKind::Data | RegionKind::Stack | RegionKind::Heap => {
                flags::PRESENT | flags::WRITABLE | flags::USER_ACCESSIBLE
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VirtualRegion
// ---------------------------------------------------------------------------

/// A contiguous, page-aligned virtual memory region within an address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualRegion {
    /// Base (lowest) virtual address of the region.  Must be 4 KiB-aligned.
    pub base: VirtualAddress,
    /// Size in bytes.  Must be a non-zero multiple of 4 KiB.
    pub size: usize,
    /// Semantic type that determines page-table flags.
    pub kind: RegionKind,
}

impl VirtualRegion {
    /// Return the first address past the end of this region.
    #[inline]
    pub fn end(self) -> u64 {
        self.base.as_u64() + self.size as u64
    }

    /// Return `true` if this region overlaps with `other`.
    #[inline]
    pub fn overlaps(self, other: VirtualRegion) -> bool {
        self.base.as_u64() < other.end() && other.base.as_u64() < self.end()
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by [`AddressSpace::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    /// The physical frame allocator could not provide a frame for the PML4.
    OutOfMemory,
}

/// Errors returned by [`AddressSpace::map_region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionMapError {
    /// The region base or end is in or above the kernel higher-half window.
    NotUserSpace,
    /// The region base or size is not 4 KiB-aligned.
    Misaligned,
    /// The region overlaps an existing region in this address space.
    Overlap,
    /// The region list is full ([`MAX_REGIONS_PER_SPACE`] reached).
    RegionListFull,
    /// A page-table frame could not be allocated during mapping.
    PageTableAlloc(MapError),
}

impl From<MapError> for RegionMapError {
    fn from(e: MapError) -> Self {
        RegionMapError::PageTableAlloc(e)
    }
}

/// Errors returned by [`AddressSpace::unmap_region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionUnmapError {
    /// No region with the given base address exists in this address space.
    NotFound,
}

// ---------------------------------------------------------------------------
// GlobalFrameAlloc — FrameAllocate shim over the global allocator
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`FrameAllocate`] by delegating to the
/// global physical frame allocator.
struct GlobalFrameAlloc;

impl FrameAllocate for GlobalFrameAlloc {
    fn allocate_frame(&mut self) -> Option<u64> {
        // SAFETY: caller of map_region must guarantee the frame allocator is
        // initialised and that access is single-threaded (Phase 1 invariant).
        unsafe { frame_allocator::allocate().map(|f| f.start_address()) }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Free all frames returned by [`unmap_4k_from`].
///
/// Handles both the leaf page frame and any intermediate page-table frames
/// that became empty.
///
/// # Safety
///
/// Every physical address in `r` must be a valid, exclusively-owned frame.
unsafe fn free_unmap_result(r: UnmapResult) {
    if let Some(pa) = r.page {
        if let Some(f) = PhysFrame::from_addr(pa) {
            frame_allocator::deallocate(f);
        }
    }
    for pa in r.tables.into_iter().flatten() {
        if let Some(f) = PhysFrame::from_addr(pa) {
            frame_allocator::deallocate(f);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal page-table helpers (walk into an arbitrary PML4)
// ---------------------------------------------------------------------------

/// Ensure `entry` points to a next-level [`PageTable`].
///
/// Like the helper in `mapper.rs` but usable from `address_space.rs`.
///
/// # Safety
///
/// Identity mapping (VA == PA) must hold.
unsafe fn ensure_table<A: FrameAllocate>(
    entry: &mut PageTableEntry,
    alloc: &mut A,
) -> Result<u64, MapError> {
    if entry.is_present() {
        Ok(entry.phys_addr())
    } else {
        let frame = alloc.allocate_frame().ok_or(MapError::OutOfMemory)?;
        let table = &mut *(frame as *mut PageTable);
        table.zero();
        *entry = PageTableEntry::new(
            frame,
            flags::PRESENT | flags::WRITABLE | flags::USER_ACCESSIBLE,
        );
        Ok(frame)
    }
}

/// Map a single 4 KiB page into the page tables rooted at `pml4_phys`.
///
/// Creates intermediate levels (PDPT, PD, PT) as needed.  Refuses to split
/// huge pages — the bootstrap identity mapping uses 2 MiB huge pages only in
/// the kernel range; user-space pages are always 4 KiB.
///
/// # Safety
///
/// - Identity mapping (VA == PA) must hold for all page-table frames.
/// - `pml4_phys` must be the physical address of a valid, 4 KiB-aligned PML4.
/// - `phys` must be a 4 KiB-aligned physical address of a valid frame.
unsafe fn map_4k_into<A: FrameAllocate>(
    pml4_phys: u64,
    virt: VirtualAddress,
    phys: u64,
    entry_flags: u64,
    alloc: &mut A,
) -> Result<(), MapError> {
    let pml4 = &mut *(pml4_phys as *mut PageTable);

    let pml4e = pml4.entry_mut(virt.pml4_index());
    let pdpt_phys = ensure_table(pml4e, alloc)?;

    let pdpt = &mut *(pdpt_phys as *mut PageTable);
    let pdpte = pdpt.entry_mut(virt.pdpt_index());
    if pdpte.is_present() && pdpte.is_huge() {
        return Err(MapError::HugePageConflict);
    }
    let pd_phys = ensure_table(pdpte, alloc)?;

    let pd = &mut *(pd_phys as *mut PageTable);
    let pde = pd.entry_mut(virt.pd_index());
    if pde.is_present() && pde.is_huge() {
        return Err(MapError::HugePageConflict);
    }
    let pt_phys = ensure_table(pde, alloc)?;

    let pt = &mut *(pt_phys as *mut PageTable);
    let pte = pt.entry_mut(virt.pt_index());
    if pte.is_present() {
        return Err(MapError::AlreadyMapped(pte.phys_addr()));
    }
    *pte = PageTableEntry::new(phys, entry_flags);
    Ok(())
}

/// Physical addresses reclaimed by a single [`unmap_4k_from`] call.
struct UnmapResult {
    /// The leaf page frame, if any was present.
    page: Option<u64>,
    /// Intermediate page-table frames that became empty and were detached.
    /// Ordered PT → PD → PDPT; `None` slots are unused.
    tables: [Option<u64>; 3],
}

impl UnmapResult {
    const fn none() -> Self {
        Self {
            page: None,
            tables: [None; 3],
        }
    }
}

/// Unmap a single 4 KiB page from the page tables rooted at `pml4_phys`.
///
/// Returns the physical address of the unmapped leaf frame and the physical
/// addresses of any intermediate page-table frames (PT, PD, PDPT) that became
/// entirely empty after the unmap and were detached from their parent.
/// Callers must free all returned frames.
///
/// Returns [`UnmapResult::none`] if the address was not mapped at 4 KiB
/// granularity.
///
/// # Safety
///
/// Identity mapping (VA == PA) must hold.
unsafe fn unmap_4k_from(pml4_phys: u64, virt: VirtualAddress) -> UnmapResult {
    let mut r = UnmapResult::none();

    // Collect physical addresses of each table level so we can check them for
    // emptiness after the PTE is cleared, without holding &mut references
    // across table boundaries (which would alias).
    let pml4 = pml4_phys as *mut PageTable;

    let pml4e = (*pml4).entry_mut(virt.pml4_index()) as *mut PageTableEntry;
    if !(*pml4e).is_present() {
        return r;
    }
    let pdpt_phys = (*pml4e).phys_addr();

    let pdpt = pdpt_phys as *mut PageTable;
    let pdpte = (*pdpt).entry_mut(virt.pdpt_index()) as *mut PageTableEntry;
    if !(*pdpte).is_present() || (*pdpte).is_huge() {
        return r;
    }
    let pd_phys = (*pdpte).phys_addr();

    let pd = pd_phys as *mut PageTable;
    let pde = (*pd).entry_mut(virt.pd_index()) as *mut PageTableEntry;
    if !(*pde).is_present() || (*pde).is_huge() {
        return r;
    }
    let pt_phys = (*pde).phys_addr();

    let pt = pt_phys as *mut PageTable;
    let pte = (*pt).entry_mut(virt.pt_index()) as *mut PageTableEntry;
    if !(*pte).is_present() {
        return r;
    }

    r.page = Some((*pte).phys_addr());
    *pte = PageTableEntry::EMPTY;

    // Prune empty intermediate tables bottom-up.  Using raw-pointer reads after
    // each EMPTY store avoids aliasing with live &mut references.
    if (*pt).iter().all(|e| !e.is_present()) {
        *pde = PageTableEntry::EMPTY;
        r.tables[0] = Some(pt_phys);

        if (*pd).iter().all(|e| !e.is_present()) {
            *pdpte = PageTableEntry::EMPTY;
            r.tables[1] = Some(pd_phys);

            if (*pdpt).iter().all(|e| !e.is_present()) {
                *pml4e = PageTableEntry::EMPTY;
                r.tables[2] = Some(pdpt_phys);
            }
        }
    }

    r
}

/// Translate a virtual address in the page tables rooted at `pml4_phys`.
///
/// Returns the physical address if a 4 KiB mapping exists, or `None`.
///
/// # Safety
///
/// Identity mapping (VA == PA) must hold.
pub unsafe fn translate_in(pml4_phys: u64, virt: VirtualAddress) -> Option<u64> {
    let pml4 = &*(pml4_phys as *const PageTable);
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
        return Some(pdpte.phys_addr() | (virt.as_u64() & 0x3FFF_FFFF));
    }

    let pd = &*(pdpte.phys_addr() as *const PageTable);
    let pde = pd.entry(virt.pd_index());
    if !pde.is_present() {
        return None;
    }
    if pde.is_huge() {
        return Some(pde.phys_addr() | (virt.as_u64() & 0x1F_FFFF));
    }

    let pt = &*(pde.phys_addr() as *const PageTable);
    let pte = pt.entry(virt.pt_index());
    if !pte.is_present() {
        return None;
    }
    Some(pte.phys_addr() | virt.page_offset())
}

// ---------------------------------------------------------------------------
// AddressSpace
// ---------------------------------------------------------------------------

/// Per-process virtual address space.
///
/// Owns one PML4 physical frame and a fixed-capacity list of user-mode
/// [`VirtualRegion`]s.  The kernel higher-half window (PML4 entries 256–511)
/// is copied from the active bootstrap PML4 at construction time and kept
/// in sync automatically — all address spaces always share the same kernel
/// view.
///
/// # Safety invariants
///
/// - `pml4_frame.start_address()` is the physical address of a valid,
///   exclusively-owned 4 KiB PML4 frame.
/// - The identity mapping (VA == PA) must hold for all page-table frames
///   accessed through this struct.
/// - All methods that modify page tables must be called from a
///   single-threaded context with interrupts disabled (Phase 1 invariant;
///   Phase 2.2 will add spinlock protection).
pub struct AddressSpace {
    /// Physical frame holding this address space's PML4.
    pml4_frame: PhysFrame,
    /// Tracked user-mode regions.  Slots `0..region_count` are `Some`.
    regions: [Option<VirtualRegion>; MAX_REGIONS_PER_SPACE],
    /// Number of active (Some) entries in `regions`.
    region_count: usize,
}

impl AddressSpace {
    /// Create a new, empty address space.
    ///
    /// Allocates a fresh PML4 frame, zeroes it, then copies PML4 entries
    /// 256–511 from the currently-active page tables so that the new address
    /// space shares the kernel higher-half mapping.
    ///
    /// # Safety
    ///
    /// - [`frame_allocator::init`] must have been called.
    /// - The identity mapping (VA == PA) must hold.
    /// - Must be called single-threaded with interrupts disabled.
    pub unsafe fn new() -> Result<Self, AddressSpaceError> {
        // Allocate and zero the PML4.
        let pml4_frame = frame_allocator::allocate().ok_or(AddressSpaceError::OutOfMemory)?;
        let pml4 = &mut *(pml4_frame.start_address() as *mut PageTable);
        pml4.zero();

        // Copy kernel higher-half entries from the current (bootstrap) PML4.
        // Mask out CR3 control bits (PCID / PWT / PCD live in bits 0–11).
        let cr3: u64;
        core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
        let boot_pml4 = &*((cr3 & !0xFFF) as *const PageTable);
        for i in KERNEL_PML4_START..512 {
            *pml4.entry_mut(i) = *boot_pml4.entry(i);
        }

        Ok(Self {
            pml4_frame,
            regions: [None; MAX_REGIONS_PER_SPACE],
            region_count: 0,
        })
    }

    /// Return the physical address of this address space's PML4.
    ///
    /// Write this value to CR3 to activate the address space.
    #[inline]
    pub fn pml4_phys(&self) -> u64 {
        self.pml4_frame.start_address()
    }

    /// Switch the CPU to this address space by loading the PML4 into CR3.
    ///
    /// This flushes all non-global TLB entries.  Must be called with
    /// interrupts disabled to avoid a window where the TLB is partially
    /// stale.
    ///
    /// # Safety
    ///
    /// - The page tables rooted at `pml4_phys` must be fully initialised.
    /// - Interrupts must be disabled at the call site.
    /// - The kernel higher-half must be identically mapped in this address
    ///   space (guaranteed by [`AddressSpace::new`]).
    pub unsafe fn switch_to(&self) {
        core::arch::asm!(
            "mov cr3, {pml4}",
            pml4 = in(reg) self.pml4_frame.start_address(),
            options(nostack),
        );
    }

    // -----------------------------------------------------------------------
    // Region management
    // -----------------------------------------------------------------------

    /// Map a [`VirtualRegion`] into this address space.
    ///
    /// For each 4 KiB page in the region, one physical frame is allocated
    /// and mapped with the flags dictated by [`RegionKind::page_flags`].
    ///
    /// # Errors
    ///
    /// - [`RegionMapError::NotUserSpace`] — region is outside user-space.
    /// - [`RegionMapError::Misaligned`] — base or size not 4 KiB-aligned.
    /// - [`RegionMapError::Overlap`] — region overlaps an existing region.
    /// - [`RegionMapError::RegionListFull`] — `MAX_REGIONS_PER_SPACE` reached.
    /// - [`RegionMapError::PageTableAlloc`] — physical memory exhausted.
    ///
    /// On error, any pages already mapped for this region are unmapped and
    /// their frames freed (atomic with respect to the region list).
    ///
    /// # Safety
    ///
    /// - Identity mapping (VA == PA) must hold.
    /// - [`frame_allocator::init`] must have been called.
    /// - Must be called single-threaded with interrupts disabled.
    pub unsafe fn map_region(&mut self, region: VirtualRegion) -> Result<(), RegionMapError> {
        // Validate user-space constraint.
        let base = region.base.as_u64();
        let end_ok = base
            .checked_add(region.size as u64)
            .map_or(false, |end| end <= USER_SPACE_END);
        if base >= USER_SPACE_END || !end_ok {
            return Err(RegionMapError::NotUserSpace);
        }

        // Validate alignment.
        if base % PAGE_SIZE as u64 != 0 || region.size == 0 || region.size % PAGE_SIZE != 0 {
            return Err(RegionMapError::Misaligned);
        }

        // Check capacity.
        if self.region_count >= MAX_REGIONS_PER_SPACE {
            return Err(RegionMapError::RegionListFull);
        }

        // Check overlap with existing regions.
        for slot in &self.regions[..self.region_count] {
            if let Some(existing) = slot {
                if region.overlaps(*existing) {
                    return Err(RegionMapError::Overlap);
                }
            }
        }

        // Map pages one-by-one; roll back on allocation failure.
        let entry_flags = region.kind.page_flags();
        let pages = region.size / PAGE_SIZE;
        let pml4_phys = self.pml4_frame.start_address();
        let mut alloc = GlobalFrameAlloc;

        for i in 0..pages {
            let virt_addr = base + (i * PAGE_SIZE) as u64;
            // SAFETY: virt_addr is in user-space; we validated alignment above.
            let virt = VirtualAddress::try_new(virt_addr)
                .expect("address was validated as user-space above");

            let phys_frame = match frame_allocator::allocate() {
                Some(f) => f,
                None => {
                    // Roll back successfully mapped pages.
                    for j in 0..i {
                        let rollback_va = base + (j * PAGE_SIZE) as u64;
                        let rollback_virt = VirtualAddress::try_new(rollback_va).unwrap();
                        free_unmap_result(unmap_4k_from(pml4_phys, rollback_virt));
                    }
                    return Err(RegionMapError::PageTableAlloc(MapError::OutOfMemory));
                }
            };

            if let Err(e) = map_4k_into(
                pml4_phys,
                virt,
                phys_frame.start_address(),
                entry_flags,
                &mut alloc,
            ) {
                // Rollback current frame and all previous pages.
                frame_allocator::deallocate(phys_frame);
                for j in 0..i {
                    let rollback_va = base + (j * PAGE_SIZE) as u64;
                    let rollback_virt = VirtualAddress::try_new(rollback_va).unwrap();
                    free_unmap_result(unmap_4k_from(pml4_phys, rollback_virt));
                }
                return Err(RegionMapError::PageTableAlloc(e));
            }
        }

        // Record the region.
        self.regions[self.region_count] = Some(region);
        self.region_count += 1;
        Ok(())
    }

    /// Unmap a region by its base address, freeing all mapped physical frames.
    ///
    /// # Errors
    ///
    /// [`RegionUnmapError::NotFound`] if no region with `base` exists.
    ///
    /// # Safety
    ///
    /// - Identity mapping (VA == PA) must hold.
    /// - The region must not be currently executing; unmapping active code
    ///   is undefined behavior.
    /// - Must be called single-threaded with interrupts disabled.
    pub unsafe fn unmap_region(&mut self, base: VirtualAddress) -> Result<(), RegionUnmapError> {
        let pos = self.regions[..self.region_count]
            .iter()
            .position(|s| s.map_or(false, |r| r.base == base))
            .ok_or(RegionUnmapError::NotFound)?;

        let region = self.regions[pos].unwrap();
        let pml4_phys = self.pml4_frame.start_address();
        let pages = region.size / PAGE_SIZE;
        let region_base = region.base.as_u64();

        for i in 0..pages {
            let virt_addr = region_base + (i * PAGE_SIZE) as u64;
            let virt = VirtualAddress::try_new(virt_addr).unwrap();
            free_unmap_result(unmap_4k_from(pml4_phys, virt));
        }

        // Compact the region list: replace the removed slot with the last entry.
        self.regions[pos] = self.regions[self.region_count - 1].take();
        self.region_count -= 1;
        Ok(())
    }

    /// Translate a virtual address within this address space.
    ///
    /// Returns the physical address if a mapping exists, or `None`.
    ///
    /// # Safety
    ///
    /// Identity mapping (VA == PA) must hold.
    pub unsafe fn translate(&self, virt: VirtualAddress) -> Option<u64> {
        translate_in(self.pml4_frame.start_address(), virt)
    }

    /// Return a slice of all tracked [`VirtualRegion`]s.
    pub fn regions(&self) -> &[Option<VirtualRegion>] {
        &self.regions[..self.region_count]
    }

    /// Return the number of mapped regions.
    pub fn region_count(&self) -> usize {
        self.region_count
    }

    // -----------------------------------------------------------------------
    // Teardown
    // -----------------------------------------------------------------------

    /// Unmap all user regions, free all mapped frames, and free the PML4
    /// frame itself.
    ///
    /// After `destroy`, this `AddressSpace` must not be used (including
    /// `switch_to`).
    ///
    /// # Safety
    ///
    /// - None of the regions may be currently executing.
    /// - Must be called single-threaded with interrupts disabled.
    /// - Must not call `switch_to` after `destroy`.
    pub unsafe fn destroy(&mut self) {
        // Unmap all user regions in reverse order to avoid index issues.
        while self.region_count > 0 {
            let region = self.regions[self.region_count - 1].take().unwrap();
            let pml4_phys = self.pml4_frame.start_address();
            let pages = region.size / PAGE_SIZE;
            let base = region.base.as_u64();
            for i in 0..pages {
                let virt = VirtualAddress::try_new(base + (i * PAGE_SIZE) as u64).unwrap();
                free_unmap_result(unmap_4k_from(pml4_phys, virt));
            }
            self.region_count -= 1;
        }
        // Free the PML4 frame itself.
        frame_allocator::deallocate(self.pml4_frame);
    }
}

impl core::fmt::Debug for AddressSpace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AddressSpace")
            .field("pml4_phys", &self.pml4_frame.start_address())
            .field("region_count", &self.region_count)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Smoke test (Phase 2.1.2)
// ---------------------------------------------------------------------------

/// Run Phase 2.1.2 smoke tests.
///
/// Tests:
/// 1. `VirtualRegion` overlap detection.
/// 2. `AddressSpace::new` allocates and shares the kernel higher-half.
/// 3. `map_region` maps pages and they translate correctly.
/// 4. `unmap_region` removes mappings and frees frames.
/// 5. Misaligned and out-of-user-space regions are rejected.
/// 6. `destroy` tears down all mappings cleanly.
pub unsafe fn smoke_test() {
    log::info!("Address space smoke test (Phase 2.1.2)");

    // 15.1 — VirtualRegion overlap detection
    let r1 = VirtualRegion {
        base: VirtualAddress::try_new(0x1000).unwrap(),
        size: 0x3000,
        kind: RegionKind::Data,
    };
    let r2 = VirtualRegion {
        base: VirtualAddress::try_new(0x2000).unwrap(),
        size: 0x1000,
        kind: RegionKind::Data,
    };
    let r3 = VirtualRegion {
        base: VirtualAddress::try_new(0x4000).unwrap(),
        size: 0x1000,
        kind: RegionKind::Data,
    };
    assert!(r1.overlaps(r2), "r1 and r2 should overlap");
    assert!(!r1.overlaps(r3), "r1 and r3 should not overlap");
    log::info!("[OK] 15.1) VirtualRegion overlap detection");

    // 15.2 — AddressSpace::new
    let mut aspace = AddressSpace::new().expect("AddressSpace::new failed");
    assert_eq!(aspace.region_count(), 0);
    log::info!(
        "[OK] 15.2) AddressSpace::new: pml4={:#x}",
        aspace.pml4_phys()
    );

    // 15.3 — map_region and translate
    let data_region = VirtualRegion {
        base: VirtualAddress::try_new(0x10_0000).unwrap(),
        size: PAGE_SIZE * 3,
        kind: RegionKind::Data,
    };
    aspace.map_region(data_region).expect("map_region failed");
    assert_eq!(aspace.region_count(), 1);

    // Each page must translate to some physical address.
    for i in 0..3usize {
        let va = VirtualAddress::try_new(0x10_0000 + (i * PAGE_SIZE) as u64).unwrap();
        let pa = aspace
            .translate(va)
            .expect("translation must succeed after map");
        assert_eq!(pa & 0xFFF, 0, "translated PA must be page-aligned");
        log::info!(
            "[OK] 15.3) page[{}]: va={:#x} -> pa={:#x}",
            i,
            va.as_u64(),
            pa
        );
    }

    // 15.4 — unmap_region frees pages
    aspace
        .unmap_region(VirtualAddress::try_new(0x10_0000).unwrap())
        .expect("unmap_region failed");
    assert_eq!(aspace.region_count(), 0);
    let gone = aspace.translate(VirtualAddress::try_new(0x10_0000).unwrap());
    assert!(gone.is_none(), "translation after unmap must return None");
    log::info!("[OK] 15.4) unmap_region: mapping removed");

    // 15.5 — invalid region rejected
    // Misaligned size.
    let bad_align = VirtualRegion {
        base: VirtualAddress::try_new(0x20_0000).unwrap(),
        size: 100,
        kind: RegionKind::Data,
    };
    assert!(
        aspace.map_region(bad_align).is_err(),
        "misaligned size must be rejected"
    );
    // Higher-half address.
    let bad_range = VirtualRegion {
        base: VirtualAddress::try_new(0xFFFF_8000_0000_0000).unwrap(),
        size: PAGE_SIZE,
        kind: RegionKind::Data,
    };
    assert!(
        aspace.map_region(bad_range).is_err(),
        "kernel-range address must be rejected"
    );
    log::info!("[OK] 15.5) invalid regions rejected");

    // 15.6 — destroy
    aspace.destroy();
    log::info!("[OK] 15.6) AddressSpace::destroy: all frames freed");

    log::info!("Address space smoke test complete");
}
