//! Ferrous Kernel UEFI Bootloader
//!
//! This is the UEFI entry point for the Ferrous kernel. It initializes
//! UEFI boot services, retrieves system information, and hands off to
//! the kernel entry point.
//!
//! # Handoff sequence
//!
//! 1. Collect memory map, ACPI RSDP, and framebuffer info via UEFI.
//! 2. Build a `KernelBootInfo` in a static buffer (no heap after this).
//! 3. Call `exit_boot_services()` — the point of no return.
//! 4. Disable interrupts, switch to the bootstrap stack.
//! 5. Jump to `kernel_entry` with `&KernelBootInfo` as the first argument.

#![no_std]
#![no_main]

extern crate alloc;

mod boot_info;
mod console;
mod memory;

use core::fmt::Write;
use uefi::boot::MemoryType;
use uefi::prelude::*;

use crate::boot_info::BootInfo;
use crate::console::Console;
use crate::memory::MemoryMap;
use ferrous_boot_info::KernelBootInfo;

// ---------------------------------------------------------------------------
// Bootstrap stack
// ---------------------------------------------------------------------------

/// Size of the kernel bootstrap stack in bytes (16 KiB).
///
/// This stack is used only during the UEFI handoff sequence
/// (exit_boot_services → kernel_entry). It is intentionally small —
/// `kernel_main` immediately switches to the larger KERNEL_STACK.
const BOOTSTRAP_STACK_SIZE: usize = 16 * 1024;

/// Bootstrap stack used after `exit_boot_services()`.
///
/// Must be 16-byte aligned for the x86-64 ABI. The stack grows downward,
/// so `kernel_entry` receives a pointer to the *top* (highest address).
#[repr(C, align(16))]
struct BootstrapStack([u8; BOOTSTRAP_STACK_SIZE]);

/// SAFETY: this static is only written once, before the first Rust code
/// on the bootstrap stack runs. After that it is read-only (the stack grows
/// into it, but that is managed by the CPU, not by Rust references).
static mut BOOTSTRAP_STACK: BootstrapStack = BootstrapStack([0u8; BOOTSTRAP_STACK_SIZE]);

// ---------------------------------------------------------------------------
// Kernel primary stack
// ---------------------------------------------------------------------------

/// Total size of the kernel primary execution stack (64 KiB).
///
/// Layout:
///   [bottom .. bottom+4KiB]  soft guard region — zeroed; future: non-present page
///   [bottom+4KiB .. top]     60 KiB usable stack depth
///
/// When the kernel becomes a separate ELF binary this constant and the static
/// below move to `kernel/src/arch/x86_64/stack.rs`, which also exports the
/// `KernelStack<N>` type for use with a linker script.
const KERNEL_STACK_SIZE: usize = 64 * 1024;

/// Soft guard region at the bottom of the kernel stack (one 4 KiB page).
///
/// Not enforced until page-table management is implemented (Task 1.3.3).
const KERNEL_STACK_GUARD_SIZE: usize = 4 * 1024;

/// The kernel's primary execution stack.
///
/// `kernel_main` switches RSP to the top of this buffer immediately on entry,
/// leaving the bootstrap stack behind. The stack is 16-byte aligned and large
/// enough for typical kernel call chains in Phase 1 (no deep recursion, no
/// interrupt frames yet).
///
/// SAFETY: switched to exactly once from `kernel_main` before any other
/// stack writes occur. Single-core, interrupts-disabled environment.
#[repr(C, align(16))]
struct KernelStack([u8; KERNEL_STACK_SIZE]);

static mut KERNEL_STACK: KernelStack = KernelStack([0u8; KERNEL_STACK_SIZE]);

// ---------------------------------------------------------------------------
// KernelBootInfo static
// ---------------------------------------------------------------------------

