//! Ferrous Kernel — main entry
//!
//! This file is the crate root for the kernel binary. It declares the
//! architecture-specific entry module and provides the global panic handler.
//!
//! The actual entry point (`kernel_entry`) is defined in the bootloader for
//! Phase 1. When ELF loading is implemented the entry point will move here.

#![no_std]
#![no_main]

pub mod arch;

/// Kernel panic handler.
///
/// In Phase 1 we have no heap and no formatted output infrastructure, so we
/// write a static message to COM1 and halt.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: Direct PIO to COM1. Safe to call from a panic handler because
    // we own the hardware at this point and the only goal is to halt visibly.
    unsafe {
        for byte in b"KERNEL PANIC\r\n" {
            loop {
                let lsr: u8;
                core::arch::asm!(
                    "in al, dx",
                    in("dx") 0x3F8u16 + 5,
                    out("al") lsr,
                    options(nomem, nostack),
                );
                if lsr & 0x20 != 0 {
                    break;
                }
            }
            core::arch::asm!(
                "out dx, al",
                in("dx") 0x3F8u16,
                in("al") *byte,
                options(nomem, nostack),
            );
        }
    }
    loop {
        // SAFETY: `hlt` safely suspends the CPU until the next interrupt.
        unsafe { core::arch::asm!("hlt") };
    }
}
