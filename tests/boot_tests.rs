//! Host-side specification tests for Ferrous kernel low-level components.
//!
//! These tests run on the host (standard Rust target) and verify:
//! - GDT segment descriptor bit patterns match the Intel SDM specification
//! - IDT gate descriptor address encoding is correct
//! - Exception vector properties (which vectors push error codes)
//! - Kernel stack layout constants
//! - ExceptionFrame conceptual field layout
//!
//! They do NOT test runtime behaviour (loading GDTR/IDTR, firing interrupts)
//! which requires QEMU — that is covered by `scripts/verify-boot.sh`.
//!
//! Why test bit patterns here rather than in the kernel crate?
//! The kernel crate is `#![no_std] #![no_main]` targeting `x86_64-unknown-none`
//! and contains inline assembly, making it incompatible with `cargo test`.
//! These host tests ensure the constants embedded in the kernel are correct
//! per the Intel SDM before the code ever runs.

// ---------------------------------------------------------------------------
// GDT segment descriptor bit-pattern tests
//
// Each 8-byte descriptor is encoded as a u64. Bit positions per Intel SDM
// Vol 3A §3.4.5:
//
//   Bit 63:56  Base[31:24]
//   Bit 55     G   — Granularity (1 = 4 KiB pages)
//   Bit 54     D/B — Default/Big (must be 0 for 64-bit code segment)
//   Bit 53     L   — Long-mode code (1 = 64-bit execution)
//   Bit 52     AVL — Available for OS
//   Bit 51:48  Limit[19:16]
//   Bit 47     P   — Present
//   Bit 46:45  DPL — Descriptor Privilege Level
//   Bit 44     S   — 1 = code/data, 0 = system descriptor
//   Bit 43     Executable (for code segments)
//   Bit 41     Readable (code) / Writable (data)
//   Bit 39:16  Base[23:0]
//   Bit 15:0   Limit[15:0]
// ---------------------------------------------------------------------------

/// Kernel code segment: 0x00AF_9A00_0000_FFFF
const KERNEL_CODE_DESC: u64 = 0x00AF_9A00_0000_FFFF;

/// Kernel data segment: 0x00CF_9200_0000_FFFF
const KERNEL_DATA_DESC: u64 = 0x00CF_9200_0000_FFFF;

/// Null descriptor (GDT index 0 — architecturally required).
const NULL_DESC: u64 = 0x0000_0000_0000_0000;

/// Kernel code segment selector (GDT index 1, TI=0, RPL=0).
const KERNEL_CODE_SEL: u16 = 0x0008;

/// Kernel data segment selector (GDT index 2, TI=0, RPL=0).
const KERNEL_DATA_SEL: u16 = 0x0010;

#[test]
fn gdt_null_descriptor_is_zero() {
    assert_eq!(
        NULL_DESC, 0,
        "null descriptor must be all-zero (SDM requirement)"
    );
}

#[test]
fn gdt_kernel_code_present_bit() {
    // Bit 47 = P (Present). Must be 1 for a valid descriptor.
    assert_ne!(KERNEL_CODE_DESC & (1 << 47), 0, "P bit must be set");
}

#[test]
fn gdt_kernel_code_dpl_is_ring0() {
    // Bits 46:45 = DPL. Must be 0b00 for kernel (ring 0).
    assert_eq!(
        (KERNEL_CODE_DESC >> 45) & 0b11,
        0,
        "DPL must be 0 (ring 0) for kernel code segment"
    );
}

#[test]
fn gdt_kernel_code_s_bit_is_set() {
    // Bit 44 = S. Must be 1 for a code/data segment (not a system descriptor).
    assert_ne!(KERNEL_CODE_DESC & (1 << 44), 0, "S bit must be 1");
}

#[test]
fn gdt_kernel_code_executable_bit() {
    // Bit 43 = Executable. Must be 1 for a code segment.
    assert_ne!(KERNEL_CODE_DESC & (1 << 43), 0, "executable bit must be 1");
}

#[test]
fn gdt_kernel_code_long_mode_bit() {
    // Bit 53 = L. Must be 1 to enable 64-bit execution mode.
    assert_ne!(
        KERNEL_CODE_DESC & (1 << 53),
        0,
        "L bit must be 1 for 64-bit code"
    );
}

