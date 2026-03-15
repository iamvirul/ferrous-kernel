//! Interrupt Descriptor Table (IDT) for x86-64.
//!
//! The IDT maps every interrupt/exception vector (0–255) to a handler function.
//! On x86-64 each entry is a 16-byte **gate descriptor**. The CPU reads this
//! table whenever an exception or interrupt fires, using the IDTR register
//! (loaded by the `LIDT` instruction) to locate the table.
//!
//! # Interrupt vs. trap gates
//!
//! | Gate type | Value | `IF` flag on entry |
//! |-----------|-------|--------------------|
//! | Interrupt | `0xE` | Cleared (further interrupts masked) |
//! | Trap      | `0xF` | Unchanged |
//!
//! Phase 1 uses interrupt gates exclusively, so additional exceptions cannot
//! nest while the handler is running.
//!
//! # Gate descriptor layout (16 bytes)
//!
//! ```text
//! Bytes  0– 1   Target offset bits [15:0]
//! Bytes  2– 3   Segment selector (kernel CS = 0x0008)
//! Byte   4      IST: bits [2:0] = IST index (0 = use current stack),
//!                    bits [7:3] = reserved (must be 0)
//! Byte   5      Type/attributes: P | DPL[1:0] | 0 | Type[3:0]
//!                    Interrupt gate: P=1, DPL=0, type=0xE → 0x8E
//! Bytes  6– 7   Target offset bits [31:16]
//! Bytes  8–11   Target offset bits [63:32]
//! Bytes 12–15   Reserved (must be 0)
//! ```
//!
//! # Exception frame (pushed by CPU before jumping to handler)
//!
//! ```text
//! ┌──────────────────────┐ ← RSP on handler entry (no error code)
//! │  RIP (return address)│  8 bytes
//! │  CS                  │  8 bytes (upper 48 bits zero)
//! │  RFLAGS              │  8 bytes
//! │  RSP (caller stack)  │  8 bytes
//! │  SS                  │  8 bytes (upper 48 bits zero)
//! └──────────────────────┘
//!
//! For exceptions WITH an error code, the CPU pushes it between the
//! saved RIP and the start of the frame above (i.e. at RSP before
//! the frame is visible to the handler):
//! ┌──────────────────────┐ ← RSP on handler entry (with error code)
//! │  Error code          │  8 bytes
//! │  RIP                 │  ↑ same frame as above
//! │  …                   │
//! └──────────────────────┘
//! ```
//!
//! Vectors that push an error code: 8, 10, 11, 12, 13, 14, 17, 21, 28, 29, 30.
//!
//! # Phase notes
//!
//! In Phase 1, the `IDT` static and `idt_init()` live in `boot/src/main.rs`
//! (same binary as the bootloader). When the kernel becomes a separate ELF the
//! IDT moves here and the duplicate in `boot` is removed.
//!
//! Phase 1 installs stub handlers for all 32 CPU exception vectors and a
//! generic stub for IRQ vectors 32–255. All stubs print the vector name over
//! serial and halt (`HLT` loop). Interrupts are **not enabled** (`STI` is not
//! called); the IDT is ready for CPU exceptions only.

// ---------------------------------------------------------------------------
// Gate type constants
// ---------------------------------------------------------------------------

/// 64-bit interrupt gate: clears IF on entry (masks further interrupts).
pub const GATE_INTERRUPT: u8 = 0xE;

/// 64-bit trap gate: preserves IF on entry.
pub const GATE_TRAP: u8 = 0xF;

/// Type/attribute byte for a ring-0 present interrupt gate: `P=1, DPL=0, 0, type=0xE`.
pub const ATTR_KERNEL_INTERRUPT: u8 = 0x8E;

/// Type/attribute byte for a ring-0 present trap gate: `P=1, DPL=0, 0, type=0xF`.
pub const ATTR_KERNEL_TRAP: u8 = 0x8F;

/// Kernel code segment selector (GDT index 1, RPL=0).
const KERNEL_CS: u16 = 0x0008;

// ---------------------------------------------------------------------------
// Gate descriptor
// ---------------------------------------------------------------------------

/// A single IDT gate descriptor (16 bytes).
///
/// Use [`IdtEntry::new`] to create a configured entry or [`IdtEntry::missing`]
/// for an empty (not-present) placeholder.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    /// Handler address bits [15:0].
    pub offset_low: u16,
    /// Segment selector to load into CS when the gate fires.
    pub selector: u16,
    /// IST index in bits [2:0]; bits [7:3] must be zero.
    ///
    /// `0` = use the current stack (RSP0 from TSS, or the interrupted stack
    /// if already at ring 0). Values 1–7 switch to the corresponding IST
    /// stack from the TSS — required for double-fault handlers in Phase 1.3+.
    pub ist: u8,
    /// Gate type and attributes: `P | DPL[1:0] | 0 | Type[3:0]`.
    ///
    /// Use [`ATTR_KERNEL_INTERRUPT`] for normal exception/interrupt handlers.
    pub type_attr: u8,
    /// Handler address bits [31:16].
    pub offset_mid: u16,
    /// Handler address bits [63:32].
    pub offset_high: u32,
    /// Reserved — must be zero.
    pub reserved: u32,
}