/// The boot information buffer passed to the kernel.
///
/// Populated before `exit_boot_services()`, its address is passed to
/// `kernel_entry`. Must be `static` so it outlives the bootloader stack.
///
/// SAFETY: written exactly once in `efi_main` before the handoff, then
/// treated as read-only by both the bootloader (during the jump) and
/// the kernel.
static mut KERNEL_BOOT_INFO: KernelBootInfo = KernelBootInfo::new();

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("BOOT PANIC: {}", info);
    loop {
        // SAFETY: `hlt` halts the CPU until the next interrupt. This is
        // safe to execute and prevents a busy-spin in a panic situation.
        unsafe { core::arch::asm!("hlt") };
    }
}

// ---------------------------------------------------------------------------
// UEFI entry point
// ---------------------------------------------------------------------------

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI helpers");

    let mut console = Console::new();
    console.clear();

    writeln!(console, "").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "  Ferrous Kernel UEFI Bootloader v0.1").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "").unwrap();

    log::info!("UEFI boot services initialized");
    writeln!(console, "[OK] UEFI boot services initialized").unwrap();

    let firmware_vendor = uefi::system::firmware_vendor();
    let firmware_revision = uefi::system::firmware_revision();
    writeln!(
        console,
        "[INFO] Firmware: {} (rev {})",
        firmware_vendor, firmware_revision
    )
    .unwrap();

    let uefi_revision = uefi::system::uefi_revision();
    writeln!(
        console,
        "[INFO] UEFI Revision: {}.{}",
        uefi_revision.major(),
        uefi_revision.minor()
    )
    .unwrap();

    // --- Collect memory map ---
    writeln!(console, "[...] Retrieving memory map").unwrap();
    let memory_map = match retrieve_memory_map(&mut console) {
        Ok(map) => {
            writeln!(console, "[OK] Memory map retrieved").unwrap();
            map
        }
        Err(e) => {
            writeln!(console, "[FAIL] Failed to retrieve memory map: {:?}", e).unwrap();
            return Status::ABORTED;
        }
    };
    print_memory_summary(&memory_map, &mut console);

    // --- Collect ACPI RSDP ---
    writeln!(console, "[...] Looking for ACPI tables").unwrap();
    let acpi_rsdp = find_acpi_tables();
    match acpi_rsdp {
        Some(addr) => writeln!(console, "[OK] ACPI RSDP found at: {:#x}", addr).unwrap(),
        None => writeln!(console, "[WARN] ACPI tables not found").unwrap(),
    }

    // --- Collect framebuffer info ---
    writeln!(console, "[...] Looking for GOP framebuffer").unwrap();
    let framebuffer = get_framebuffer_info();
    match &framebuffer {
        Some(fb) => writeln!(
            console,
            "[OK] Framebuffer: {}x{} @ {:#x}",
            fb.width, fb.height, fb.base_address
        )
        .unwrap(),
        None => writeln!(console, "[WARN] GOP framebuffer not available").unwrap(),
    }

    // --- Build BootInfo and convert to KernelBootInfo ---
    let mut boot_info = BootInfo::new(memory_map);
    if let Some(addr) = acpi_rsdp {
        boot_info.set_acpi_rsdp_address(addr);
    }
    if let Some(fb) = framebuffer {
        let kfb = boot_info::FramebufferInfo::new(
            fb.base_address,
            fb.width,
            fb.height,
            fb.stride,
            fb.pixel_format,
        );
        boot_info.set_framebuffer(kfb);
    }

    let kernel_boot_info = boot_info.to_kernel_boot_info();

    writeln!(console, "").unwrap();
    writeln!(
        console,
        "[INFO] Total memory:  {} MB",
        boot_info.total_memory_mb()
    )
    .unwrap();
    writeln!(
        console,
        "[INFO] Usable memory: {} MB",
        boot_info.usable_memory_mb()
    )
    .unwrap();

    writeln!(console, "").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "  Preparing for kernel handoff...").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "").unwrap();

    // --- Write KernelBootInfo to the static buffer ---
    //
    // SAFETY: We are the only writer. This runs before the handoff, on the
    // single-threaded UEFI executor. KERNEL_BOOT_INFO is never aliased here.
    unsafe {
        core::ptr::write(core::ptr::addr_of_mut!(KERNEL_BOOT_INFO), kernel_boot_info);
    }

    writeln!(
        console,
        "[OK] KernelBootInfo populated (magic={:#x})",
        ferrous_boot_info::BOOT_INFO_MAGIC
    )
    .unwrap();

    // --- Exit UEFI boot services — point of no return ---
    //
    // After this call:
    // - No UEFI functions may be called.
    // - The UEFI console is gone; all output must go via serial.
    // - The UEFI heap is gone; no heap allocations are permitted.
    //
    // `exit_boot_services` internally retries if the memory map key is
    // stale, so it is safe to call it directly here.
    //
    // SAFETY: We have collected all required UEFI data above. The
    // KernelBootInfo static is fully populated. There are no outstanding
    // UEFI resources that require cleanup.
    let _final_map = unsafe { uefi::boot::exit_boot_services(MemoryType::LOADER_DATA) };

    // Forget the map — dropping it would attempt a UEFI dealloc, which is
    // no longer valid. The memory persists as LOADER_DATA.
    core::mem::forget(_final_map);

    // --- Switch stack and jump to kernel_entry ---
    //
    // From this point the UEFI stack is invalid (reclaimed). We switch to
    // our statically allocated bootstrap stack before calling any Rust code.
    //
    // SAFETY:
    // - BOOTSTRAP_STACK is a valid 16 KiB, 16-byte-aligned static buffer.
    // - stack_top points one byte past the end, which is the initial RSP
    //   value (x86-64 stack grows downward).
    // - Interrupts are disabled with `cli` to prevent an interrupt handler
    //   from using the now-invalid UEFI stack during the transition.
    // - `kernel_entry` is `-> !` and never returns, so the `call`
    //   instruction's implicit return address on the stack is never used.
    // - RDI is set to the address of KERNEL_BOOT_INFO per the SysV AMD64
    //   calling convention (first argument).
    unsafe {
        let stack_top =
            (core::ptr::addr_of!(BOOTSTRAP_STACK) as usize + BOOTSTRAP_STACK_SIZE) as u64;
        let boot_info_ptr = core::ptr::addr_of!(KERNEL_BOOT_INFO) as u64;

        core::arch::asm!(
            "cli",
            "mov rsp, {stack}",
            "xor rbp, rbp",
            "mov rdi, {info}",
            "call {entry}",
            stack = in(reg) stack_top,
            info  = in(reg) boot_info_ptr,
            entry = sym kernel_entry,
            options(noreturn),
        );
    }
}

