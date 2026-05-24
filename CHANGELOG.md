# Changelog

All notable changes to Ferrous Kernel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

#### Phase 2.1.1 - Task and Process Data Structures
- `TaskId` and `ProcessId` newtypes over `u64`; opaque and unforgeable
- `TaskState` enum (Ready / Running / Blocked / Exiting / Zombie) with
  `const can_transition_to()` and atomic compare-and-swap via
  `try_transition()`; invalid transitions return `TaskStateError`
- `TaskPriority` enum (Idle / Low / Normal / High / RealTime)
- `RegisterState` (`repr(C)`) holding callee-saved registers plus
  rsp / rip / rflags; field offsets documented for the Phase 2.2.3
  context-switch assembly stub
- `TaskControlBlock` (`repr(C)`) with saved register state, kernel stack
  bounds behind safe wrappers, atomic state, priority, time-slice, and
  owner PID (`kernel/src/task/task.rs`)
- `ProcessState` enum (Active / Exiting / Zombie) with validated
  transitions and `ProcessStateError`
- `Process` with fixed-capacity task list (16 slots), atomic state,
  exit code storage, and stubs for address-space and capability-space
  fields to be populated in Phases 2.1.2 and 2.3.1
  (`kernel/src/task/process.rs`)
- `kernel::task::smoke_test()` with 8 test cases covering ID
  round-trips, state machine enforcement, atomic CAS behaviour, task
  registration, capacity overflow rejection, and exit-code storage

#### Phase 2.1.2 - Address Space Management
- `AddressSpace` struct owning an isolated PML4 physical frame and a
  fixed-capacity region list (32 slots) to avoid heap dependency during
  early boot (`kernel/src/memory/address_space.rs`)
- Kernel higher-half entries (PML4[256-511]) copied from the bootstrap
  PML4 at construction so all address spaces share one kernel mapping
- `RegionKind` enum (Code / Data / Stack / Heap) with `page_flags()`
  returning correct x86-64 PTE flags (`PRESENT | USER_ACCESSIBLE`, plus
  `WRITABLE` for non-code kinds)
- `VirtualRegion` with `overlaps()` guard enforced by `map_region`
- `map_4k_into` / `unmap_4k_from` — page-table walkers operating on an
  explicit PML4 physical address; exploit VA==PA identity mapping
  established in Phase 1.3.3
- `AddressSpace::map_region` validates user-space bounds, alignment, and
  no-overlap; rolls back already-mapped pages on partial failure
- `AddressSpace::unmap_region` clears PTEs, frees per-page frames, and
  compacts the region list via swap-with-last
- `AddressSpace::translate` translates a VA in this space without
  touching CR3
- `AddressSpace::switch_to` loads this space's PML4 into CR3 (full TLB
  flush)
- `AddressSpace::destroy` tears down all user regions and frees the PML4
  frame
- `memory::address_space::smoke_test()` with 6 test cases covering
  overlap detection, construction, map+translate, unmap, invalid-input
  rejection, and destroy

#### Phase 2.1.3 - ELF Binary Loader
- `kernel/src/task/elf.rs` module added for statically-linked ELF64 executable parsing and loading.
- Safe `read_struct` parsing to map byte slices into `#[repr(C)]` ELF headers without allocations or external dependencies.
- `load_elf` loads `PT_LOAD` segments into a target `AddressSpace` with page-aligned mappings (`RegionKind::Code` and `RegionKind::Data`), including BSS zero-filling.
- `ParseError` enum provides robust error handling for invalid binaries.
- `elf::smoke_test()` added to test loader parsing and mapping functionality.

#### Documentation and Tooling
- README badges: CI, Release, CodeQL, latest release version, license,
  Rust language, and architecture
- ROADMAP.md: current phase updated to Phase 2; tasks 2.1.1 and 2.1.2
  marked complete
- ARCHITECTURE.md: Scheduler abstractions section updated with
  implemented types and file locations; boot sequence annotated with
  v0.1.0 completion status

---

## [0.1.0] - 2026-05-16

First tagged release. The kernel boots from UEFI firmware, initialises physical
and virtual memory, enters the kernel proper, and produces observable structured
output over COM1 serial before halting cleanly. Foundation milestone only; see
Known Limitations below.

### Added