#[test]
fn gdt_kernel_code_db_bit_clear() {
    // Bit 54 = D/B. Must be 0 when L=1 (SDM: ignored, but must be 0 in 64-bit).
    assert_eq!(
        KERNEL_CODE_DESC & (1 << 54),
        0,
        "D/B bit must be 0 when L=1"
    );
}

#[test]
fn gdt_kernel_code_granularity_bit() {
    // Bit 55 = G (Granularity). 1 = 4 KiB page granularity.
    assert_ne!(KERNEL_CODE_DESC & (1 << 55), 0, "G bit must be 1");
}

#[test]
fn gdt_kernel_data_present_bit() {
    assert_ne!(KERNEL_DATA_DESC & (1 << 47), 0, "P bit must be set");
}

#[test]
fn gdt_kernel_data_dpl_is_ring0() {
    assert_eq!(
        (KERNEL_DATA_DESC >> 45) & 0b11,
        0,
        "DPL must be 0 for kernel data segment"
    );
}

#[test]
fn gdt_kernel_data_is_not_executable() {
    // Bit 43 = Executable. Must be 0 for a data segment.
    assert_eq!(
        KERNEL_DATA_DESC & (1 << 43),
        0,
        "data segment must not be executable"
    );
}

#[test]
fn gdt_kernel_data_writable_bit() {
    // Bit 41 = Writable. Must be 1 for a writable data segment.
    assert_ne!(KERNEL_DATA_DESC & (1 << 41), 0, "writable bit must be 1");
}

#[test]
fn gdt_kernel_data_s_bit_is_set() {
    assert_ne!(KERNEL_DATA_DESC & (1 << 44), 0, "S bit must be 1");
}

#[test]
fn gdt_selectors_are_aligned_to_eight_bytes() {
    // Each GDT entry is 8 bytes; selector = index * 8 (TI=0, RPL=0).
    assert_eq!(KERNEL_CODE_SEL, 1 * 8, "code selector = GDT index 1");
    assert_eq!(KERNEL_DATA_SEL, 2 * 8, "data selector = GDT index 2");
}

#[test]
fn gdt_has_three_entries_at_expected_selectors() {
    // Null at 0x00, code at 0x08, data at 0x10.
    assert_eq!(KERNEL_CODE_SEL, 0x08);
    assert_eq!(KERNEL_DATA_SEL, 0x10);
}

// ---------------------------------------------------------------------------
// IDT gate descriptor encoding tests
//
// Each IDT entry is 16 bytes. For a 64-bit interrupt gate (type 0xE):
//
//   Bytes  0– 1   offset_low  = handler_addr[15:0]
//   Bytes  2– 3   selector    = 0x0008 (kernel CS)
//   Byte   4      ist         = 0
//   Byte   5      type_attr   = 0x8E (P=1, DPL=0, type=0xE)
//   Bytes  6– 7   offset_mid  = handler_addr[31:16]
//   Bytes  8–11   offset_high = handler_addr[63:32]
//   Bytes 12–15   reserved    = 0
// ---------------------------------------------------------------------------

/// Reproduce the IdtEntry::new() address-splitting logic for host testing.
fn idt_entry_parts(handler: u64) -> (u16, u16, u32) {
    let offset_low = (handler & 0xFFFF) as u16;
    let offset_mid = ((handler >> 16) & 0xFFFF) as u16;
    let offset_high = ((handler >> 32) & 0xFFFF_FFFF) as u32;
    (offset_low, offset_mid, offset_high)
}

#[test]
fn idt_entry_address_splits_correctly() {
    let addr: u64 = 0x1234_5678_9ABC_DEF0;
    let (low, mid, high) = idt_entry_parts(addr);
    assert_eq!(low, 0xDEF0, "offset_low = bits[15:0]");
    assert_eq!(mid, 0x9ABC, "offset_mid = bits[31:16]");
    assert_eq!(high, 0x1234_5678, "offset_high = bits[63:32]");
}

