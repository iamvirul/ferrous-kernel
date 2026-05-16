//! Boot-phase serial console driver (Phase 1.4.4).
//!
//! Provides [`BootSerialPort`]: a 16550-compatible UART driver for the UEFI
//! boot environment.  This module is the authoritative home for all post-UEFI
//! serial I/O in the bootloader binary.
//!
//! # Relationship to the kernel driver
//!
//! The production-quality UART driver lives in
//! `kernel/src/drivers/serial.rs` + `kernel/src/drivers/console.rs`.
//! This boot-side implementation is a deliberately lightweight duplicate:
//! the bootloader is a separate crate and cannot depend on the kernel.
//!
//! Both implementations share the same hardware model (16550A, COM1 0x3F8,
//! 115200/8N1) and the same inline-assembly PIO primitives.
//!
//! # Module-level helpers
//!
//! The free functions [`serial_init`], [`serial_write_str`],
//! [`serial_write_usize`], and [`serial_write_usize_hex`] provide a
//! zero-allocation interface used by the panic handler, the stack-trace
//! walker, and the early boot path — contexts where `fmt::Write` or heap
//! allocation may be unavailable.

use core::fmt;

// ---------------------------------------------------------------------------
// Hardware constants — COM1 (0x3F8), 115200 baud, 8N1
// ---------------------------------------------------------------------------

/// I/O base address for COM1.
const COM1_BASE: u16 = 0x3F8;

/// REG offsets (from base).
const REG_DATA: u16 = 0; // THR (write) / RBR (read), DLAB=0
const REG_IER: u16 = 1; // Interrupt Enable, DLAB=0
const REG_DLL: u16 = 0; // Divisor Latch Low, DLAB=1
const REG_DLM: u16 = 1; // Divisor Latch High, DLAB=1
const REG_FCR: u16 = 2; // FIFO Control
const REG_LCR: u16 = 3; // Line Control
const REG_MCR: u16 = 4; // Modem Control
const REG_LSR: u16 = 5; // Line Status

/// LCR: Divisor Latch Access Bit — enables baud-rate programming.
const LCR_DLAB: u8 = 0x80;

/// LCR: 8 data bits, no parity, 1 stop bit (8N1).
const LCR_8N1: u8 = 0x03;

/// FCR: Enable FIFO, flush Rx/Tx, 14-byte receive trigger.
const FCR_ENABLE_CLEAR: u8 = 0xC7;

/// MCR: Assert DTR + RTS + AUX2.
const MCR_DTR_RTS_AUX2: u8 = 0x0B;

/// LSR bit 0: Data Ready — a byte is available in the Receive Buffer Register.
const LSR_DATA_READY: u8 = 0x01;

/// LSR bit 5: Transmit Holding Register Empty.
const LSR_THRE: u8 = 0x20;

/// Baud-rate divisor for 115200 from the 16550's 1.8432 MHz base clock.
///
/// `divisor = 1_843_200 / (16 × 115_200) = 1`
const BAUD_115200_DIVISOR: u16 = 1;

// ---------------------------------------------------------------------------
// BootSerialPort
// ---------------------------------------------------------------------------

/// A lightweight 16550-compatible UART driver for the boot phase.
///
/// `BootSerialPort` is `Copy` and zero-sized at run time (the I/O base port
/// is a compile-time constant for COM1).  Multiple instances can coexist
/// safely — they all address the same hardware register file.
///
/// # Invariant
///
/// The caller must have called [`BootSerialPort::init`] (or the
/// module-level [`serial_init`]) before invoking any transmit or receive
/// method.  All I/O requires CPL=0.
#[derive(Clone, Copy)]
pub struct BootSerialPort {
    base: u16,
}

