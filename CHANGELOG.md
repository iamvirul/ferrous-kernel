# Changelog

All notable changes to Ferrous Kernel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

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
