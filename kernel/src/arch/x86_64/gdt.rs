//! Global Descriptor Table (GDT) for x86-64.
//!
//! Even though x86-64 long mode largely ignores segmentation (base/limit are
//! zero-extended and effectively unused), the GDT must still be loaded before
//! the CPU can operate correctly:
//!
//! - The **code segment** descriptor determines whether the CPU executes in
//!   64-bit mode (`L=1, D=0`) or legacy compatibility mode.
//! - The **data segment** descriptors must be valid for `SS`, `DS`, `ES`, etc.
//! - A **null descriptor** at index 0 is architecturally required.
//! - The **TSS descriptor** (not in Phase 1) will be required for syscall and
//!   interrupt handling once those subsystems are implemented.
//!
//! # GDT Layout (Phase 1 — minimal)
//!
//! ```text
//! Index │ Selector │ Description
//! ──────┼──────────┼──────────────────────────────
//!   0   │  0x0000  │ Null descriptor (required)
//!   1   │  0x0008  │ Kernel code segment (64-bit)
//!   2   │  0x0010  │ Kernel data segment
//! ```
//!
//! # Segment descriptor bit layout
//!
//! Each GDT entry is 8 bytes, encoded as a `u64` in little-endian:
//!
//! ```text
//! Bits 63:56  Base[31:24]
//! Bit  55     G   — Granularity (1 = 4 KiB pages, 0 = bytes)
//! Bit  54     D/B — Default/Big (must be 0 for 64-bit code segment)
//! Bit  53     L   — Long mode code (1 = 64-bit code segment)
//! Bit  52     AVL — Available for system software (0)
//! Bits 51:48  Limit[19:16]
//! Bit  47     P   — Present (must be 1 for valid segment)
//! Bits 46:45  DPL — Descriptor Privilege Level (0 = kernel)
//! Bit  44     S   — System (1 = code/data, 0 = system descriptor)
//! Bits 43:40  Type (see below)
//! Bits 39:16  Base[23:0]
//! Bits 15:0   Limit[15:0]
//! ```
//!
//! Type field for code segments (bit 44 S=1):
//! - Bit 43 = 1 (executable)
//! - Bit 42 = conforming (0 = non-conforming)
//! - Bit 41 = readable (1 = can read via CS override)
//! - Bit 40 = accessed (set by CPU on first use)
//!
//! Type field for data segments (bit 44 S=1, bit 43 = 0):
//! - Bit 42 = expand-down (0 = normal)
//! - Bit 41 = writable (1 = writeable)
//! - Bit 40 = accessed (set by CPU on first use)
//!
//! # Phase notes
//!
//! In Phase 1, the `GDT` static and `init()` function are mirrored inline in
//! `boot/src/main.rs` (since the bootloader and kernel run as the same binary).
//! When the kernel becomes a separate ELF binary this module becomes the
//! authoritative source and the duplicate in the boot crate is removed.

// ---------------------------------------------------------------------------
// Descriptor values
// ---------------------------------------------------------------------------

/// Null descriptor — required at GDT index 0 by the x86-64 architecture.
const NULL_DESCRIPTOR: u64 = 0x0000_0000_0000_0000;

/// 64-bit kernel code segment descriptor.
///
/// Bit breakdown:
/// - Base = 0, Limit = 0xFFFFF (ignored in 64-bit mode)
/// - G=1 (4 KiB granularity), D=0 (must be 0 for 64-bit), L=1 (64-bit code),
///   AVL=0
/// - P=1 (present), DPL=0 (ring 0), S=1 (code/data), Type=0xA
///   (executable + readable)
///
/// Encoded bytes (low → high): `FF FF 00 00 00 9A AF 00`
const KERNEL_CODE_DESCRIPTOR: u64 = 0x00AF_9A00_0000_FFFF;

/// Kernel data segment descriptor.
///
/// Bit breakdown:
/// - Base = 0, Limit = 0xFFFFF (ignored in 64-bit mode)
/// - G=1 (4 KiB granularity), D/B=1 (32-bit default operand size — harmless
///   for data segments in 64-bit mode), L=0, AVL=0
/// - P=1 (present), DPL=0 (ring 0), S=1 (code/data), Type=0x2
///   (data, readable + writable)
///
/// Encoded bytes (low → high): `FF FF 00 00 00 92 CF 00`
const KERNEL_DATA_DESCRIPTOR: u64 = 0x00CF_9200_0000_FFFF;

// ---------------------------------------------------------------------------
// Segment selectors
//
// A segment selector is a 16-bit value:
//   Bits 15:3  — Index into GDT (or LDT)
//   Bit  2     — TI: Table Indicator (0 = GDT, 1 = LDT)
//   Bits 1:0   — RPL: Requested Privilege Level
// ---------------------------------------------------------------------------

