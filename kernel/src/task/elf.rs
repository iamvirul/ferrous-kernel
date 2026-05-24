//! Minimal ELF64 loader (Phase 2.1.3).
//!
//! Parses statically linked ELF executables and maps their PT_LOAD segments
//! into a given [`AddressSpace`].

use crate::memory::address_space::{AddressSpace, RegionKind, VirtualRegion};
use crate::memory::paging::VirtualAddress;
use alloc::vec::Vec;

/// Errors that can occur during ELF parsing and loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The binary is too small to contain the required headers.
    TooSmall,
    /// Invalid ELF magic bytes.
    InvalidMagic,
    /// Binary is not a 64-bit ELF.
    Not64Bit,
    /// Binary is not little-endian.
    NotLittleEndian,
    /// Unsupported ELF version.
    InvalidVersion,
    /// Binary is not an executable (`ET_EXEC`). Dynamic binaries are rejected.
    NotExecutable,
    /// Binary is not built for x86_64.
    InvalidMachine,
    /// Program header table is malformed or out of bounds.
    InvalidProgramHeaders,
    /// A program segment is malformed or out of bounds.
    InvalidSegment,
    /// Failed to map a segment into the address space.
    MapError,
    /// The entry point is invalid or does not fall within an executable segment.
    InvalidEntryPoint,
}

/// Raw ELF structures and constants.
pub mod raw {
    use super::ParseError;
    use core::mem::size_of;

    pub const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
    pub const ELFCLASS64: u8 = 2;
    pub const ELFDATA2LSB: u8 = 1;
    pub const EV_CURRENT: u8 = 1;

    pub const ET_EXEC: u16 = 2;
    pub const EM_X86_64: u16 = 62;

    pub const PT_LOAD: u32 = 1;

