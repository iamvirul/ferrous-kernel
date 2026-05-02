//! The [`PageTable`] type — one level of the x86-64 page-table hierarchy.

use super::entry::PageTableEntry;

/// One level of the x86-64 four-level page table hierarchy.
///
/// A `PageTable` is exactly 4 KiB: 512 × 8-byte [`PageTableEntry`] values.
/// It must be **4 KiB-aligned** so its physical address can be stored in a
/// parent entry without losing low-order bits.
///
/// The same type is used at all four levels (PML4, PDPT, PD, PT).  The
/// interpretation of individual entry flags varies by level; see
/// [`entry::flags`].
///
/// # Static placement
///
/// `PageTable::new()` is a `const fn` returning all-zero (absent) entries.
/// A `static mut PageTable` therefore lives in BSS — zero-initialised by the
/// loader with no binary-image cost.
///
/// # Indexing
///
/// Use [`PageTable::entry`] and [`PageTable::entry_mut`] rather than
/// direct array indexing so that bounds checks are expressed clearly.
///
/// [`entry::flags`]: super::entry::flags
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Create an all-absent (not-present) page table.
    ///
    /// Suitable for placement in a `static` or `static mut` — produces the
    /// all-zero BSS pattern, so no binary-image overhead.
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::EMPTY; 512],
        }
    }

    /// Set every entry to absent ([`PageTableEntry::EMPTY`]).
    ///
    /// Use this to reset a table that may have been populated previously.
    pub fn zero(&mut self) {
        for entry in &mut self.entries {
            *entry = PageTableEntry::EMPTY;
        }
    }

    /// Return a shared reference to the entry at `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= 512`.
    #[inline]
    pub fn entry(&self, index: usize) -> &PageTableEntry {
        &self.entries[index]
    }

    /// Return a mutable reference to the entry at `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= 512`.
    #[inline]
    pub fn entry_mut(&mut self, index: usize) -> &mut PageTableEntry {
        &mut self.entries[index]
    }

    /// Iterate over all 512 entries (shared).
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, PageTableEntry> {
        self.entries.iter()
    }

    /// Iterate over all 512 entries (mutable).
    #[inline]
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, PageTableEntry> {
        self.entries.iter_mut()
    }

    /// Return the number of present entries in this table.
    pub fn present_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_present()).count()
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for PageTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PageTable({} present entries)", self.present_count())
    }
}
