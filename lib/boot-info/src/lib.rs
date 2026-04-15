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
// Memory map parsing — higher-level view of KernelMemoryMap
//
// These types are consumed by the kernel's physical memory allocator.
// They live here (rather than in the kernel crate) because they operate
// purely on KernelMemoryMap data and can be tested on the host without
// targeting x86_64-unknown-none.
// ---------------------------------------------------------------------------

/// High-level classification of a UEFI memory region.
///
/// Groups the raw UEFI memory type codes into kernel-relevant categories.
/// Use this to decide whether a region may be allocated, reclaimed, or
/// left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionKind {
    /// Available for physical page allocation immediately after boot.
    Usable,
    /// Occupied by bootloader code/data; reclaimable once the kernel no
    /// longer needs the bootloader (after Phase 1 handoff is complete).
    BootloaderReclaimable,
    /// Holds ACPI tables; reclaimable after ACPI initialisation (Phase 2+).
    AcpiReclaimable,
    /// ACPI firmware non-volatile storage — preserve indefinitely.
    AcpiNonVolatile,
    /// Firmware runtime code/data — must remain identity-mapped forever.
    FirmwareRuntime,
    /// Memory-mapped I/O — not RAM, never allocate.
    Mmio,
    /// NVDIMM persistent memory — requires special driver handling.
    PersistentMemory,
    /// Defective RAM reported by firmware.
    Unusable,
    /// Firmware-reserved or unrecognised type — do not touch.
    Reserved,
}

impl MemoryRegionKind {
    /// True if the physical allocator may use this region after boot
    /// reclamation completes (conventional + bootloader + ACPI reclaimable).
    #[inline]
    pub fn is_reclaimable_after_boot(self) -> bool {
        matches!(
            self,
            Self::Usable | Self::BootloaderReclaimable | Self::AcpiReclaimable
        )
    }

    /// True only for conventional memory — immediately available before any
    /// reclamation pass.
    #[inline]
    pub fn is_immediately_usable(self) -> bool {
        self == Self::Usable
    }

    /// Short human-readable label for serial diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Usable => "Conventional",
            Self::BootloaderReclaimable => "BootloaderReclaimable",
            Self::AcpiReclaimable => "AcpiReclaimable",
            Self::AcpiNonVolatile => "AcpiNonVolatile",
            Self::FirmwareRuntime => "FirmwareRuntime",
            Self::Mmio => "Mmio",
            Self::PersistentMemory => "PersistentMemory",
            Self::Unusable => "Unusable",
            Self::Reserved => "Reserved",
        }
    }
}

impl From<u32> for MemoryRegionKind {
    fn from(ty: u32) -> Self {
        match ty {
            memory_type::CONVENTIONAL => Self::Usable,
            memory_type::LOADER_CODE
            | memory_type::LOADER_DATA
            | memory_type::BOOT_SERVICES_CODE
            | memory_type::BOOT_SERVICES_DATA => Self::BootloaderReclaimable,
            memory_type::ACPI_RECLAIM => Self::AcpiReclaimable,
            memory_type::ACPI_NON_VOLATILE => Self::AcpiNonVolatile,
            memory_type::RUNTIME_SERVICES_CODE | memory_type::RUNTIME_SERVICES_DATA => {
                Self::FirmwareRuntime
            }
            memory_type::MMIO | memory_type::MMIO_PORT_SPACE => Self::Mmio,
            memory_type::PERSISTENT_MEMORY => Self::PersistentMemory,
            memory_type::UNUSABLE => Self::Unusable,
            _ => Self::Reserved, // RESERVED (0) and any unknown type
        }
    }
}

/// Statistics derived from a parsed memory map.
///
/// Computed once during [`MemoryMap::parse()`] and cached inside the map.
#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    /// Total physical RAM in bytes (excludes MMIO and firmware runtime).
    pub total_bytes: u64,
    /// Bytes of immediately-usable conventional memory.
    pub usable_bytes: u64,
    /// Bytes reclaimable after boot (bootloader + ACPI reclaimable).
    pub reclaimable_bytes: u64,
    /// Number of valid (non-zero-size) regions in the parsed map.
    pub region_count: usize,
    /// Number of immediately-usable (conventional) regions.
    pub usable_region_count: usize,
    /// True if the bootloader truncated the map (source had more entries than
    /// [`MAX_MEMORY_DESCRIPTORS`]).
    pub is_truncated: bool,
}

