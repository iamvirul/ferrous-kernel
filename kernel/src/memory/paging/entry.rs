//! Page table entry type and flag bit constants.
//!
//! A single `PageTableEntry` is an 8-byte quantity used at every level of the
//! x86-64 four-level page table hierarchy (PML4, PDPT, PD, PT).  The
//! interpretation of some flag bits varies by level; see the Intel SDM
//! Vol 3A §4.5 for the full specification.

// ---------------------------------------------------------------------------
// Flag constants
// ---------------------------------------------------------------------------

/// Page table entry flag bits for x86-64.
pub mod flags {
    /// **Present** (P, bit 0).
    ///
    /// The CPU raises a `#PF` on any access to an entry where this bit is
    /// clear.  Must be set for all entries in the active page-table walk.
    pub const PRESENT: u64 = 1 << 0;

    /// **Writable** (R/W, bit 1).
    ///
    /// When clear the mapped page (or the sub-table it points to) is
    /// read-only.  A write triggers a `#PF` with error-code bit 1 set.
    pub const WRITABLE: u64 = 1 << 1;

    /// **User-accessible** (U/S, bit 2).
    ///
    /// When clear only ring-0 code can access the mapping.  Kernel page
    /// tables leave this clear for all entries.
    pub const USER_ACCESSIBLE: u64 = 1 << 2;

    /// **Page-level write-through** (PWT, bit 3).
    ///
    /// Selects write-through caching for this mapping (versus write-back).
    /// Rarely needed outside MMIO.
    pub const WRITE_THROUGH: u64 = 1 << 3;

    /// **Page-level cache disable** (PCD, bit 4).
    ///
    /// Disables caching entirely for this mapping.  Use for MMIO regions.
    pub const NO_CACHE: u64 = 1 << 4;

    /// **Accessed** (A, bit 5).
    ///
    /// Set by the CPU the first time this entry is used in a page-table walk.
    /// Software may clear it to implement page-replacement algorithms.
    pub const ACCESSED: u64 = 1 << 5;

    /// **Dirty** (D, bit 6).
    ///
    /// Set by the CPU on the first write to the page.  Only meaningful in
    /// PT (4 KiB) and PD huge-page entries; ignored in PML4 / PDPT.
    pub const DIRTY: u64 = 1 << 6;

    /// **Page size / huge page** (PS, bit 7).
    ///
    /// When set in a PD entry the entry maps a **2 MiB** page directly
    /// (skipping the PT level).  When set in a PDPT entry it maps a **1 GiB**
    /// page.  Must be clear in PML4 entries.
    pub const HUGE_PAGE: u64 = 1 << 7;

    /// **Global** (G, bit 8).
    ///
    /// When `CR4.PGE = 1`, global TLB entries survive a `MOV CR3` reload.
    /// Use for kernel pages that are never removed from the address space.
    pub const GLOBAL: u64 = 1 << 8;

    /// **No-execute** (XD/NX, bit 63).
    ///
    /// Prevents instruction fetches from this page.  Requires `EFER.NXE = 1`.
    /// Phase 1 does not enable NX; reserved for Phase 2+.
    pub const NO_EXECUTE: u64 = 1 << 63;

    /// Mask for the **physical address** bits in a page table entry.
    ///
    /// Bits [51:12] hold the physical page-frame number.  The remaining bits
    /// carry flags, available bits, or must be zero.
    pub const PHYS_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
}

// ---------------------------------------------------------------------------
// Entry type
// ---------------------------------------------------------------------------

/// A single 64-bit x86-64 page table entry.
///
/// The same struct is used at all four levels of the hierarchy:
///
/// | Level | Entry covers | `HUGE_PAGE` bit meaning    |
/// |-------|-------------|----------------------------|
/// | PML4  | 512 GiB     | must be 0 (reserved)       |
/// | PDPT  | 1 GiB       | 1 → 1 GiB huge page        |
/// | PD    | 2 MiB       | 1 → 2 MiB huge page        |
/// | PT    | 4 KiB       | n/a (always a 4 KiB frame) |
///
/// Construction is always explicit via [`PageTableEntry::new`]; the default
/// value is the absent entry `0` (present-bit clear).
#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// An absent (not-present) entry.  The CPU treats this as an unmapped
    /// address and raises `#PF` on access.
    pub const EMPTY: Self = Self(0);

    /// Create an entry mapping `phys_addr` with `entry_flags`.
    ///
    /// `phys_addr` is masked with [`flags::PHYS_ADDR_MASK`] before storing,
    /// so the caller does not need to clear the low 12 bits manually.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // PD entry: 2 MiB huge page at physical 0x200000
    /// let e = PageTableEntry::new(
    ///     0x200000,
    ///     flags::PRESENT | flags::WRITABLE | flags::HUGE_PAGE,
    /// );
    /// ```
    #[inline]
    pub const fn new(phys_addr: u64, entry_flags: u64) -> Self {
        Self((phys_addr & flags::PHYS_ADDR_MASK) | entry_flags)
    }

    /// Return the raw 64-bit value of this entry.
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Return the physical address this entry points to, with flag bits masked
    /// off.
    #[inline]
    pub const fn phys_addr(self) -> u64 {
        self.0 & flags::PHYS_ADDR_MASK
    }

    /// Return `true` if the [`flags::PRESENT`] bit is set.
    #[inline]
    pub const fn is_present(self) -> bool {
        self.0 & flags::PRESENT != 0
    }

    /// Return `true` if the [`flags::HUGE_PAGE`] bit is set.
    #[inline]
    pub const fn is_huge(self) -> bool {
        self.0 & flags::HUGE_PAGE != 0
    }

    /// Return `true` if the [`flags::WRITABLE`] bit is set.
    #[inline]
    pub const fn is_writable(self) -> bool {
        self.0 & flags::WRITABLE != 0
    }
}

impl core::fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PageTableEntry(phys={:#012x}, flags={:#05x})",
            self.phys_addr(),
            self.0 & !flags::PHYS_ADDR_MASK,
        )
    }
}