impl IdtEntry {
    /// Create a not-present placeholder entry (all fields zero).
    ///
    /// If a vector fires while its entry is `missing`, the CPU raises a
    /// #NP (Segment Not Present) fault, which in turn hits the #NP handler.
    /// If that is also missing the CPU triple-faults and resets.
    pub const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0, // P = 0 → not present
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Create a kernel-mode interrupt gate pointing to `handler`.
    ///
    /// Sets:
    /// - `selector` = [`KERNEL_CS`] (0x0008)
    /// - `type_attr` = [`ATTR_KERNEL_INTERRUPT`] (interrupt gate, ring 0)
    /// - `ist` = 0 (use current stack)
    ///
    /// # Safety
    ///
    /// `handler` must be the address of a function that:
    /// 1. Runs entirely at CPL=0.
    /// 2. Ends with `IRETQ` (or diverges — never returns).
    /// 3. Correctly handles the presence or absence of an error code on the
    ///    stack for the target vector.
    pub fn new(handler: u64) -> Self {
        Self {
            offset_low: (handler & 0xFFFF) as u16,
            selector: KERNEL_CS,
            ist: 0,
            type_attr: ATTR_KERNEL_INTERRUPT,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFF_FFFF) as u32,
            reserved: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// IDT
// ---------------------------------------------------------------------------

/// The Interrupt Descriptor Table — 256 gate descriptors.
///
/// Must be declared `static` so the CPU can access it indefinitely after
/// [`IdtPointer`] is loaded into IDTR.
#[repr(C, align(16))]
pub struct Idt(pub [IdtEntry; 256]);

// ---------------------------------------------------------------------------
// IDT pointer (passed to LIDT)
// ---------------------------------------------------------------------------

/// Pointer structure passed to the `LIDT` instruction.
///
/// Must be `#[repr(C, packed)]` so the CPU sees exactly:
/// - 2 bytes: table size minus 1 (`limit`)
/// - 8 bytes: linear base address of the table (`base`)
#[repr(C, packed)]
pub struct IdtPointer {
    /// IDT size in bytes, minus 1. For 256 entries: `256 * 16 - 1 = 4095`.
    pub limit: u16,
    /// Linear address of the IDT.
    pub base: u64,
}

// ---------------------------------------------------------------------------
// Exception frame
// ---------------------------------------------------------------------------

/// CPU-pushed exception frame, present at RSP when an exception handler runs.
///
/// For vectors **without** an error code, RSP points here on handler entry.
/// For vectors **with** an error code (8, 10–14, 17, 21, 28–30), the error
/// code is pushed **below** the saved RIP — i.e., at `RSP` and this frame
/// starts at `RSP + 8`.
///
/// The compiler may not generate correct code for reads from a `#[repr(C)]`
/// struct pointed to by a stack pointer obtained from asm, so callers should
/// use raw pointer reads or the `x86-interrupt` ABI (nightly).
#[repr(C)]
pub struct ExceptionFrame {
    /// Instruction pointer to return to after `IRETQ`.
    pub rip: u64,
    /// Code segment selector at the time of the exception.
    pub cs: u64,
    /// CPU flags (`RFLAGS`) at the time of the exception.
    pub rflags: u64,
    /// Stack pointer at the time of the exception.
    pub rsp: u64,
    /// Stack segment at the time of the exception.
    pub ss: u64,
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Load the IDTR with the address and size of `idt`.
///
/// # Safety
///
/// - Caller must be at CPL=0.
/// - `idt` must remain valid (mapped and unmodified) for as long as the CPU
///   may receive interrupts or exceptions.
/// - All handlers registered in `idt` must conform to the x86-64 interrupt
///   calling convention (preserve CPU state, end with `IRETQ` or diverge).
/// - Interrupts should be disabled while modifying or loading the IDT to
///   prevent the CPU from reading a partially-initialised table.
pub unsafe fn load(idt: &'static Idt) {
    let ptr = IdtPointer {
        limit: (core::mem::size_of::<Idt>() - 1) as u16,
        base: idt as *const Idt as u64,
    };

    // SAFETY: `ptr` holds the address and size of a valid static IDT.
    // LIDT writes only to the IDTR register; no memory is modified.
    // The caller guarantees CPL=0 and that `idt` outlives IDTR.
    core::arch::asm!(
        "lidt [{ptr}]",
        ptr = in(reg) &ptr,
        options(readonly, nostack, preserves_flags),
    );
}
