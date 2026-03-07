//! Ferrous Kernel — main entry
//!
//! This file is the crate root for the kernel binary. It declares the
//! architecture-specific entry module, the device driver collection, and
//! provides the global panic handler.
//!
//! The actual entry point (`kernel_entry`) is defined in the bootloader for
//! Phase 1. When ELF loading is implemented the entry point will move here.

#![no_std]
#![no_main]

pub mod arch;
pub mod drivers;

use drivers::serial::SerialPort;

/// Kernel panic handler.
///
/// Writes "KERNEL PANIC" to COM1 and halts. The `SerialPort` driver is used
/// here without calling `init()` first — we rely on the UART having been
/// initialised during `kernel_main`. If a panic occurs before that point the
/// output may be garbled, but the alternative (silently looping) is worse.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let serial = SerialPort::new();
    serial.write_str("\r\nKERNEL PANIC\r\n");
    loop {
        // SAFETY: `hlt` safely suspends the CPU until the next interrupt.
        unsafe { core::arch::asm!("hlt") };
    }
}
