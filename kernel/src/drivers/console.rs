//! Console driver abstraction (Phase 1.4.4).
//!
//! Defines the [`Console`] trait — the canonical interface for character-based
//! I/O devices — and provides [`SerialConsole`]: a 16550-backed implementation
//! that wraps [`SerialPort`].
//!
//! # Design
//!
//! The [`Console`] trait is intentionally minimal:
//!
//! - **Output**: [`Console::write_char`], [`Console::write_str`] (required);
//!   `core::fmt::Write` is a supertrait so `write!` / `writeln!` work on any
//!   `Console`.
//! - **Input**: [`Console::read_char`] (optional, non-blocking — returns
//!   `None` when no input is available).
//! - **Flush**: [`Console::flush`] (no-op for polling serial; meaningful for
//!   buffered implementations in future phases).
//!
//! # Usage
//!
//! ```ignore
//! use kernel::drivers::console::{Console, SerialConsole};
//!
//! let mut con = SerialConsole::com1();
//! // SAFETY: CPL=0, exclusive COM1 access.
//! unsafe { con.init(); }
//!
//! con.write_str("Hello from Ferrous!\n");
//! write!(con, "frame count: {}\n", n).ok();
//!
//! if let Some(c) = con.read_char() {
//!     // process keypress
//! }
//! ```

use core::fmt;

use super::serial::{BaudRate, ComPort, SerialConfig, SerialError, SerialPort};

// ---------------------------------------------------------------------------
// Console trait
// ---------------------------------------------------------------------------

/// A character-oriented I/O console.
///
/// `Console` is a supertrait of [`core::fmt::Write`], so any `Console`
/// implementation automatically supports `write!` and `writeln!`.
///
/// # Implementation requirements
///
/// Implementors must provide:
/// - [`write_char`](Console::write_char) — emit one character.
/// - [`write_str`](Console::write_str) — emit a string slice.
/// - [`flush`](Console::flush) — flush any internal output buffer.
/// - [`read_char`](Console::read_char) — non-blocking input poll.
///
/// `core::fmt::Write::write_str` is provided automatically by blanket
/// delegation to [`write_str`](Console::write_str).
pub trait Console: fmt::Write {
    /// Write a single character to the console output.
    ///
    /// Implementations must handle `'\n'` by emitting `\r\n` so output is
    /// readable on a standard serial terminal.
    fn write_char(&mut self, c: char);

    /// Write a string slice to the console output.
    ///
    /// The default implementation calls [`write_char`](Console::write_char)
    /// for each character.  Override for better performance when the
    /// underlying device accepts byte bursts.
    fn write_str_console(&mut self, s: &str) {
        for c in s.chars() {
            Console::write_char(self, c);
        }
    }

    /// Flush any internally buffered output.
    ///
    /// For polling serial ports this is a no-op (there is no internal buffer).
    /// Buffered console implementations must drain and transmit all pending
    /// bytes.
    fn flush(&mut self);

    /// Non-blocking input: return the next character if one is available,
    /// or `None` if the input buffer is empty.
    ///
    /// Callers must not spin on this in a tight loop without yielding —
    /// use interrupt-driven input (Phase 3+) for latency-sensitive tasks.
    fn read_char(&mut self) -> Option<char>;
}

// ---------------------------------------------------------------------------
// SerialConsole
// ---------------------------------------------------------------------------

/// A [`Console`] backed by a 16550-compatible UART [`SerialPort`].
///
/// `SerialConsole` wraps a [`SerialPort`] and implements both [`Console`] and
/// [`core::fmt::Write`].  It is the preferred way to emit kernel debug output
/// and log messages to a serial terminal.
///
/// # Construction
///
/// ```ignore
/// // COM1 with default 115200/8N1 config:
/// let con = SerialConsole::com1();
///
/// // COM2 with custom config:
/// let cfg = SerialConfig { baud_rate: BaudRate::Baud9600, .. };
/// let con = SerialConsole::new(SerialPort::with_port(ComPort::Com2), cfg);
/// ```
pub struct SerialConsole {
    port: SerialPort,
    config: SerialConfig,
}

impl SerialConsole {
    /// Create a `SerialConsole` for **COM1** with 115200/8N1 defaults.
    ///
    /// Call [`init`](Self::init) before any I/O.
    pub fn com1() -> Self {
        Self {
            port: SerialPort::new(),
            config: SerialConfig::default_115200_8n1(),
        }
    }

    /// Create a `SerialConsole` for the given [`ComPort`] with 115200/8N1 defaults.
    pub fn with_port(com: ComPort) -> Self {
        Self {
            port: SerialPort::with_port(com),
            config: SerialConfig::default_115200_8n1(),
        }
    }

    /// Create a `SerialConsole` with a fully custom [`SerialConfig`].
    pub fn with_config(port: SerialPort, config: SerialConfig) -> Self {
        Self { port, config }
    }

    /// Initialise the underlying UART with the configured parameters.
    ///
    /// Must be called before any [`Console`] method.
    ///
    /// # Safety
    ///
    /// - Caller must be at CPL=0 (ring 0).
    /// - No other code may access the UART registers concurrently.
    pub unsafe fn init(&self) -> Result<(), SerialError> {
        // SAFETY: pre-conditions delegated to caller.
        self.port.init_with_config(&self.config)
    }

    /// Return the configured [`BaudRate`] for diagnostics.
    pub fn baud_rate(&self) -> BaudRate {
        self.config.baud_rate
    }
}

// ---------------------------------------------------------------------------
// Console impl
// ---------------------------------------------------------------------------

impl Console for SerialConsole {
    fn write_char(&mut self, c: char) {
        if c == '\n' {
            self.port.write_byte(b'\r');
        }
        // ASCII fast path; non-ASCII chars are replaced with '?'
        if c.is_ascii() {
            self.port.write_byte(c as u8);
        } else {
            self.port.write_byte(b'?');
        }
    }

    fn write_str_console(&mut self, s: &str) {
        // Delegate to SerialPort::write_str which handles \n→\r\n translation.
        self.port.write_str(s);
    }

    fn flush(&mut self) {
        // Polling transmit path — the UART's shift register empties
        // synchronously; there is nothing to flush.
    }

    fn read_char(&mut self) -> Option<char> {
        self.port.try_read_byte().map(|b| {
            // Return the raw byte as a char; non-ASCII serial input is unusual
            // but represented faithfully for higher layers to handle.
            if b.is_ascii() {
                b as char
            } else {
                char::REPLACEMENT_CHARACTER
            }
        })
    }
}

// ---------------------------------------------------------------------------
// fmt::Write impl — required supertrait of Console
// ---------------------------------------------------------------------------

impl fmt::Write for SerialConsole {
    /// Write a string slice to COM1, translating `\n` to `\r\n`.
    ///
    /// Never returns `Err` — the polling serial path is infallible.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.port.write_str(s);
        Ok(())
    }
}