impl BootSerialPort {
    /// Create a `BootSerialPort` bound to **COM1** (I/O base 0x3F8).
    ///
    /// `const fn` so this can be used in static initialisers.
    pub const fn com1() -> Self {
        Self { base: COM1_BASE }
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Initialise the UART: 115200 baud, 8 data bits, no parity, 1 stop bit.
    ///
    /// # Safety
    ///
    /// - Caller must be executing at CPL=0 (ring 0).
    /// - No other code may access COM1 registers concurrently.
    pub fn init(&self) {
        // SAFETY: CPL=0 required; delegated to caller.
        unsafe {
            self.outb(REG_IER, 0x00); // disable interrupts
            self.outb(REG_LCR, LCR_DLAB); // enable DLAB
            self.outb(REG_DLL, (BAUD_115200_DIVISOR & 0xFF) as u8);
            self.outb(REG_DLM, (BAUD_115200_DIVISOR >> 8) as u8);
            self.outb(REG_LCR, LCR_8N1); // 8N1, clears DLAB
            self.outb(REG_FCR, FCR_ENABLE_CLEAR); // enable + flush FIFOs
            self.outb(REG_MCR, MCR_DTR_RTS_AUX2); // DTR + RTS + AUX2
        }
    }

    // -----------------------------------------------------------------------
    // Transmit
    // -----------------------------------------------------------------------

    /// Write a single byte, polling until the Transmit Holding Register is
    /// empty.  Safe to call from a panic handler.
    pub fn write_byte(&self, byte: u8) {
        self.poll_tx_ready();
        // SAFETY: CPL=0 invariant maintained from init().
        unsafe { self.outb(REG_DATA, byte) };
    }

    /// Write every byte of `s`, translating `\n` to `\r\n`.
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    /// Write `n` as a decimal ASCII string with no heap allocation.
    pub fn write_usize(&self, mut n: usize) {
        if n == 0 {
            self.write_byte(b'0');
            return;
        }
        let mut buf = [0u8; 20]; // 20 decimal digits cover u64::MAX
        let mut len = 0usize;
        while n > 0 {
            buf[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
        for i in (0..len).rev() {
            self.write_byte(buf[i]);
        }
    }

    /// Write `n` as a lowercase hexadecimal ASCII string (no leading `0x`).
    ///
    /// Leading zeroes are suppressed; `0` is emitted as `"0"`.
    pub fn write_usize_hex(&self, n: usize) {
        const HEX: &[u8] = b"0123456789abcdef";
        if n == 0 {
            self.write_byte(b'0');
            return;
        }
        let mut buf = [0u8; 16];
        let mut len = 0usize;
        let mut val = n;
        while val > 0 {
            buf[len] = HEX[val & 0xF];
            val >>= 4;
            len += 1;
        }
        for i in (0..len).rev() {
            self.write_byte(buf[i]);
        }
    }

    // -----------------------------------------------------------------------
    // Receive
    // -----------------------------------------------------------------------

    /// Return `true` if at least one byte is waiting in the Receive Buffer
    /// Register (non-blocking, checks LSR bit 0).
    pub fn data_available(&self) -> bool {
        // SAFETY: CPL=0 invariant maintained from init().
        (unsafe { self.inb(REG_LSR) } & LSR_DATA_READY) != 0
    }

    /// Non-blocking receive: return the next byte if available, else `None`.
    pub fn try_read_byte(&self) -> Option<u8> {
        if self.data_available() {
            // SAFETY: data_available() confirmed DR=1; RBR read is valid.
            Some(unsafe { self.inb(REG_DATA) })
        } else {
            None
        }
    }

    /// Non-blocking receive: return the next character if available.
    ///
    /// Non-ASCII bytes are represented as [`char::REPLACEMENT_CHARACTER`].
    pub fn try_read_char(&self) -> Option<char> {
        self.try_read_byte().map(|b| {
            if b.is_ascii() {
                b as char
            } else {
                char::REPLACEMENT_CHARACTER
            }
        })
    }

    // -----------------------------------------------------------------------
    // Private PIO helpers
    // -----------------------------------------------------------------------

    /// Spin until LSR bit 5 (THRE) is set.
    fn poll_tx_ready(&self) {
        loop {
            // SAFETY: CPL=0 invariant maintained from init().
            if (unsafe { self.inb(REG_LSR) } & LSR_THRE) != 0 {
                return;
            }
        }
    }

    /// Write `value` to `self.base + offset`.
    ///
    /// # Safety
    ///
    /// Caller must be at CPL=0. `offset` must be a valid 16550 register offset.
    unsafe fn outb(&self, offset: u16, value: u8) {
        core::arch::asm!(
            "out dx, al",
            in("dx") self.base + offset,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }

    /// Read a byte from `self.base + offset`.
    ///
    /// # Safety
    ///
    /// Caller must be at CPL=0. `offset` must be a valid 16550 register offset.
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

// ---------------------------------------------------------------------------
// fmt::Write — enables write! / writeln! on BootSerialPort
// ---------------------------------------------------------------------------

impl fmt::Write for BootSerialPort {
    /// Write a string slice to COM1, translating `\n` to `\r\n`.
    ///
    /// Never returns `Err` — the polling serial path is infallible.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Delegate to the inherent write_str (different receiver — no ambiguity).
        BootSerialPort::write_str(self, s);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Module-level free functions
//
// These thin wrappers over BootSerialPort::com1() are the stable internal API
// used by:
//   - boot/src/main.rs   (panic handler, kernel_main steps)
//   - boot/src/unwind.rs (stack-trace printer)
//   - boot/src/logger.rs (SerialWriter)
//
// Constructing BootSerialPort::com1() is zero-cost (just loads a u16
// constant); it does NOT re-initialise the hardware.
// ---------------------------------------------------------------------------

/// Initialise COM1 to 115200 baud, 8N1.
///
/// Must be called once at the top of `kernel_main`, before any serial output
/// from the kernel phase.  The UEFI boot phase may have left the UART in a
/// different state.
pub fn serial_init() {
    BootSerialPort::com1().init();
}

/// Write a string slice to COM1, translating `\n` to `\r\n`.
pub fn serial_write_str(s: &str) {
    BootSerialPort::com1().write_str(s);
}

/// Write `n` as a decimal ASCII string to COM1 (no allocation).
pub fn serial_write_usize(n: usize) {
    BootSerialPort::com1().write_usize(n);
}

/// Write `n` as a lowercase hex ASCII string to COM1 (no allocation).
pub fn serial_write_usize_hex(n: usize) {
    BootSerialPort::com1().write_usize_hex(n);
}
