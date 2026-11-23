//! Task State Segment (TSS) for x86-64.

/// The 64-bit Task State Segment (TSS).
///
/// In 64-bit mode, hardware task switching is no longer supported, but the TSS
/// is still strictly required. It holds:
/// 1. The absolute stack pointers (`RSP0`, `RSP1`, `RSP2`) to load on privilege
///    level changes (e.g., an interrupt arriving while executing in Ring 3).
/// 2. The Interrupt Stack Table (IST) pointers (`IST1` through `IST7`) for
///    handling critical exceptions (like Double Fault) on known-good stacks.
/// 3. The I/O Port Bitmap offset.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct TaskStateSegment {
    reserved_1: u32,
    /// Stack pointer for Ring 0 (Kernel).
    pub rsp0: u64,
    /// Stack pointer for Ring 1.
    pub rsp1: u64,
    /// Stack pointer for Ring 2.
    pub rsp2: u64,
    reserved_2: u64,
    /// Interrupt Stack Table.
    pub ist: [u64; 7],
    reserved_3: u64,
    reserved_4: u16,
    /// I/O Map Base Address (offset from TSS base).
    pub iopb_offset: u16,
}

/// IST index (1-based, as encoded in an IDT gate) used for NMI delivery.
///
/// NMIs are not masked by `IF`, so one can arrive in the SYSCALL entry/exit
/// windows where CPL is 0 but RSP is still the user stack. With IST=0 the CPU
/// would push the NMI frame onto that user-controlled stack. A non-zero IST
/// forces a switch to `ist[NMI_IST_INDEX - 1]`, a kernel-owned stack.
pub const NMI_IST_INDEX: u8 = 1;

impl TaskStateSegment {
    /// Create a new, empty TSS.
    pub const fn new() -> Self {
        Self {
            reserved_1: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved_2: 0,
            ist: [0; 7],
            reserved_3: 0,
            reserved_4: 0,
            // Point the IOPB offset past the end of the TSS to deny all I/O
            // port access from user mode.
            iopb_offset: core::mem::size_of::<Self>() as u16,
        }
    }
}
