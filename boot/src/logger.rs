//! Post-UEFI serial logger (Phase 1.4.1).
//!
//! Implements [`log::Log`] over COM1 for the kernel execution phase that
//! begins after `exit_boot_services()`.  The UEFI logger installed by
//! `uefi::helpers::init()` becomes invalid at that point; [`init`] replaces
//! it with this serial-backed implementation.
//!
//! # Message format
//!
//! Every log record is emitted as a single line:
//!
//! ```text
//! [ERROR] kernel::memory::heap: allocation failed for layout 4096/8
//! [WARN ] kernel::arch::x86_64::idt: spurious IRQ on vector 0x27
//! [INFO ] ferrous_boot: kernel logger active (max_level=Debug)
//! [DEBUG] kernel::memory::frame_allocator: allocated frame 0x1000
//! [TRACE] kernel::memory::paging::mapper: walk PML4[0] = 0xde1d023
//! ```
//!
//! The level tag is always 7 characters wide (`[XXXXX]`) so log lines column-
//! align cleanly on a serial terminal.
//!
//! # Overhead
//!
//! Logging is synchronous and blocking — each byte polls COM1's Transmit
//! Holding Register Empty bit.  This is appropriate for Phase 1 (single-core,
//! interrupts disabled).  Phase 3+ may introduce a lock-free ring buffer.
//!
//! # Compile-time level filtering
//!
//! Add `features = ["max_level_debug"]` (dev) or `features =
//! ["max_level_info"]` (release) to the `log` dependency to eliminate
//! filtered log calls at compile time with zero run-time cost.

use core::fmt;

use log::{Level, LevelFilter, Log, Metadata, Record};

// ---------------------------------------------------------------------------
// SerialWriter — fmt::Write adapter for COM1
// ---------------------------------------------------------------------------

/// Stateless `fmt::Write` adapter that writes bytes to COM1 via
/// [`crate::serial_write_str`].
///
/// A `\n` in a format string is forwarded as-is; the logger appends an
/// explicit `\r\n` after each record so lines terminate correctly on a serial
/// terminal even if the message itself contains newlines.
struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        crate::serial_write_str(s);
        Ok(()) // serial_write_str never fails
    }
}

// ---------------------------------------------------------------------------
// SerialLogger — log::Log implementation
// ---------------------------------------------------------------------------

/// Level-prefixed serial logger for the post-UEFI kernel phase.
///
/// # Format
///
/// `[LEVEL] <target>: <message>\r\n`
///
/// where `LEVEL` is one of `ERROR`, `WARN `, `INFO `, `DEBUG`, `TRACE`
/// (space-padded to 5 characters so the colon always falls in the same column).
pub struct SerialLogger;

impl Log for SerialLogger {
    #[inline]
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // 7-character level tag: `[XXXXX]` — constant width keeps columns aligned.
        let tag = match record.level() {
            Level::Error => "[ERROR]",
            Level::Warn => "[WARN ]",
            Level::Info => "[INFO ]",
            Level::Debug => "[DEBUG]",
            Level::Trace => "[TRACE]",
        };

        // Emit: `[LEVEL] <target>: <message>\r\n`
        //
        // `fmt::write` calls `SerialWriter::write_str` for each formatted
        // segment.  It never returns `Err` because our write_str is infallible;
        // the `let _ =` suppresses the unused-result lint.
        crate::serial_write_str(tag);
        crate::serial_write_str(" ");
        crate::serial_write_str(record.target());
        crate::serial_write_str(": ");
        let _ = fmt::write(&mut SerialWriter, *record.args());
        crate::serial_write_str("\r\n");
    }

    #[inline]
    fn flush(&self) {
        // COM1 uses a polling transmit path — no internal buffer; nothing to flush.
    }
}

// ---------------------------------------------------------------------------
// Global instance + init
// ---------------------------------------------------------------------------

/// The static serial logger registered as the global `log` logger by [`init`].
pub static SERIAL_LOGGER: SerialLogger = SerialLogger;

/// Install [`SERIAL_LOGGER`] as the global `log` logger.
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
/// - COM1 must have been initialised by `serial_init()` before this call.
/// - Must be called from a **single-threaded** context (always true in Phase 1
///   — single-core, interrupts disabled).
/// - Uses `log::set_logger_racy` internally, which is the unsafe, non-atomic
///   variant of `log::set_logger`.  The safe variant would return
///   `Err(SetLoggerError)` here because the UEFI logger was already installed
///   by `uefi::helpers::init()`.  The UEFI logger is no longer valid after
///   `exit_boot_services()`, so replacing it is correct.
/// - Calling `init` a second time is harmless: `set_logger_racy` silently
///   ignores duplicate registrations.
pub unsafe fn init(max_level: LevelFilter) {
    // SAFETY: see doc comment above.
    let _ = log::set_logger_racy(&SERIAL_LOGGER);
    log::set_max_level(max_level);
}