impl MemoryStats {
    /// Total bytes the allocator may use after all reclamation passes.
    #[inline]
    pub fn total_usable_after_boot(self) -> u64 {
        self.usable_bytes.saturating_add(self.reclaimable_bytes)
    }
}

/// Errors returned by [`MemoryMap::parse()`].
#[derive(Debug)]
pub enum ParseError {
    /// The memory map contains zero valid (non-zero-size) entries.
    Empty,
    /// A descriptor has a non-page-aligned base address (UEFI spec violation).
    UnalignedBase {
        /// Zero-based index of the offending descriptor in the source map.
        index: usize,
        /// The misaligned physical address.
        phys_start: u64,
    },
}

/// Zero-value descriptor used to initialise the fixed-size array.
///
/// `KernelMemoryDescriptor` is `Copy` and all-zero is a valid sentinel value
/// (type 0 = RESERVED, size 0 — filtered out during parse).
const ZERO_DESC: KernelMemoryDescriptor = KernelMemoryDescriptor {
    ty: 0,
    _pad: 0,
    phys_start: 0,
    page_count: 0,
    attribute: 0,
};

/// Parsed and validated physical memory map.
///
/// Constructed from the bootloader-provided [`KernelMemoryMap`] during early
/// kernel initialisation. After construction this is effectively immutable and
/// serves as the authoritative physical memory layout for the allocator
/// (Phase 1.3.2 and beyond).
///
/// # Obtaining an instance
///
/// In the kernel, use `kernel::memory::init(&boot_info.memory_map)` to
/// initialise the global instance, then `kernel::memory::get()` to borrow it.
pub struct MemoryMap {
    // Not derived — KernelMemoryDescriptor's large fixed array makes Debug
    // output impractical and KernelMemoryDescriptor doesn't derive Debug.
    // Use stats() for diagnostic output.
    descriptors: [KernelMemoryDescriptor; MAX_MEMORY_DESCRIPTORS],
    count: usize,
    stats: MemoryStats,
}

impl MemoryMap {
    /// Parse and validate the bootloader-provided memory map.
    ///
    /// Entries with `page_count == 0` are silently skipped — some firmware
    /// produces zero-size descriptors as padding.
    ///
    /// # Errors
    ///
    /// - [`ParseError::Empty`]: every entry has zero pages (or `count == 0`).
    /// - [`ParseError::UnalignedBase`]: a descriptor's `phys_start` is not
    ///   4 KiB aligned (guaranteed by the UEFI spec; this catches corrupt maps).
    pub fn parse(source: &KernelMemoryMap) -> Result<Self, ParseError> {
        let mut descriptors = [ZERO_DESC; MAX_MEMORY_DESCRIPTORS];
        let mut count = 0usize;

        let mut total_bytes: u64 = 0;
        let mut usable_bytes: u64 = 0;
        let mut reclaimable_bytes: u64 = 0;
        let mut usable_region_count: usize = 0;

        for (i, desc) in source.entries().iter().enumerate() {
            // Skip zero-size entries emitted by some firmware.
            if desc.page_count == 0 {
                continue;
            }

            // UEFI spec §7.2: EFI_PHYSICAL_ADDRESS must be page-aligned.
            if desc.phys_start & 0xFFF != 0 {
                return Err(ParseError::UnalignedBase {
                    index: i,
                    phys_start: desc.phys_start,
                });
            }

            let size = desc.size_bytes();
            let kind = MemoryRegionKind::from(desc.ty);

            // Accumulate RAM totals, excluding address-space holes (MMIO).
            match kind {
                MemoryRegionKind::Mmio | MemoryRegionKind::FirmwareRuntime => {}
                _ => total_bytes = total_bytes.saturating_add(size),
            }

            match kind {
                MemoryRegionKind::Usable => {
                    usable_bytes = usable_bytes.saturating_add(size);
                    usable_region_count += 1;
                }
                MemoryRegionKind::BootloaderReclaimable | MemoryRegionKind::AcpiReclaimable => {
                    reclaimable_bytes = reclaimable_bytes.saturating_add(size);
                }
                _ => {}
            }

            descriptors[count] = *desc;
            count += 1;
        }

        if count == 0 {
            return Err(ParseError::Empty);
        }

        Ok(Self {
            descriptors,
            count,
            stats: MemoryStats {
                total_bytes,
                usable_bytes,
                reclaimable_bytes,
                region_count: count,
                usable_region_count,
                is_truncated: source.truncated,
            },
        })
    }