/// Kernel code segment selector: GDT index 1, TI=0 (GDT), RPL=0.
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;

/// Kernel data segment selector: GDT index 2, TI=0 (GDT), RPL=0.
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;

// ---------------------------------------------------------------------------
// GDT structure
// ---------------------------------------------------------------------------

/// The kernel's Global Descriptor Table.
///
/// Three entries: null, kernel code, kernel data. Aligned to 8 bytes so the
/// CPU can fetch descriptors with a single aligned load.
#[repr(C, align(8))]
pub struct Gdt([u64; 3]);

/// The kernel's static GDT instance.
///
/// Declared `static` so the CPU can reference it indefinitely after the LGDT
/// instruction executes. The `GdtPointer` below holds its address.
pub static GDT: Gdt = Gdt([
    NULL_DESCRIPTOR,
    KERNEL_CODE_DESCRIPTOR,
    KERNEL_DATA_DESCRIPTOR,
]);

// ---------------------------------------------------------------------------
// GDT pointer (passed to LGDT)
// ---------------------------------------------------------------------------

/// Descriptor-table pointer used by the `LGDT` instruction.
///
/// Must be `#[repr(C, packed)]` so the CPU sees exactly:
/// - 2 bytes: table size minus 1 (`limit`)
/// - 8 bytes: linear base address of the table (`base`)
///
/// Total: 10 bytes, starting at the struct's address.
#[repr(C, packed)]
pub struct GdtPointer {
    /// Size of the GDT in bytes, minus 1.
    pub limit: u16,
    /// Linear (virtual) address of the GDT.
    pub base: u64,
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Load the GDT and reload all segment registers.
///
/// After this function returns, the CPU is executing with the new GDT active
/// and all segment registers pointing to the Phase-1 kernel segments.
///
/// # Steps
///
/// 1. Execute `LGDT` to load the GDT register with the address and size of
///    [`GDT`].
/// 2. Reload `CS` via a far return (`RETFQ`) — the only reliable way to
///    change the code segment in 64-bit mode without a task switch.
/// 3. Reload `DS`, `ES`, `FS`, `GS`, `SS` with [`KERNEL_DATA_SELECTOR`].
///
/// # Safety
///
/// - Caller must be executing at CPL=0 (ring 0).
/// - Interrupts must be disabled (`CLI` executed before this call). An
///   interrupt arriving between `LGDT` and the far-return reload of CS could
///   deliver control to an interrupt handler whose `IRETQ` would restore a
///   stale CS, causing a General Protection Fault.
/// - The [`GDT`] static must have been placed in memory that is mapped and
///   accessible at its linear address for the lifetime of the CPU's operation
///   with this GDT loaded.
pub unsafe fn init() {
    let ptr = GdtPointer {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base: core::ptr::addr_of!(GDT) as u64,
    };

    // Step 1: Load the GDT register.
    //
    // SAFETY: `ptr` is a valid GdtPointer on the stack. LGDT reads the 10
    // bytes at `ptr`'s address and writes them into the GDTR. No other
    // memory is modified. The instruction is valid at CPL=0.
    core::arch::asm!(
        "lgdt [{ptr}]",
        ptr = in(reg) &ptr,
        options(readonly, nostack, preserves_flags),
    );

    // Step 2: Reload CS via far return.
    //
    // In 64-bit mode there is no `LJMP` encoding; the only way to reload CS
    // is a far call/return or an IRETQ. We use a far return:
    //
    //   push new_cs    ← pushed first, lands at RSP+8 for RETFQ
    //   push new_rip   ← label immediately after RETFQ; lands at RSP+0
    //   retfq          ← pops RIP then CS; jumps to `2:`
    //   2:             ← execution resumes here with CS = KERNEL_CODE_SELECTOR
    //
    // SAFETY:
    // - KERNEL_CODE_SELECTOR points to a valid 64-bit code segment.
    // - The label `1:` is within the same function; the far return lands on
    //   the very next instruction, so no ABI invariants are violated.
    // - `tmp` is a caller-save scratch register allocated by the compiler;
    //   its value after the asm block is intentionally discarded.
    core::arch::asm!(
        "push {cs}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        cs  = in(reg) KERNEL_CODE_SELECTOR as u64,
        tmp = lateout(reg) _,
    );

    // Step 3: Reload data segment registers.
    //
    // In 64-bit mode DS, ES, FS, GS, and SS are largely unused (base/limit
    // ignored), but they must be loaded with a valid selector to avoid a
    // General Protection Fault when an instruction references them.
    //
    // SAFETY: KERNEL_DATA_SELECTOR points to a valid data segment. All of
    // these moves are valid at CPL=0 with the new GDT loaded.
    core::arch::asm!(
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        "mov ss, ax",
        in("ax") KERNEL_DATA_SELECTOR,
        options(nomem, nostack, preserves_flags),
    );
}
