//! Ferrous Kernel UEFI Bootloader
//!
//! This is the UEFI entry point for the Ferrous kernel. It initializes
//! UEFI boot services, retrieves system information, and prepares for
//! kernel handoff.
//!
//! # Boot Process
//!
//! 1. UEFI firmware loads this application
//! 2. Initialize UEFI boot services
//! 3. Set up console output (for debugging)
//! 4. Retrieve memory map from UEFI
//! 5. Gather system information (ACPI, framebuffer, etc.)
//! 6. Exit boot services
//! 7. Transfer control to the kernel

#![no_std]
#![no_main]

extern crate alloc;

mod boot_info;
mod console;
mod memory;

use core::fmt::Write;
use uefi::prelude::*;
use uefi::table::boot::MemoryType;

use crate::boot_info::BootInfo;
use crate::console::Console;
use crate::memory::MemoryMap;

/// UEFI application entry point.
///
/// # Safety
///
/// This function is called by UEFI firmware. The `image_handle` and `system_table`
/// pointers are guaranteed to be valid by the UEFI specification when this
/// function is called as the entry point of a UEFI application.
#[entry]
fn efi_main(_image_handle: Handle, mut system_table: SystemTable<Boot>) -> Status {
    // Initialize UEFI services (logger, allocator)
    uefi::helpers::init(&mut system_table).expect("Failed to initialize UEFI helpers");

    // Get boot services reference
    let boot_services = system_table.boot_services();

    // Initialize console output
    let mut console = Console::new(&system_table);
    console.clear();

    // Print boot banner
    writeln!(console, "").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "  Ferrous Kernel UEFI Bootloader v0.1").unwrap();
    writeln!(console, "========================================").unwrap();
    writeln!(console, "").unwrap();

    // Log boot services initialization
    log::info!("UEFI boot services initialized");
    writeln!(console, "[OK] UEFI boot services initialized").unwrap();

    // Get firmware vendor and revision
    let firmware_vendor = system_table.firmware_vendor();
    let firmware_revision = system_table.firmware_revision();
    writeln!(
        console,
        "[INFO] Firmware: {} (rev {})",
        firmware_vendor, firmware_revision
    )
    .unwrap();

    // Get UEFI revision
    let uefi_revision = system_table.uefi_revision();
    writeln!(
        console,
        "[INFO] UEFI Revision: {}.{}",
        uefi_revision.major(),
        uefi_revision.minor()
    )
    .unwrap();

    // Retrieve memory map
    writeln!(console, "[...] Retrieving memory map").unwrap();
    let memory_map = match retrieve_memory_map(boot_services, &mut console) {
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

    // Print memory summary
    print_memory_summary(&memory_map, &mut console);

    // Try to get ACPI tables
    writeln!(console, "[...] Looking for ACPI tables").unwrap();
    if let Some(acpi_addr) = find_acpi_tables(&system_table) {
        writeln!(console, "[OK] ACPI RSDP found at: {:#x}", acpi_addr).unwrap();
        log::info!("ACPI RSDP found at: {:#x}", acpi_addr);
    } else {
        writeln!(console, "[WARN] ACPI tables not found").unwrap();
        log::warn!("ACPI tables not found");
    }

    // Try to get framebuffer info
    writeln!(console, "[...] Looking for GOP framebuffer").unwrap();
    if let Some(fb_info) = get_framebuffer_info(boot_services) {
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

    // Log boot info
    log::info!("Boot info created successfully");
    log::info!("Total memory: {} MB", boot_info.total_memory_mb());
    log::info!("Usable memory: {} MB", boot_info.usable_memory_mb());

    writeln!(console, "[INFO] Total memory: {} MB", boot_info.total_memory_mb()).unwrap();
    writeln!(console, "[INFO] Usable memory: {} MB", boot_info.usable_memory_mb()).unwrap();
    writeln!(console, "").unwrap();
    writeln!(console, "Boot loader ready for kernel handoff.").unwrap();
    writeln!(console, "(Kernel loading will be implemented in next phase)").unwrap();

    // For now, just halt - kernel handoff will be implemented later
    log::info!("Bootloader complete - halting");

    // Exit boot services would happen here before kernel handoff
    // For now we just return success since kernel isn't loaded yet
    Status::SUCCESS
}

/// Retrieves the UEFI memory map.
fn retrieve_memory_map(
    boot_services: &BootServices,
    console: &mut Console,
) -> Result<MemoryMap, uefi::Error> {
    // Get the memory map size first
    let map_size = boot_services.memory_map_size();

    // Allocate buffer for memory map (with some extra space for changes)
    let buffer_size = map_size.map_size + 2 * map_size.entry_size;
    let buffer = boot_services.allocate_pool(MemoryType::LOADER_DATA, buffer_size)?;

    // Safety: buffer was just allocated with the correct size
    let buffer_slice = unsafe { core::slice::from_raw_parts_mut(buffer, buffer_size) };

    // Get the actual memory map
    let (_, descriptors) = boot_services.memory_map(buffer_slice)?;

    // Convert to our memory map format
    let memory_map = MemoryMap::from_uefi_descriptors(descriptors);

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
fn find_acpi_tables(system_table: &SystemTable<Boot>) -> Option<u64> {
    use uefi::table::cfg::{ACPI2_GUID, ACPI_GUID};

    // Try ACPI 2.0 first (RSDP 2.0)
    for entry in system_table.config_table() {
        if entry.guid == ACPI2_GUID {
            return Some(entry.address as u64);
        }
    }

    // Fall back to ACPI 1.0
    for entry in system_table.config_table() {
        if entry.guid == ACPI_GUID {
            return Some(entry.address as u64);
        }
    }

    None
}

/// Framebuffer information from GOP.
struct FramebufferInfo {
    base_address: u64,
    width: u32,
    height: u32,
}

/// Gets framebuffer information from GOP (Graphics Output Protocol).
fn get_framebuffer_info(boot_services: &BootServices) -> Option<FramebufferInfo> {
    use uefi::proto::console::gop::GraphicsOutput;

    // Try to get GOP handle
    let gop_handle = boot_services
        .get_handle_for_protocol::<GraphicsOutput>()
        .ok()?;

    // Open GOP protocol
    // Safety: We just obtained this handle from the boot services, and we're
    // only using it to read framebuffer information.
    let gop = unsafe {
        boot_services
            .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
            .ok()?
    };

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();

    let frame_buffer = gop.frame_buffer();
    let base_address = frame_buffer.as_mut_ptr() as u64;

    Some(FramebufferInfo {
        base_address,
        width: width as u32,
        height: height as u32,
    })
}
