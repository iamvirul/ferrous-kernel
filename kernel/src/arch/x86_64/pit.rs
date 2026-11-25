//! 8254 programmable interval timer, channel 0 (ADR-0009 §2).

use super::port::outb;

/// Timer interrupt rate. 100 Hz = 10 ms tick, as #75 specifies. With
/// `TIME_SLICE_TICKS = 10` that is a 100 ms slice; raise this (e.g. to 1000)
/// for finer slices.
pub const TIMER_HZ: u32 = 100;

/// The PIT's input clock (Hz).
const PIT_INPUT_HZ: u32 = 1_193_182;

/// Channel-0 reload value, rounded to nearest.
pub const DIVISOR: u32 = (PIT_INPUT_HZ + TIMER_HZ / 2) / TIMER_HZ;
// Mode 2 needs a divisor in [2, 65535]; 65536 would be written as 0.
const _: () = assert!(
    DIVISOR >= 2 && DIVISOR <= 0xFFFF,
    "TIMER_HZ out of PIT range"
);

const PIT_CHANNEL0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;
/// Channel 0, access lobyte/hibyte, mode 2 (rate generator), binary.
/// Fields: bits 7–6 channel = 00, 5–4 access = 11, 3–1 mode = 010,
/// bit 0 BCD = 0.
const CMD_CH0_LOHI_MODE2: u8 = 0b0011_0100;

/// Program channel 0 to fire IRQ0 at [`TIMER_HZ`].
///
/// # Safety
///
/// CPL 0, interrupts disabled. Call once.
pub unsafe fn init() {
    let [lo, hi, ..] = DIVISOR.to_le_bytes();
    // SAFETY: the documented 8254 programming sequence on the standard
    // ports: a mode command, then the 16-bit reload value low byte first.
    outb(PIT_COMMAND, CMD_CH0_LOHI_MODE2);
    outb(PIT_CHANNEL0, lo);
    outb(PIT_CHANNEL0, hi);
}
