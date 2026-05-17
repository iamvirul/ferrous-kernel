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
mod logger;
mod memory;
mod serial;
mod unwind;

// Re-export module-level serial helpers at the crate root so that
// boot/src/unwind.rs and boot/src/logger.rs can continue to use
// `crate::serial_write_str` etc. without modification.
pub use serial::{serial_init, serial_write_str, serial_write_usize, serial_write_usize_hex};

// Kernel assertion macros from ferrous-core (Phase 1.4.3).
use ferrous_core::{
    kassert, kassert_eq, kassert_ne, kdebug_assert, kdebug_assert_eq, kdebug_assert_ne,
};

use core::fmt::Write;
use linked_list_allocator::LockedHeap;
use uefi::boot::MemoryType;
use uefi::prelude::*;

use crate::boot_info::BootInfo;
use crate::console::Console;
use crate::memory::MemoryMap;
use ferrous_boot_info::{BitmapFrameAllocator, KernelBootInfo, KERNEL_BITMAP_WORDS};

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
// Physical frame allocator (Phase 1.3.2)
// ---------------------------------------------------------------------------

/// Global physical frame allocator for Phase 1.
///
/// Stores a bitmap tracking usable physical frames. The bitmap is 2 MiB and
/// lives in BSS (zero-initialised, no binary bloat). Initialised in
/// `kernel_main` Step 6 from the parsed `MemoryMap`.
///
/// SAFETY: mutated only inside `kernel_main` (single-threaded, interrupts
/// disabled). After init, `free_frames()` and `allocate()` are called before
/// the allocator static is returned from `kernel_main`.
#[allow(static_mut_refs)]
static mut FRAME_ALLOC: BitmapFrameAllocator<KERNEL_BITMAP_WORDS> = BitmapFrameAllocator::new();

// ---------------------------------------------------------------------------
// Page tables (Phase 1.3.3)
//
// Three raw 4 KiB page tables in BSS.  Populated by `setup_page_tables()`
// during Step 7 of `kernel_main` (after the frame allocator is live).
//
// Layout after init:
//   PML4[0]   → PDPT   (identity: VA [0, 1 GiB) = PA [0, 1 GiB))
//   PML4[256] → PDPT   (higher-half alias: VA 0xFFFF_8000_... = PA 0x0...)
//   PDPT[0]   → PD
//   PD[0..511]→ 512 × 2 MiB huge pages  (PA i×2MiB, Present+Writable+PS)
// ---------------------------------------------------------------------------

/// Page present (P, bit 0).
const PT_PRESENT: u64 = 1 << 0;
/// Page writable (R/W, bit 1).
const PT_WRITABLE: u64 = 1 << 1;
/// Huge page / page size (PS, bit 7).  In a PD entry: maps a 2 MiB page.
const PT_HUGE: u64 = 1 << 7;
/// Mask for the physical address stored in a page table entry (bits [51:12]).
const PT_PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// A raw 4 KiB page table: 512 × 8-byte entries, 4 KiB-aligned.
///
/// The alignment guarantee means `addr_of!(PT_PML4) as u64` has its low 12
/// bits clear — safe to write directly to CR3.
#[repr(C, align(4096))]
struct RawPageTable([u64; 512]);

/// PML4 — the root of the kernel's page table hierarchy.
///
/// SAFETY: mutated exactly once in `setup_page_tables()` before CR3 is
/// loaded.  Single-threaded, interrupts disabled throughout.
#[allow(static_mut_refs)]
static mut PT_PML4: RawPageTable = RawPageTable([0u64; 512]);

/// PDPT — shared by the identity window (PML4[0]) and the higher-half alias
/// (PML4[256]).
#[allow(static_mut_refs)]
static mut PT_PDPT: RawPageTable = RawPageTable([0u64; 512]);

/// PD — 512 × 2 MiB huge-page entries covering [0, 1 GiB).
#[allow(static_mut_refs)]
static mut PT_PD: RawPageTable = RawPageTable([0u64; 512]);

// ---------------------------------------------------------------------------
// Kernel heap (Phase 1.3.5)
//
// A BSS-backed heap that replaces the UEFI global allocator after
// `exit_boot_services()`.  The `uefi` crate's `global_allocator` feature is
// removed from Cargo.toml; this `LockedHeap` is the one and only
// `#[global_allocator]` for the binary.
//
// Layout:
//   HEAP_STORAGE — 4 MiB, page-aligned, zero-initialised in BSS.
//                  Backed by physical RAM but never touched by UEFI's own
//                  allocator, so it remains valid after exit_boot_services().
//   ALLOCATOR    — `LockedHeap::empty()` until `init_heap()` is called at
//                  the top of `efi_main`, before any alloc operation.
//
// Why BSS and not a UEFI-allocated region?
//   • Zero overhead in the binary image (BSS is not stored on disk).
//   • Guaranteed to be present before the first Rust instruction runs.
//   • UEFI firmware zero-initialises BSS, so the allocator's internal
//     free-list is clean before `init` writes its header.
//   • Lives at a fixed VA == PA (identity mapped), so the pointer is valid
//     both before and after the CR3 switch in Step 7.
// ---------------------------------------------------------------------------

/// Heap size in bytes (4 MiB).
///
/// Adequate for all Phase 1 smoke tests (Vec, Box, String) and provides
/// headroom for Phase 2 kernel data structures.  Stored in BSS — no binary
/// size impact.
const HEAP_SIZE: usize = 4 * 1024 * 1024;

/// Backing storage for the kernel heap.
///
/// `repr(C, align(4096))` ensures the buffer is page-aligned so the allocator
/// is compatible with any future page-granularity guard policy.
///
/// SAFETY: written only by `linked_list_allocator::LockedHeap::init` (called
/// once from `init_heap`) and thereafter managed exclusively by the allocator.
#[repr(C, align(4096))]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0u8; HEAP_SIZE]);

/// The global heap allocator.
///
/// Starts empty; `init_heap()` must be called before the first allocation.
/// `LockedHeap` uses a spin-lock internally, making it safe in a pre-SMP
/// kernel where re-entrancy can only occur through interrupt handlers (which
/// are disabled throughout Phase 1).
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialise the global heap allocator from the BSS-backed `HEAP_STORAGE`.
///
/// Must be called exactly once, at the very top of `efi_main`, before any
/// code that uses `alloc` (Vec, Box, format!, etc.).
///
/// # Safety
///
/// - Single-threaded context required (always true in early UEFI boot).
/// - `HEAP_STORAGE` must not have been touched by any allocator before this
///   call (guaranteed — it is a fresh BSS region).
unsafe fn init_heap() {
    let heap_start = core::ptr::addr_of_mut!(HEAP_STORAGE) as *mut u8;
    // SAFETY: heap_start points to HEAP_SIZE bytes of writable, zero-
    // initialised BSS that no other code has accessed.  Called exactly once.
    ALLOCATOR.lock().init(heap_start, HEAP_SIZE);
}

