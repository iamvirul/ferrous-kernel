//! 8259A programmable interrupt controller pair (ADR-0009 §1).
//!
//! Remaps the master to vectors 32–39 and the slave to 40–47, masks
//! everything except IRQ0 (the PIT), sends EOIs, and recognises spurious
//! IRQ7/IRQ15.

use super::port::{inb, io_wait, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// ICW1: begin initialisation, ICW4 follows (edge-triggered, cascaded).
const ICW1_INIT_WITH_ICW4: u8 = 0x11;
/// ICW3 (master): a slave is attached on IRQ2.
const ICW3_MASTER_SLAVE_ON_IRQ2: u8 = 1 << 2;
/// ICW3 (slave): its cascade identity is 2.
const ICW3_SLAVE_CASCADE_ID: u8 = 2;
/// ICW4: 8086/88 mode, normal (non-automatic) EOI.
const ICW4_8086: u8 = 0x01;
/// OCW2: non-specific end of interrupt.
const OCW2_EOI: u8 = 0x20;
/// OCW3: the next command-port read returns the in-service register.
const OCW3_READ_ISR: u8 = 0x0B;

/// First vector of the master PIC (IRQ0..7 → 32..39).
pub const PIC1_VECTOR_BASE: u8 = 32;
/// First vector of the slave PIC (IRQ8..15 → 40..47).
pub const PIC2_VECTOR_BASE: u8 = 40;
/// Number of PIC lines (IRQ0..15).
pub const IRQ_LINES: u8 = 16;

/// The timer line (PIT channel 0).
pub const IRQ_TIMER: u8 = 0;
/// Master line that a spurious interrupt is reported on.
pub const IRQ_SPURIOUS_MASTER: u8 = 7;
/// Slave line that a spurious interrupt is reported on.
pub const IRQ_SPURIOUS_SLAVE: u8 = 15;

/// Interrupt mask after [`init`]: bit set = line masked. Only IRQ0 open.
pub const MASTER_MASK: u8 = !(1 << IRQ_TIMER);
/// The slave is fully masked (no slave devices yet).
pub const SLAVE_MASK: u8 = 0xFF;

/// Remap both PICs to vectors 32–47 and apply [`MASTER_MASK`] /
/// [`SLAVE_MASK`].
///
/// # Safety
///
/// CPL 0, interrupts disabled, IDT gates 32–47 installed (a line could
/// fire as soon as it is unmasked). Call once.
pub unsafe fn init() {
    // SAFETY (all writes below): these are the documented 8259A ICW1–ICW4
    // initialisation sequences on the standard PC ports. Interrupts are
    // disabled, so nothing can observe the half-initialised state.
    outb(PIC1_CMD, ICW1_INIT_WITH_ICW4);
    io_wait();
    outb(PIC2_CMD, ICW1_INIT_WITH_ICW4);
    io_wait();
    outb(PIC1_DATA, PIC1_VECTOR_BASE); // ICW2: vector offset
    io_wait();
    outb(PIC2_DATA, PIC2_VECTOR_BASE);
    io_wait();
    outb(PIC1_DATA, ICW3_MASTER_SLAVE_ON_IRQ2); // ICW3: cascade wiring
    io_wait();
    outb(PIC2_DATA, ICW3_SLAVE_CASCADE_ID);
    io_wait();
    outb(PIC1_DATA, ICW4_8086); // ICW4: mode
    io_wait();
    outb(PIC2_DATA, ICW4_8086);
    io_wait();
    outb(PIC1_DATA, MASTER_MASK); // OCW1: masks
    outb(PIC2_DATA, SLAVE_MASK);
}

/// Current interrupt masks `(master, slave)` (bit set = masked).
pub fn masks() -> (u8, u8) {
    // SAFETY: reading the IMR (data port) has no side effects.
    unsafe { (inb(PIC1_DATA), inb(PIC2_DATA)) }
}

/// Signal end of interrupt for `irq`. Must be sent for every *real* IRQ
/// before the line can fire again (for slave lines, both PICs need it).
///
/// # Safety
///
/// CPL 0; `irq` is the line currently being serviced.
pub unsafe fn send_eoi(irq: u8) {
    // SAFETY: OCW2 non-specific EOI to the PIC(s) that delivered `irq`.
    if irq >= 8 {
        outb(PIC2_CMD, OCW2_EOI);
    }
    outb(PIC1_CMD, OCW2_EOI);
}

/// Send EOI to the master only: for a spurious IRQ15, the master *did*
/// deliver the cascade interrupt, but the slave has nothing in service.
///
/// # Safety
///
/// CPL 0; servicing a spurious slave interrupt.
pub unsafe fn send_eoi_master_only() {
    // SAFETY: OCW2 non-specific EOI to the master PIC.
    outb(PIC1_CMD, OCW2_EOI);
}

/// In-service register of both PICs (bit n = IRQ n in service).
fn in_service() -> u16 {
    // SAFETY: OCW3 selects the ISR for the next command-port read; reading
    // it has no side effects.
    unsafe {
        outb(PIC1_CMD, OCW3_READ_ISR);
        outb(PIC2_CMD, OCW3_READ_ISR);
        (u16::from(inb(PIC2_CMD)) << 8) | u16::from(inb(PIC1_CMD))
    }
}

/// `true` if an interrupt reported on IRQ7 / IRQ15 is spurious (the line
/// is not actually in service). Spurious IRQs must not be acknowledged on
/// the PIC that raised them.
pub fn is_spurious(irq: u8) -> bool {
    debug_assert!(irq == IRQ_SPURIOUS_MASTER || irq == IRQ_SPURIOUS_SLAVE);
    in_service() & (1 << irq) == 0
}
