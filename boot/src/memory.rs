//! Memory map handling for UEFI boot.
//!
//! This module provides structures and utilities for working with the
//! UEFI memory map.

use alloc::vec::Vec;
use uefi::boot::MemoryType as UefiMemoryType;
use uefi::mem::memory_map::{MemoryMap as UefiMemoryMapTrait, MemoryMapOwned};

/// Type of memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionType {
    /// Available for general use.
    Usable,
    /// Reserved by firmware or hardware.
    Reserved,
    /// ACPI reclaimable memory.
    AcpiReclaimable,
    /// ACPI NVS.
    AcpiNvs,
    /// Memory-mapped I/O.
    Mmio,
    /// Memory-mapped I/O port space.
    MmioPortSpace,
    /// Boot services code.
    BootServicesCode,
    /// Boot services data.
    BootServicesData,
    /// Runtime services code.
    RuntimeServicesCode,
    /// Runtime services data.
    RuntimeServicesData,
    /// Loader code.
    LoaderCode,
    /// Loader data.
    LoaderData,
    /// Conventional memory.
    Conventional,
    /// Unusable memory.
    Unusable,
    /// Persistent memory.
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
    /// Physical start address.
    pub start: u64,
    /// Size in bytes.
    pub size: u64,
    /// Region type.
    pub region_type: MemoryRegionType,
    /// Memory attributes.
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

    /// Returns the end address.
    pub fn end(&self) -> u64 {
        self.start + self.size
    }
}

/// The system memory map.
#[derive(Debug, Clone)]
pub struct MemoryMap {
    regions: Vec<MemoryRegion>,
    total_memory: u64,
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

    /// Creates a memory map from UEFI MemoryMapOwned.
    pub fn from_uefi_memory_map(uefi_map: &MemoryMapOwned) -> Self {
        let mut regions = Vec::new();
        let mut total_memory = 0u64;
        let mut usable_memory = 0u64;

        const PAGE_SIZE: u64 = 4096;

        for desc in uefi_map.entries() {
            let start = desc.phys_start;
            let size = desc.page_count * PAGE_SIZE;
            let region_type = MemoryRegionType::from(desc.ty);
            let attributes = desc.att.bits();

            if !matches!(
                region_type,
                MemoryRegionType::Mmio | MemoryRegionType::MmioPortSpace
            ) {
                total_memory = total_memory.saturating_add(size);
            }

            if region_type.is_usable() {
                usable_memory = usable_memory.saturating_add(size);
            }

            regions.push(MemoryRegion::new(start, size, region_type, attributes));
        }

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
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}
