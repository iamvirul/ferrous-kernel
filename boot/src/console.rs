//! Console output for UEFI boot.
//!
//! This module provides a simple console abstraction for printing
//! messages during the boot process using UEFI console output.

use core::fmt::{self, Write};
use uefi::prelude::*;

/// Console wrapper for UEFI text output.
///
/// This provides a simple interface for printing boot messages
/// using the UEFI console output protocol.
pub struct Console<'a> {
    /// Reference to the UEFI system table.
    system_table: &'a SystemTable<Boot>,
}

impl<'a> Console<'a> {
    /// Creates a new console instance.
    pub fn new(system_table: &'a SystemTable<Boot>) -> Self {
        Self { system_table }
    }

    /// Clears the console screen.
    pub fn clear(&mut self) {
        if let Some(stdout) = self.system_table.stdout() {
            let _ = stdout.clear();
        }
    }

    /// Writes a string to the console.
    fn write_str_internal(&self, s: &str) {
        if let Some(stdout) = self.system_table.stdout() {
            // UEFI uses UCS-2, so we need to convert character by character
            for c in s.chars() {
                if c == '\n' {
                    // Handle newline by also printing carriage return
                    let _ = stdout.output_string(cstr16!("\r\n"));
                } else if c.is_ascii() {
                    // For ASCII characters, we can create a simple buffer
                    let mut buf = [0u16; 2];
                    buf[0] = c as u16;
                    // We need to null-terminate the string
                    // Create a static buffer for the character
                    // Safety: We're creating a valid UCS-2 string with null terminator
                    unsafe {
                        let cstr = uefi::CStr16::from_u16_with_nul_unchecked(&buf);
                        let _ = stdout.output_string(cstr);
                    }
                }
                // Non-ASCII characters are skipped for simplicity
            }
        }
    }
}

impl<'a> Write for Console<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str_internal(s);
        Ok(())
    }
}

/// Boot log level for console messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Debug messages (verbose).
    Debug,
    /// Informational messages.
    Info,
    /// Warning messages.
    Warning,
    /// Error messages.
    Error,
}

impl LogLevel {
    /// Returns the prefix string for this log level.
    pub fn prefix(&self) -> &'static str {
        match self {
            LogLevel::Debug => "[DEBUG]",
            LogLevel::Info => "[INFO]",
            LogLevel::Warning => "[WARN]",
            LogLevel::Error => "[ERROR]",
        }
    }
}

/// Logs a message at the specified level.
#[macro_export]
macro_rules! boot_log {
    ($console:expr, $level:expr, $($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($console, "{} ", $level.prefix());
            let _ = writeln!($console, $($arg)*);
        }
    };
}

/// Logs a debug message.
#[macro_export]
macro_rules! boot_debug {
    ($console:expr, $($arg:tt)*) => {
        $crate::boot_log!($console, $crate::console::LogLevel::Debug, $($arg)*)
    };
}

/// Logs an info message.
#[macro_export]
macro_rules! boot_info {
    ($console:expr, $($arg:tt)*) => {
        $crate::boot_log!($console, $crate::console::LogLevel::Info, $($arg)*)
    };
}

/// Logs a warning message.
#[macro_export]
macro_rules! boot_warn {
    ($console:expr, $($arg:tt)*) => {
        $crate::boot_log!($console, $crate::console::LogLevel::Warning, $($arg)*)
    };
}

/// Logs an error message.
#[macro_export]
macro_rules! boot_error {
    ($console:expr, $($arg:tt)*) => {
        $crate::boot_log!($console, $crate::console::LogLevel::Error, $($arg)*)
    };
}
