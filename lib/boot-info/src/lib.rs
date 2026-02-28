//! Shared boot information ABI between bootloader and kernel.
//!
//! This crate defines the `KernelBootInfo` structure and its supporting
//! types. These are passed from the UEFI bootloader to the kernel entry
//! point and form the sole contract between them.
//!
//! All types are `#[repr(C)]` with fixed-size arrays — no heap, no Vec,
//! no UEFI dependency. This ensures the layout is stable across separately
//! compiled crates and remains valid after boot services have exited.

#![no_std]

/// Magic sentinel stored in `KernelBootInfo.magic`.
///
/// The kernel checks this at entry to detect stale or corrupt pointers.
pub const BOOT_INFO_MAGIC: u64 = 0xFE220B00_CAFE0001;

/// ABI version. Increment when the layout of `KernelBootInfo` changes.
pub const BOOT_INFO_VERSION: u32 = 1;

/// Maximum number of UEFI memory descriptors stored in `KernelMemoryMap`.
///
/// OVMF (QEMU) typically produces 20–40 entries. Real hardware rarely
/// exceeds 128. 256 is a safe upper bound; if firmware exceeds this the
/// kernel will detect truncation and halt.
pub const MAX_MEMORY_DESCRIPTORS: usize = 256;

/// Pixel format codes stored in `KernelFramebuffer.pixel_format`.
pub mod pixel_format {
    pub const RGB: u32 = 0;
    pub const BGR: u32 = 1;
    pub const BITMASK: u32 = 2;
    pub const UNKNOWN: u32 = 0xFF;
}

/// UEFI memory type codes mirrored for the kernel (no uefi crate dependency).
pub mod memory_type {
    pub const RESERVED: u32 = 0;
    pub const LOADER_CODE: u32 = 1;
    pub const LOADER_DATA: u32 = 2;
    pub const BOOT_SERVICES_CODE: u32 = 3;
    pub const BOOT_SERVICES_DATA: u32 = 4;
    pub const RUNTIME_SERVICES_CODE: u32 = 5;
    pub const RUNTIME_SERVICES_DATA: u32 = 6;
    pub const CONVENTIONAL: u32 = 7;
    pub const UNUSABLE: u32 = 8;
    pub const ACPI_RECLAIM: u32 = 9;
    pub const ACPI_NON_VOLATILE: u32 = 10;
    pub const MMIO: u32 = 11;
    pub const MMIO_PORT_SPACE: u32 = 12;
    pub const PERSISTENT_MEMORY: u32 = 14;
}

/// A single UEFI memory descriptor, mirrored for the kernel.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct KernelMemoryDescriptor {
    /// UEFI memory type (see `memory_type` module constants).
    pub ty: u32,
    pub _pad: u32,
    /// Physical start address (page-aligned).
    pub phys_start: u64,
    /// Number of 4 KiB pages in this region.
    pub page_count: u64,
    /// UEFI memory attribute flags.
    pub attribute: u64,
}

impl KernelMemoryDescriptor {
    /// Returns the size of this region in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.page_count * 4096
    }

    /// Returns true if this region is usable by the kernel after boot.
    ///
    /// Conventional memory and boot-services memory become available after
    /// `exit_boot_services()`. Loader memory is also reclaimable once the
    /// kernel no longer needs the bootloader.
    pub fn is_usable(&self) -> bool {
        matches!(
            self.ty,
            memory_type::CONVENTIONAL
                | memory_type::BOOT_SERVICES_CODE
                | memory_type::BOOT_SERVICES_DATA
                | memory_type::LOADER_CODE
                | memory_type::LOADER_DATA
                | memory_type::ACPI_RECLAIM
        )
    }
}

/// Fixed-size memory map passed to the kernel.
#[repr(C)]
pub struct KernelMemoryMap {
    /// Descriptors copied from the UEFI memory map.
    pub descriptors: [KernelMemoryDescriptor; MAX_MEMORY_DESCRIPTORS],
    /// Number of valid entries in `descriptors`.
    pub count: usize,
    /// True if the source map had more entries than `MAX_MEMORY_DESCRIPTORS`.
    pub truncated: bool,
    pub _pad: [u8; 7],
}

impl KernelMemoryMap {
    pub const fn new() -> Self {
        Self {
            descriptors: [KernelMemoryDescriptor {
                ty: 0,
                _pad: 0,
                phys_start: 0,
                page_count: 0,
                attribute: 0,
            }; MAX_MEMORY_DESCRIPTORS],
            count: 0,
            truncated: false,
            _pad: [0; 7],
        }
    }

    /// Returns an iterator over the valid (populated) descriptors.
    pub fn entries(&self) -> &[KernelMemoryDescriptor] {
        &self.descriptors[..self.count]
    }
}

/// Framebuffer information from UEFI GOP, or zeroed if unavailable.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct KernelFramebuffer {
    /// Physical base address of the framebuffer.
    pub base: u64,
    /// Total size of the framebuffer in bytes.
    pub size: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row (stride/pitch).
    pub stride: u32,
    /// Pixel format (see `pixel_format` module constants).
    pub pixel_format: u32,
}

impl KernelFramebuffer {
    pub const fn zeroed() -> Self {
        Self {
            base: 0,
            size: 0,
            width: 0,
            height: 0,
            stride: 0,
            pixel_format: pixel_format::UNKNOWN,
        }
    }
}

/// The boot information contract passed from bootloader to kernel.
///
/// This struct is populated by the bootloader before `exit_boot_services()`,
/// stored in a statically allocated buffer, and its address is passed to
/// `kernel_entry` as the first argument (RDI register, SysV AMD64 ABI).
///
/// The kernel **must** validate `magic` and `version` before accessing any
/// other field. If validation fails, the kernel must halt immediately.
#[repr(C)]
pub struct KernelBootInfo {
    /// Must equal `BOOT_INFO_MAGIC`. Detects stale/corrupt pointers.
    pub magic: u64,

    /// Must equal `BOOT_INFO_VERSION`. Detects ABI drift.
    pub version: u32,
    pub _pad: u32,

    /// Physical memory map captured before boot services exit.
    pub memory_map: KernelMemoryMap,

    /// Physical address of the ACPI RSDP, or 0 if not found.
    pub acpi_rsdp: u64,

    /// Framebuffer information. Check `has_framebuffer` before use.
    pub framebuffer: KernelFramebuffer,

    /// True if `framebuffer` contains valid data.
    pub has_framebuffer: bool,
    pub _pad2: [u8; 7],

    /// Null-terminated bootloader name string (ASCII).
    pub bootloader_name: [u8; 32],
}

impl KernelBootInfo {
    /// Creates a zeroed `KernelBootInfo` with magic and version set.
    ///
    /// Use this to initialise the static buffer in the bootloader, then
    /// populate each field before calling `exit_boot_services()`.
    pub const fn new() -> Self {
        Self {
            magic: BOOT_INFO_MAGIC,
            version: BOOT_INFO_VERSION,
            _pad: 0,
            memory_map: KernelMemoryMap::new(),
            acpi_rsdp: 0,
            framebuffer: KernelFramebuffer::zeroed(),
            has_framebuffer: false,
            _pad2: [0; 7],
            bootloader_name: *b"ferrous-boot\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        }
    }

    /// Returns true if this `KernelBootInfo` has a valid magic and version.
    ///
    /// The kernel must call this before accessing any other field.
    pub fn is_valid(&self) -> bool {
        self.magic == BOOT_INFO_MAGIC && self.version == BOOT_INFO_VERSION
    }
}