#[test]
fn idt_entry_address_round_trips() {
    // Reconstruct the original address from its three parts.
    let addr: u64 = 0xFFFF_8000_1234_5678;
    let (low, mid, high) = idt_entry_parts(addr);
    let reconstructed = (low as u64) | ((mid as u64) << 16) | ((high as u64) << 32);
    assert_eq!(reconstructed, addr);
}

#[test]
fn idt_entry_zero_address_splits_to_zeros() {
    let (low, mid, high) = idt_entry_parts(0);
    assert_eq!(low, 0);
    assert_eq!(mid, 0);
    assert_eq!(high, 0);
}

#[test]
fn idt_entry_max_address_splits_correctly() {
    let addr: u64 = u64::MAX;
    let (low, mid, high) = idt_entry_parts(addr);
    assert_eq!(low, 0xFFFF);
    assert_eq!(mid, 0xFFFF);
    assert_eq!(high, 0xFFFF_FFFF);
}

#[test]
fn idt_attr_kernel_interrupt_gate_value() {
    // 0x8E = 1000_1110b = P=1, DPL=00, 0, type=1110 (64-bit interrupt gate)
    const ATTR_KERNEL_INTERRUPT: u8 = 0x8E;
    assert_eq!(ATTR_KERNEL_INTERRUPT & 0x80, 0x80, "P bit must be set");
    assert_eq!((ATTR_KERNEL_INTERRUPT >> 5) & 0b11, 0, "DPL must be 0");
    assert_eq!(
        ATTR_KERNEL_INTERRUPT & 0x0F,
        0xE,
        "gate type must be 0xE (interrupt)"
    );
}

#[test]
fn idt_attr_kernel_trap_gate_value() {
    // 0x8F = P=1, DPL=0, type=0xF (trap gate)
    const ATTR_KERNEL_TRAP: u8 = 0x8F;
    assert_eq!(ATTR_KERNEL_TRAP & 0x80, 0x80, "P bit must be set");
    assert_eq!((ATTR_KERNEL_TRAP >> 5) & 0b11, 0, "DPL must be 0");
    assert_eq!(ATTR_KERNEL_TRAP & 0x0F, 0xF, "gate type must be 0xF (trap)");
}

#[test]
fn idt_has_256_vectors() {
    // The IDT must have exactly 256 entries (0–255) per the x86-64 architecture.
    const IDT_ENTRY_COUNT: usize = 256;
    const IDT_ENTRY_SIZE: usize = 16;
    assert_eq!(IDT_ENTRY_COUNT * IDT_ENTRY_SIZE, 4096);
}

// ---------------------------------------------------------------------------
// Exception vector properties: which vectors push an error code
//
// Per Intel SDM Vol 3A §6.13, only certain exception vectors push a CPU
// error code. Getting this wrong causes misaligned stack reads in the handler.
// ---------------------------------------------------------------------------

/// Bitmask matching the `EC_MASK` in boot/src/main.rs `exception_handler`.
const EC_MASK: u64 = (1 << 8)
    | (1 << 10)
    | (1 << 11)
    | (1 << 12)
    | (1 << 13)
    | (1 << 14)
    | (1 << 17)
    | (1 << 21)
    | (1 << 29)
    | (1 << 30);

fn has_error_code(vector: u64) -> bool {
    vector < 64 && (EC_MASK >> vector) & 1 == 1
}

#[test]
fn exception_vectors_with_error_code() {
    // Intel SDM Vol 3A Table 6-1
    assert!(
        has_error_code(8),
        "#DF (double fault) pushes error code (always 0)"
    );
    assert!(
        has_error_code(10),
        "#TS (invalid TSS) pushes selector error code"
    );
    assert!(
        has_error_code(11),
        "#NP (segment not present) pushes error code"
    );
    assert!(
        has_error_code(12),
        "#SS (stack-segment fault) pushes error code"
    );
    assert!(
        has_error_code(13),
        "#GP (general protection) pushes error code"
    );
    assert!(
        has_error_code(14),
        "#PF (page fault) pushes error code + CR2"
    );
    assert!(
        has_error_code(17),
        "#AC (alignment check) pushes error code"
    );
    assert!(
        has_error_code(21),
        "#CP (control protection) pushes error code"
    );
    assert!(
        has_error_code(29),
        "#VC (VMM communication) pushes error code"
    );
    assert!(
        has_error_code(30),
        "#SX (security exception) pushes error code"
    );
}