    pub const PF_X: u32 = 1;
    pub const PF_W: u32 = 2;
    pub const PF_R: u32 = 4;

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct Elf64_Ehdr {
        pub e_ident: [u8; 16],
        pub e_type: u16,
        pub e_machine: u16,
        pub e_version: u32,
        pub e_entry: u64,
        pub e_phoff: u64,
        pub e_shoff: u64,
        pub e_flags: u32,
        pub e_ehsize: u16,
        pub e_phentsize: u16,
        pub e_phnum: u16,
        pub e_shentsize: u16,
        pub e_shnum: u16,
        pub e_shstrndx: u16,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct Elf64_Phdr {
        pub p_type: u32,
        pub p_flags: u32,
        pub p_offset: u64,
        pub p_vaddr: u64,
        pub p_paddr: u64,
        pub p_filesz: u64,
        pub p_memsz: u64,
        pub p_align: u64,
    }

    /// Read a `repr(C)` structure from a byte slice at a given offset.
    ///
    /// # Safety
    /// This function uses `ptr::read_unaligned` to safely read the struct without
    /// requiring the byte slice to be correctly aligned. It ensures the slice is
    /// large enough to hold the struct.
    pub fn read_struct<T: Copy>(data: &[u8], offset: usize) -> Result<T, ParseError> {
        if offset.saturating_add(size_of::<T>()) > data.len() {
            return Err(ParseError::TooSmall);
        }
        // SAFETY: We checked the bounds above. `ptr::read_unaligned` handles
        // unaligned reads safely.
        unsafe {
            let ptr = data.as_ptr().add(offset) as *const T;
            Ok(core::ptr::read_unaligned(ptr))
        }
    }
}

/// Load an ELF64 executable into the given address space.
///
/// Returns the virtual address of the entry point upon success.
///
/// # Safety
///
/// Callers must guarantee the following preconditions:
/// - Paging is initialized and active
/// - An identity mapping exists for the kernel (higher-half is mapped)
/// - Execution is in a single-threaded context (no other threads can run)
/// - Interrupts are disabled
///
/// These preconditions are required by [`load_elf`], [`AddressSpace::map_region`],
/// and [`AddressSpace::switch_to`].
pub unsafe fn load_elf(elf_data: &[u8], aspace: &mut AddressSpace) -> Result<VirtualAddress, ParseError> {
    use raw::*;

    let ehdr: Elf64_Ehdr = read_struct(elf_data, 0)?;

    if ehdr.e_ident[0..4] != ELFMAG {
        return Err(ParseError::InvalidMagic);
    }
    if ehdr.e_ident[4] != ELFCLASS64 {
        return Err(ParseError::Not64Bit);
    }
    if ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ParseError::NotLittleEndian);
    }
    if ehdr.e_ident[6] != EV_CURRENT {
        return Err(ParseError::InvalidVersion);
    }
    if ehdr.e_type != ET_EXEC {
        return Err(ParseError::NotExecutable);
    }
    if ehdr.e_machine != EM_X86_64 {
        return Err(ParseError::InvalidMachine);
    }

    let phoff = ehdr.e_phoff as usize;
    let phnum = ehdr.e_phnum as usize;
    let phentsize = ehdr.e_phentsize as usize;

    if phentsize < core::mem::size_of::<Elf64_Phdr>() {
        return Err(ParseError::InvalidProgramHeaders);
    }

    let mut entry_point_valid = false;
    let entry_va = ehdr.e_entry;
    let mut mapped_regions: Vec<VirtualAddress> = Vec::new();

    for i in 0..phnum {
        let offset = phoff.saturating_add(i.saturating_mul(phentsize));
        let phdr: Elf64_Phdr = read_struct(elf_data, offset).map_err(|e| {
            // Unmap all successfully mapped regions before returning error
            unsafe {
                for &region_base in &mapped_regions {
                    let _ = aspace.unmap_region(region_base);
                }
            }
            e
        })?;

        if phdr.p_type == PT_LOAD {
            if phdr.p_memsz == 0 {
                continue;
            }

            if phdr.p_filesz > phdr.p_memsz {
                // Unmap all successfully mapped regions before returning error
                unsafe {
                    for &region_base in &mapped_regions {
                        let _ = aspace.unmap_region(region_base);
                    }
                }
                return Err(ParseError::InvalidSegment);
            }

            let kind = if (phdr.p_flags & PF_X) != 0 {
                RegionKind::Code
            } else if (phdr.p_flags & PF_W) != 0 {
                RegionKind::Data
            } else {
                // Read-only data (PF_R only)
                RegionKind::Data
            };

            let base = phdr.p_vaddr & !0xFFF;
            let end = (phdr.p_vaddr.saturating_add(phdr.p_memsz) + 0xFFF) & !0xFFF;
            let size = (end - base) as usize;

            let va = VirtualAddress::try_new(base).ok_or_else(|| {
                // Unmap all successfully mapped regions before returning error
                unsafe {
                    for &region_base in &mapped_regions {
                        let _ = aspace.unmap_region(region_base);
                    }
                }
                ParseError::MapError
            })?;
            let region = VirtualRegion {
                base: va,
                size,
                kind,
            };

            // SAFETY: aspace is exclusively borrowed. We are in single-threaded context.
            unsafe {
                aspace
                    .map_region(region)
                    .map_err(|_| {
                        // Unmap all successfully mapped regions before returning error
                        for &region_base in &mapped_regions {
                            let _ = aspace.unmap_region(region_base);
                        }
                        ParseError::MapError
                    })?;
            }

            // Track successfully mapped region for potential cleanup
            mapped_regions.push(va);

            // SAFETY: We switch to the new address space to copy data into it.
            // The kernel higher-half is shared across all address spaces, so we
            // continue executing here seamlessly. We disable interrupts to prevent
            // context switches while operating in another process's address space.
            unsafe {
                let current_cr3: u64;
                core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) current_cr3, options(nomem, nostack));

                aspace.switch_to();

                // Zero out the entire mapped region first (to cover BSS)
                core::ptr::write_bytes(base as *mut u8, 0, size);

                // Copy filesz bytes from elf_data
                let file_start = phdr.p_offset as usize;
                let file_end = file_start.saturating_add(phdr.p_filesz as usize);

                let mut copy_ok = true;
                if file_end > elf_data.len() {
                    copy_ok = false;
                } else {
                    let src = elf_data[file_start..file_end].as_ptr();
                    let dst = phdr.p_vaddr as *mut u8;
                    core::ptr::copy_nonoverlapping(src, dst, phdr.p_filesz as usize);
                }

                // Restore previous address space
                core::arch::asm!("mov cr3, {cr3}", cr3 = in(reg) current_cr3, options(nostack));

                if !copy_ok {
                    // Unmap all successfully mapped regions before returning error
                    for &region_base in &mapped_regions {
                        let _ = aspace.unmap_region(region_base);
                    }
                    return Err(ParseError::InvalidSegment);
                }
            }

            // Check if entry point is within this segment
            if (phdr.p_flags & PF_X) != 0 {
                if entry_va >= phdr.p_vaddr && entry_va < phdr.p_vaddr + phdr.p_memsz {
                    entry_point_valid = true;
                }
            }
        }
    }

    if !entry_point_valid {
        // Unmap all successfully mapped regions before returning error
        unsafe {
            for &region_base in &mapped_regions {
                let _ = aspace.unmap_region(region_base);
            }
        }
        return Err(ParseError::InvalidEntryPoint);
    }

    VirtualAddress::try_new(entry_va).ok_or_else(|| {
        // Unmap all successfully mapped regions before returning error
        unsafe {
            for &region_base in &mapped_regions {
                let _ = aspace.unmap_region(region_base);
            }
        }
        ParseError::InvalidEntryPoint
    })
}

