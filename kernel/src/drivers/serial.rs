//! 16550-compatible UART serial driver (Phase 1.4.4).
//!
//! Provides [`SerialPort`]: a fully-featured, polling-based 16550A UART driver
//! for kernel use.  The driver supports:
//!
//! - **Configurable baud rate** via [`BaudRate`] (9600 – 115200).
//! - **Configurable frame format** via [`DataBits`], [`Parity`], [`StopBits`].
//! - **Multiple COM ports** via [`ComPort`] (COM1–COM4).
//! - **Transmit** (blocking, polling THRE).
//! - **Receive** (non-blocking [`SerialPort::try_read_byte`] and blocking
//!   [`SerialPort::read_byte`]).
//! - **`core::fmt::Write`** for use with `write!` / `writeln!`.
//!
//! # Hardware
//!
//! COM1 is mapped to I/O ports 0x3F8–0x3FF. All register offsets below are
//! relative to the port base. The UART model is the National Semiconductor
//! 16550A (or compatible), universally present in x86 PC-compatible systems.
//!
//! # Safety model
//!
//! Raw I/O port access requires CPL=0 (ring 0). All `unsafe` is confined to
//! the two `inb`/`outb` helpers; everything above them is safe.

use core::fmt;

// ---------------------------------------------------------------------------
// Register offsets (relative to COM port base)
// ---------------------------------------------------------------------------

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

/// FCR: Enable FIFO, clear Rx/Tx FIFOs, 14-byte receive trigger level.
const FCR_ENABLE_CLEAR: u8 = 0xC7;

/// MCR: Assert DTR + RTS, enable AUX output 2 (required for interrupts on
/// real hardware; harmless in polling mode).
const MCR_DTR_RTS_AUX2: u8 = 0x0B;

/// LSR bit 0: Data Ready — at least one byte is available in the Receive
/// Buffer Register (RBR). Used by [`SerialPort::data_available`].
const LSR_DATA_READY: u8 = 0x01;

/// LSR bit 5: Transmit Holding Register Empty — safe to write the next byte.
const LSR_THRE: u8 = 0x20;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during serial port operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialError {
    /// The requested baud rate is not supported by this driver.
    InvalidBaudRate,
    /// A hardware operation (e.g. loopback self-test) timed out.
    PortTimeout,
}

// ---------------------------------------------------------------------------
// Baud rate
// ---------------------------------------------------------------------------

/// Standard UART baud rates supported by the 16550A.
///
/// The 16550A derives baud rate from a 1.8432 MHz base clock using a 16×
/// oversampling prescaler:
///
/// `divisor = 1_843_200 / (16 × baud_rate)`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaudRate {
    /// 9600 baud — divisor 12.
    Baud9600,
    /// 19200 baud — divisor 6.
    Baud19200,
    /// 38400 baud — divisor 3.
    Baud38400,
    /// 57600 baud — divisor 2.
    Baud57600,
    /// 115200 baud — divisor 1 (maximum standard rate).
    Baud115200,
}

impl BaudRate {
    /// Return the 16-bit divisor value to write to the Divisor Latch registers.
    pub const fn divisor(self) -> u16 {
        match self {
            BaudRate::Baud9600 => 12,
            BaudRate::Baud19200 => 6,
            BaudRate::Baud38400 => 3,
            BaudRate::Baud57600 => 2,
            BaudRate::Baud115200 => 1,
        }
    }

    /// Human-readable baud rate string for diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            BaudRate::Baud9600 => "9600",
            BaudRate::Baud19200 => "19200",
            BaudRate::Baud38400 => "38400",
            BaudRate::Baud57600 => "57600",
            BaudRate::Baud115200 => "115200",
        }
    }
}

// ---------------------------------------------------------------------------
// Frame format
// ---------------------------------------------------------------------------

/// Number of data bits per UART frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataBits {
    /// 5 data bits.
    Five,
    /// 6 data bits.
    Six,
    /// 7 data bits.
    Seven,
    /// 8 data bits (standard).
    Eight,
}

impl DataBits {
    /// LCR bit pattern for this data-bit count (bits [1:0]).
    pub const fn lcr_bits(self) -> u8 {
        match self {
            DataBits::Five => 0x00,
            DataBits::Six => 0x01,
            DataBits::Seven => 0x02,
            DataBits::Eight => 0x03,
        }
    }
}

/// Parity mode for error detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    /// No parity bit (standard for most serial terminals).
    None,
    /// Odd parity — parity bit is set so the total number of 1-bits is odd.
    Odd,
    /// Even parity — parity bit is set so the total number of 1-bits is even.
    Even,
}

impl Parity {
    /// LCR bit pattern for this parity mode (bits [5:3]).
    pub const fn lcr_bits(self) -> u8 {
        match self {
            Parity::None => 0x00,
            Parity::Odd => 0x08,
            Parity::Even => 0x18,
        }
    }
}