// ---------------------------------------------------------------------------
// Kernel entry point
// ---------------------------------------------------------------------------

/// Kernel entry point — called after UEFI boot services have exited.
///
/// This function runs on the bootstrap stack with interrupts disabled.
/// It validates the `KernelBootInfo`, zeroes BSS, and calls `kernel_main`.
///
/// # Safety
///
/// Must only be called from the handoff asm block in `efi_main`:
/// - RSP must point to a valid stack (the bootstrap stack).
/// - RDI must contain the address of a fully populated `KernelBootInfo`.
/// - Interrupts must be disabled (`cli` must have been executed).
/// - UEFI boot services must have already exited.
#[no_mangle]
extern "C" fn kernel_entry(boot_info: *const KernelBootInfo) -> ! {
    // Validate the boot info pointer before touching anything else.
    //
    // SAFETY: `boot_info` is the address of KERNEL_BOOT_INFO, a valid
    // static. We check it is non-null and has the correct magic before
    // treating it as a reference.
    // SAFETY: `boot_info` is the address of KERNEL_BOOT_INFO, a valid static
    // populated before exit_boot_services(). We check non-null and magic
    // before constructing a reference.
    if boot_info.is_null() {
        serial_write_str("FATAL: kernel_entry received null BootInfo pointer\r\n");
        halt();
    }

    let info = unsafe { &*boot_info };
    if !info.is_valid() {
        serial_write_str("FATAL: KernelBootInfo magic/version mismatch\r\n");
        halt();
    }

    // Note: BSS zeroing is not needed here because this is a UEFI PE/COFF
    // binary — the UEFI firmware zero-initialises BSS before calling efi_main.
    // When the kernel becomes a separate flat binary, zero_bss() will be
    // performed at the start of kernel_entry in kernel/src/arch/x86_64/entry.rs.

    kernel_main(info);
}

