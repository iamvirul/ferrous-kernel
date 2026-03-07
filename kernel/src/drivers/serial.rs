//! 16550-compatible UART serial driver (COM1, I/O port 0x3F8).
//!
//! This driver initialises the UART to 115200 baud, 8N1 and provides a
//! polling-based transmit path. It is used for kernel debug output from
//! the earliest moment of kernel execution.
//!
//! # Hardware
//!
//! COM1 is mapped to I/O ports 0x3F8–0x3FF. All register offsets below are
//! relative to this base. The UART model is the National Semiconductor 16550A
//! (or compatible), universally present in x86 PC-compatible systems.
//!
//! # Safety model
//!
//! Raw I/O port access requires CPL=0 (ring 0). All `unsafe` is confined to
//! the two `inb`/`outb` helpers; everything above them is safe.

// ---------------------------------------------------------------------------
// Register map (offsets from COM1 base 0x3F8)
// ---------------------------------------------------------------------------

/// I/O base address for COM1.
const COM1_BASE: u16 = 0x3F8;

/// Data register: Transmit Holding (write) / Receive Buffer (read), DLAB=0.
const REG_DATA: u16 = 0;
/// Interrupt Enable Register, DLAB=0.
const REG_IER: u16 = 1;
/// Divisor Latch Low byte, DLAB=1.
const REG_DLL: u16 = 0;
/// Divisor Latch High byte, DLAB=1.
const REG_DLM: u16 = 1;
/// FIFO Control Register.
const REG_FCR: u16 = 2;
/// Line Control Register.
const REG_LCR: u16 = 3;
/// Modem Control Register.
const REG_MCR: u16 = 4;
/// Line Status Register.
const REG_LSR: u16 = 5;

// ---------------------------------------------------------------------------
// Register bit masks
// ---------------------------------------------------------------------------

/// LCR: Divisor Latch Access Bit. When set, offsets 0–1 address the baud
/// rate divisor instead of the data/IER registers.
const LCR_DLAB: u8 = 0x80;

/// LCR: 8 data bits, no parity, 1 stop bit (8N1).
const LCR_8N1: u8 = 0x03;

/// FCR: Enable FIFO, clear Rx/Tx FIFOs, 14-byte receive trigger level.
const FCR_ENABLE_CLEAR: u8 = 0xC7;

/// MCR: Assert DTR + RTS, enable AUX output 2 (required for interrupts on
/// real hardware; harmless in polling mode).
const MCR_DTR_RTS_AUX2: u8 = 0x0B;

/// LSR bit 5: Transmit Holding Register Empty — safe to write the next byte.
const LSR_THRE: u8 = 0x20;

// ---------------------------------------------------------------------------
// Baud rate
// ---------------------------------------------------------------------------

/// Baud rate divisor for 115200 from the 16550's 1.8432 MHz base clock.
///
/// divisor = 1_843_200 / (16 * 115_200) = 1
const BAUD_115200_DIVISOR: u16 = 1;

// ---------------------------------------------------------------------------
// SerialPort
// ---------------------------------------------------------------------------

/// A 16550-compatible UART serial port.
///
/// # Usage
///
/// ```ignore
/// // At kernel init, before any output:
/// let mut serial = SerialPort::new();
/// // SAFETY: ring 0, no other code is touching COM1.
/// unsafe { serial.init(); }
/// serial.write_str("Hello from Ferrous!\n");
/// ```
pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    /// Create a new `SerialPort` bound to COM1 (0x3F8).
    ///
    /// This is a `const fn` so a `SerialPort` can be used as a static.
    /// Call [`init`](Self::init) before writing any data.
    pub const fn new() -> Self {
        Self { base: COM1_BASE }
    }

    /// Initialise the UART: 115200 baud, 8 data bits, no parity, 1 stop bit.
    ///
    /// Sequence:
    /// 1. Disable all UART interrupts (we use polling only).
    /// 2. Set DLAB=1, write divisor latch (115200 baud).
    /// 3. Set DLAB=0, configure line format (8N1).
    /// 4. Enable and flush FIFOs.
    /// 5. Assert DTR + RTS on the modem control register.
    ///
    /// # Safety
    ///
    /// - The caller must be executing at CPL=0 (ring 0).
    /// - No other execution context may access COM1 registers concurrently.
    pub unsafe fn init(&self) {
        // Step 1: disable all UART-generated interrupts.
        // We poll the LSR instead of using IRQ4.
        self.outb(REG_IER, 0x00);

        // Step 2: enable the Divisor Latch so we can set the baud rate.
        self.outb(REG_LCR, LCR_DLAB);
        self.outb(REG_DLL, (BAUD_115200_DIVISOR & 0xFF) as u8);
        self.outb(REG_DLM, (BAUD_115200_DIVISOR >> 8) as u8);

        // Step 3: configure 8N1. Writing LCR without DLAB clears the
        // Divisor Latch Access Bit, returning offsets 0–1 to data/IER.
        self.outb(REG_LCR, LCR_8N1);

        // Step 4: enable FIFOs and flush any stale bytes.
        self.outb(REG_FCR, FCR_ENABLE_CLEAR);

        // Step 5: assert DTR and RTS so the UART is ready to transmit.
        self.outb(REG_MCR, MCR_DTR_RTS_AUX2);
    }

    /// Write a single byte to the serial port.
    ///
    /// Blocks (polls the Line Status Register) until the Transmit Holding
    /// Register is empty, then writes the byte.
    ///
    /// # Panics
    ///
    /// Does not panic. This function is safe to call from a panic handler.
    pub fn write_byte(&self, byte: u8) {
        self.poll_until_ready();
        // SAFETY: CPL=0 required; inherited from the invariant on `init`.
        unsafe { self.outb(REG_DATA, byte) };
    }

    /// Write every byte in `s` to the serial port.
    ///
    /// A bare `\n` is translated to `\r\n` so output is readable on a
    /// standard serial terminal.
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Spin until LSR bit 5 (THRE) is set, meaning the THR is empty and
    /// the UART is ready to accept the next transmit byte.
    fn poll_until_ready(&self) {
        loop {
            // SAFETY: CPL=0 required; inherited from the invariant on `init`.
            let lsr = unsafe { self.inb(REG_LSR) };
            if lsr & LSR_THRE != 0 {
                return;
            }
        }
    }

    /// Write `value` to the UART register at `self.base + offset`.
    ///
    /// # Safety
    ///
    /// - Caller must be at CPL=0.
    /// - `offset` must be a valid register offset for a 16550-compatible UART.
    unsafe fn outb(&self, offset: u16, value: u8) {
        core::arch::asm!(
            "out dx, al",
            in("dx") self.base + offset,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    /// Read a byte from the UART register at `self.base + offset`.
    ///
    /// # Safety
    ///
    /// - Caller must be at CPL=0.
    /// - `offset` must be a valid register offset for a 16550-compatible UART.
    unsafe fn inb(&self, offset: u16) -> u8 {
        let value: u8;
        core::arch::asm!(
            "in al, dx",
            in("dx") self.base + offset,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
        value
    }
}
