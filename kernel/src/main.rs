//! Ferrous Kernel — binary entry
//!
//! Declares architecture-specific and driver modules, and provides the global
//! panic handler.  The `memory` and `task` subsystems live in `lib.rs` and are
//! re-used by the bootloader for early smoke tests (Steps 14–15).
//!
//! The actual entry point (`kernel_entry`) is defined in the bootloader for
//! Phase 1. When ELF loading is implemented the entry point will move here.

#![no_std]
#![no_main]

pub mod arch;
pub mod drivers;
pub mod logger;

use drivers::serial::SerialPort;

/// Kernel panic handler (Phase 1.4.2).
///
/// Writes a structured panic report to COM1 then halts via `hlt`. The report
/// contains:
///
/// 1. A banner (`--- KERNEL PANIC ---`) always emitted via direct serial
///    writes so it is visible even if the logger is not yet initialised.
/// 2. Source location (`file:line:column`) from [`core::panic::PanicInfo`].
/// 3. The panic message at ERROR level via the `log` framework.
/// 4. A frame-pointer stack trace (raw return addresses, resolvable with
///    `addr2line -e <kernel-elf> <addr>`).
///
/// # Resolving stack addresses
///
/// ```bash
/// addr2line -e target/x86_64-unknown-uefi/debug/ferrous-boot.efi <addr>
/// ```
///
/// # Note on `SerialPort::new()`
///
/// `SerialPort` is used here without calling `init()` first — we rely on the
/// UART having been configured by the bootloader before kernel handoff. If a
/// panic fires before that point the output may be garbled, but that is far
/// better than silently looping forever.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let serial = SerialPort::new();

    // --- Banner ---
    serial.write_str("\r\n");
    serial.write_str("--- KERNEL PANIC ---\r\n");

    // --- Source location ---
    if let Some(loc) = info.location() {
        serial.write_str("  at ");
        serial.write_str(loc.file());
        serial.write_str(":");
        // Write line number as decimal digits without heap allocation.
        write_u32_serial(&serial, loc.line());
        serial.write_str("\r\n");
    }

    // --- Panic message via log framework ---
    //
    // If the logger is not yet initialised this is a no-op; the banner still
    // provides a minimal diagnosis.
    log::error!("PANIC: {}", info);

    // --- Frame-pointer stack trace ---
    serial.write_str("\r\n");
    // SAFETY:
    // - `force-frame-pointers = true` is set in workspace Cargo.toml.
    // - The kernel stack is mapped for the entire execution lifetime.
    // - This is the panic path: single-threaded, non-reentrant.
    unsafe { walk_stack_kernel(&serial) };

    serial.write_str("--- END PANIC ---\r\n");

    loop {
        // SAFETY: `hlt` safely suspends the CPU until the next interrupt.
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Write a `u32` in decimal to the serial port without any heap allocation.
fn write_u32_serial(serial: &SerialPort, mut n: u32) {
    if n == 0 {
        serial.write_str("0");
        return;
    }
    // Build digits in reverse.
    let mut buf = [0u8; 10]; // max 10 decimal digits for u32
    let mut len = 0usize;
    while n > 0 {
        buf[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    // Print in forward order.
    for i in (0..len).rev() {
        // SAFETY: buf[i] is always in '0'..='9' — valid ASCII.
        let ch = &buf[i..=i];
        if let Ok(s) = core::str::from_utf8(ch) {
            serial.write_str(s);
        }
    }
}

/// Walk the RBP frame-pointer chain and write return addresses to `serial`.
///
/// See `boot/src/unwind.rs` for a full explanation of the walk mechanics.
///
/// # Safety
///
/// - `force-frame-pointers = true` must hold for all crates in the build.
/// - The kernel stack must be fully mapped (always true after Phase 1.3.3).
/// - Must only be called from a single-threaded, non-reentrant context.
unsafe fn walk_stack_kernel(serial: &SerialPort) {
    const MAX_FRAMES: usize = 16;

    serial.write_str("[TRACE] Stack trace (RBP chain):\r\n");

    // Capture current RBP.
    let mut rbp: u64;
    // SAFETY: reading RBP is always valid at ring-0.
    core::arch::asm!(
        "mov {out}, rbp",
        out = out(reg) rbp,
        options(nomem, nostack, preserves_flags),
    );

    let mut frame = 0usize;
    while frame < MAX_FRAMES {
        if rbp == 0 {
            break;
        }
        if rbp % 8 != 0 {
            serial.write_str("  (corrupt frame pointer — walk aborted)\r\n");
            break;
        }

        // SAFETY: rbp is non-null, 8-byte aligned; [rbp+8] is the return addr.
        let ret_addr: u64 = unsafe { *((rbp as *const u64).add(1)) };
        if ret_addr == 0 {
            break;
        }

        serial.write_str("  #");
        // Write frame index as decimal.
        write_u32_serial(serial, frame as u32);
        serial.write_str("  0x");
        write_hex_u64_serial(serial, ret_addr);
        serial.write_str("\r\n");

        // SAFETY: [rbp] holds saved RBP from the previous frame.
        rbp = unsafe { *(rbp as *const u64) };
        frame += 1;
    }

    if frame == 0 {
        serial.write_str("  (no frames — frame pointers unavailable at this depth)\r\n");
    } else {
        serial.write_str("  (end of trace)\r\n");
    }
}

/// Write a `u64` as a zero-padded 16-digit hex string to `serial`.
fn write_hex_u64_serial(serial: &SerialPort, mut n: u64) {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    for i in (0..16).rev() {
        buf[i] = HEX[(n & 0xF) as usize];
        n >>= 4;
    }
    // SAFETY: all bytes are valid ASCII hex digits.
    if let Ok(s) = core::str::from_utf8(&buf) {
        serial.write_str(s);
    }
}