/// First Rust function executing in the kernel context.
///
/// On entry RSP points to the bootstrap stack (set up by the bootloader).
/// The very first action is to switch to the kernel's own primary stack
/// (`KERNEL_STACK`, 64 KiB) so we have adequate depth for kernel execution.
///
/// At this point:
/// - Boot services have exited.
/// - We are on the bootstrap stack with interrupts disabled.
/// - `boot_info` is a reference into the `KERNEL_BOOT_INFO` static — valid
///   for the lifetime of the kernel.
fn kernel_main(boot_info: &KernelBootInfo) -> ! {
    // -----------------------------------------------------------------------
    // Step 1: Switch to the kernel primary stack.
    //
    // We leave the bootstrap stack behind. From this point forward RSP
    // points into KERNEL_STACK.
    //
    // Implementation note — register spilling:
    //
    // After `mov rsp`, the compiler's RSP-relative loads for any locals
    // computed before the switch would read from the zeroed kernel stack
    // rather than the bootstrap stack, producing garbage values. We avoid
    // this by using an `inlateout` constraint to keep `boot_info` in a
    // physical register throughout the switch; the register is not modified
    // by the asm, so `info_out == info_in` after. Stack bounds are computed
    // fresh from the static address AFTER the switch — accessed by absolute
    // address, never RSP-relative.
    //
    // SAFETY:
    // - KERNEL_STACK is a valid 64 KiB, 16-byte-aligned static buffer.
    // - stack_top is one past the end of the array — the correct initial RSP
    //   for a downward-growing x86-64 stack.
    // - boot_info points to KERNEL_BOOT_INFO, a valid static that outlives
    //   the kernel. Reconstructing the reference after the switch is safe.
    // - kernel_main is `-> !`; the return address on the bootstrap stack is
    //   never consumed.
    // - Interrupts are disabled; no context switch can race this.
    // Switch RSP to the kernel stack. After this instruction the bootstrap
    // stack is abandoned. We must not read any local variables that the
    // compiler might have spilled to the old stack after this point.
    //
    // Solution: use only static-address loads after the switch.
    // - KERNEL_STACK and KERNEL_BOOT_INFO are statics; their addresses are
    //   embedded as absolute values in the code, never RSP-relative.
    // - We ignore the `boot_info` parameter and re-derive it from
    //   KERNEL_BOOT_INFO directly after the switch.
    unsafe {
        let stack_top = (core::ptr::addr_of!(KERNEL_STACK) as usize + KERNEL_STACK_SIZE) as u64;
        core::arch::asm!(
            "mov rsp, {top}",
            top = in(reg) stack_top,
            options(nomem, nostack, preserves_flags),
        );
    }

    // Re-derive boot_info from the static — absolute address, never spilled.
    // SAFETY: KERNEL_BOOT_INFO is fully populated before exit_boot_services()
    // and is immutable from this point forward.
    let boot_info = unsafe { &*core::ptr::addr_of!(KERNEL_BOOT_INFO) };

    // Stack bounds computed from static address, not RSP-relative.
    let stack_bottom = unsafe { core::ptr::addr_of!(KERNEL_STACK) as usize };
    let stack_top = stack_bottom + KERNEL_STACK_SIZE;

    // -----------------------------------------------------------------------
    // Step 2: UART init — kernel now owns COM1 configuration.
    serial_init();

    serial_write_str("\r\n");
    serial_write_str("=== Ferrous Kernel ===\r\n");
    serial_write_str("[OK] kernel_entry: BootInfo validated\r\n");
    serial_write_str("[OK] Kernel stack active\r\n");

    // Print stack bounds so we can verify the switch worked.
    serial_write_str("[INFO] Kernel stack: 0x");
    serial_write_usize_hex(stack_bottom);
    serial_write_str(" - 0x");
    serial_write_usize_hex(stack_top);
    serial_write_str(" (");
    serial_write_usize(KERNEL_STACK_SIZE / 1024);
    serial_write_str(" KiB, guard=");
    serial_write_usize(KERNEL_STACK_GUARD_SIZE / 1024);
    serial_write_str(" KiB)\r\n");

    // -----------------------------------------------------------------------
    // Step 3: Load GDT — set up kernel code/data segments.
    //
    // The UEFI firmware may have installed its own GDT, which is no longer
    // mapped or valid after exit_boot_services().  We install a minimal GDT
    // with exactly the segments needed for kernel operation.
    //
    // SAFETY:
    // - We are at CPL=0 (ring 0) throughout the boot sequence.
    // - Interrupts have been disabled since the `cli` in efi_main.
    // - GDT is a valid static in permanently mapped memory.
    unsafe { gdt_init() };

    serial_write_str("[OK] GDT loaded (null / kernel-code 0x08 / kernel-data 0x10)\r\n");

    // -----------------------------------------------------------------------
    serial_write_str("[OK] Kernel entered successfully!\r\n");
    serial_write_str("Hello from Ferrous!\r\n");
    serial_write_str("\r\n");

    // Report memory map summary via serial.
    serial_write_str("Memory map entries: ");
    serial_write_usize(boot_info.memory_map.count);
    serial_write_str("\r\n");

    if boot_info.acpi_rsdp != 0 {
        serial_write_str("[INFO] ACPI RSDP present\r\n");
    }

    if boot_info.has_framebuffer {
        serial_write_str("[INFO] Framebuffer present\r\n");
    }

    serial_write_str("\r\nKernel halting. Phase 1.2.3 (IDT) not yet implemented.\r\n");

    halt()
}

