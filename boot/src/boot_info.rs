//! Boot information structure.
//!
//! This module defines the `BootInfo` structure used internally by the
//! bootloader to collect UEFI data. Before handing off to the kernel,
//! this is converted to `KernelBootInfo` — the `#[repr(C)]` ABI struct
//! defined in `ferrous-boot-info` that uses no heap allocation.

use crate::memory::MemoryMap;
use ferrous_boot_info::{
    pixel_format, KernelBootInfo, KernelFramebuffer, KernelMemoryDescriptor,
};

/// Information passed from the bootloader to the kernel.
///
/// This structure contains all the data gathered during the UEFI boot
/// process that is needed for kernel initialization.
#[derive(Debug, Clone)]
pub struct BootInfo {
    /// Memory map from UEFI.
    memory_map: MemoryMap,

    /// ACPI RSDP address (if found).
    acpi_rsdp_address: Option<u64>,

    /// Framebuffer information (if available).
    framebuffer: Option<FramebufferInfo>,
}

impl BootInfo {
    /// Creates a new `BootInfo` with the given memory map.
    pub fn new(memory_map: MemoryMap) -> Self {
        Self {
            memory_map,
            acpi_rsdp_address: None,
            framebuffer: None,
        }
    }

    /// Creates a new `BootInfo` with all fields specified.
    pub fn with_all(
        memory_map: MemoryMap,
        acpi_rsdp_address: Option<u64>,
        framebuffer: Option<FramebufferInfo>,
    ) -> Self {
        Self {
            memory_map,
            acpi_rsdp_address,
            framebuffer,
        }
    }

    /// Returns a reference to the memory map.
    pub fn memory_map(&self) -> &MemoryMap {
        &self.memory_map
    }

    /// Returns the total memory in bytes.
    pub fn total_memory(&self) -> u64 {
        self.memory_map.total_memory()
    }

    /// Returns the total memory in megabytes.
    pub fn total_memory_mb(&self) -> u64 {
        self.total_memory() / (1024 * 1024)
    }

    /// Returns the usable memory in bytes.
    pub fn usable_memory(&self) -> u64 {
        self.memory_map.usable_memory()
    }

    /// Returns the usable memory in megabytes.
    pub fn usable_memory_mb(&self) -> u64 {
        self.usable_memory() / (1024 * 1024)
    }

    /// Returns the ACPI RSDP address if available.
    pub fn acpi_rsdp_address(&self) -> Option<u64> {
        self.acpi_rsdp_address
    }

    /// Sets the ACPI RSDP address.
    pub fn set_acpi_rsdp_address(&mut self, address: u64) {
        self.acpi_rsdp_address = Some(address);
    }

    /// Returns the framebuffer information if available.
    pub fn framebuffer(&self) -> Option<&FramebufferInfo> {
        self.framebuffer.as_ref()
    }

    /// Sets the framebuffer information.
    pub fn set_framebuffer(&mut self, framebuffer: FramebufferInfo) {
        self.framebuffer = Some(framebuffer);
    }
}

/// Framebuffer information from UEFI GOP.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Physical base address of the framebuffer.
    pub base_address: u64,

    /// Width in pixels.
    pub width: u32,

    /// Height in pixels.
    pub height: u32,

    /// Bytes per row (pitch).
    pub stride: u32,

    /// Pixel format.
    pub pixel_format: PixelFormat,
}

impl FramebufferInfo {
    /// Creates new framebuffer information.
    pub fn new(
        base_address: u64,
        width: u32,
        height: u32,
        stride: u32,
        pixel_format: PixelFormat,
    ) -> Self {
        Self {
            base_address,
            width,
            height,
            stride,
            pixel_format,
        }
    }

    /// Returns the size of the framebuffer in bytes.
    pub fn size(&self) -> u64 {
        self.stride as u64 * self.height as u64
    }

    /// Returns the bytes per pixel for this format.
    pub fn bytes_per_pixel(&self) -> u32 {
        self.pixel_format.bytes_per_pixel()
    }
}

