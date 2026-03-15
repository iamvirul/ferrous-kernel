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

// ---------------------------------------------------------------------------
// Tests
//
// Even though this crate is #![no_std], cargo test links std for the test
// binary. `extern crate std` makes it explicit so #[test] resolves correctly.
// ---------------------------------------------------------------------------

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ABI constants
    // -----------------------------------------------------------------------

    /// Magic value must never change — it is embedded in the bootloader and
    /// kernel independently. Any change breaks the boot ABI silently.
    #[test]
    fn boot_info_magic_is_stable() {
        assert_eq!(BOOT_INFO_MAGIC, 0xFE220B00_CAFE0001);
    }

    #[test]
    fn boot_info_version_is_one() {
        assert_eq!(BOOT_INFO_VERSION, 1);
    }

    #[test]
    fn max_memory_descriptors_fits_real_hardware() {
        // OVMF produces ~40 entries; real hardware rarely exceeds 128.
        // 256 must remain the upper bound so the struct fits in a static.
        assert_eq!(MAX_MEMORY_DESCRIPTORS, 256);
        assert!(MAX_MEMORY_DESCRIPTORS >= 128);
    }

    // -----------------------------------------------------------------------
    // KernelBootInfo validity
    // -----------------------------------------------------------------------

    #[test]
    fn new_boot_info_is_valid() {
        let info = KernelBootInfo::new();
        assert!(
            info.is_valid(),
            "KernelBootInfo::new() must produce a valid struct (magic + version set)"
        );
    }

    #[test]
    fn corrupt_magic_fails_validation() {
        let mut info = KernelBootInfo::new();
        info.magic = 0xDEAD_BEEF;
        assert!(!info.is_valid(), "wrong magic must fail is_valid()");
    }

    #[test]
    fn corrupt_version_fails_validation() {
        let mut info = KernelBootInfo::new();
        info.version = 0;
        assert!(!info.is_valid(), "wrong version must fail is_valid()");
    }

    #[test]
    fn zeroed_magic_fails_validation() {
        let mut info = KernelBootInfo::new();
        info.magic = 0;
        assert!(!info.is_valid());
    }

    // -----------------------------------------------------------------------
    // KernelBootInfo defaults
    // -----------------------------------------------------------------------

    #[test]
    fn new_boot_info_has_no_acpi() {
        let info = KernelBootInfo::new();
        assert_eq!(info.acpi_rsdp, 0);
    }

    #[test]
    fn new_boot_info_has_no_framebuffer() {
        let info = KernelBootInfo::new();
        assert!(!info.has_framebuffer);
    }

    #[test]
    fn new_boot_info_has_empty_memory_map() {
        let info = KernelBootInfo::new();
        assert_eq!(info.memory_map.count, 0);
        assert!(!info.memory_map.truncated);
        assert_eq!(info.memory_map.entries().len(), 0);
    }

    #[test]
    fn bootloader_name_is_ferrous_boot() {
        let info = KernelBootInfo::new();
        let name_bytes = &info.bootloader_name;
        // Null-terminated ASCII: "ferrous-boot" followed by zeros.
        assert_eq!(&name_bytes[..12], b"ferrous-boot");
        assert_eq!(name_bytes[12], 0, "name must be null-terminated");
    }

    // -----------------------------------------------------------------------
    // KernelMemoryDescriptor
    // -----------------------------------------------------------------------

    #[test]
    fn memory_descriptor_size_bytes() {
        let desc = KernelMemoryDescriptor {
            ty: memory_type::CONVENTIONAL,
            _pad: 0,
            phys_start: 0x1000,
            page_count: 16,
            attribute: 0,
        };
        // 16 pages × 4096 bytes/page = 65536 bytes
        assert_eq!(desc.size_bytes(), 16 * 4096);
    }

    #[test]
    fn zero_pages_gives_zero_bytes() {
        let desc = KernelMemoryDescriptor {
            ty: memory_type::CONVENTIONAL,
            _pad: 0,
            phys_start: 0,
            page_count: 0,
            attribute: 0,
        };
        assert_eq!(desc.size_bytes(), 0);
    }

    #[test]
    fn usable_memory_types() {
        let usable = [
            memory_type::CONVENTIONAL,
            memory_type::BOOT_SERVICES_CODE,
            memory_type::BOOT_SERVICES_DATA,
            memory_type::LOADER_CODE,
            memory_type::LOADER_DATA,
            memory_type::ACPI_RECLAIM,
        ];
        for ty in usable {
            let desc = KernelMemoryDescriptor {
                ty,
                _pad: 0,
                phys_start: 0,
                page_count: 1,
                attribute: 0,
            };
            assert!(desc.is_usable(), "memory type {} should be usable", ty);
        }
    }

    #[test]
    fn reserved_memory_types_are_not_usable() {
        let reserved = [
            memory_type::RESERVED,
            memory_type::RUNTIME_SERVICES_CODE,
            memory_type::RUNTIME_SERVICES_DATA,
            memory_type::ACPI_NON_VOLATILE,
            memory_type::MMIO,
            memory_type::MMIO_PORT_SPACE,
            memory_type::UNUSABLE,
        ];
        for ty in reserved {
            let desc = KernelMemoryDescriptor {
                ty,
                _pad: 0,
                phys_start: 0,
                page_count: 1,
                attribute: 0,
            };
            assert!(!desc.is_usable(), "memory type {} should NOT be usable", ty);
        }
    }

    // -----------------------------------------------------------------------
    // KernelMemoryMap
    // -----------------------------------------------------------------------

    #[test]
    fn memory_map_entries_respects_count() {
        let mut map = KernelMemoryMap::new();
        map.count = 3;
        assert_eq!(map.entries().len(), 3);
    }

    #[test]
    fn memory_map_full_count_is_max() {
        let mut map = KernelMemoryMap::new();
        map.count = MAX_MEMORY_DESCRIPTORS;
        assert_eq!(map.entries().len(), MAX_MEMORY_DESCRIPTORS);
    }

    // -----------------------------------------------------------------------
    // KernelFramebuffer
    // -----------------------------------------------------------------------

    #[test]
    fn zeroed_framebuffer_has_unknown_pixel_format() {
        let fb = KernelFramebuffer::zeroed();
        assert_eq!(fb.pixel_format, pixel_format::UNKNOWN);
        assert_eq!(fb.base, 0);
        assert_eq!(fb.width, 0);
        assert_eq!(fb.height, 0);
    }

    #[test]
    fn pixel_format_constants_are_distinct() {
        assert_ne!(pixel_format::RGB, pixel_format::BGR);
        assert_ne!(pixel_format::RGB, pixel_format::BITMASK);
        assert_ne!(pixel_format::RGB, pixel_format::UNKNOWN);
        assert_ne!(pixel_format::BGR, pixel_format::BITMASK);
        assert_ne!(pixel_format::BGR, pixel_format::UNKNOWN);
        assert_ne!(pixel_format::BITMASK, pixel_format::UNKNOWN);
    }

    // -----------------------------------------------------------------------
    // Struct size stability (part of the boot ABI — changing these is a
    // breaking change that requires a BOOT_INFO_VERSION bump)
    // -----------------------------------------------------------------------

    #[test]
    fn kernel_memory_descriptor_size() {
        // 4 (ty) + 4 (_pad) + 8 (phys_start) + 8 (page_count) + 8 (attribute) = 32
        assert_eq!(core::mem::size_of::<KernelMemoryDescriptor>(), 32);
    }

    #[test]
    fn kernel_memory_descriptor_alignment() {
        assert_eq!(core::mem::align_of::<KernelMemoryDescriptor>(), 8);
    }

    #[test]
    fn kernel_framebuffer_size() {
        // 8 (base) + 8 (size) + 4 (width) + 4 (height) + 4 (stride) + 4 (pixel_format) = 32
        assert_eq!(core::mem::size_of::<KernelFramebuffer>(), 32);
    }
}