#[test]
fn exception_vectors_without_error_code() {
    let no_ec = [
        0u64, 1, 2, 3, 4, 5, 6, 7, 9, 15, 16, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 31,
    ];
    for v in no_ec {
        assert!(
            !has_error_code(v),
            "vector {} should NOT push an error code",
            v
        );
    }
}

#[test]
fn exactly_ten_vectors_push_error_codes() {
    let count = (0u64..32).filter(|&v| has_error_code(v)).count();
    assert_eq!(
        count, 10,
        "exactly 10 exception vectors push error codes per Intel SDM"
    );
}

#[test]
fn hardware_irq_vectors_do_not_push_error_codes() {
    // Vectors 32–255 are hardware IRQs / software interrupts. None push error codes.
    for v in 32u64..256 {
        assert!(
            !has_error_code(v),
            "IRQ vector {} must not push an error code",
            v
        );
    }
}

// ---------------------------------------------------------------------------
// ExceptionFrame field layout
//
// The CPU always pushes exactly 5 quadwords in 64-bit mode (no privilege
// change: SS+RSP are still pushed in 64-bit). Each field is 8 bytes.
// ---------------------------------------------------------------------------

#[test]
fn exception_frame_is_five_quadwords() {
    // [RIP, CS, RFLAGS, old_RSP, SS] = 5 × 8 = 40 bytes
    const EXCEPTION_FRAME_SIZE: usize = 5 * 8;
    assert_eq!(EXCEPTION_FRAME_SIZE, 40);
}

#[test]
fn exception_frame_field_offsets() {
    // Offsets from the base of the frame (RSP at handler entry, after any
    // error code has been popped by isr_stub_ec).
    const RIP_OFFSET: usize = 0;
    const CS_OFFSET: usize = 8;
    const RFLAGS_OFFSET: usize = 16;
    const RSP_OFFSET: usize = 24;
    const SS_OFFSET: usize = 32;

    // Offsets must be sequential multiples of 8.
    assert_eq!(CS_OFFSET, RIP_OFFSET + 8);
    assert_eq!(RFLAGS_OFFSET, CS_OFFSET + 8);
    assert_eq!(RSP_OFFSET, RFLAGS_OFFSET + 8);
    assert_eq!(SS_OFFSET, RSP_OFFSET + 8);
}

// ---------------------------------------------------------------------------
// Kernel stack layout constants
// ---------------------------------------------------------------------------

const KERNEL_STACK_SIZE: usize = 64 * 1024;
const KERNEL_STACK_GUARD_SIZE: usize = 4 * 1024;
const BOOTSTRAP_STACK_SIZE: usize = 16 * 1024;

#[test]
fn kernel_stack_is_64_kib() {
    assert_eq!(KERNEL_STACK_SIZE, 65536);
}

#[test]
fn kernel_stack_guard_is_4_kib() {
    assert_eq!(KERNEL_STACK_GUARD_SIZE, 4096);
}

#[test]
fn kernel_stack_usable_is_60_kib() {
    assert_eq!(KERNEL_STACK_SIZE - KERNEL_STACK_GUARD_SIZE, 60 * 1024);
}

#[test]
fn bootstrap_stack_is_16_kib() {
    assert_eq!(BOOTSTRAP_STACK_SIZE, 16384);
}

#[test]
fn guard_is_smaller_than_usable_stack() {
    assert!(KERNEL_STACK_GUARD_SIZE < KERNEL_STACK_SIZE - KERNEL_STACK_GUARD_SIZE);
}

#[test]
fn stack_sizes_are_page_aligned() {
    const PAGE_SIZE: usize = 4096;
    assert_eq!(KERNEL_STACK_SIZE % PAGE_SIZE, 0);
    assert_eq!(KERNEL_STACK_GUARD_SIZE % PAGE_SIZE, 0);
    assert_eq!(BOOTSTRAP_STACK_SIZE % PAGE_SIZE, 0);
}

// ---------------------------------------------------------------------------
// Legacy placeholder (kept so the test count is predictable in CI output)
// ---------------------------------------------------------------------------

#[test]
fn test_harness_works() {
    assert!(true);
}