    /// Returns memory statistics computed during parsing.
    #[inline]
    pub fn stats(&self) -> &MemoryStats {
        &self.stats
    }

    /// Returns all valid (non-zero-size) memory descriptors.
    #[inline]
    pub fn regions(&self) -> &[KernelMemoryDescriptor] {
        &self.descriptors[..self.count]
    }

    /// Iterates over all immediately-usable (conventional) regions.
    pub fn usable_regions(&self) -> impl Iterator<Item = &KernelMemoryDescriptor> {
        self.regions()
            .iter()
            .filter(|d| MemoryRegionKind::from(d.ty) == MemoryRegionKind::Usable)
    }

    /// Iterates over all regions usable after boot reclamation.
    ///
    /// Includes conventional, bootloader-reclaimable, and ACPI-reclaimable
    /// regions.
    pub fn reclaimable_regions(&self) -> impl Iterator<Item = &KernelMemoryDescriptor> {
        self.regions()
            .iter()
            .filter(|d| MemoryRegionKind::from(d.ty).is_reclaimable_after_boot())
    }

    /// Iterates over all regions matching the given UEFI memory type constant.
    ///
    /// Use the [`memory_type`] module constants as the `ty` argument.
    pub fn regions_of_type(&self, ty: u32) -> impl Iterator<Item = &KernelMemoryDescriptor> {
        self.regions().iter().filter(move |d| d.ty == ty)
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
    // MemoryRegionKind — classification
    // -----------------------------------------------------------------------

    fn make_desc(ty: u32, phys_start: u64, page_count: u64) -> KernelMemoryDescriptor {
        KernelMemoryDescriptor {
            ty,
            _pad: 0,
            phys_start,
            page_count,
            attribute: 0,
        }
    }

    fn make_map(descs: &[KernelMemoryDescriptor]) -> KernelMemoryMap {
        let mut map = KernelMemoryMap::new();
        for (i, desc) in descs.iter().enumerate() {
            map.descriptors[i] = *desc;
        }
        map.count = descs.len();
        map
    }

    #[test]
    fn conventional_memory_is_usable() {
        let kind = MemoryRegionKind::from(memory_type::CONVENTIONAL);
        assert_eq!(kind, MemoryRegionKind::Usable);
        assert!(kind.is_immediately_usable());
        assert!(kind.is_reclaimable_after_boot());
    }

    #[test]
    fn boot_services_are_bootloader_reclaimable() {
        for ty in [
            memory_type::BOOT_SERVICES_CODE,
            memory_type::BOOT_SERVICES_DATA,
            memory_type::LOADER_CODE,
            memory_type::LOADER_DATA,
        ] {
            let kind = MemoryRegionKind::from(ty);
            assert_eq!(
                kind,
                MemoryRegionKind::BootloaderReclaimable,
                "type {} should be BootloaderReclaimable",
                ty
            );
            assert!(
                !kind.is_immediately_usable(),
                "type {} should not be immediately usable",
                ty
            );
            assert!(
                kind.is_reclaimable_after_boot(),
                "type {} should be reclaimable after boot",
                ty
            );
        }
    }

    #[test]
    fn acpi_reclaim_is_reclaimable_after_boot() {
        let kind = MemoryRegionKind::from(memory_type::ACPI_RECLAIM);
        assert_eq!(kind, MemoryRegionKind::AcpiReclaimable);
        assert!(!kind.is_immediately_usable());
        assert!(kind.is_reclaimable_after_boot());
    }

    #[test]
    fn mmio_is_not_reclaimable() {
        for ty in [memory_type::MMIO, memory_type::MMIO_PORT_SPACE] {
            let kind = MemoryRegionKind::from(ty);
            assert_eq!(kind, MemoryRegionKind::Mmio, "type {}", ty);
            assert!(!kind.is_immediately_usable());
            assert!(!kind.is_reclaimable_after_boot());
        }
    }

    #[test]
    fn firmware_runtime_is_not_reclaimable() {
        for ty in [
            memory_type::RUNTIME_SERVICES_CODE,
            memory_type::RUNTIME_SERVICES_DATA,
        ] {
            let kind = MemoryRegionKind::from(ty);
            assert_eq!(kind, MemoryRegionKind::FirmwareRuntime, "type {}", ty);
            assert!(!kind.is_reclaimable_after_boot());
        }
    }

    #[test]
    fn reserved_type_zero_is_reserved() {
        let kind = MemoryRegionKind::from(memory_type::RESERVED);
        assert_eq!(kind, MemoryRegionKind::Reserved);
        assert!(!kind.is_immediately_usable());
        assert!(!kind.is_reclaimable_after_boot());
    }

    #[test]
    fn unknown_type_is_reserved() {
        // Any type not explicitly mapped must fall through to Reserved.
        let kind = MemoryRegionKind::from(0xFF_u32);
        assert_eq!(kind, MemoryRegionKind::Reserved);
    }

    #[test]
    fn region_kind_names_are_non_empty() {
        let kinds = [
            MemoryRegionKind::Usable,
            MemoryRegionKind::BootloaderReclaimable,
            MemoryRegionKind::AcpiReclaimable,
            MemoryRegionKind::AcpiNonVolatile,
            MemoryRegionKind::FirmwareRuntime,
            MemoryRegionKind::Mmio,
            MemoryRegionKind::PersistentMemory,
            MemoryRegionKind::Unusable,
            MemoryRegionKind::Reserved,
        ];
        for kind in kinds {
            assert!(
                !kind.name().is_empty(),
                "{:?}.name() must not be empty",
                kind
            );
        }
    }

    // -----------------------------------------------------------------------
    // MemoryMap::parse — error paths
    // -----------------------------------------------------------------------

    #[test]
    fn parse_empty_map_returns_empty_error() {
        let map = KernelMemoryMap::new();
        assert!(matches!(MemoryMap::parse(&map), Err(ParseError::Empty)));
    }

    #[test]
    fn parse_all_zero_page_count_returns_empty_error() {
        // Firmware padding entries (page_count == 0) are skipped; if all
        // entries are zero-size the result must be Empty.
        let map = make_map(&[make_desc(memory_type::CONVENTIONAL, 0x1000, 0)]);
        assert!(matches!(MemoryMap::parse(&map), Err(ParseError::Empty)));
    }

    #[test]
    fn parse_unaligned_base_returns_error() {
        // phys_start must be 4 KiB aligned (UEFI spec).
        let map = make_map(&[make_desc(memory_type::CONVENTIONAL, 0x1001, 10)]);
        match MemoryMap::parse(&map) {
            Err(ParseError::UnalignedBase {
                index: 0,
                phys_start: 0x1001,
            }) => {}
            Err(e) => panic!("expected UnalignedBase(0, 0x1001), got {:?}", e),
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }

    // -----------------------------------------------------------------------
    // MemoryMap::parse — happy paths and statistics
    // -----------------------------------------------------------------------

    #[test]
    fn parse_single_conventional_region() {
        let map = make_map(&[make_desc(memory_type::CONVENTIONAL, 0x1000, 100)]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let stats = parsed.stats();
        assert_eq!(stats.region_count, 1);
        assert_eq!(stats.usable_bytes, 100 * 4096);
        assert_eq!(stats.reclaimable_bytes, 0);
        assert_eq!(stats.usable_region_count, 1);
        assert!(!stats.is_truncated);
    }

    #[test]
    fn parse_skips_zero_page_count_entries() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 0), // skipped
            make_desc(memory_type::CONVENTIONAL, 0x2000, 10), // counted
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        assert_eq!(parsed.stats().region_count, 1);
        assert_eq!(parsed.stats().usable_bytes, 10 * 4096);
    }

    #[test]
    fn parse_mixed_regions_computes_correct_stats() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x0000_1000, 100), // 400 KiB usable
            make_desc(memory_type::BOOT_SERVICES_DATA, 0x0010_0000, 50), // 200 KiB reclaimable
            make_desc(memory_type::MMIO, 0xFEC0_0000, 4),           // excluded from RAM
            make_desc(memory_type::RESERVED, 0x0000_F000, 1),       // not usable
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let stats = parsed.stats();