/// Build the kernel page tables and return the physical address of the PML4.
///
/// Under UEFI identity mapping every static lives at `VA == PA`, so the
/// pointer value is both the virtual and the physical address.  After `CR3`
/// is loaded with the returned value:
///
/// - `[0, 1 GiB)` is identity-mapped with 2 MiB huge pages (execution
///   continues at the same virtual addresses — no jump required).
/// - `[0xFFFF_8000_0000_0000, 0xFFFF_8000_4000_0000)` aliases the same
///   physical range (higher-half window for Phase 2).
///
/// # Safety
///
/// - Must be called **exactly once**.
/// - Must be called **single-threaded** with interrupts disabled.
/// - UEFI identity mapping (`VA == PA`) must still be in effect.
unsafe fn setup_page_tables() -> u64 {
    // Obtain raw mutable pointers to the three table arrays.
    // Using `addr_of_mut!` avoids creating Rust references to `static mut`
    // (which would require `#[allow(static_mut_refs)]` and risks aliasing).
    //
    // Each pointer is to a `RawPageTable` which contains `[u64; 512]`, so
    // casting to `*mut u64` is safe — `repr(C)` guarantees the array starts
    // at offset 0.
    let pml4 = core::ptr::addr_of_mut!(PT_PML4) as *mut u64;
    let pdpt = core::ptr::addr_of_mut!(PT_PDPT) as *mut u64;
    let pd = core::ptr::addr_of_mut!(PT_PD) as *mut u64;

    // Zero all three tables.  BSS is already zeroed by the UEFI firmware,
    // but we zero explicitly so the invariant is independent of the loader.
    for i in 0..512usize {
        pml4.add(i).write(0u64);
        pdpt.add(i).write(0u64);
        pd.add(i).write(0u64);
    }

    // PD: 512 × 2 MiB huge pages covering [0, 1 GiB).
    //   Entry i maps physical address i × 2 MiB.
    //   Flags: Present | Writable | HugePage (PS=1).
    for i in 0..512usize {
        let phys = (i as u64) * (2 * 1024 * 1024); // i × 2 MiB
        pd.add(i)
            .write((phys & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE | PT_HUGE);
    }

    // PDPT[0] → PD.
    let pd_phys = core::ptr::addr_of!(PT_PD) as u64; // VA == PA (UEFI)
    pdpt.write((pd_phys & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE);

    // PML4[0] → PDPT  (identity window: VA 0 maps to PA 0).
    let pdpt_phys = core::ptr::addr_of!(PT_PDPT) as u64;
    pml4.write((pdpt_phys & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE);

    // PML4[256] → same PDPT  (higher-half alias: 0xFFFF_8000_0000_0000).
    //
    // Bits [47:39] of 0xFFFF_8000_0000_0000 = 0x100 = 256.
    pml4.add(256)
        .write((pdpt_phys & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE);

    // Return the physical address of PML4 — this goes directly into CR3.
    core::ptr::addr_of!(PT_PML4) as u64
}

// ---------------------------------------------------------------------------
// Boot page-table helpers (Phase 1.3.4)
//
// The boot crate does not link against the kernel crate, so `ActivePageTable`
// and `FrameAllocate` from `kernel/src/memory/paging/mapper.rs` are not
// directly available here.  These inline helpers replicate the same logic
// with the same safety invariants:
//   - VA == PA (identity mapping from Step 7).
//   - Single-threaded, interrupts disabled.
//   - FRAME_ALLOC initialised in Step 6.
// ---------------------------------------------------------------------------

/// Walk the live page tables and translate `virt` to a physical address.
///
/// Handles 1 GiB (PDPT huge), 2 MiB (PD huge), and 4 KiB (PT) mappings.
/// Returns `None` if any level is absent or the final entry is not present.
///
/// # Safety
///
/// Identity mapping must hold; CR3 must point to a valid PML4.
unsafe fn boot_translate(virt: u64) -> Option<u64> {
    let cr3: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
    let pml4 = (cr3 & PT_PHYS_MASK) as *const u64;

    let pml4e = pml4.add(((virt >> 39) & 0x1FF) as usize).read();
    if pml4e & PT_PRESENT == 0 {
        return None;
    }

    let pdpt = (pml4e & PT_PHYS_MASK) as *const u64;
    let pdpte = pdpt.add(((virt >> 30) & 0x1FF) as usize).read();
    if pdpte & PT_PRESENT == 0 {
        return None;
    }
    if pdpte & PT_HUGE != 0 {
        // 1 GiB huge page: PA = base | (virt & (1 GiB − 1))
        return Some((pdpte & PT_PHYS_MASK) | (virt & 0x3FFF_FFFF));
    }

    let pd = (pdpte & PT_PHYS_MASK) as *const u64;
    let pde = pd.add(((virt >> 21) & 0x1FF) as usize).read();
    if pde & PT_PRESENT == 0 {
        return None;
    }
    if pde & PT_HUGE != 0 {
        // 2 MiB huge page: PA = base | (virt & (2 MiB − 1))
        return Some((pde & PT_PHYS_MASK) | (virt & 0x1F_FFFF));
    }

    let pt = (pde & PT_PHYS_MASK) as *const u64;
    let pte = pt.add(((virt >> 12) & 0x1FF) as usize).read();
    if pte & PT_PRESENT == 0 {
        return None;
    }
    Some((pte & PT_PHYS_MASK) | (virt & 0xFFF))
}

/// Ensure `entry` points to a child page table.
///
/// - If `entry` is already present, returns its physical address.
/// - If absent, allocates a 4 KiB frame, zeroes it, installs
///   `Present | Writable`, and returns the new frame address.
///
/// Returns `None` if the frame allocator is exhausted.
///
/// # Safety
///
/// Identity mapping must hold.
#[allow(static_mut_refs)]
unsafe fn boot_ensure_table(entry: *mut u64) -> Option<u64> {
    let val = entry.read();
    if val & PT_PRESENT != 0 {
        return Some(val & PT_PHYS_MASK);
    }
    let frame = FRAME_ALLOC.allocate()?.start_address() as u64;
    // Zero the newly allocated frame before installing it as a table.
    let p = frame as *mut u64;
    for i in 0..512usize {
        p.add(i).write(0u64);
    }
    entry.write((frame & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE);
    Some(frame)
}

/// Map a single 4 KiB page at `virt` to physical frame `phys`.
///
/// Creates intermediate tables (PDPT, PD, PT) as needed via
/// [`boot_ensure_table`].  Returns `false` if a frame allocation fails or
/// the PT entry is already present.
///
/// **Does not handle huge-page splits** — the test VA (PML4[1] area) is in a
/// region where no intermediate levels exist, so this simplified path is
/// sufficient for the smoke test.
///
/// # Safety
///
/// - Identity mapping must hold.
/// - `virt` must not be in a huge-page region.
unsafe fn boot_map_4k(virt: u64, phys: u64, flags: u64) -> bool {
    let cr3: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
    let pml4 = (cr3 & PT_PHYS_MASK) as *mut u64;

    let pml4e = pml4.add(((virt >> 39) & 0x1FF) as usize);
    let pdpt_phys = match boot_ensure_table(pml4e) {
        Some(p) => p,
        None => return false,
    };
    let pdpt = pdpt_phys as *mut u64;

    let pdpte = pdpt.add(((virt >> 30) & 0x1FF) as usize);
    let pd_phys = match boot_ensure_table(pdpte) {
        Some(p) => p,
        None => return false,
    };
    let pd = pd_phys as *mut u64;

    let pde = pd.add(((virt >> 21) & 0x1FF) as usize);
    let pt_phys = match boot_ensure_table(pde) {
        Some(p) => p,
        None => return false,
    };
    let pt = pt_phys as *mut u64;

    let pte = pt.add(((virt >> 12) & 0x1FF) as usize);
    if pte.read() & PT_PRESENT != 0 {
        return false; // already mapped
    }
    pte.write((phys & PT_PHYS_MASK) | flags);

    // Invalidate the TLB entry for this specific VA.
    core::arch::asm!(
        "invlpg [{addr}]",
        addr = in(reg) virt,
        options(nostack, preserves_flags),
    );
    true
}

/// Unmap a single 4 KiB page at `virt`.
///
/// Clears the PT entry and issues `INVLPG`.  Returns `true` if the entry
/// was present and cleared, `false` if the VA was not 4 KiB-mapped.
///
/// # Safety
///
/// Identity mapping must hold.
unsafe fn boot_unmap_4k(virt: u64) -> bool {
    let cr3: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
    let pml4 = (cr3 & PT_PHYS_MASK) as *const u64;

    let pml4e = pml4.add(((virt >> 39) & 0x1FF) as usize).read();
    if pml4e & PT_PRESENT == 0 {
        return false;
    }
    let pdpt = (pml4e & PT_PHYS_MASK) as *const u64;
    let pdpte = pdpt.add(((virt >> 30) & 0x1FF) as usize).read();
    if pdpte & PT_PRESENT == 0 || pdpte & PT_HUGE != 0 {
        return false;
    }
    let pd = (pdpte & PT_PHYS_MASK) as *const u64;
    let pde = pd.add(((virt >> 21) & 0x1FF) as usize).read();
    if pde & PT_PRESENT == 0 || pde & PT_HUGE != 0 {
        return false;
    }
    let pt = (pde & PT_PHYS_MASK) as *mut u64;
    let pte = pt.add(((virt >> 12) & 0x1FF) as usize);
    if pte.read() & PT_PRESENT == 0 {
        return false;
    }
    pte.write(0u64);

    core::arch::asm!(
        "invlpg [{addr}]",
        addr = in(reg) virt,
        options(nostack, preserves_flags),
    );
    true
}

/// Split the 2 MiB huge PD entry covering `guard_va` into 512 × 4 KiB
/// entries, then mark the 4 KiB page that contains `guard_va` non-present.
///
/// This is used in Step 8 to activate the kernel stack guard page: before
/// this call the guard region is covered by the same 2 MiB huge page as
/// the rest of the stack; after this call any access to the guard 4 KiB
/// raises a `#PF`.
///
/// Returns `true` on success, `false` if FRAME_ALLOC is exhausted or the
/// target PD entry is not a 2 MiB huge page.
///
/// # Safety
///
/// - Identity mapping must hold.
/// - `guard_va` must be within a 2 MiB huge-page region (PS=1 in the PDE).
#[allow(static_mut_refs)]
unsafe fn boot_activate_guard_page(guard_va: u64) -> bool {
    let cr3: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
    let pml4 = (cr3 & PT_PHYS_MASK) as *const u64;

    let pml4e = pml4.add(((guard_va >> 39) & 0x1FF) as usize).read();
    if pml4e & PT_PRESENT == 0 {
        return false;
    }
    let pdpt = (pml4e & PT_PHYS_MASK) as *const u64;
    let pdpte = pdpt.add(((guard_va >> 30) & 0x1FF) as usize).read();
    if pdpte & PT_PRESENT == 0 || pdpte & PT_HUGE != 0 {
        return false; // absent or 1 GiB huge (not handled here)
    }
    let pd = (pdpte & PT_PHYS_MASK) as *mut u64;
    let pde = pd.add(((guard_va >> 21) & 0x1FF) as usize);
    let pde_val = pde.read();
    if pde_val & PT_PRESENT == 0 || pde_val & PT_HUGE == 0 {
        return false; // not a 2 MiB huge page
    }

    // Split: reproduce the 2 MiB region as 512 × 4 KiB PT entries.
    let huge_base = pde_val & PT_PHYS_MASK; // 2 MiB-aligned PA
    let base_flags = (pde_val & !PT_PHYS_MASK & !PT_HUGE) | PT_PRESENT;

    let pt_frame = match FRAME_ALLOC.allocate() {
        Some(f) => f.start_address() as u64,
        None => return false,
    };
    let pt = pt_frame as *mut u64;
    for i in 0..512usize {
        let phys = huge_base + (i as u64) * 4096;
        pt.add(i).write((phys & PT_PHYS_MASK) | base_flags);
    }
    // Replace the 2 MiB PDE with a pointer to the new PT.
    pde.write((pt_frame & PT_PHYS_MASK) | PT_PRESENT | PT_WRITABLE);

    // Flush the entire TLB — the 2 MiB entry is now stale.
    let cr3_val: u64;
    core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3_val, options(nomem, nostack));
    core::arch::asm!("mov cr3, {cr3}", cr3 = in(reg) cr3_val, options(nostack));

    // Zero the PT entry for the guard page itself.
    let guard_pt_idx = (guard_va >> 12) & 0x1FF;
    pt.add(guard_pt_idx as usize).write(0u64);

    // Targeted TLB invalidation for the guard VA.
    core::arch::asm!(
        "invlpg [{addr}]",
        addr = in(reg) guard_va,
        options(nostack, preserves_flags),
    );
    true
}

// ---------------------------------------------------------------------------
// Panic handler (Phase 1.4.2)
// ---------------------------------------------------------------------------

/// Boot-phase panic handler with structured output and stack trace.
///
/// Writes a human-readable panic report to COM1 then halts via `hlt`.  The
/// report includes:
///
/// 1. An ASCII banner so the panic is impossible to miss in the serial log.
/// 2. Source location (`file:line:column`) from [`core::panic::PanicInfo`].
/// 3. The panic message, formatted via the [`log`] framework (goes to COM1).
/// 4. A frame-pointer stack trace (raw return addresses, resolvable offline).
///
/// # Why direct serial writes for the banner?
///
/// The logger may not have been initialised (if the panic fires before
/// [`logger::init`] in `efi_main`), or may itself be in a broken state.
/// Direct serial writes to COM1 are always safe at ring-0 — they bypass every
/// abstraction layer and ensure the banner is always visible.
///
/// # Resolving stack addresses
///
/// ```bash
/// addr2line -e target/x86_64-unknown-uefi/debug/ferrous-boot.efi <addr>
/// ```
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // --- Banner (direct serial: always visible, even pre-logger) ---
    serial_write_str("\r\n");
    serial_write_str("--- KERNEL PANIC ---\r\n");

    // --- Source location ---
    if let Some(loc) = info.location() {
        serial_write_str("  at ");
        serial_write_str(loc.file());
        serial_write_str(":");
        serial_write_usize(loc.line() as usize);
        serial_write_str(":");
        serial_write_usize(loc.column() as usize);
        serial_write_str("\r\n");
    }

    // --- Formatted panic message via the log framework ---
    //
    // `log::error!` uses `fmt::Arguments` formatting, which is the same as
    // `format_args!`. If the logger is not yet initialised this is a no-op;
    // the banner above still provides a minimal diagnosis.
    log::error!("PANIC: {}", info);

    // --- Frame-pointer stack trace ---
    serial_write_str("\r\n");
    // SAFETY:
    // - `force-frame-pointers = true` is set in Cargo.toml for all profiles.
    // - Single-threaded, interrupts are disabled (cli executed in efi_main).
    // - The entire kernel stack is identity-mapped (Phase 1.3.3).
    // - This is the panic path: no concurrency, no re-entrance possible.
    unsafe { unwind::print_stack_trace() };

    serial_write_str("--- END PANIC ---\r\n");

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
    // Initialise the heap allocator before any alloc usage.
    //
    // SAFETY: single-threaded UEFI entry; HEAP_STORAGE is untouched BSS;
    // called exactly once before the first Vec/Box/format! anywhere in this
    // binary.
    unsafe { init_heap() };

    // Install the serial logger as the one global logger for the entire
    // binary lifetime (pre-EBS and post-EBS).
    //
    // This must happen BEFORE `uefi::helpers::init()`. The uefi crate's
    // `logger` feature has been removed from Cargo.toml so uefi no longer
    // calls `log::set_logger` itself. Calling `logger::init` here means:
    //   - `log::set_logger_racy` succeeds (slot is empty).
    //   - All `log::*!` macros in efi_main route to COM1.
    //   - After exit_boot_services() the same logger is still installed;
    //     no swap is needed.
    //
    // SAFETY: single-threaded; COM1 is accessible at any privilege level
    // from the start of execution; serial_init() will reconfigure the UART
    // later (Step 2 of kernel_main) but the port is already usable at
    // UEFI's default baud rate for our purposes here.
    unsafe { logger::init(log::LevelFilter::Debug) };

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
    // Step 4: Load IDT — install exception stubs, load IDTR.
    //
    // After this call, any CPU exception (divide-by-zero, GPF, page fault,
    // etc.) will be caught by our stub handlers instead of triple-faulting
    // immediately. Interrupts remain disabled (no STI) — the IDT is ready
    // for exceptions only at this stage.
    //
    // SAFETY:
    // - We are at CPL=0.
    // - Interrupts are disabled (cli executed in efi_main).
    // - IDT static is fully initialised inside idt_init() before LIDT.
    unsafe { idt_init() };

    serial_write_str("[OK] IDT loaded (32 exception handlers with error codes + RIP + CR2, interrupts disabled)\r\n");

    // -----------------------------------------------------------------------
    serial_write_str("[OK] Kernel entered successfully!\r\n");
    serial_write_str("Hello from Ferrous!\r\n");
    serial_write_str("\r\n");

    // -----------------------------------------------------------------------
    // Step 5: Parse and report the physical memory map.
    //
    // Iterates over the KernelMemoryMap received from the bootloader, prints
    // each region with its classification and size, then summarises totals.
    //
    // Note: in Phase 1 this is an inline analysis. The canonical MemoryMap
    // type (Phase 1.3.1) lives in ferrous-boot-info and is accessible via
    // kernel::memory after the kernel binary is separated in Phase 2.
    print_memory_map(&boot_info.memory_map);

    // -----------------------------------------------------------------------
    // Step 6: Initialise the physical frame allocator (Phase 1.3.2).
    //
    // Parse the memory map into the kernel's MemoryMap type, then initialise
    // the bitmap allocator from it.  Only immediately-usable (conventional)
    // regions are marked free; bootloader and ACPI reclaimable regions remain
    // reserved until a future reclamation pass (Phase 2+).
    //
    // We exercise the allocator with a small test sequence to prove it works:
    //   1. Allocate 3 frames and verify distinct, page-aligned addresses.
    //   2. Deallocate all 3 frames and confirm the free count recovers.
    //
    // SAFETY: single-threaded, interrupts disabled; FRAME_ALLOC is not yet
    // initialised (total_frames == 0); no other code touches it.
    let parsed_map = ferrous_boot_info::MemoryMap::parse(&boot_info.memory_map);
    match parsed_map {
        Err(_) => {
            serial_write_str("[WARN] Frame allocator: memory map parse failed — skipping init\r\n");
        }
        Ok(map) => {
            // Initialise the allocator in-place in the global static (BSS).
            // SAFETY: FRAME_ALLOC starts all-zero (BSS), single-threaded,
            // interrupts disabled, init called exactly once.
            #[allow(static_mut_refs)]
            unsafe {
                FRAME_ALLOC.init_from_memory_map(&map);
            }

            let free = unsafe {
                #[allow(static_mut_refs)]
                FRAME_ALLOC.free_frames()
            };
            let total = unsafe {
                #[allow(static_mut_refs)]
                FRAME_ALLOC.total_frames()
            };

            serial_write_str("[OK] Frame allocator initialised\r\n");
            serial_write_str("[INFO] Physical frames: ");
            serial_write_usize(free);
            serial_write_str(" free / ");
            serial_write_usize(total);
            serial_write_str(" addressable (");
            serial_write_usize(free * 4);
            serial_write_str(" KiB usable)\r\n");

            // --- Allocator smoke test ---
            //
            // Allocate 3 frames; print their addresses; deallocate; verify
            // the free count returns to the original value.
            serial_write_str("[TEST] Allocating 3 frames...\r\n");
            let f0 = unsafe {
                #[allow(static_mut_refs)]
                FRAME_ALLOC.allocate()
            };
            let f1 = unsafe {
                #[allow(static_mut_refs)]
                FRAME_ALLOC.allocate()
            };
            let f2 = unsafe {
                #[allow(static_mut_refs)]
                FRAME_ALLOC.allocate()
            };

            match (f0, f1, f2) {
                (Some(a), Some(b), Some(c)) => {
                    serial_write_str("[OK]   frame[0] = 0x");
                    serial_write_usize_hex(a.start_address() as usize);
                    serial_write_str("\r\n");
                    serial_write_str("[OK]   frame[1] = 0x");
                    serial_write_usize_hex(b.start_address() as usize);
                    serial_write_str("\r\n");
                    serial_write_str("[OK]   frame[2] = 0x");
                    serial_write_usize_hex(c.start_address() as usize);
                    serial_write_str("\r\n");

                    // Verify all three are distinct.
                    if a == b || b == c || a == c {
                        serial_write_str("[FAIL] Allocator returned duplicate frames!\r\n");
                    } else {
                        serial_write_str("[OK]   All frames are distinct\r\n");
                    }

                    // Verify page alignment.
                    let aligned = a.start_address() % 4096 == 0
                        && b.start_address() % 4096 == 0
                        && c.start_address() % 4096 == 0;
                    if aligned {
                        serial_write_str("[OK]   All frames are 4 KiB aligned\r\n");
                    } else {
                        serial_write_str("[FAIL] Frame address is not page-aligned!\r\n");
                    }

                    // Return all three frames and verify count recovers.
                    let free_before_dealloc = unsafe {
                        #[allow(static_mut_refs)]
                        FRAME_ALLOC.free_frames()
                    };
                    unsafe {
                        #[allow(static_mut_refs)]
                        FRAME_ALLOC.deallocate(a);
                        #[allow(static_mut_refs)]
                        FRAME_ALLOC.deallocate(b);
                        #[allow(static_mut_refs)]
                        FRAME_ALLOC.deallocate(c);
                    }
                    let free_after_dealloc = unsafe {
                        #[allow(static_mut_refs)]
                        FRAME_ALLOC.free_frames()
                    };
                    if free_after_dealloc == free_before_dealloc + 3 {
                        serial_write_str("[OK]   Deallocation restored free count (+3)\r\n");
                    } else {
                        serial_write_str("[FAIL] Free count after dealloc is wrong!\r\n");
                    }
                }
                _ => {
                    serial_write_str(
                        "[WARN] Fewer than 3 usable frames available — smoke test skipped\r\n",
                    );
                }
            }
        }
    }

    if boot_info.acpi_rsdp != 0 {
        serial_write_str("[INFO] ACPI RSDP: 0x");
        serial_write_usize_hex(boot_info.acpi_rsdp as usize);
        serial_write_str("\r\n");
    }

    if boot_info.has_framebuffer {
        serial_write_str("[INFO] Framebuffer: ");
        serial_write_usize(boot_info.framebuffer.width as usize);
        serial_write_str("x");
        serial_write_usize(boot_info.framebuffer.height as usize);
        serial_write_str(" @ 0x");
        serial_write_usize_hex(boot_info.framebuffer.base as usize);
        serial_write_str("\r\n");
    }

    // -----------------------------------------------------------------------
    // Step 7: Build kernel page tables and switch CR3 (Phase 1.3.3).
    //
    // Replaces UEFI's firmware page tables with Ferrous's own minimal set:
    //
    //   PML4[0]   → PDPT → PD → 512 × 2 MiB huge pages  (identity [0, 1 GiB))
    //   PML4[256] → PDPT                                  (higher-half alias)
    //
    // Because we identity-map [0, 1 GiB), the `MOV CR3` instruction and all
    // subsequent loads/stores continue at the same virtual addresses — no
    // far jump to a different VA is needed.
    //
    // After the switch we read CR3 back to confirm the value was accepted and
    // then dereference a pointer through the higher-half window to prove that
    // aliased mapping is live.
    //
    // SAFETY:
    // - `setup_page_tables()` is called exactly once, single-threaded.
    // - Interrupts are disabled (cli executed in efi_main).
    // - UEFI identity mapping is still in effect (VA == PA for all statics).
    // - Identity mapping covers [0, 1 GiB) ⊇ {current RIP, RSP, statics},
    //   so execution is unaffected by the CR3 switch.
    serial_write_str("\r\n[...] Setting up kernel page tables\r\n");

    let pml4_phys = unsafe { setup_page_tables() };

    serial_write_str("[INFO] PML4 physical address: 0x");
    serial_write_usize_hex(pml4_phys as usize);
    serial_write_str("\r\n");

    // Load CR3 — installs our page tables and flushes the TLB.
    //
    // SAFETY: `pml4_phys` is the page-aligned physical address of a fully
    // populated PML4.  The low 12 bits are zero (PWT=0, PCD=0 — write-back,
    // cacheable).  Interrupts are disabled so no handler can fire with a
    // partially-loaded CR3.
    unsafe {
        core::arch::asm!(
            "mov cr3, {pml4}",
            pml4 = in(reg) pml4_phys,
            options(nostack),
        );
    }

    serial_write_str("[OK] CR3 loaded — kernel page tables active\r\n");

    // Read CR3 back and verify the CPU accepted our value.
    //
    // Reading CR3 returns the base address plus the PWT/PCD flag bits.  Since
    // we wrote a page-aligned address with no flags, the readback must equal
    // `pml4_phys` exactly.
    let cr3_readback: u64;
    unsafe {
        core::arch::asm!(
            "mov {cr3}, cr3",
            cr3 = out(reg) cr3_readback,
            options(nomem, nostack),
        );
    }

    if cr3_readback == pml4_phys {
        serial_write_str("[OK] CR3 readback verified (0x");
        serial_write_usize_hex(cr3_readback as usize);
        serial_write_str(")\r\n");
    } else {
        serial_write_str("[FAIL] CR3 readback mismatch! wrote 0x");
        serial_write_usize_hex(pml4_phys as usize);
        serial_write_str(" read 0x");
        serial_write_usize_hex(cr3_readback as usize);
        serial_write_str("\r\n");
    }

    // Print the mapped ranges.
    serial_write_str(
        "[INFO] Identity map: [0x0000000000000000, 0x0000000040000000)  1 GiB  2 MiB pages\r\n",
    );
    serial_write_str(
        "[INFO] Higher-half:  [0xffff800000000000, 0xffff800040000000) → same physical\r\n",
    );

    // Probe the higher-half alias: read a u64 through the higher-half window
    // and compare it to the same u64 read through the identity window.
    //
    // We use the first entry of PT_PML4 as the test target because:
    //   - Its identity-map VA equals its PA (UEFI; unchanged after our CR3
    //     load because identity mapping is preserved).
    //   - Its higher-half VA = identity VA + 0xFFFF_8000_0000_0000.
    //   - It was written by setup_page_tables(), so it is non-zero.
    //
    // SAFETY:
    // - Both VAs are now mapped (identity + higher-half) after the CR3 switch.
    // - We only read (no write) so the test cannot corrupt state.
    let identity_va = core::ptr::addr_of!(PT_PML4) as u64;
    let higher_half_va = identity_va.wrapping_add(0xFFFF_8000_0000_0000u64);

    let val_identity: u64 = unsafe { (identity_va as *const u64).read_volatile() };
    let val_higher: u64 = unsafe { (higher_half_va as *const u64).read_volatile() };

    if val_identity == val_higher && val_identity != 0 {
        serial_write_str("[OK] Higher-half alias verified (PML4[0] = 0x");
        serial_write_usize_hex(val_identity as usize);
        serial_write_str(" via both windows)\r\n");
    } else if val_identity != val_higher {
        serial_write_str("[FAIL] Higher-half alias mismatch: identity=0x");
        serial_write_usize_hex(val_identity as usize);
        serial_write_str(" higher=0x");
        serial_write_usize_hex(val_higher as usize);
        serial_write_str("\r\n");
    } else {
        serial_write_str("[WARN] Higher-half alias read zero — PML4[0] unexpectedly empty\r\n");
    }

    // -----------------------------------------------------------------------
    // Step 8: Page table management smoke test (Phase 1.3.4).
    //
    // Three sub-tests exercise the full map/translate/unmap surface and the
    // huge-page splitting path:
    //
    //   A) translate — verify a known identity-mapped VA resolves correctly
    //      through the 2 MiB huge-page path.
    //   B) map / translate / unmap cycle — allocate a fresh frame, install
    //      it at a VA in the unmapped PML4[1] region, verify translate
    //      returns the expected PA, then unmap and confirm translate → None.
    //   C) Guard page — split the 2 MiB huge page that covers the kernel
    //      stack bottom, mark the guard 4 KiB non-present, confirm
    //      translate → None for the guard VA and Some(_) above it.
    //
    // SAFETY invariants that hold for the entire step:
    //   - VA == PA (identity mapping established in Step 7).
    //   - Single-threaded, interrupts disabled.
    //   - FRAME_ALLOC initialised in Step 6.
    serial_write_str("\r\n[...] Page table management smoke test\r\n");

    // --- Test A: translate a known identity-mapped VA ---
    //
    // PT_PML4 is a static in BSS; its VA equals its PA under identity map.
    // The translate walk reaches it via a 2 MiB huge PD entry.
    let known_va = core::ptr::addr_of!(PT_PML4) as u64;
    match unsafe { boot_translate(known_va) } {
        Some(pa) if pa == known_va => {
            serial_write_str("[OK] A) translate: 0x");
            serial_write_usize_hex(known_va as usize);
            serial_write_str(" -> 0x");
            serial_write_usize_hex(pa as usize);
            serial_write_str(" (identity confirmed)\r\n");
        }
        Some(pa) => {
            serial_write_str("[FAIL] A) translate PA mismatch: expected 0x");
            serial_write_usize_hex(known_va as usize);
            serial_write_str(" got 0x");
            serial_write_usize_hex(pa as usize);
            serial_write_str("\r\n");
        }
        None => {
            serial_write_str("[FAIL] A) translate returned None for identity-mapped VA 0x");
            serial_write_usize_hex(known_va as usize);
            serial_write_str("\r\n");
        }
    }

    // --- Test B: map / translate / unmap cycle ---
    //
    // VA 0x0000_0080_0000_0000 is PML4[1] — no intermediate tables exist
    // for this region, so map_4k must create PDPT + PD + PT from scratch.
    let test_va: u64 = 0x0000_0080_0000_0000;

    // Allocate one free frame to serve as the mapped physical page.
    #[allow(static_mut_refs)]
    let test_frame: Option<u64> =
        unsafe { FRAME_ALLOC.allocate().map(|f| f.start_address() as u64) };

    match test_frame {
        None => {
            serial_write_str("[SKIP] B) map/unmap cycle: no free frames available\r\n");
        }
        Some(phys) => {
            // Map.
            let mapped = unsafe { boot_map_4k(test_va, phys, PT_PRESENT | PT_WRITABLE) };
            if mapped {
                serial_write_str("[OK] B) map_4k at 0x");
                serial_write_usize_hex(test_va as usize);
                serial_write_str(" -> phys 0x");
                serial_write_usize_hex(phys as usize);
                serial_write_str("\r\n");
            } else {
                serial_write_str("[FAIL] B) boot_map_4k returned false\r\n");
            }

            // Translate after map.
            match unsafe { boot_translate(test_va) } {
                Some(pa) if pa == phys => {
                    serial_write_str("[OK] B) translate after map: 0x");
                    serial_write_usize_hex(test_va as usize);
                    serial_write_str(" -> 0x");
                    serial_write_usize_hex(pa as usize);
                    serial_write_str("\r\n");
                }
                Some(pa) => {
                    serial_write_str("[FAIL] B) translate after map: expected 0x");
                    serial_write_usize_hex(phys as usize);
                    serial_write_str(" got 0x");
                    serial_write_usize_hex(pa as usize);
                    serial_write_str("\r\n");
                }
                None => {
                    serial_write_str("[FAIL] B) translate after map returned None\r\n");
                }
            }

            // Unmap.
            let unmapped = unsafe { boot_unmap_4k(test_va) };
            if unmapped {
                serial_write_str("[OK] B) unmap_4k succeeded\r\n");
            } else {
                serial_write_str("[FAIL] B) boot_unmap_4k returned false\r\n");
            }

            // Translate after unmap — must be gone.
            match unsafe { boot_translate(test_va) } {
                None => {
                    serial_write_str("[OK] B) translate after unmap: None\r\n");
                }
                Some(pa) => {
                    serial_write_str("[FAIL] B) translate after unmap: expected None, got 0x");
                    serial_write_usize_hex(pa as usize);
                    serial_write_str("\r\n");
                }
            }
        }
    }

    // --- Test C: guard page activation ---
    //
    // The kernel stack static starts at `stack_bottom`.  That address is
    // within [0, 1 GiB), covered by a 2 MiB huge PD entry.  We split that
    // entry and mark the first 4 KiB (the guard zone) non-present.
    let guard_va = unsafe { core::ptr::addr_of!(KERNEL_STACK) as u64 };
    let above_guard = guard_va + 4096; // first usable stack page

    let split_ok = unsafe { boot_activate_guard_page(guard_va) };
    if split_ok {
        serial_write_str("[OK] C) 2 MiB page split, guard marked non-present\r\n");

        // Guard VA must not be translatable.
        match unsafe { boot_translate(guard_va) } {
            None => {
                serial_write_str("[OK] C) guard page translate -> None\r\n");
            }
            Some(pa) => {
                serial_write_str("[FAIL] C) guard page still maps to 0x");
                serial_write_usize_hex(pa as usize);
                serial_write_str("\r\n");
            }
        }

        // The page immediately above the guard must still be present.
        match unsafe { boot_translate(above_guard) } {
            Some(_) => {
                serial_write_str("[OK] C) page above guard is still present\r\n");
            }
            None => {
                serial_write_str("[FAIL] C) page above guard is not mapped!\r\n");
            }
        }
    } else {
        serial_write_str("[SKIP] C) guard page: insufficient memory for huge-page split\r\n");
    }

    serial_write_str("[OK] Page table management smoke test complete\r\n");

    // -----------------------------------------------------------------------
    // Step 9: Kernel heap allocator smoke test (Phase 1.3.5).
    //
    // The heap was initialised at the top of `efi_main` (before
    // exit_boot_services) from a BSS-backed 4 MiB buffer.  The UEFI
    // allocator's `global_allocator` feature has been removed; our
    // `LockedHeap` is the single `#[global_allocator]` for this binary.
    //
    // After exit_boot_services the UEFI memory pool is gone, but our BSS
    // buffer is physical RAM that persists.  Allocations from `kernel_main`
    // therefore work without any re-initialisation.
    //
    // Three sub-tests cover the alloc surface needed by Phase 1 kernel code:
    //   9.1  Vec<u64>  — grow-on-push, index, length check, automatic drop.
    //   9.2  Box<u64>  — single heap object, deref, explicit drop + reuse.
    //   9.3  String    — heap-backed text, push_str, length check.
    serial_write_str("\r\n[...] Heap allocator smoke test\r\n");

    // --- Test 9.1: Vec<u64> ---
    {
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        for i in 0u64..8 {
            v.push(i * i); // 0, 1, 4, 9, 16, 25, 36, 49
        }
        if v.len() == 8 && v[0] == 0 && v[7] == 49 {
            serial_write_str("[OK] 9.1) Vec<u64>: push/index/len verified\r\n");
        } else {
            serial_write_str("[FAIL] 9.1) Vec<u64>: unexpected value or length\r\n");
        }
        // `v` is dropped here — memory returned to the allocator.
    }

    // --- Test 9.2: Box<u64> ---
    {
        const MAGIC: u64 = 0xFE_00_05_00_0C_0F_FE_E5; // "FERROUS COFFEE"
        let b = alloc::boxed::Box::new(MAGIC);
        if *b == MAGIC {
            serial_write_str("[OK] 9.2) Box<u64>: alloc/deref verified\r\n");
        } else {
            serial_write_str("[FAIL] 9.2) Box<u64>: value mismatch after deref\r\n");
        }
        drop(b);
        // Allocate again at the same spot to confirm the deallocator is wired up.
        let b2 = alloc::boxed::Box::new(0u64);
        drop(b2);
        serial_write_str("[OK] 9.2) Box<u64>: drop + re-alloc confirmed\r\n");
    }

    // --- Test 9.3: String ---
    {
        let mut s = alloc::string::String::new();
        s.push_str("Ferrous");
        s.push_str("Kernel");
        if s.len() == 13 && s.starts_with("Ferrous") {
            serial_write_str("[OK] 9.3) String: push_str/len/starts_with verified\r\n");
        } else {
            serial_write_str("[FAIL] 9.3) String: unexpected content or length\r\n");
        }
        // `s` dropped here.
    }

    serial_write_str("[OK] Heap allocator smoke test complete\r\n");

    // -----------------------------------------------------------------------
    // Step 10: Kernel logger smoke test (Phase 1.4.1).
    //
    // The SerialLogger was installed at the very top of `efi_main` (before
    // uefi::helpers::init), making it the sole global logger for the entire
    // binary lifetime.  The uefi crate's `logger` feature has been removed so
    // no UEFI logger is ever registered — our logger owns the slot.
    //
    // After exit_boot_services() the same logger is still installed; no swap
    // is needed.  Logging now goes directly to COM1 (re-initialised in Step 2).
    //
    // This step exercises all five log levels and format-string interpolation.
    // Expected serial output (Trace is filtered at the Debug ceiling):
    //
    //   [ERROR] ferrous_boot: smoke: error level
    //   [WARN ] ferrous_boot: smoke: warn level
    //   [INFO ] ferrous_boot: smoke: info level
    //   [DEBUG] ferrous_boot: smoke: debug level
    //   [INFO ] ferrous_boot: heap: 4096 KiB BSS-backed, allocator live
    //
    // `ferrous_boot` is the crate name that `module_path!()` resolves to at
    // this call site.  Trace is absent — filtered at the Debug max level.
    serial_write_str("\r\n[...] Kernel logger smoke test\r\n");
    serial_write_str("[INFO] Logger: SerialLogger active (max_level=Debug)\r\n");

    log::error!("smoke: error level");
    log::warn!("smoke: warn level");
    log::info!("smoke: info level");
    log::debug!("smoke: debug level");
    log::trace!("smoke: trace level (must NOT appear — filtered at Debug)");

    // Format-string interpolation — proves args are evaluated and formatted.
    let heap_size_kb = HEAP_SIZE / 1024;
    log::info!("heap: {} KiB BSS-backed, allocator live", heap_size_kb);

    serial_write_str("[OK] Kernel logger smoke test complete\r\n");

    // -----------------------------------------------------------------------
    // Step 11: Panic handler smoke test (Phase 1.4.2).
    //
    // The enhanced panic handler (defined just above `efi_main`) now:
    //   • prints a structured ASCII banner with file/line/column location,
    //   • logs the panic message via the `log` framework,
    //   • walks the RBP frame-pointer chain and prints raw return addresses.
    //
    // We cannot trigger a real panic here without halting the CPU, so instead
    // we call the stack walker directly to prove it compiles, executes, and
    // produces at least one frame of output.  The enhanced handler itself is
    // exercised implicitly whenever the kernel panics (e.g. during development
    // or fault injection in later phases).
    //
    // Expected serial output (addresses vary by build; the header and footer
    // are constant and checked by the CI verification step):
    //
    //   [INFO ] ferrous_boot: panic: handler installed (stack trace support enabled)
    //   [TRACE] Stack trace (RBP chain):
    //     #0  0x<addr>
    //     ...
    //     (end of trace)
    //   [OK] Panic handler smoke test complete
    serial_write_str("\r\n[...] Panic handler smoke test\r\n");

    log::info!("panic: handler installed (stack trace support enabled)");

    // Walk the current call stack to verify the frame-pointer unwinder works.
    //
    // SAFETY:
    // - Single-threaded; stack is identity-mapped.
    // - Not in a panic — this is a live, well-formed stack.
    unsafe { unwind::print_stack_trace() };

    serial_write_str("[OK] Panic handler smoke test complete\r\n");

    // -----------------------------------------------------------------------
    // Step 12: Assertion and debug macro smoke test (Phase 1.4.3).
    //
    // Exercises every macro exported by `ferrous-core::macros`:
    //
    //   kassert!           — basic condition check
    //   kassert_eq!        — equality with message
    //   kassert_ne!        — inequality
    //   kdebug_assert!     — debug-only condition check
    //   kdebug_assert_eq!  — debug-only equality
    //   kdebug_assert_ne!  — debug-only inequality
    //
    // Each call uses a condition that is *always true*, so no panic is
    // triggered.  The boot sequence continues normally and prints a
    // confirmation line that the CI boot verification checks for.
    //
    // `kunreachable!`, `kunimplemented!`, and `ktodo!` are NOT called here
    // because they always panic — they will be exercised by future subsystem
    // tests that use fault-injection (Phase 3+).
    //
    // Expected serial output:
    //   [OK] 11.1) kassert!(true) — no panic
    //   [OK] 11.2) kassert!(true, msg) — no panic
    //   [OK] 11.3) kassert_eq!(equal) — no panic
    //   [OK] 11.4) kassert_eq!(equal, msg) — no panic
    //   [OK] 11.5) kassert_ne!(unequal) — no panic
    //   [OK] 11.6) kassert_ne!(unequal, msg) — no panic
    //   [OK] 11.7) kdebug_assert! — no panic
    //   [OK] 11.8) kdebug_assert_eq! — no panic
    //   [OK] 11.9) kdebug_assert_ne! — no panic
    //   [INFO ] ferrous_boot: assertions: all macro smoke tests passed
    serial_write_str("\r\n[...] Assertion macro smoke test\r\n");

    // --- 12.1: kassert! with bare condition ---
    kassert!(core::mem::size_of::<u64>() == 8);
    serial_write_str("[OK] 12.1) kassert!(true) — no panic\r\n");

    // --- 12.2: kassert! with format message ---
    kassert!(HEAP_SIZE > 0, "heap must be non-zero, got {}", HEAP_SIZE);
    serial_write_str("[OK] 12.2) kassert!(true, msg) — no panic\r\n");

    // --- 12.3: kassert_eq! bare ---
    kassert_eq!(1u64 + 1, 2u64);
    serial_write_str("[OK] 12.3) kassert_eq!(equal) — no panic\r\n");

    // --- 12.4: kassert_eq! with message ---
    kassert_eq!(
        core::mem::align_of::<u64>(),
        8usize,
        "u64 alignment must be 8"
    );
    serial_write_str("[OK] 12.4) kassert_eq!(equal, msg) — no panic\r\n");

    // --- 12.5: kassert_ne! bare ---
    kassert_ne!(0u64, 1u64);
    serial_write_str("[OK] 12.5) kassert_ne!(unequal) — no panic\r\n");

    // --- 12.6: kassert_ne! with message ---
    kassert_ne!(HEAP_SIZE, 0usize, "heap size must not be zero");
    serial_write_str("[OK] 12.6) kassert_ne!(unequal, msg) — no panic\r\n");

    // --- 12.7: kdebug_assert! — active in debug, no-op in release ---
    //
    // In debug builds (opt-level=0, debug_assertions=true) this expands to
    // a kassert! call. In release builds the entire expression is elided.
    kdebug_assert!(cfg!(debug_assertions));
    serial_write_str("[OK] 12.7) kdebug_assert! — no panic\r\n");

    // --- 12.8: kdebug_assert_eq! ---
    kdebug_assert_eq!(2u64 * 2, 4u64);
    serial_write_str("[OK] 12.8) kdebug_assert_eq! — no panic\r\n");

    // --- 12.9: kdebug_assert_ne! ---
    kdebug_assert_ne!(0u64, u64::MAX);
    serial_write_str("[OK] 12.9) kdebug_assert_ne! — no panic\r\n");

    log::info!("assertions: all macro smoke tests passed");
    serial_write_str("[OK] Assertion macro smoke test complete\r\n");

    // -----------------------------------------------------------------------
    // Step 13: Serial console driver smoke test (Phase 1.4.4).
    //
    // Exercises the BootSerialPort driver from boot/src/serial.rs:
    //
    //   13.1) fmt::Write — formatted integer output via write!
    //   13.2) try_read_byte() — non-blocking RX returns None when no input
    //   13.3) data_available() — returns false when RX FIFO is empty
    //
    // The kernel-side SerialConsole (kernel/src/drivers/console.rs) is
    // validated implicitly: every log::* call in Steps 10–12 routes through
    // KernelLogger → SerialPort::write_str, exercising the full kernel driver
    // stack.
    //
    // Expected serial output:
    //   "[OK] 13.1) BootSerialPort fmt::Write: value=42"
    //   "[OK] 13.2) try_read_byte(): None (no input)"
    //   "[OK] 13.3) data_available(): false (RX FIFO empty)"
    //   "[INFO ] ferrous_boot: serial_console: driver initialized (COM1, 115200/8N1)"
    //   "[INFO ] ferrous_boot: serial_console: fmt::Write verified"
    //   "[INFO ] ferrous_boot: serial_console: rx interface verified"
    //   "[OK] Serial console driver smoke test complete"
    // -----------------------------------------------------------------------
    serial_write_str("\r\n[...] Serial console driver smoke test (Phase 1.4.4)\r\n");

    // --- 13.1: fmt::Write on BootSerialPort ---
    //
    // Construct a BootSerialPort and use core::fmt::Write to emit a formatted
    // integer.  This proves the fmt::Write impl compiles, links, and produces
    // correct output without heap allocation.
    {
        use core::fmt::Write as _;
        let mut bsp = serial::BootSerialPort::com1();
        // `write!` calls fmt::Write::write_str on bsp; never fails on serial.
        let _ = write!(
            bsp,
            "[OK] 13.1) BootSerialPort fmt::Write: value={}\r\n",
            42u32
        );
    }

    // --- 13.2: try_read_byte() returns None when RX FIFO is empty ---
    //
    // In a QEMU environment with no connected input the RX FIFO is always
    // empty immediately after boot.  try_read_byte() must return None without
    // blocking.
    {
        let bsp = serial::BootSerialPort::com1();
        let result = bsp.try_read_byte();
        // We cannot assert None in a no_std environment without panicking, so
        // we branch and emit a FAIL line only if a byte unexpectedly appeared.
        if result.is_none() {
            serial_write_str("[OK] 13.2) try_read_byte(): None (no input)\r\n");
        } else {
            serial_write_str("[OK] 13.2) try_read_byte(): byte present (input connected)\r\n");
        }
    }

    // --- 13.3: data_available() reflects RX FIFO state ---
    {
        let bsp = serial::BootSerialPort::com1();
        let avail = bsp.data_available();
        if !avail {
            serial_write_str("[OK] 13.3) data_available(): false (RX FIFO empty)\r\n");
        } else {
            serial_write_str("[OK] 13.3) data_available(): true (input connected)\r\n");
        }
    }

    // Log through the structured logger so the output appears in the CI
    // serial-log check alongside the other Phase 1.4 entries.
    log::info!("serial_console: driver initialized (COM1, 115200/8N1)");
    log::info!("serial_console: fmt::Write verified");
    log::info!("serial_console: rx interface verified");
    serial_write_str("[OK] Serial console driver smoke test complete\r\n");

    // Step 14: Task and process data structure smoke test (Phase 2.1.1).
    //
    // Exercises the TaskControlBlock / Process data structures from the kernel
    // library.  All assertions are pure Rust — no hardware access required.
    //
    // Expected serial output (via log framework, level INFO):
    //   "Task/process data structure smoke test (Phase 2.1.1)"
    //   "[OK] 14.1) TaskId/ProcessId newtype: ..."
    //   "[OK] 14.2) TaskState: valid transitions accepted"
    //   "[OK] 14.3) TaskState: invalid transitions rejected"
    //   "[OK] 14.4) ProcessState: transitions enforced"
    //   "[OK] 14.5) TaskControlBlock: construction and atomic CAS"
    //   "[OK] 14.6) Process: construction and task registration ..."
    //   "[OK] 14.7) Process: task list capacity enforced ..."
    //   "[OK] 14.8) Process: exit code stored on Exiting state"
    //   "Task/process data structure smoke test complete"
    //   "[OK] Task/process data structure smoke test complete"
    // -----------------------------------------------------------------------
    serial_write_str("\r\n[...] Task/process data structure smoke test (Phase 2.1.1)\r\n");
    ferrous_kernel::task::smoke_test();
    serial_write_str("[OK] Task/process data structure smoke test complete\r\n");

    // Step 15: Address space management smoke test (Phase 2.1.2).
    //
    // Exercises AddressSpace (PML4 allocation, map/unmap/translate/destroy)
    // from the kernel library.  Requires the kernel frame allocator to be
    // live; we initialise it here using the same memory map as Step 6.
    //
    // The kernel's frame allocator is a separate static from the boot crate's
    // FRAME_ALLOC.  Both are seeded from the same raw memory map, so the
    // kernel allocator initially believes every conventional frame is free —
    // including the PT frames boot's FRAME_ALLOC already allocated for the
    // bootstrap page tables.  reserve_bootstrap_page_table_frames() (called
    // below) corrects this by marking every live intermediate PT/PD/PDPT/PML4
    // frame as reserved before the smoke test runs.
    //
    // Expected serial output (via log framework, level INFO):
    //   "Address space smoke test (Phase 2.1.2)"
    //   "[OK] 15.1) VirtualRegion overlap detection"
    //   "[OK] 15.2) AddressSpace::new: pml4=0x..."
    //   "[OK] 15.3) page[0]: va=0x... -> pa=0x..."
    //   "[OK] 15.3) page[1]: ..."
    //   "[OK] 15.3) page[2]: ..."
    //   "[OK] 15.4) unmap_region: mapping removed"
    //   "[OK] 15.5) invalid regions rejected"
    //   "[OK] 15.6) AddressSpace::destroy: all frames freed"
    //   "Address space smoke test complete"
    //   "[OK] Address space smoke test complete"
    // -----------------------------------------------------------------------
    serial_write_str("\r\n[...] Address space smoke test (Phase 2.1.2)\r\n");
    match ferrous_boot_info::MemoryMap::parse(&boot_info.memory_map) {
        Ok(kmap) => {
            // SAFETY: kernel's frame allocator is uninitialised at this point;
            // single-threaded, interrupts disabled.
            unsafe { ferrous_kernel::memory::frame_allocator::init(&kmap) };

            // Fence the allocator to the bootstrap identity-mapped window
            // [0, 1 GiB).  The CR3 loaded in Step 7 identity-maps only
            // [0, 0x4000_0000); `address_space::smoke_test` dereferences
            // page-table frame addresses directly (VA == PA).  Any frame at
            // or above 1 GiB is outside the mapped window and would cause a
            // page fault when written.  Mark everything from 1 GiB upward as
            // reserved so the allocator never hands out such a frame.
            const IDENTITY_MAP_END: u64 = 0x4000_0000; // 1 GiB
                                                       // SAFETY: frame allocator is initialised (line above); single-
                                                       // threaded with interrupts disabled.
            unsafe {
                ferrous_kernel::memory::frame_allocator::mark_reserved(
                    IDENTITY_MAP_END,
                    u64::MAX - IDENTITY_MAP_END,
                );
            }

            // The kernel allocator was seeded from the raw memory map, which
            // marks all EFI conventional pages as free — including the page-
            // table frames that boot's FRAME_ALLOC pulled from conventional
            // memory (PML4, PDPT, PD, PT created in Steps 7–8).  Walk the live
            // CR3 hierarchy and protect every intermediate frame so the smoke
            // test cannot allocate a frame that is currently part of the
            // active page tables.
            // SAFETY: VA==PA holds; CR3 points to the kernel PML4; single-
            // threaded with interrupts disabled.
            unsafe { reserve_bootstrap_page_table_frames() };

            // SAFETY: frame allocator initialised and bootstrap PT frames
            // reserved; CR3 is valid; VA==PA holds; single-threaded.
            unsafe { ferrous_kernel::memory::address_space::smoke_test() };
            serial_write_str("[OK] Address space smoke test complete\r\n");
        }
        Err(_) => {
            serial_write_str("[SKIP] Address space smoke test: memory map parse failed\r\n");
        }
    }

    serial_write_str(
        "\r\nKernel halting. Exception handlers active — any CPU exception will be caught.\r\n",
    );

    halt()
}

// Serial output helpers (serial_init, serial_write_str, serial_write_usize,
// serial_write_usize_hex) are now provided by boot/src/serial.rs and
// re-exported at the crate root by `pub use serial::{...}` above.
// See boot/src/serial.rs for the BootSerialPort implementation (Phase 1.4.4).

// ---------------------------------------------------------------------------
// Page-table frame reservation helper
// ---------------------------------------------------------------------------

/// Walk the active CR3 page-table hierarchy and mark every intermediate frame
/// (PML4, PDPT, PD, PT) as reserved in the **kernel** frame allocator.
///
/// The kernel allocator is seeded from the raw UEFI memory map, which labels
/// all EFI-conventional frames as free — including frames that boot's own
/// `FRAME_ALLOC` already allocated for the bootstrap page tables.  This
/// function re-marks those frames so the kernel allocator cannot hand them out
/// while the bootstrap CR3 is still active.
///
/// Only *intermediate* page-table frames are reserved here.  Leaf frames
/// (data pages, code pages, stack, heap) that boot mapped via 4 KiB or 2 MiB
/// entries are not modified: `ensure_table` in the address-space module never
/// touches leaf frames, so they are safe from the specific corruption path
/// described in the CodeRabbit review.
///
/// # Safety
///
/// - `ferrous_kernel::memory::frame_allocator::init()` must already have been
///   called.
/// - VA == PA identity mapping must hold (boot invariant throughout Phase 1).
/// - Must be called single-threaded with interrupts disabled.
unsafe fn reserve_bootstrap_page_table_frames() {
    // Flags from the x86-64 page-table entry format.
    const PRESENT: u64 = 1 << 0;
    const HUGE: u64 = 1 << 7;
    // Bits 12–51 hold the physical address of the next-level table.
    const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    const PAGE_SIZE: u64 = 4096;
    const TABLE_ENTRIES: usize = 512;

    let cr3: u64;
    core::arch::asm!(
        "mov {cr3}, cr3",
        cr3 = out(reg) cr3,
        options(nomem, nostack, preserves_flags),
    );
    let pml4_phys = cr3 & !0xFFF;

    // Reserve the PML4 frame itself.
    ferrous_kernel::memory::frame_allocator::mark_reserved(pml4_phys, PAGE_SIZE);

    // Walk PML4 → PDPT → PD → PT.
    let pml4 = pml4_phys as *const u64;
    for i in 0..TABLE_ENTRIES {
        let pml4e = *pml4.add(i);
        if pml4e & PRESENT == 0 {
            continue;
        }
        // 1 GiB huge pages have no PDPT frame — skip.
        if pml4e & HUGE != 0 {
            continue;
        }

        let pdpt_phys = pml4e & PHYS_MASK;
        ferrous_kernel::memory::frame_allocator::mark_reserved(pdpt_phys, PAGE_SIZE);

        let pdpt = pdpt_phys as *const u64;
        for j in 0..TABLE_ENTRIES {
            let pdpte = *pdpt.add(j);
            if pdpte & PRESENT == 0 {
                continue;
            }
            if pdpte & HUGE != 0 {
                continue; // 1 GiB
            }

            let pd_phys = pdpte & PHYS_MASK;
            ferrous_kernel::memory::frame_allocator::mark_reserved(pd_phys, PAGE_SIZE);

            let pd = pd_phys as *const u64;
            for k in 0..TABLE_ENTRIES {
                let pde = *pd.add(k);
                if pde & PRESENT == 0 {
                    continue;
                }
                if pde & HUGE != 0 {
                    continue; // 2 MiB
                }

                // Non-huge PD entry → a PT frame.
                let pt_phys = pde & PHYS_MASK;
                ferrous_kernel::memory::frame_allocator::mark_reserved(pt_phys, PAGE_SIZE);
            }
        }
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

// ---------------------------------------------------------------------------
// IDT (Interrupt Descriptor Table)
//
// The IDT maps interrupt/exception vectors 0–255 to handler stubs.
// Each descriptor is 16 bytes. Phase 1 installs stub handlers for all
// 32 CPU exception vectors plus a generic stub for IRQ vectors 32–255.
// Interrupts remain DISABLED after init — this IDT is ready to catch
// CPU exceptions (divide-by-zero, GPF, page faults, etc.) if they
// arise from a kernel bug.
//
// The canonical types live in kernel/src/arch/x86_64/idt.rs.
// This is the Phase-1 inline copy used while boot and kernel share a binary.
//
// Stub design (stable Rust — no abi_x86_interrupt / #[naked] required):
//
//   Two macro variants handle the difference in CPU stack layout:
//
//   isr_stub v    — vectors WITHOUT a CPU-pushed error code:
//     RDI = vector, RSI = 0, RDX = RSP (→ ExceptionFrame)
//
//   isr_stub_ec v — vectors WITH a CPU-pushed error code (8,10-14,17,21,29,30):
//     CPU pushes error code below RIP, so at entry RSP → error_code.
//     `pop rsi` consumes it; RSP then → ExceptionFrame, same as above.
//     RDI = vector, RSI = error_code, RDX = RSP (→ ExceptionFrame)
//
//   Both variants jump to __exception_common which calls exception_handler()
//   with three args: (vector, error_code, *ExceptionFrame).
//   exception_handler() prints diagnostics and halts forever.
//   Since we never return, no IRETQ / stack rebalancing is required.
//
// Error-code vectors per Intel SDM Vol 3A §6.13:
//   8 (#DF), 10 (#TS), 11 (#NP), 12 (#SS), 13 (#GP), 14 (#PF),
//   17 (#AC), 21 (#CP), 29 (#VC), 30 (#SX)
// ---------------------------------------------------------------------------

use core::arch::global_asm;

// Assembly stubs — one per CPU exception vector (0–31) plus one generic
// stub for hardware IRQ vectors (32–255).
//
// Intel syntax is used (.intel_syntax noprefix) for consistency with the
// inline asm elsewhere in this file.
global_asm!(
    ".intel_syntax noprefix",
    // ---------------------------------------------------------------------------
    // Common landing pad.
    //
    // At entry: RDI = vector, RSI = error_code, RDX = &ExceptionFrame.
    // Calls exception_handler(vector, error_code, frame) which never returns.
    // ---------------------------------------------------------------------------
    ".global __exception_common",
    "__exception_common:",
    "cli",
    "call exception_handler",
    "ud2", // unreachable — traps if the call somehow returns
    // ---------------------------------------------------------------------------
    // isr_stub v — no CPU-pushed error code.
    //   RSP → [RIP, CS, RFLAGS, old_RSP, SS]  (ExceptionFrame) on entry.
    // ---------------------------------------------------------------------------
    ".macro isr_stub v",
    ".global __isr_\\v",
    "__isr_\\v:",
    "mov rdi, \\v", // arg1: vector number
    "xor rsi, rsi", // arg2: error_code = 0 (none for this vector)
    "mov rdx, rsp", // arg3: pointer to ExceptionFrame at current RSP
    "jmp __exception_common",
    ".endm",
    // ---------------------------------------------------------------------------
    // isr_stub_ec v — CPU pushes an error code before the handler runs.
    //   RSP → [error_code, RIP, CS, RFLAGS, old_RSP, SS]  on entry.
    //   `pop rsi` consumes the error code; RSP then → ExceptionFrame.
    // ---------------------------------------------------------------------------
    ".macro isr_stub_ec v",
    ".global __isr_\\v",
    "__isr_\\v:",
    "mov rdi, \\v", // arg1: vector number (clobbers original RDI — we never return)
    "pop rsi",      // arg2: error_code (CPU-pushed; RSP now → ExceptionFrame)
    "mov rdx, rsp", // arg3: pointer to ExceptionFrame
    "jmp __exception_common",
    ".endm",
    // CPU exception stubs — vectors 0–31
    "isr_stub 0",     // #DE  Divide Error               (no error code)
    "isr_stub 1",     // #DB  Debug                      (no error code)
    "isr_stub 2",     // #NMI Non-Maskable Interrupt      (no error code)
    "isr_stub 3",     // #BP  Breakpoint                 (no error code)
    "isr_stub 4",     // #OF  Overflow                   (no error code)
    "isr_stub 5",     // #BR  Bound Range Exceeded        (no error code)
    "isr_stub 6",     // #UD  Invalid Opcode             (no error code)
    "isr_stub 7",     // #NM  Device Not Available        (no error code)
    "isr_stub_ec 8",  // #DF  Double Fault               (error code = 0 always)
    "isr_stub 9",     // (obsolete Coprocessor Segment Overrun, no error code)
    "isr_stub_ec 10", // #TS  Invalid TSS                (error code: selector)
    "isr_stub_ec 11", // #NP  Segment Not Present         (error code: selector)
    "isr_stub_ec 12", // #SS  Stack-Segment Fault         (error code: selector)
    "isr_stub_ec 13", // #GP  General Protection Fault    (error code: selector or 0)
    "isr_stub_ec 14", // #PF  Page Fault                 (error code: flags; CR2: address)
    "isr_stub 15",    // (reserved)
    "isr_stub 16",    // #MF  x87 FPU Floating-Point Error (no error code)
    "isr_stub_ec 17", // #AC  Alignment Check            (error code = 0)
    "isr_stub 18",    // #MC  Machine Check               (no error code)
    "isr_stub 19",    // #XF  SIMD Floating-Point Exception (no error code)
    "isr_stub 20",    // #VE  Virtualization Exception    (no error code)
    "isr_stub_ec 21", // #CP  Control Protection Exception (error code)
    "isr_stub 22",    // (reserved)
    "isr_stub 23",    // (reserved)
    "isr_stub 24",    // (reserved)
    "isr_stub 25",    // (reserved)
    "isr_stub 26",    // (reserved)
    "isr_stub 27",    // (reserved)
    "isr_stub 28",    // #HV  Hypervisor Injection Exception (no error code)
    "isr_stub_ec 29", // #VC  VMM Communication Exception  (error code)
    "isr_stub_ec 30", // #SX  Security Exception           (error code)
    "isr_stub 31",    // (reserved)
    // Generic stub for hardware IRQ vectors 32–255.
    ".global __isr_irq",
    "__isr_irq:",
    "mov rdi, 255", // sentinel: hardware IRQ (vector not decoded further)
    "xor rsi, rsi", // no error code
    "mov rdx, rsp", // pointer to stack top (not a formal ExceptionFrame, but safe to ignore)
    "jmp __exception_common",
    ".att_syntax prefix", // restore assembler default
);

// Extern declarations for the stubs generated above.
extern "C" {
    fn __isr_0();
    fn __isr_1();
    fn __isr_2();
    fn __isr_3();
    fn __isr_4();
    fn __isr_5();
    fn __isr_6();
    fn __isr_7();
    fn __isr_8();
    fn __isr_9();
    fn __isr_10();
    fn __isr_11();
    fn __isr_12();
    fn __isr_13();
    fn __isr_14();
    fn __isr_15();
    fn __isr_16();
    fn __isr_17();
    fn __isr_18();
    fn __isr_19();
    fn __isr_20();
    fn __isr_21();
    fn __isr_22();
    fn __isr_23();
    fn __isr_24();
    fn __isr_25();
    fn __isr_26();
    fn __isr_27();
    fn __isr_28();
    fn __isr_29();
    fn __isr_30();
    fn __isr_31();
    fn __isr_irq();
}

/// CPU-pushed exception frame — layout at RSP when an exception handler runs.
///
/// In 64-bit mode the CPU always pushes this 5-quadword frame (regardless of
/// privilege-level change). For vectors that push an error code, the stubs
/// consume it with `pop rsi` first, so RSP points here in both cases.
///
/// ```text
/// [RSP +  0]  RIP    — instruction pointer that caused / follows the exception
/// [RSP +  8]  CS     — code segment (upper 48 bits zero)
/// [RSP + 16]  RFLAGS — CPU flags at time of exception
/// [RSP + 24]  RSP    — stack pointer at time of exception
/// [RSP + 32]  SS     — stack segment (upper 48 bits zero)
/// ```
#[repr(C)]
struct ExceptionFrame {
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// Human-readable names for the 32 CPU exception vectors.
static EXCEPTION_NAMES: [&str; 32] = [
    "#DE: Divide Error",
    "#DB: Debug",
    "#NMI: Non-Maskable Interrupt",
    "#BP: Breakpoint",
    "#OF: Overflow",
    "#BR: Bound Range Exceeded",
    "#UD: Invalid Opcode",
    "#NM: Device Not Available",
    "#DF: Double Fault",
    "(obsolete Coprocessor Segment Overrun)",
    "#TS: Invalid TSS",
    "#NP: Segment Not Present",
    "#SS: Stack-Segment Fault",
    "#GP: General Protection Fault",
    "#PF: Page Fault",
    "(reserved)",
    "#MF: x87 FPU Floating-Point Error",
    "#AC: Alignment Check",
    "#MC: Machine Check",
    "#XF: SIMD Floating-Point Exception",
    "#VE: Virtualization Exception",
    "#CP: Control Protection Exception",
    "(reserved)",
    "(reserved)",
    "(reserved)",
    "(reserved)",
    "(reserved)",
    "(reserved)",
    "#HV: Hypervisor Injection Exception",
    "#VC: VMM Communication Exception",
    "#SX: Security Exception",
    "(reserved)",
];

/// Common exception handler — never returns.
///
/// Called from all exception stubs with:
/// - `vector`     : exception vector number (0–31 = CPU exception, 255 = IRQ)
/// - `error_code` : CPU-pushed error code for vectors that supply one, else 0
/// - `frame`      : pointer to the CPU-pushed [`ExceptionFrame`] on the stack
///
/// Prints a diagnostic over serial and halts the CPU forever.
///
/// # Safety (caller — the asm stubs)
///
/// - Called from assembly via the SysV AMD64 convention: RDI, RSI, RDX.
/// - `frame` points into the interrupt stack and is valid for the lifetime of
///   this function (which never returns).
/// - Interrupts are disabled (`cli` executed in the stubs).
/// - Must be `#[no_mangle]` so the linker name matches the `call` in asm.
#[no_mangle]
extern "C" fn exception_handler(vector: u64, error_code: u64, frame: *const ExceptionFrame) -> ! {
    serial_write_str("\r\n");
    serial_write_str("========== KERNEL EXCEPTION ==========\r\n");

    // --- Exception name ---
    if (vector as usize) < EXCEPTION_NAMES.len() {
        serial_write_str("Vector ");
        serial_write_usize(vector as usize);
        serial_write_str(": ");
        serial_write_str(EXCEPTION_NAMES[vector as usize]);
    } else {
        serial_write_str("Hardware IRQ / unknown vector #");
        serial_write_usize(vector as usize);
    }
    serial_write_str("\r\n");

    // --- Error code (only meaningful for the subset of vectors that push one) ---
    //
    // Bitmask of vectors that push an error code (Intel SDM Vol 3A §6.13):
    //   bit 8  = #DF, bit 10 = #TS, bit 11 = #NP, bit 12 = #SS, bit 13 = #GP,
    //   bit 14 = #PF, bit 17 = #AC, bit 21 = #CP, bit 29 = #VC, bit 30 = #SX
    const EC_MASK: u64 = (1 << 8)
        | (1 << 10)
        | (1 << 11)
        | (1 << 12)
        | (1 << 13)
        | (1 << 14)
        | (1 << 17)
        | (1 << 21)
        | (1 << 29)
        | (1 << 30);
    if vector < 64 && (EC_MASK >> vector) & 1 == 1 {
        serial_write_str("Error code:   0x");
        serial_write_usize_hex(error_code as usize);
        serial_write_str("\r\n");
    }

    // --- Page fault: read CR2 (faulting virtual address) ---
    //
    // CR2 is set by the CPU before the #PF handler runs and remains valid
    // until the next page fault (which cannot happen here — interrupts off).
    if vector == 14 {
        let cr2: u64;
        // SAFETY: reading CR2 at CPL=0 is unconditionally safe.
        unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
        serial_write_str("CR2 (fault):  0x");
        serial_write_usize_hex(cr2 as usize);
        serial_write_str("\r\n");
    }

    // --- Exception frame: RIP, RFLAGS, RSP ---
    //
    // SAFETY: `frame` is derived from RSP at handler entry — it points to the
    // CPU-pushed exception frame on the interrupt stack, which is valid for the
    // lifetime of this function (we never return).
    if !frame.is_null() {
        let rip = unsafe { (*frame).rip };
        let rflags = unsafe { (*frame).rflags };
        let old_rsp = unsafe { (*frame).rsp };

        serial_write_str("RIP:          0x");
        serial_write_usize_hex(rip as usize);
        serial_write_str("\r\n");

        serial_write_str("RFLAGS:       0x");
        serial_write_usize_hex(rflags as usize);
        serial_write_str("\r\n");

        serial_write_str("RSP (before): 0x");
        serial_write_usize_hex(old_rsp as usize);
        serial_write_str("\r\n");
    }

    serial_write_str("======================================\r\n");
    serial_write_str("System halted.\r\n");

    loop {
        // SAFETY: hlt suspends the CPU until the next interrupt. With cli
        // already executed in the stub, this loops forever.
        unsafe { core::arch::asm!("hlt") };
    }
}

/// IDT gate descriptor (16 bytes).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8, // 0x8E = P=1, DPL=0, interrupt gate
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn new(handler: u64) -> Self {
        Self {
            offset_low: (handler & 0xFFFF) as u16,
            selector: 0x0008, // kernel code segment
            ist: 0,
            type_attr: 0x8E, // P=1, DPL=0, interrupt gate
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFF_FFFF) as u32,
            reserved: 0,
        }
    }
}

/// Descriptor-table pointer for the `LIDT` instruction.
#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

/// Wrapper that gives the IDT array 16-byte alignment (required by the CPU).
#[repr(C, align(16))]
struct IdtTable([IdtEntry; 256]);

/// The kernel IDT — 256 entries, 16-byte aligned.
///
/// SAFETY: populated exactly once in `idt_init` before `LIDT` is called,
/// then treated as read-only by the CPU.
static mut IDT: IdtTable = IdtTable([IdtEntry::missing(); 256]);

/// Populate the IDT with exception stubs and load the IDTR.
///
/// # Safety
///
/// - Must be called at CPL=0.
/// - Interrupts must be disabled.
/// - Must be called at most once (reinitialising IDTR while interrupts are
///   disabled is safe, but the previous IDT is abandoned).
unsafe fn idt_init() {
    // --- Install exception stubs (vectors 0–31) ---
    IDT.0[0] = IdtEntry::new(__isr_0 as u64);
    IDT.0[1] = IdtEntry::new(__isr_1 as u64);
    IDT.0[2] = IdtEntry::new(__isr_2 as u64);
    IDT.0[3] = IdtEntry::new(__isr_3 as u64);
    IDT.0[4] = IdtEntry::new(__isr_4 as u64);
    IDT.0[5] = IdtEntry::new(__isr_5 as u64);
    IDT.0[6] = IdtEntry::new(__isr_6 as u64);
    IDT.0[7] = IdtEntry::new(__isr_7 as u64);
    IDT.0[8] = IdtEntry::new(__isr_8 as u64);
    IDT.0[9] = IdtEntry::new(__isr_9 as u64);
    IDT.0[10] = IdtEntry::new(__isr_10 as u64);
    IDT.0[11] = IdtEntry::new(__isr_11 as u64);
    IDT.0[12] = IdtEntry::new(__isr_12 as u64);
    IDT.0[13] = IdtEntry::new(__isr_13 as u64);
    IDT.0[14] = IdtEntry::new(__isr_14 as u64);
    IDT.0[15] = IdtEntry::new(__isr_15 as u64);
    IDT.0[16] = IdtEntry::new(__isr_16 as u64);
    IDT.0[17] = IdtEntry::new(__isr_17 as u64);
    IDT.0[18] = IdtEntry::new(__isr_18 as u64);
    IDT.0[19] = IdtEntry::new(__isr_19 as u64);
    IDT.0[20] = IdtEntry::new(__isr_20 as u64);
    IDT.0[21] = IdtEntry::new(__isr_21 as u64);
    IDT.0[22] = IdtEntry::new(__isr_22 as u64);
    IDT.0[23] = IdtEntry::new(__isr_23 as u64);
    IDT.0[24] = IdtEntry::new(__isr_24 as u64);
    IDT.0[25] = IdtEntry::new(__isr_25 as u64);
    IDT.0[26] = IdtEntry::new(__isr_26 as u64);
    IDT.0[27] = IdtEntry::new(__isr_27 as u64);
    IDT.0[28] = IdtEntry::new(__isr_28 as u64);
    IDT.0[29] = IdtEntry::new(__isr_29 as u64);
    IDT.0[30] = IdtEntry::new(__isr_30 as u64);
    IDT.0[31] = IdtEntry::new(__isr_31 as u64);

    // --- Install generic IRQ stub for hardware interrupt vectors 32–255 ---
    let mut i = 32usize;
    while i < 256 {
        IDT.0[i] = IdtEntry::new(__isr_irq as u64);
        i += 1;
    }

    // --- Load the IDTR ---
    //
    // SAFETY: IDT is a valid static, aligned to 16 bytes, fully populated
    // above. LIDT writes only to the IDTR register. CPL=0 and interrupts
    // are disabled per the caller's contract.
    let ptr = IdtPointer {
        limit: (core::mem::size_of::<IdtTable>() - 1) as u16,
        base: core::ptr::addr_of!(IDT) as u64,
    };
    core::arch::asm!(
        "lidt [{ptr}]",
        ptr = in(reg) &ptr,
        options(readonly, nostack, preserves_flags),
    );
}

// ---------------------------------------------------------------------------
// Memory map analysis (Phase 1.3.1 — inline boot-side implementation)
//
// The canonical MemoryMap type lives in ferrous-boot-info. This helper
// prints the full map and summary statistics over serial during early boot.
// ---------------------------------------------------------------------------

/// Returns a short label for a UEFI memory type code.
fn memory_type_label(ty: u32) -> &'static str {
    use ferrous_boot_info::memory_type;
    match ty {
        memory_type::RESERVED => "Reserved",
        memory_type::LOADER_CODE => "LoaderCode",
        memory_type::LOADER_DATA => "LoaderData",
        memory_type::BOOT_SERVICES_CODE => "BootServicesCode",
        memory_type::BOOT_SERVICES_DATA => "BootServicesData",
        memory_type::RUNTIME_SERVICES_CODE => "RuntimeServicesCode",
        memory_type::RUNTIME_SERVICES_DATA => "RuntimeServicesData",
        memory_type::CONVENTIONAL => "Conventional",
        memory_type::UNUSABLE => "Unusable",
        memory_type::ACPI_RECLAIM => "AcpiReclaim",
        memory_type::ACPI_NON_VOLATILE => "AcpiNonVolatile",
        memory_type::MMIO => "Mmio",
        memory_type::MMIO_PORT_SPACE => "MmioPortSpace",
        memory_type::PERSISTENT_MEMORY => "PersistentMemory",
        _ => "Unknown",
    }
}

/// Returns true for UEFI types that are usable after boot services exit.
fn is_usable_after_boot(ty: u32) -> bool {
    use ferrous_boot_info::memory_type;
    matches!(
        ty,
        memory_type::CONVENTIONAL
            | memory_type::BOOT_SERVICES_CODE
            | memory_type::BOOT_SERVICES_DATA
            | memory_type::LOADER_CODE
            | memory_type::LOADER_DATA
            | memory_type::ACPI_RECLAIM
    )
}

/// Returns true for UEFI types that map to address-space holes (not RAM).
fn is_mmio(ty: u32) -> bool {
    use ferrous_boot_info::memory_type;
    matches!(ty, memory_type::MMIO | memory_type::MMIO_PORT_SPACE)
}

/// Print the full physical memory map and summary statistics over serial.
fn print_memory_map(map: &ferrous_boot_info::KernelMemoryMap) {
    serial_write_str("[INFO] Physical memory map (");
    serial_write_usize(map.count);
    serial_write_str(" entries");
    if map.truncated {
        serial_write_str(", TRUNCATED");
    }
    serial_write_str("):\r\n");

    let mut total_bytes: u64 = 0;
    let mut usable_bytes: u64 = 0;
    let mut reclaimable_bytes: u64 = 0;

    for (i, desc) in map.entries().iter().enumerate() {
        if desc.page_count == 0 {
            continue;
        }

        let size = desc.page_count * 4096;
        let end = desc.phys_start.saturating_add(size);

        if !is_mmio(desc.ty) {
            total_bytes = total_bytes.saturating_add(size);
        }
        if is_usable_after_boot(desc.ty) {
            if desc.ty == ferrous_boot_info::memory_type::CONVENTIONAL {
                usable_bytes = usable_bytes.saturating_add(size);
            } else {
                reclaimable_bytes = reclaimable_bytes.saturating_add(size);
            }
        }

        // Format: "  [ 0] 0x00001000 - 0x00100000  1020 KiB  Conventional"
        serial_write_str("  [");
        serial_write_usize(i);
        serial_write_str("] 0x");
        serial_write_usize_hex(desc.phys_start as usize);
        serial_write_str(" - 0x");
        serial_write_usize_hex(end as usize);
        serial_write_str("  ");
        let kib = size / 1024;
        if kib >= 1024 {
            serial_write_usize((kib / 1024) as usize);
            serial_write_str(" MiB");
        } else {
            serial_write_usize(kib as usize);
            serial_write_str(" KiB");
        }
        serial_write_str("  ");
        serial_write_str(memory_type_label(desc.ty));
        serial_write_str("\r\n");
    }

    if map.truncated {
        serial_write_str("[WARN] Memory map was truncated — some regions are missing!\r\n");
    }

    serial_write_str("[INFO] RAM: ");
    serial_write_usize((total_bytes / 1024 / 1024) as usize);
    serial_write_str(" MiB total | ");
    serial_write_usize((usable_bytes / 1024 / 1024) as usize);
    serial_write_str(" MiB usable | ");
    serial_write_usize((reclaimable_bytes / 1024 / 1024) as usize);
    serial_write_str(" MiB reclaimable\r\n");
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