/// Number of stop bits appended to each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    /// 1 stop bit (standard).
    One,
    /// 2 stop bits (1.5 stop bits for 5-bit data frames).
    Two,
}

impl StopBits {
    /// LCR bit pattern for this stop-bit count (bit 2).
    pub const fn lcr_bits(self) -> u8 {
        match self {
            StopBits::One => 0x00,
            StopBits::Two => 0x04,
        }
    }
}

// ---------------------------------------------------------------------------
// SerialConfig
// ---------------------------------------------------------------------------

/// Complete UART frame-format configuration.
///
/// Combine with [`SerialPort::init_with_config`] to program the UART.
///
/// # Example
///
/// ```ignore
/// // 9600 baud, 7 data bits, even parity, 2 stop bits:
/// let cfg = SerialConfig {
///     baud_rate: BaudRate::Baud9600,
///     data_bits: DataBits::Seven,
///     parity:    Parity::Even,
///     stop_bits: StopBits::Two,
/// };
/// unsafe { serial.init_with_config(&cfg).expect("UART init failed"); }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialConfig {
    /// UART baud rate.
    pub baud_rate: BaudRate,
    /// Number of data bits per frame.
    pub data_bits: DataBits,
    /// Parity mode.
    pub parity: Parity,
    /// Number of stop bits.
    pub stop_bits: StopBits,
}

impl SerialConfig {
    /// Standard configuration: 115200 baud, 8 data bits, no parity, 1 stop
    /// bit (8N1). This matches the QEMU default and nearly all serial monitors.
    pub const fn default_115200_8n1() -> Self {
        Self {
            baud_rate: BaudRate::Baud115200,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
        }
    }

    /// Compute the LCR value (without DLAB) for this configuration.
    ///
    /// The returned byte encodes data bits [1:0], stop bits [2], and parity
    /// [5:3] as specified by the 16550A data sheet.
    pub const fn lcr_byte(&self) -> u8 {
        self.data_bits.lcr_bits() | self.stop_bits.lcr_bits() | self.parity.lcr_bits()
    }
}

// ---------------------------------------------------------------------------
// ComPort — I/O base addresses for COM1–COM4
// ---------------------------------------------------------------------------

/// Standard x86 COM port I/O base addresses.
///
/// These are the conventional addresses for COM1–COM4 as defined by the
/// IBM PC architecture.  Not all systems expose COM3/COM4; check firmware
/// tables before assuming they are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComPort {
    /// COM1 — I/O base 0x3F8 (IRQ4).
    Com1,
    /// COM2 — I/O base 0x2F8 (IRQ3).
    Com2,
    /// COM3 — I/O base 0x3E8 (IRQ4, shared with COM1).
    Com3,
    /// COM4 — I/O base 0x2E8 (IRQ3, shared with COM2).
    Com4,
}

impl ComPort {
    /// Return the I/O base address for this COM port.
    pub const fn base_address(self) -> u16 {
        match self {
            ComPort::Com1 => 0x3F8,
            ComPort::Com2 => 0x2F8,
            ComPort::Com3 => 0x3E8,
            ComPort::Com4 => 0x2E8,
        }
    }
}

// ---------------------------------------------------------------------------
// SerialPort
// ---------------------------------------------------------------------------

