//! Virtual memory management — address types and page table infrastructure.
//!
//! This module provides the building blocks for the x86-64 four-level
//! page table hierarchy and type-safe virtual/physical address wrappers.
//!
//! # x86-64 paging overview (4-level mode)
//!
//! ```text
//! Virtual Address (48-bit canonical)
//!   Bits [47:39] → PML4 index    (512 GiB per entry)
//!   Bits [38:30] → PDPT index    (  1 GiB per entry)
//!   Bits [29:21] → PD index      (  2 MiB per entry; or 2 MiB huge page)
//!   Bits [20:12] → PT index      (  4 KiB per entry)
//!   Bits [11:0]  → page offset   (within the mapped frame)
//! ```
//!
//! Phase 1 uses **2 MiB huge pages** (PS=1 in PD entries), skipping the PT
//! level entirely.  Phase 2 will add 4 KiB pages, fine-grained access
//! control, and demand paging.
//!
//! # Sub-modules
//!
//! | Module            | Contents                                       |
//! |-------------------|------------------------------------------------|
//! | [`entry`]         | [`PageTableEntry`] + [`flags`] constants       |
//! | [`table`]         | [`PageTable`] — 512-entry, 4 KiB-aligned table |
//! | [`mapper`]        | [`KernelPageTable`] — Phase-1 root structure   |

pub mod entry;
pub mod mapper;
pub mod table;

pub use entry::{flags, PageTableEntry};
pub use mapper::{
    flush_tlb_all, invlpg, ActivePageTable, FrameAllocate, KernelPageTable, MapError, UnmapError,
};
pub use table::PageTable;

// ---------------------------------------------------------------------------
// Virtual address
// ---------------------------------------------------------------------------

/// A canonical x86-64 virtual address.
///
/// In 4-level paging mode, only bits [47:0] are significant.  Bits [63:48]
/// must be identical copies of bit 47 — this is called the **canonical form**
/// requirement.  Accessing a non-canonical address causes a `#GP` fault.
///
/// Two canonical ranges exist:
///
/// | Range                                       | Bits [63:48] |
/// |---------------------------------------------|--------------|
/// | `0x0000_0000_0000_0000..=0x0000_7FFF_FFFF_FFFF` | all-zero     |
/// | `0xFFFF_8000_0000_0000..=0xFFFF_FFFF_FFFF_FFFF` | all-one      |
///
/// The gap between them is the **non-canonical hole**.  Ferrous places the
/// kernel higher-half window at [`VirtualAddress::HIGHER_HALF_BASE`]
/// (`0xFFFF_8000_0000_0000`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtualAddress(u64);

impl VirtualAddress {
    /// The first address of the kernel's higher-half window.
    ///
    /// Corresponds to PML4 index 256 in 4-level paging.
    pub const HIGHER_HALF_BASE: Self = Self(0xFFFF_8000_0000_0000);

    /// Construct a [`VirtualAddress`], returning `None` for non-canonical
    /// values (bits [63:48] not a sign-extension of bit 47).
    #[inline]
    pub const fn try_new(addr: u64) -> Option<Self> {
        // A canonical address has bits [63:48] either all-zero (bit 47 = 0)
        // or all-one (bit 47 = 1).  Shift right 47 produces 0x0 or 0x1_FFFF.
        let top = addr >> 47;
        if top == 0 || top == 0x1_FFFF {
            Some(Self(addr))
        } else {
            None
        }
    }

    /// Construct without a canonicality check.
    ///
    /// # Safety
    ///
    /// `addr` must satisfy the canonical-form requirement (bits [63:48] are a
    /// sign-extension of bit 47).  A non-canonical value stored here will
    /// cause a `#GP` if the address is ever used in a memory reference.
    #[inline]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        Self(addr)
    }

    /// Return the raw 64-bit value.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// PML4 index: bits [47:39].
    #[inline]
    pub const fn pml4_index(self) -> usize {
        ((self.0 >> 39) & 0x1FF) as usize
    }

    /// PDPT index: bits [38:30].
    #[inline]
    pub const fn pdpt_index(self) -> usize {
        ((self.0 >> 30) & 0x1FF) as usize
    }

    /// PD index: bits [29:21].
    #[inline]
    pub const fn pd_index(self) -> usize {
        ((self.0 >> 21) & 0x1FF) as usize
    }

    /// PT index: bits [20:12].
    #[inline]
    pub const fn pt_index(self) -> usize {
        ((self.0 >> 12) & 0x1FF) as usize
    }

    /// 12-bit page offset: bits [11:0].
    #[inline]
    pub const fn page_offset(self) -> u64 {
        self.0 & 0xFFF
    }
}

impl core::fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VirtualAddress({:#018x})", self.0)
    }
}

impl core::fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Physical address
// ---------------------------------------------------------------------------

/// A physical memory address.
///
/// On x86-64 the physical address space is up to 52 bits wide (MAXPHYADDR,
/// queryable via CPUID).  Ferrous Phase 1 tracks up to 64 GiB (36 bits),
/// matching the bitmap allocator's capacity.
///
/// Unlike [`VirtualAddress`], physical addresses have no canonical-form
/// requirement — any value in the supported range is valid.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    /// Construct a `PhysicalAddress` from a raw value.
    ///
    /// No alignment or range validation is performed; the caller is
    /// responsible for ensuring the value is a meaningful physical address.
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Return the raw 64-bit value.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Return `true` if the address is 4 KiB-aligned (bits [11:0] are zero).
    #[inline]
    pub const fn is_page_aligned(self) -> bool {
        self.0 & 0xFFF == 0
    }

    /// Return `true` if the address is 2 MiB-aligned (bits [20:0] are zero).
    #[inline]
    pub const fn is_huge_page_aligned(self) -> bool {
        self.0 & 0x1F_FFFF == 0
    }
}

impl core::fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PhysicalAddress({:#018x})", self.0)
    }
}

impl core::fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}