        assert_eq!(stats.region_count, 4);
        assert_eq!(stats.usable_bytes, 100 * 4096);
        assert_eq!(stats.reclaimable_bytes, 50 * 4096);
        assert_eq!(stats.usable_region_count, 1);
        // MMIO is excluded; RESERVED is included in total RAM
        assert_eq!(stats.total_bytes, (100 + 50 + 1) * 4096);
    }

    #[test]
    fn mmio_excluded_from_total_ram() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::MMIO, 0xFEC0_0000, 100),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        // Only conventional counts toward total_bytes.
        assert_eq!(parsed.stats().total_bytes, 10 * 4096);
    }

    #[test]
    fn firmware_runtime_excluded_from_total_ram() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::RUNTIME_SERVICES_DATA, 0x10_0000, 5),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        // Runtime services are not RAM we can use or count.
        assert_eq!(parsed.stats().total_bytes, 10 * 4096);
    }

    #[test]
    fn truncated_flag_propagates_to_stats() {
        let mut map = KernelMemoryMap::new();
        map.descriptors[0] = make_desc(memory_type::CONVENTIONAL, 0x1000, 10);
        map.count = 1;
        map.truncated = true;
        let parsed = MemoryMap::parse(&map).unwrap();
        assert!(parsed.stats().is_truncated);
    }

    #[test]
    fn total_usable_after_boot_sums_usable_and_reclaimable() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 100),
            make_desc(memory_type::BOOT_SERVICES_DATA, 0x10_0000, 50),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let stats = parsed.stats();
        assert_eq!(
            stats.total_usable_after_boot(),
            stats.usable_bytes + stats.reclaimable_bytes
        );
    }

    // -----------------------------------------------------------------------
    // MemoryMap query methods
    // -----------------------------------------------------------------------

    #[test]
    fn usable_regions_returns_only_conventional() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::RESERVED, 0xF000, 1),
            make_desc(memory_type::CONVENTIONAL, 0x10_0000, 200),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let usable: std::vec::Vec<_> = parsed.usable_regions().collect();
        assert_eq!(usable.len(), 2);
        assert!(usable.iter().all(|d| d.ty == memory_type::CONVENTIONAL));
    }

    #[test]
    fn reclaimable_regions_includes_bootloader_and_conventional() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::BOOT_SERVICES_CODE, 0x10_0000, 5),
            make_desc(memory_type::LOADER_DATA, 0x20_0000, 3),
            make_desc(memory_type::MMIO, 0xFEC0_0000, 4), // excluded
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let reclaimable: std::vec::Vec<_> = parsed.reclaimable_regions().collect();
        assert_eq!(reclaimable.len(), 3); // CONVENTIONAL + BOOT_SERVICES_CODE + LOADER_DATA
    }

    #[test]
    fn regions_of_type_returns_only_matching() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::MMIO, 0xFEC0_0000, 4),
            make_desc(memory_type::CONVENTIONAL, 0x10_0000, 200),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        let conventional: std::vec::Vec<_> =
            parsed.regions_of_type(memory_type::CONVENTIONAL).collect();
        assert_eq!(conventional.len(), 2);
    }

    #[test]
    fn regions_returns_all_non_zero_entries() {
        let map = make_map(&[
            make_desc(memory_type::CONVENTIONAL, 0x1000, 10),
            make_desc(memory_type::RESERVED, 0xF000, 1),
            make_desc(memory_type::MMIO, 0xFEC0_0000, 4),
        ]);
        let parsed = MemoryMap::parse(&map).unwrap();
        assert_eq!(parsed.regions().len(), 3);
    }

    // -----------------------------------------------------------------------
    // Large map (stress test at MAX_MEMORY_DESCRIPTORS)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_max_descriptors() {
        let mut map = KernelMemoryMap::new();
        for i in 0..MAX_MEMORY_DESCRIPTORS {
            map.descriptors[i] = make_desc(memory_type::CONVENTIONAL, (i as u64 + 1) * 0x1000, 1);
        }
        map.count = MAX_MEMORY_DESCRIPTORS;
        let parsed = MemoryMap::parse(&map).unwrap();
        assert_eq!(parsed.stats().region_count, MAX_MEMORY_DESCRIPTORS);
        assert_eq!(parsed.stats().usable_region_count, MAX_MEMORY_DESCRIPTORS);
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