/// Pixel format for the framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Red-Green-Blue (8 bits each, little endian).
    Rgb,
    /// Blue-Green-Red (8 bits each, little endian).
    Bgr,
    /// Custom bitmask format.
    Bitmask {
        red: u32,
        green: u32,
        blue: u32,
        reserved: u32,
    },
    /// Unknown format.
    Unknown,
}

impl PixelFormat {
    /// Returns the bytes per pixel for this format.
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            PixelFormat::Rgb | PixelFormat::Bgr => 4,
            PixelFormat::Bitmask { .. } => 4,
            PixelFormat::Unknown => 4,
        }
    }
}

impl Default for PixelFormat {
    fn default() -> Self {
        PixelFormat::Unknown
    }
}

impl BootInfo {
    /// Converts this heap-based `BootInfo` into a `KernelBootInfo`.
    ///
    /// The returned struct uses only fixed-size arrays and is safe to pass
    /// to the kernel after `exit_boot_services()` when the UEFI allocator
    /// is no longer available.
    pub fn to_kernel_boot_info(&self) -> KernelBootInfo {
        let mut kbi = KernelBootInfo::new();

        // Copy memory map descriptors into the fixed-size array.
        let mut count = 0;
        for region in self.memory_map.regions() {
            if count >= ferrous_boot_info::MAX_MEMORY_DESCRIPTORS {
                kbi.memory_map.truncated = true;
                break;
            }
            kbi.memory_map.descriptors[count] = KernelMemoryDescriptor {
                ty: memory_region_type_to_uefi_u32(&region.region_type),
                _pad: 0,
                phys_start: region.start,
                page_count: region.size / 4096,
                attribute: region.attributes,
            };
            count += 1;
        }
        kbi.memory_map.count = count;

        // Copy ACPI RSDP address.
        kbi.acpi_rsdp = self.acpi_rsdp_address.unwrap_or(0);

        // Copy framebuffer info if present.
        if let Some(fb) = &self.framebuffer {
            kbi.framebuffer = KernelFramebuffer {
                base: fb.base_address,
                size: fb.size(),
                width: fb.width,
                height: fb.height,
                stride: fb.stride,
                pixel_format: match fb.pixel_format {
                    PixelFormat::Rgb => pixel_format::RGB,
                    PixelFormat::Bgr => pixel_format::BGR,
                    PixelFormat::Bitmask { .. } => pixel_format::BITMASK,
                    PixelFormat::Unknown => pixel_format::UNKNOWN,
                },
            };
            kbi.has_framebuffer = true;
        }

        kbi
    }
}

/// Maps our `MemoryRegionType` back to the raw UEFI memory type u32.
fn memory_region_type_to_uefi_u32(ty: &crate::memory::MemoryRegionType) -> u32 {
    use crate::memory::MemoryRegionType::*;
    use ferrous_boot_info::memory_type;
    match ty {
        Reserved => memory_type::RESERVED,
        LoaderCode => memory_type::LOADER_CODE,
        LoaderData => memory_type::LOADER_DATA,
        BootServicesCode => memory_type::BOOT_SERVICES_CODE,
        BootServicesData => memory_type::BOOT_SERVICES_DATA,
        RuntimeServicesCode => memory_type::RUNTIME_SERVICES_CODE,
        RuntimeServicesData => memory_type::RUNTIME_SERVICES_DATA,
        Conventional | Usable => memory_type::CONVENTIONAL,
        Unusable => memory_type::UNUSABLE,
        AcpiReclaimable => memory_type::ACPI_RECLAIM,
        AcpiNvs => memory_type::ACPI_NON_VOLATILE,
        Mmio => memory_type::MMIO,
        MmioPortSpace => memory_type::MMIO_PORT_SPACE,
        Persistent => memory_type::PERSISTENT_MEMORY,
        Unknown => memory_type::RESERVED,
    }
}
