//! Kernel serial logger (Phase 1.4.1).
//!
//! Provides [`KernelLogger`]: the canonical `log::Log` implementation for the
//! standalone kernel binary introduced in Phase 2.  It writes level-prefixed
//! log records to COM1 via the [`SerialPort`][crate::drivers::serial::SerialPort]
//! driver.
//!
//! # Relationship to the boot logger
//!
//! In Phase 1 the kernel code runs inside the UEFI boot binary
//! (`boot/src/main.rs`).  The equivalent logger for that phase lives in
//! `boot/src/logger.rs` and uses the boot crate's inline serial helpers.
//! Both share the same format and level semantics; when the kernel becomes a
//! standalone ELF in Phase 2, this `KernelLogger` takes over.
//!
//! # Message format
//!
//! ```text
//! [ERROR] kernel::memory::heap: allocation failed for layout 4096/8
//! [WARN ] kernel::arch::x86_64::idt: spurious IRQ on vector 0x27
//! [INFO ] ferrous_kernel: kernel logger active (max_level=Debug)
//! [DEBUG] kernel::memory::frame_allocator: allocated frame 0x1000
//! [TRACE] kernel::memory::paging::mapper: walk PML4[0] = 0xde1d023
//! ```
//!
//! # Usage (Phase 2+)
//!
//! ```ignore
//! // In the kernel binary crate root (kernel/src/main.rs):
//! #[global_logger_init]
//! fn kernel_init() {
//!     // SAFETY: COM1 already initialised; single-threaded.
//!     unsafe { logger::init(log::LevelFilter::Debug); }
//!     log::info!("Kernel logger active");
//! }
//! ```
//!
//! # Compile-time level filtering
//!
//! Add `features = ["max_level_debug"]` (dev) or `features =
//! ["max_level_info"]` (release) to the `log` dependency in `Cargo.toml` to
//! eliminate filtered log calls at compile time with zero run-time cost.

use core::fmt;

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::drivers::serial::SerialPort;

// ---------------------------------------------------------------------------
// SerialWriter — fmt::Write adapter for the kernel SerialPort
// ---------------------------------------------------------------------------

/// Stateless `fmt::Write` adapter that emits bytes to COM1 via
/// [`SerialPort::write_str`].
struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // SAFETY: COM1 init is a pre-condition enforced by `KernelLogger::init`.
        // `SerialPort::new()` is zero-cost (just stores the base port address);
        // it is safe to construct multiple instances — they all address the same
        // hardware register set.
        SerialPort::new().write_str(s);
        Ok(()) // write_str never fails on a polling serial path
    }
}

// ---------------------------------------------------------------------------
// KernelLogger — log::Log implementation
// ---------------------------------------------------------------------------

/// Level-prefixed serial logger for the standalone Phase 2 kernel binary.
///
/// Writes every enabled log record to COM1 as a single line:
///
/// `[LEVEL] <target>: <message>\r\n`
///
/// The level tag is always 7 characters (`[XXXXX]`) so log lines align cleanly
/// on a serial terminal regardless of which level is active.
///
/// # Thread safety
///
/// `KernelLogger` is `Sync` because `SerialPort` is stateless (only contains
/// `base: u16`).  In Phase 1–2 logging is single-core and interrupt-free;
/// Phase 3+ must add a spinlock around the transmit path if interrupts are
/// enabled and interrupt handlers may log.
pub struct KernelLogger;

impl Log for KernelLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // 7-character level tag: constant width keeps columns aligned.
        let tag = match record.level() {
            Level::Error => "[ERROR]",
            Level::Warn => "[WARN ]",
            Level::Info => "[INFO ]",
            Level::Debug => "[DEBUG]",
            Level::Trace => "[TRACE]",
        };

        let serial = SerialPort::new();

        // Emit: `[LEVEL] <target>: <message>\r\n`
        serial.write_str(tag);
        serial.write_str(" ");
        serial.write_str(record.target());
        serial.write_str(": ");
        // `fmt::write` calls `SerialWriter::write_str` for each formatted
        // segment.  Infallible on our sink; `let _ =` suppresses the lint.
        let _ = fmt::write(&mut SerialWriter, *record.args());
        serial.write_str("\r\n");
    }

    #[inline]
    fn flush(&self) {
        // Polling transmit path — no internal buffer; nothing to flush.
    }
}

// ---------------------------------------------------------------------------
// Global instance + init
// ---------------------------------------------------------------------------

/// The static kernel logger registered as the global `log` logger by [`init`].
pub static KERNEL_LOGGER: KernelLogger = KernelLogger;

/// Initialise the kernel serial logger.
///
/// After this call every `log::error!`, `log::warn!`, `log::info!`,
/// `log::debug!`, and `log::trace!` invocation writes a formatted line to
/// COM1.
///
/// # Arguments
///
/// * `max_level` — runtime log level ceiling.  Records above this level are
///   discarded without formatting.  Recommended values:
///   - [`LevelFilter::Debug`] for kernel development builds.
///   - [`LevelFilter::Info`]  for production / CI builds.
///
/// # Safety
///
/// - COM1 must have been initialised (via [`SerialPort::init`]) before this call.
/// - Must be called from a **single-threaded** context (always true in Phase 1–2).
/// - Uses `log::set_logger_racy` — the unsafe, non-atomic variant.  The safe
///   `set_logger` is reserved for environments where multiple threads may race
///   to set the logger simultaneously, which cannot happen during single-core
///   early boot.
pub unsafe fn init(max_level: LevelFilter) {
    // SAFETY: see doc comment above.
    let _ = log::set_logger_racy(&KERNEL_LOGGER);
    log::set_max_level(max_level);
}
