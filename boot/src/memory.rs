//! Memory map handling for UEFI boot.
//!
//! This module provides structures and utilities for working with the
//! UEFI memory map. It converts UEFI memory descriptors into a format
//! that can be used by the kernel.

use alloc::vec::Vec;
use uefi::table::boot::{MemoryDescriptor, MemoryType as UefiMemoryType};

/// Type of memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionType {
    /// Available for general use.
    Usable,
    /// Reserved by firmware or hardware.
    Reserved,
    /// ACPI reclaimable memory (can be used after reading ACPI tables).
    AcpiReclaimable,
    /// ACPI NVS (non-volatile storage).
    AcpiNvs,
    /// Memory-mapped I/O.
    Mmio,
    /// Memory-mapped I/O port space.
    MmioPortSpace,
    /// Boot services code (reclaimable after exit boot services).
    BootServicesCode,
    /// Boot services data (reclaimable after exit boot services).
    BootServicesData,
    /// Runtime services code (must be preserved).
    RuntimeServicesCode,
    /// Runtime services data (must be preserved).
    RuntimeServicesData,
    /// Loader code (our bootloader code).
    LoaderCode,
    /// Loader data (our bootloader data).
    LoaderData,
    /// Conventional memory (usable).
    Conventional,
    /// Unusable memory (bad memory).
    Unusable,
    /// Persistent memory (NVDIMM).
    Persistent,
    /// Unknown memory type.
    Unknown,
}

impl From<UefiMemoryType> for MemoryRegionType {
    fn from(uefi_type: UefiMemoryType) -> Self {
        match uefi_type {
            UefiMemoryType::RESERVED => MemoryRegionType::Reserved,
            UefiMemoryType::LOADER_CODE => MemoryRegionType::LoaderCode,
            UefiMemoryType::LOADER_DATA => MemoryRegionType::LoaderData,
            UefiMemoryType::BOOT_SERVICES_CODE => MemoryRegionType::BootServicesCode,
            UefiMemoryType::BOOT_SERVICES_DATA => MemoryRegionType::BootServicesData,
            UefiMemoryType::RUNTIME_SERVICES_CODE => MemoryRegionType::RuntimeServicesCode,
            UefiMemoryType::RUNTIME_SERVICES_DATA => MemoryRegionType::RuntimeServicesData,
            UefiMemoryType::CONVENTIONAL => MemoryRegionType::Conventional,
            UefiMemoryType::UNUSABLE => MemoryRegionType::Unusable,
            UefiMemoryType::ACPI_RECLAIM => MemoryRegionType::AcpiReclaimable,
            UefiMemoryType::ACPI_NON_VOLATILE => MemoryRegionType::AcpiNvs,
            UefiMemoryType::MMIO => MemoryRegionType::Mmio,
            UefiMemoryType::MMIO_PORT_SPACE => MemoryRegionType::MmioPortSpace,
            UefiMemoryType::PERSISTENT_MEMORY => MemoryRegionType::Persistent,
            _ => MemoryRegionType::Unknown,
        }
    }
}

impl MemoryRegionType {
    /// Returns true if this memory region is usable by the kernel.
    ///
    /// After exiting boot services, boot services code/data become usable.
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            MemoryRegionType::Usable
                | MemoryRegionType::Conventional
                | MemoryRegionType::BootServicesCode
                | MemoryRegionType::BootServicesData
                | MemoryRegionType::LoaderCode
                | MemoryRegionType::LoaderData
                | MemoryRegionType::AcpiReclaimable
        )
    }
}

/// A single memory region.
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Physical start address of the region.
    pub start: u64,
    /// Size of the region in bytes.
    pub size: u64,
    /// Type of the memory region.
    pub region_type: MemoryRegionType,
    /// UEFI memory attributes.
    pub attributes: u64,
}

impl MemoryRegion {
    /// Creates a new memory region.
    pub fn new(start: u64, size: u64, region_type: MemoryRegionType, attributes: u64) -> Self {
        Self {
            start,
            size,
            region_type,
            attributes,
        }
    }

    /// Returns the end address of the region (exclusive).
    pub fn end(&self) -> u64 {
        self.start + self.size
    }
}

/// The system memory map.
#[derive(Debug, Clone)]
pub struct MemoryMap {
    /// List of memory regions.
    regions: Vec<MemoryRegion>,
    /// Total memory in the system.
    total_memory: u64,
    /// Total usable memory (after boot services exit).
    usable_memory: u64,
}

impl MemoryMap {
    /// Creates a new empty memory map.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            total_memory: 0,
            usable_memory: 0,
        }
    }

    /// Creates a memory map from UEFI memory descriptors.
    pub fn from_uefi_descriptors<'a>(
        descriptors: impl ExactSizeIterator<Item = &'a MemoryDescriptor>,
    ) -> Self {
        let mut regions = Vec::new();
        let mut total_memory = 0u64;
        let mut usable_memory = 0u64;

        // Page size is 4KB
        const PAGE_SIZE: u64 = 4096;

        for desc in descriptors {
            let start = desc.phys_start;
            let size = desc.page_count * PAGE_SIZE;
            let region_type = MemoryRegionType::from(desc.ty);
            let attributes = desc.att.bits();

            // Track total memory (excluding MMIO)
            if !matches!(
                region_type,
                MemoryRegionType::Mmio | MemoryRegionType::MmioPortSpace
            ) {
                total_memory = total_memory.saturating_add(size);
            }

            // Track usable memory
            if region_type.is_usable() {
                usable_memory = usable_memory.saturating_add(size);
            }

            regions.push(MemoryRegion::new(start, size, region_type, attributes));
        }

        // Sort regions by start address
        regions.sort_by_key(|r| r.start);

        Self {
            regions,
            total_memory,
            usable_memory,
        }
    }

    /// Returns the number of memory regions.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Returns an iterator over the memory regions.
    pub fn regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.iter()
    }

    /// Returns the total memory in bytes.
    pub fn total_memory(&self) -> u64 {
        self.total_memory
    }

    /// Returns the total usable memory in bytes.
    pub fn usable_memory(&self) -> u64 {
        self.usable_memory
    }

    /// Returns an iterator over only usable memory regions.
    pub fn usable_regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.iter().filter(|r| r.region_type.is_usable())
    }

    /// Finds the largest usable memory region.
    pub fn largest_usable_region(&self) -> Option<&MemoryRegion> {
        self.usable_regions().max_by_key(|r| r.size)
    }
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_region_type_usable() {
        assert!(MemoryRegionType::Conventional.is_usable());
        assert!(MemoryRegionType::BootServicesCode.is_usable());
        assert!(MemoryRegionType::BootServicesData.is_usable());
        assert!(!MemoryRegionType::Reserved.is_usable());
        assert!(!MemoryRegionType::Mmio.is_usable());
        assert!(!MemoryRegionType::RuntimeServicesCode.is_usable());
    }

    #[test]
    fn test_memory_region_end() {
        let region = MemoryRegion::new(0x1000, 0x2000, MemoryRegionType::Conventional, 0);
        assert_eq!(region.end(), 0x3000);
    }
}