pub unsafe fn smoke_test() {
    log::info!("ELF loader smoke test (Phase 2.1.3)");

    #[repr(C, packed)]
    struct DummyElf {
        ehdr: raw::Elf64_Ehdr,
        phdr: raw::Elf64_Phdr,
        data: [u8; 8],
    }

    let mut dummy = DummyElf {
        ehdr: raw::Elf64_Ehdr {
            e_ident: [0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            e_type: raw::ET_EXEC,
            e_machine: raw::EM_X86_64,
            e_version: 1,
            e_entry: 0x400000,
            e_phoff: core::mem::size_of::<raw::Elf64_Ehdr>() as u64,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: core::mem::size_of::<raw::Elf64_Ehdr>() as u16,
            e_phentsize: core::mem::size_of::<raw::Elf64_Phdr>() as u16,
            e_phnum: 1,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        },
        phdr: raw::Elf64_Phdr {
            p_type: raw::PT_LOAD,
            p_flags: raw::PF_R | raw::PF_X,
            p_offset: core::mem::size_of::<raw::Elf64_Ehdr>() as u64
                + core::mem::size_of::<raw::Elf64_Phdr>() as u64,
            p_vaddr: 0x400000,
            p_paddr: 0x400000,
            p_filesz: 8,
            p_memsz: 8,
            p_align: 0x1000,
        },
        data: [0x90; 8],
    };

    let dummy_bytes = core::slice::from_raw_parts(
        &dummy as *const _ as *const u8,
        core::mem::size_of::<DummyElf>(),
    );

    let mut aspace = AddressSpace::new().expect("aspace new");
    let va = load_elf(dummy_bytes, &mut aspace).expect("load valid elf");
    assert_eq!(va.as_u64(), 0x400000);
    assert_eq!(aspace.region_count(), 1);

    dummy.ehdr.e_ident[0] = 0x00;
    let bad_magic_bytes = core::slice::from_raw_parts(
        &dummy as *const _ as *const u8,
        core::mem::size_of::<DummyElf>(),
    );
    let mut aspace2 = AddressSpace::new().unwrap();
    assert_eq!(
        load_elf(bad_magic_bytes, &mut aspace2).err(),
        Some(ParseError::InvalidMagic)
    );

    dummy.ehdr.e_ident[0] = 0x7f;
    dummy.ehdr.e_ident[4] = 1; // 32-bit
    let bad_class_bytes = core::slice::from_raw_parts(
        &dummy as *const _ as *const u8,
        core::mem::size_of::<DummyElf>(),
    );
    assert_eq!(
        load_elf(bad_class_bytes, &mut aspace2).err(),
        Some(ParseError::Not64Bit)
    );

    dummy.ehdr.e_ident[4] = 2;
    dummy.ehdr.e_type = 3; // ET_DYN
    let bad_type_bytes = core::slice::from_raw_parts(
        &dummy as *const _ as *const u8,
        core::mem::size_of::<DummyElf>(),
    );
    assert_eq!(
        load_elf(bad_type_bytes, &mut aspace2).err(),
        Some(ParseError::NotExecutable)
    );

    aspace.destroy();
    aspace2.destroy();

    log::info!("[OK] 14.9) ELF loader tests passed");
}
