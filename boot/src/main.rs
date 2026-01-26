//! Ferrous Kernel UEFI Bootloader
//!
//! This is the UEFI entry point for the Ferrous kernel. It initializes
//! UEFI boot services, retrieves system information, and prepares for
//! kernel handoff.

#![no_std]
#![no_main]

extern crate alloc;

mod boot_info;
mod console;
mod memory;

use core::fmt::Write;
use uefi::prelude::*;
use uefi::boot::MemoryType;

use crate::boot_info::BootInfo;
use crate::console::Console;
use crate::memory::MemoryMap;

/// Panic handler for the bootloader.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("BOOT PANIC: {}", info);
    loop {
        // Safety: hlt instruction is safe to execute, it just halts the CPU
        // until the next interrupt.
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// UEFI application entry point.
#[entry]
fn efi_main() -> Status {
    // Initialize UEFI services (logger, allocator)
    uefi::helpers::init().expect("Failed to initialize UEFI helpers");

    // Initialize console output
    let mut console = Console::new();
    console.clear();

    // Print boot banner
    writeln!(console, "").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "  Ferrous Kernel UEFI Bootloader v0.1").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "").unwrap();

    log::info!("UEFI boot services initialized");
    writeln!(console, "[OK] UEFI boot services initialized").unwrap();

    // Get firmware info
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

    // Retrieve memory map
    writeln!(console, "[...] Retrieving memory map").unwrap();
    let memory_map = match retrieve_memory_map(&mut console) {
        Ok(map) => {
            writeln!(console, "[OK] Memory map retrieved").unwrap();
            map
        }
        Err(e) => {
            writeln!(console, "[FAIL] Failed to retrieve memory map: {:?}", e).unwrap();
            log::error!("Failed to retrieve memory map: {:?}", e);
            return Status::ABORTED;
        }
    };

    print_memory_summary(&memory_map, &mut console);

    // Try to get ACPI tables
    writeln!(console, "[...] Looking for ACPI tables").unwrap();
    if let Some(acpi_addr) = find_acpi_tables() {
        writeln!(console, "[OK] ACPI RSDP found at: {:#x}", acpi_addr).unwrap();
        log::info!("ACPI RSDP found at: {:#x}", acpi_addr);
    } else {
        writeln!(console, "[WARN] ACPI tables not found").unwrap();
        log::warn!("ACPI tables not found");
    }

    // Try to get framebuffer info
    writeln!(console, "[...] Looking for GOP framebuffer").unwrap();
    if let Some(fb_info) = get_framebuffer_info() {
        writeln!(
            console,
            "[OK] Framebuffer: {}x{} @ {:#x}",
            fb_info.width, fb_info.height, fb_info.base_address
        )
        .unwrap();
        log::info!(
            "Framebuffer: {}x{} at {:#x}",
            fb_info.width,
            fb_info.height,
            fb_info.base_address
        );
    } else {
        writeln!(console, "[WARN] GOP framebuffer not available").unwrap();
        log::warn!("GOP framebuffer not available");
    }

    // Create boot info structure
    let boot_info = BootInfo::new(memory_map);

    writeln!(console, "").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "  Boot services initialization complete").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "").unwrap();

    log::info!("Boot info created successfully");
    log::info!("Total memory: {} MB", boot_info.total_memory_mb());
    log::info!("Usable memory: {} MB", boot_info.usable_memory_mb());

    writeln!(console, "[INFO] Total memory: {} MB", boot_info.total_memory_mb()).unwrap();
    writeln!(console, "[INFO] Usable memory: {} MB", boot_info.usable_memory_mb()).unwrap();
    writeln!(console, "").unwrap();
    writeln!(console, "Boot loader ready for kernel handoff.").unwrap();
    writeln!(console, "(Kernel loading will be implemented in next phase)").unwrap();

    log::info!("Bootloader complete - halting");

    Status::SUCCESS
}

/// Retrieves the UEFI memory map.
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

/// Prints a summary of the memory map.
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

/// Finds ACPI tables in the UEFI configuration table.
fn find_acpi_tables() -> Option<u64> {
    use uefi::table::cfg::{ACPI2_GUID, ACPI_GUID};

    uefi::system::with_config_table(|config_table| {
        // Try ACPI 2.0 first
        for entry in config_table {
            if entry.guid == ACPI2_GUID {
                return Some(entry.address as u64);
            }
        }

        // Fall back to ACPI 1.0
        for entry in config_table {
            if entry.guid == ACPI_GUID {
                return Some(entry.address as u64);
            }
        }

        None
    })
}

/// Framebuffer information from GOP.
struct FramebufferInfo {
    base_address: u64,
    width: u32,
    height: u32,
}

/// Gets framebuffer information from GOP (Graphics Output Protocol).
fn get_framebuffer_info() -> Option<FramebufferInfo> {
    use uefi::proto::console::gop::GraphicsOutput;

    let gop_handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle).ok()?;

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();

    let mut frame_buffer = gop.frame_buffer();
    let base_address = frame_buffer.as_mut_ptr() as u64;

    Some(FramebufferInfo {
        base_address,
        width: width as u32,
        height: height as u32,
    })
}
