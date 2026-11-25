//! x86 port I/O (`in` / `out`), the single audited wrapper (ADR-0009).
//!
//! Every PIC and PIT access goes through these. Port I/O needs CPL 0, and
//! writing the wrong port can reprogram arbitrary hardware, so the
//! functions are `unsafe` and each call site states which device register
//! it targets.

/// Write `value` to I/O port `port`.
///
/// # Safety
///
/// CPL 0, and `port` must be a device register for which writing `value`
/// is valid in the device's current state.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags),
    );
}

/// Read a byte from I/O port `port`.
///
/// # Safety
///
/// CPL 0, and `port` must be a device register that is safe to read (some
/// reads have side effects, e.g. acknowledging a status).
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nomem, nostack, preserves_flags),
    );
    value
}

/// Unused diagnostic port, written to give slow legacy devices (the 8259)
/// time to settle between initialisation words.
const IO_WAIT_PORT: u16 = 0x80;

/// Wait roughly 1 µs by writing to the POST diagnostic port.
///
/// # Safety
///
/// CPL 0. Port 0x80 is unused after POST; writing it has no effect.
#[inline]
pub unsafe fn io_wait() {
    outb(IO_WAIT_PORT, 0);
}