/// A 16550-compatible UART serial port.
///
/// # Construction
///
/// ```ignore
/// let serial = SerialPort::new();          // COM1, default config
/// let serial = SerialPort::with_port(ComPort::Com2); // COM2
/// ```
///
/// # Initialisation
///
/// Always call [`init`](Self::init) or [`init_with_config`](Self::init_with_config)
/// before writing or reading:
///
/// ```ignore
/// // SAFETY: ring 0, exclusive access to COM1.
/// unsafe { serial.init(); }
/// ```
///
/// # Usage
///
/// ```ignore
/// serial.write_str("Hello from Ferrous!\n");
///
/// // fmt::Write
/// use core::fmt::Write;
/// let mut s = SerialPort::new();
/// unsafe { s.init(); }
/// write!(s, "frame {:08x}\n", addr).ok();
///
/// // Non-blocking receive
/// if let Some(byte) = serial.try_read_byte() {
///     // handle input
/// }
/// ```
pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    /// Create a new [`SerialPort`] bound to **COM1** (I/O base 0x3F8).
    ///
    /// This is a `const fn` so the port can be used as a static.
    /// Call [`init`](Self::init) before any I/O.
    pub const fn new() -> Self {
        Self {
            base: ComPort::Com1.base_address(),
        }
    }

    /// Create a [`SerialPort`] bound to the specified [`ComPort`].
    ///
    /// Call [`init`](Self::init) before any I/O.
    pub const fn with_port(port: ComPort) -> Self {
        Self {
            base: port.base_address(),
        }
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Initialise the UART with the **default configuration**: 115200 baud,
    /// 8 data bits, no parity, 1 stop bit (8N1).
    ///
    /// # Safety
    ///
    /// - The caller must be executing at CPL=0 (ring 0).
    /// - No other execution context may access this COM port's registers
    ///   concurrently.
    pub unsafe fn init(&self) {
        // SAFETY: pre-conditions inherited from caller (CPL=0, exclusive access).
        let _ = self.init_with_config(&SerialConfig::default_115200_8n1());
    }

    /// Initialise the UART with a **custom [`SerialConfig`]**.
    ///
    /// Sequence:
    /// 1. Disable all UART interrupts (polling mode only).
    /// 2. Enable Divisor Latch (DLAB=1); write baud rate divisor.
    /// 3. Clear DLAB; set frame format (data bits, parity, stop bits).
    /// 4. Enable and flush Rx/Tx FIFOs.
    /// 5. Assert DTR + RTS + AUX2 on the Modem Control Register.
    ///
    /// Returns `Ok(())` always (the 16550 does not provide a reliable
    /// hardware-present detection path without a loopback test; that is
    /// reserved for Phase 3 driver self-test).
    ///
    /// # Safety
    ///
    /// Same pre-conditions as [`init`](Self::init).
    pub unsafe fn init_with_config(&self, config: &SerialConfig) -> Result<(), SerialError> {
        // 1. Disable all UART-generated interrupts — polling mode.
        self.outb(REG_IER, 0x00);

        // 2. Enable Divisor Latch; write baud rate divisor (16-bit).
        self.outb(REG_LCR, LCR_DLAB);
        let div = config.baud_rate.divisor();
        self.outb(REG_DLL, (div & 0xFF) as u8);
        self.outb(REG_DLM, (div >> 8) as u8);

        // 3. Set frame format (clears DLAB).
        self.outb(REG_LCR, config.lcr_byte());

        // 4. Enable FIFOs, clear Rx/Tx, set 14-byte trigger level.
        self.outb(REG_FCR, FCR_ENABLE_CLEAR);

        // 5. Assert DTR, RTS, and AUX2 so the UART is ready to transmit.
        self.outb(REG_MCR, MCR_DTR_RTS_AUX2);

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Transmit
    // -----------------------------------------------------------------------

    /// Write a single byte to the serial port.
    ///
    /// Blocks (polls LSR) until the Transmit Holding Register is empty, then
    /// writes the byte.  Safe to call from a panic handler.
    pub fn write_byte(&self, byte: u8) {
        self.poll_tx_ready();
        // SAFETY: CPL=0 invariant is maintained from `init`.
        unsafe { self.outb(REG_DATA, byte) };
    }

    /// Write every byte of `s` to the serial port.
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
    // Receive
    // -----------------------------------------------------------------------

    /// Return `true` if at least one byte is available in the Receive Buffer
    /// Register (non-blocking).
    ///
    /// Checks LSR bit 0 (Data Ready).
    pub fn data_available(&self) -> bool {
        // SAFETY: CPL=0 invariant maintained from `init`.
        (unsafe { self.inb(REG_LSR) } & LSR_DATA_READY) != 0
    }

    /// Non-blocking receive: return the next byte if one is available,
    /// otherwise return `None`.
    ///
    /// Does **not** block; returns immediately.
    pub fn try_read_byte(&self) -> Option<u8> {
        if self.data_available() {
            // SAFETY: data_available() confirmed DR=1; reading RBR is valid.
            Some(unsafe { self.inb(REG_DATA) })
        } else {
            None
        }
    }

    /// Blocking receive: spin until a byte is available, then return it.
    ///
    /// Use [`try_read_byte`](Self::try_read_byte) in latency-sensitive or
    /// interrupt-driven contexts.
    pub fn read_byte(&self) -> u8 {
        loop {
            if let Some(b) = self.try_read_byte() {
                return b;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Spin until LSR bit 5 (THRE) is set: the Transmit Holding Register is
    /// empty and the UART is ready to accept the next byte.
    fn poll_tx_ready(&self) {
        loop {
            // SAFETY: CPL=0 invariant maintained from `init`.
            if (unsafe { self.inb(REG_LSR) } & LSR_THRE) != 0 {
                return;
            }
        }
    }

    /// Write `value` to the UART register at `self.base + offset`.
    ///
    /// # Safety
    ///
    /// - Caller must be at CPL=0.
    /// - `offset` must be a valid 16550 register offset (0–7).
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
    /// - `offset` must be a valid 16550 register offset (0–7).
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
// fmt::Write — enables `write!` / `writeln!` on SerialPort
// ---------------------------------------------------------------------------

impl fmt::Write for SerialPort {
    /// Write a string slice to the serial port.
    ///
    /// Delegates to [`SerialPort::write_str`] (the inherent method).
    /// Translates `\n` to `\r\n` for terminal compatibility.
    /// Never returns `Err` — the serial path is infallible.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Call the inherent method via explicit path to avoid name ambiguity.
        SerialPort::write_str(self, s);
        Ok(())
    }
}
