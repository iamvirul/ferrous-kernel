//! Console output for UEFI boot.

use core::fmt::{self, Write};

/// Console wrapper for UEFI text output.
pub struct Console;

impl Console {
    /// Creates a new console instance.
    pub fn new() -> Self {
        Self
    }

    /// Clears the console screen.
    pub fn clear(&mut self) {
        uefi::system::with_stdout(|stdout| {
            let _ = stdout.clear();
        });
    }
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        uefi::system::with_stdout(|stdout| {
            for c in s.chars() {
                if c == '\n' {
                    let _ = stdout.output_string(uefi::cstr16!("\r\n"));
                } else if c.is_ascii() {
                    let mut buf = [0u16; 2];
                    buf[0] = c as u16;
                    // Safety: Creating a valid UCS-2 string with null terminator.
                    // Single ASCII char followed by null is always valid UCS-2.
                    unsafe {
                        let cstr = uefi::CStr16::from_u16_with_nul_unchecked(&buf);
                        let _ = stdout.output_string(cstr);
                    }
                }
            }
        });
        Ok(())
    }
}