#### Phase 1.1 - Bare Metal Boot
- UEFI bootloader (`ferrous-boot`) targeting `x86_64-unknown-uefi`
- Kernel entry point receives typed `BootInfo` from the bootloader
- Private 64 KiB kernel stack with guard page established before kernel entry
- GDT and IDT loaded; 32 CPU exception handlers installed
- Serial output available from the first instruction after UEFI handoff
- Boot identity message: `=== Ferrous Kernel ===` followed by `Hello from Ferrous!`

#### Phase 1.2 - Memory Map
- Physical memory map parsed from UEFI memory descriptors
- Usable RAM regions identified, deduplicated, and reported at boot
- Memory map passed to the kernel via `BootInfo`

#### Phase 1.3.2 - Physical Frame Allocator
- Bitmap-based physical frame allocator covering all usable RAM
- 4 KiB frame allocation and deallocation
- Reports free / total frame counts at initialisation

#### Phase 1.3.3 - Virtual Memory
- Kernel page tables (PML4) built from scratch after `exit_boot_services`
- CR3 loaded with new tables; higher-half kernel alias verified at boot
- 1 GiB identity map and direct-physical map established

#### Phase 1.3.4 - Page Table Management
- `map_4k` and `unmap_4k` for individual 4 KiB page mappings
- Guard page support: unmapped addresses translate to `None`
- Smoke tests verify translate, map, unmap, and guard page behaviour

#### Phase 1.3.5 - Heap Allocator
- Linked-list heap allocator backed by physical frames
- `alloc` crate available in kernel code (`Vec`, `Box`, `String`, `BTreeMap`)
- Smoke tests verify push/index/deref/starts_with on heap-allocated types

#### Phase 1.4.1 - Logging Framework
- `log` crate integration with `SerialLogger` backend
- Log levels ERROR, WARN, INFO, DEBUG routed to COM1
- Format: `[LEVEL] crate: message`

#### Phase 1.4.2 - Panic Handler
- `#[panic_handler]` emits ASCII banner, source location, and message to COM1
- RBP-chain stack trace walker prints raw frame return addresses
- Smoke test verifies handler survives normal execution

#### Phase 1.4.3 - Assertion and Debug Macros (`ferrous-core`)
- `kassert!`, `kassert_eq!`, `kassert_ne!` - panic with expression context on failure
- `kdebug_assert!`, `kdebug_assert_eq!`, `kdebug_assert_ne!` - elided in release builds
- `kunreachable!` and `ktodo!` for path-impossible markers
- Smoke tests verify all nine macro variants

#### Phase 1.4.4 - Serial Console Driver
- `kernel/src/drivers/serial.rs`: production `SerialPort` with configurable baud
  rate (`BaudRate`), framing (`DataBits`, `Parity`, `StopBits`), multi-port
  support (`ComPort`), and typed `SerialError`
- `kernel/src/drivers/console.rs`: `Console` trait (supertrait of `fmt::Write`)
  and `SerialConsole` implementation
- `boot/src/serial.rs`: `BootSerialPort` for the UEFI boot phase; zero-cost
  `Copy` type; mirrors the hardware model without a kernel-crate dependency
- Module-level free functions (`serial_init`, `serial_write_str`, etc.) preserve
  the existing API used by the panic handler and stack-trace walker
- Smoke tests cover `fmt::Write`, non-blocking receive (`try_read_byte`), and
  RX FIFO status (`data_available`)

#### Tooling
- `scripts/make-image.sh`: produces a 64 MiB FAT32 bootable disk image
  (`EFI/BOOT/BOOTX64.EFI`); supports macOS (`hdiutil` + `newfs_msdos`) and
  Linux (`mtools`)
- `scripts/verify-boot.sh`: headless QEMU boot verification; checks all expected
  serial output strings within a configurable timeout; suitable for CI
- GitHub Actions CI: build and boot-verify on every push and pull request
- GitHub CodeQL: static analysis and security scanning

### Known Limitations

- Single core only; no SMP
- No interrupt-driven I/O; all serial I/O is polled
- No keyboard input or display / framebuffer output
- No filesystem or persistent storage
- No networking
- No user-space processes or syscall interface

---

[Unreleased]: https://github.com/iamvirul/ferrous-kernel/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/iamvirul/ferrous-kernel/releases/tag/v0.1.0