// ---------------------------------------------------------------------------
// Minimal serial output (COM1, 0x3F8)
//
// UEFI console is gone after exit_boot_services(). We write directly to
// the 16550-compatible UART at I/O port 0x3F8 (COM1, 115200 8N1).
//
// The full, reusable serial driver lives in kernel/src/drivers/serial.rs.
// These boot-side helpers are a lightweight duplicate kept here so the
// bootloader has zero dependencies on the kernel crate.
// ---------------------------------------------------------------------------

/// Initialise the UART to 115200 baud, 8N1.
///
/// Mirrors `SerialPort::init()` in `kernel/src/drivers/serial.rs`.
/// Called once at the top of `kernel_main` so the kernel owns the UART
/// configuration from this point forward.
/// Write `value` to I/O port `port`.
///
/// # Safety
///
/// Caller must be at CPL=0. `port` must be a valid I/O address for the
/// device being configured.
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags),
    );
}

fn serial_init() {
    // SAFETY: we are at CPL=0 and no other code touches COM1 at this stage.
    unsafe {
        outb(0x3F8 + 1, 0x00); // disable interrupts
        outb(0x3F8 + 3, 0x80); // enable DLAB
        outb(0x3F8, 0x01); // divisor low byte (115200 baud)
        outb(0x3F8 + 1, 0x00); // divisor high byte
        outb(0x3F8 + 3, 0x03); // 8N1, clear DLAB
        outb(0x3F8 + 2, 0xC7); // enable + flush FIFOs
        outb(0x3F8 + 4, 0x0B); // DTR + RTS + AUX2
    }
}

/// Write a byte to COM1 (I/O port 0x3F8), polling until the THR is empty.
///
/// SAFETY: Direct PIO to a known-safe I/O port. On x86 this requires CPL=0
/// (we are in ring 0 after boot services exit).
unsafe fn serial_write_byte(byte: u8) {
    // Poll Line Status Register (0x3F8 + 5) until bit 5 (THRE) is set.
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
        in("al") byte,
        options(nomem, nostack),
    );
}

fn serial_write_str(s: &str) {
    for byte in s.bytes() {
        // SAFETY: see `serial_write_byte`.
        unsafe { serial_write_byte(byte) };
    }
}

