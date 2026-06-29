//! Global Descriptor Table (GDT) for x86-64.
//!
//! Even though x86-64 long mode largely ignores segmentation, the GDT must
//! still be loaded before the CPU can operate correctly, and it is strictly
//! required for privilege level transitions (SYSCALL/SYSRET and Interrupts).
//!
//! # GDT Layout (Phase 2)
//!
//! ```text
//! Index │ Selector │ Description
//! ──────┼──────────┼─────────────────────────────────────────────────
//!   0   │  0x0000  │ Null descriptor (required)
//!   1   │  0x0008  │ Kernel code segment (64-bit)
//!   2   │  0x0010  │ Kernel data segment
//!   3   │  0x001B  │ User code segment (32-bit compat) - SYSRET requires this!
//!   4   │  0x0023  │ User data segment
//!   5   │  0x002B  │ User code segment (64-bit)
//!  6,7  │  0x0030  │ TSS descriptor (takes two slots in 64-bit mode)
//! ```

use super::tss::TaskStateSegment;

// ---------------------------------------------------------------------------
// Descriptor values
// ---------------------------------------------------------------------------

const NULL_DESCRIPTOR: u64 = 0;

// Kernel segments (Ring 0)
const KERNEL_CODE_DESCRIPTOR: u64 = 0x00AF_9A00_0000_FFFF;
const KERNEL_DATA_DESCRIPTOR: u64 = 0x00CF_9200_0000_FFFF;

// User segments (Ring 3, DPL=3 -> Type bit 45-46 = 11 -> OR with 0x600000000000)
// User Code 32-bit: L=0, D=1 (32-bit default), S=1, P=1, DPL=3, Type=0xA
const USER_CODE_32_DESCRIPTOR: u64 = 0x00CF_FA00_0000_FFFF;

// User Data: L=0, D=1, S=1, P=1, DPL=3, Type=0x2
const USER_DATA_DESCRIPTOR: u64 = 0x00CF_F200_0000_FFFF;

// User Code 64-bit: L=1, D=0, S=1, P=1, DPL=3, Type=0xA
const USER_CODE_64_DESCRIPTOR: u64 = 0x00AF_FA00_0000_FFFF;

// ---------------------------------------------------------------------------
// Segment selectors
// ---------------------------------------------------------------------------

pub const KERNEL_CODE_SELECTOR: u16 = 0x08; // RPL 0
pub const KERNEL_DATA_SELECTOR: u16 = 0x10; // RPL 0
pub const USER_CODE_32_SELECTOR: u16 = 0x18 | 3; // RPL 3
pub const USER_DATA_SELECTOR: u16 = 0x20 | 3; // RPL 3
pub const USER_CODE_64_SELECTOR: u16 = 0x28 | 3; // RPL 3
pub const TSS_SELECTOR: u16 = 0x30; // RPL 0

// ---------------------------------------------------------------------------
// TSS Descriptor (16 bytes in long mode)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
pub struct TssDescriptor {
    pub low: u64,
    pub high: u64,
}

impl TssDescriptor {
    pub const fn new(tss_addr: u64) -> Self {
        let base_low = (tss_addr & 0xFFFFFF) << 16;
        let base_mid = ((tss_addr >> 24) & 0xFF) << 56;
        let base_high = tss_addr >> 32;

        let limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;

        // P=1, DPL=0, Type=0x9 (64-bit available TSS)
        let flags_type = 0x0000_8900_0000_0000;

        Self {
            low: limit | base_low | base_mid | flags_type,
            high: base_high,
        }
    }
}

// ---------------------------------------------------------------------------
// GDT structure
// ---------------------------------------------------------------------------

#[repr(C, align(8))]
pub struct Gdt {
    entries: [u64; 6],
    tss: TssDescriptor,
}

pub static mut TSS: TaskStateSegment = TaskStateSegment::new();

pub static mut GDT: Gdt = Gdt {
    entries: [
        NULL_DESCRIPTOR,
        KERNEL_CODE_DESCRIPTOR,
        KERNEL_DATA_DESCRIPTOR,
        USER_CODE_32_DESCRIPTOR,
        USER_DATA_DESCRIPTOR,
        USER_CODE_64_DESCRIPTOR,
    ],
    // The TSS address must be calculated at runtime since Rust const
    // evaluation cannot take addresses of statics (yet).
    tss: TssDescriptor { low: 0, high: 0 },
};

#[repr(C, packed)]
pub struct GdtPointer {
    pub limit: u16,
    pub base: u64,
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

pub unsafe fn init() {
    // 1. Initialize the TSS descriptor with the actual runtime address of TSS
    let tss_addr = core::ptr::addr_of!(TSS) as u64;
    GDT.tss = TssDescriptor::new(tss_addr);

    let ptr = GdtPointer {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base: core::ptr::addr_of!(GDT) as u64,
    };

    core::arch::asm!(
        "lgdt [{ptr}]",
        ptr = in(reg) &ptr,
        options(readonly, nostack, preserves_flags),
    );

    core::arch::asm!(
        "push {cs}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        cs  = in(reg) KERNEL_CODE_SELECTOR as u64,
        tmp = lateout(reg) _,
    );

    core::arch::asm!(
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        "mov ss, ax",
        in("ax") KERNEL_DATA_SELECTOR,
        options(nomem, nostack, preserves_flags),
    );

    // Load Task Register (TR)
    core::arch::asm!(
        "ltr ax",
        in("ax") TSS_SELECTOR,
        options(nomem, nostack, preserves_flags),
    );
}