fn serial_write_usize(mut n: usize) {
    if n == 0 {
        serial_write_str("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in (0..i).rev() {
        // SAFETY: see `serial_write_byte`.
        unsafe { serial_write_byte(buf[j]) };
    }
}

fn serial_write_usize_hex(n: usize) {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    let mut i = 0;
    let mut val = n;
    if val == 0 {
        serial_write_str("0");
        return;
    }
    while val > 0 {
        buf[i] = HEX[val & 0xf];
        val >>= 4;
        i += 1;
    }
    for j in (0..i).rev() {
        // SAFETY: see `serial_write_byte`.
        unsafe { serial_write_byte(buf[j]) };
    }
}

// ---------------------------------------------------------------------------
// GDT (Global Descriptor Table)
//
// Even in 64-bit long mode, the GDT must be present and loaded.  The CPU
// uses the code-segment descriptor to confirm 64-bit execution mode (L=1)
// and the data-segment selector to satisfy segment-register requirements.
//
// Minimal layout for Phase 1:
//   Index 0 — 0x0000 : Null descriptor (architecturally required)
//   Index 1 — 0x0008 : Kernel code segment (64-bit, ring 0)
//   Index 2 — 0x0010 : Kernel data segment (ring 0)
//
// The canonical types live in kernel/src/arch/x86_64/gdt.rs.
// This is the Phase-1 inline copy used while boot and kernel share a binary.
// ---------------------------------------------------------------------------

/// 64-bit kernel code segment: P=1, DPL=0, S=1, Type=0xA, L=1, D=0, G=1.
const GDT_KERNEL_CODE: u64 = 0x00AF_9A00_0000_FFFF;

/// Kernel data segment: P=1, DPL=0, S=1, Type=0x2, D=1, G=1.
const GDT_KERNEL_DATA: u64 = 0x00CF_9200_0000_FFFF;

/// Kernel code segment selector (GDT index 1, TI=0, RPL=0).
const KERNEL_CODE_SELECTOR: u64 = 0x08;

/// Kernel data segment selector (GDT index 2, TI=0, RPL=0).
const KERNEL_DATA_SELECTOR: u16 = 0x10;

/// The kernel's GDT — three entries, 8-byte aligned.
#[repr(C, align(8))]
struct Gdt([u64; 3]);

/// SAFETY: written once at compile time and never mutated; the CPU reads it
/// as raw bytes via the GDTR, which does not go through Rust's reference model.
static GDT: Gdt = Gdt([0x0000_0000_0000_0000, GDT_KERNEL_CODE, GDT_KERNEL_DATA]);

/// Pointer structure passed to the `LGDT` instruction.
///
/// Must be `#[repr(C, packed)]` so the CPU sees exactly 2 bytes of limit
/// followed by 8 bytes of base — no padding.
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// Load the GDT and reload all segment registers.
///
/// # Safety
///
/// - Must be called at CPL=0 with interrupts disabled.
/// - `GDT` must be mapped and accessible at its linear address for the
///   lifetime of the kernel.
unsafe fn gdt_init() {
    let ptr = GdtPointer {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base: core::ptr::addr_of!(GDT) as u64,
    };

    // Load GDTR.
    //
    // SAFETY: `ptr` is a valid GdtPointer on the current stack; its address
    // is stable for the duration of this inline-asm block. LGDT is a
    // privileged instruction valid at CPL=0.
    core::arch::asm!(
        "lgdt [{ptr}]",
        ptr = in(reg) &ptr,
        options(readonly, nostack, preserves_flags),
    );

    // Reload CS via far return.
    //
    // There is no direct way to load CS in 64-bit mode; a far return is the
    // standard technique.  Stack layout for RETFQ (grows downward):
    //   RSP+0  new RIP  (address of label 1f, immediately after RETFQ)
    //   RSP+8  new CS   (KERNEL_CODE_SELECTOR = 0x08)
    //
    // After RETFQ the CPU fetches the next instruction from label 2: with
    // CS holding the kernel code selector.
    //
    // SAFETY: KERNEL_CODE_SELECTOR references a valid 64-bit code descriptor
    // in GDT.  The far return lands on the very next instruction (label 1:),
    // so control flow remains within this function.
    core::arch::asm!(
        "push {cs}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        cs  = in(reg) KERNEL_CODE_SELECTOR,
        tmp = lateout(reg) _,
    );

    // Reload data segment registers.
    //
    // DS, ES, FS, GS, SS must hold a valid selector; in 64-bit mode their
    // base/limit are ignored, but a null or invalid selector causes a #GP.
    //
    // SAFETY: KERNEL_DATA_SELECTOR references a valid data descriptor.
    core::arch::asm!(
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        "mov ss, ax",
        in("ax") KERNEL_DATA_SELECTOR,
        options(nomem, nostack, preserves_flags),
    );
}

/// Halt the CPU permanently.
fn halt() -> ! {
    loop {
        // SAFETY: `hlt` suspends the CPU until the next interrupt. With
        // interrupts disabled this loops forever, which is the intended
        // behaviour at end-of-life for Phase 1.
        unsafe { core::arch::asm!("hlt") };
    }
}

// ---------------------------------------------------------------------------
// UEFI helper functions (same as before, now only used pre-handoff)
// ---------------------------------------------------------------------------

fn retrieve_memory_map(console: &mut Console) -> Result<MemoryMap, uefi::Error> {
    let memory_map_owned = uefi::boot::memory_map(MemoryType::LOADER_DATA)?;
    let memory_map = MemoryMap::from_uefi_memory_map(&memory_map_owned);
    writeln!(
        console,
        "    Found {} memory regions",
        memory_map.region_count()
    )
    .unwrap();
    Ok(memory_map)
}

fn print_memory_summary(memory_map: &MemoryMap, console: &mut Console) {
    writeln!(console, "").unwrap();
    writeln!(console, "Memory Map Summary:").unwrap();
    writeln!(console, "-------------------").unwrap();
    for region in memory_map.regions() {
        let size_kb = region.size / 1024;
        let size_mb = size_kb / 1024;
        let size_str = if size_mb > 0 {
            alloc::format!("{} MB", size_mb)
        } else {
            alloc::format!("{} KB", size_kb)
        };
        writeln!(
            console,
            "  {:#012x} - {:#012x}: {:?} ({})",
            region.start,
            region.start + region.size,
            region.region_type,
            size_str
        )
        .unwrap();
    }
    writeln!(console, "").unwrap();
}

fn find_acpi_tables() -> Option<u64> {
    use uefi::table::cfg::{ACPI2_GUID, ACPI_GUID};
    uefi::system::with_config_table(|config_table| {
        for entry in config_table {
            if entry.guid == ACPI2_GUID {
                return Some(entry.address as u64);
            }
        }
        for entry in config_table {
            if entry.guid == ACPI_GUID {
                return Some(entry.address as u64);
            }
        }
        None
    })
}

// ---------------------------------------------------------------------------
// Framebuffer — local struct used only pre-handoff
// ---------------------------------------------------------------------------

struct RawFramebufferInfo {
    base_address: u64,
    width: u32,
    height: u32,
    stride: u32,
    pixel_format: boot_info::PixelFormat,
}

fn get_framebuffer_info() -> Option<RawFramebufferInfo> {
    use uefi::proto::console::gop::{GraphicsOutput, PixelFormat as GopPixelFormat};

    let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).ok()?;

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride() as u32 * 4; // bytes per row

    let pixel_format = match mode_info.pixel_format() {
        GopPixelFormat::Rgb => boot_info::PixelFormat::Rgb,
        GopPixelFormat::Bgr => boot_info::PixelFormat::Bgr,
        GopPixelFormat::Bitmask => boot_info::PixelFormat::Bitmask {
            red: 0,
            green: 0,
            blue: 0,
            reserved: 0,
        },
        _ => boot_info::PixelFormat::Unknown,
    };

    let mut frame_buffer = gop.frame_buffer();
    let base_address = frame_buffer.as_mut_ptr() as u64;

    Some(RawFramebufferInfo {
        base_address,
        width: width as u32,
        height: height as u32,
        stride,
        pixel_format,
    })
}
