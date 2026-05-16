# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-05-16

### Overview
v0.1.0 is a minimal, verified bare-metal foundation: it boots, owns the hardware, manages memory, catches exceptions, logs structured output, and halts cleanly. It is now ready for Phase 2 (scheduling, IPC, user-space) to build on top.

#### What v0.1.0 does
- **Boots on real UEFI hardware / QEMU**: Loads as a proper UEFI application (`BOOTX64.EFI`), reads memory maps, calls `exit_boot_services()`, switches to a private kernel stack (64 KiB with guard region), and jumps to the kernel entry point.
- **Runs bare-metal x86-64**: Loads its own GDT and installs an IDT with 32 exception handlers. All CPU exceptions are caught and reported, preventing silent crashes.
- **Manages physical memory**: Parses the UEFI memory map, initializes a bitmap frame allocator covering all usable RAM, and tracks 4 KiB physical frames.
- **Manages virtual memory**: Builds page tables from scratch (PML4 $\rightarrow$ PDPT $\rightarrow$ PD). Maps 1 GiB identity for early boot and a higher-half alias for the kernel image. Loads CR3 for native paging and supports individual 4 KiB page mapping/unmapping with guard pages.
- **Working heap**: Provides a 4 MiB BSS-backed heap via `linked_list_allocator` with full `alloc` crate support (`Vec`, `Box`, `String`, `BTreeMap`).
- **Structured serial logging**: COM1 serial output at 115200/8N1 with five log levels (ERROR to TRACE) and runtime filtering.
- **Panic handler with stack traces**: Includes ASCII banner, source location, and a best-effort RBP frame-pointer walk to print return addresses.
- **Assertion and debug macros**: Provides `kassert!`, `kdebug_assert!`, and path-impossible markers like `kunreachable!` and `ktodo!` in `ferrous-core`.
- **Serial console driver**: Full `SerialPort` implementation with configurable parameters and a `Console` trait abstraction for `fmt::Write` support.

#### What it cannot do yet
- No keyboard input handling
- No display / framebuffer output
- No filesystem or networking
- No user-space processes or SMP (single core only)
- No interrupt-driven I/O (polling only)
- No persistent storage

### Detailed Changes

#### Core Kernel & Architecture
- **Execution Environment**: Implemented GDT initialization and IDT configuration to establish a stable execution environment.
- **Exception Handling**: Integrated basic exception handlers for system faults and interrupts.
- **Kernel Stack**: Implemented a dedicated kernel stack setup to ensure isolated execution context.
- **Boot Handoff**: Established the critical transition from the UEFI bootloader to the kernel entry point.

#### Memory Management
- **Physical Memory**: 
    - Implemented a global bitmap-based Physical Frame Allocator.
    - Added `PhysFrame` and `BitmapFrameAllocator` types for structured memory tracking.
    - Developed memory map parsing to understand system memory layout at boot.
- **Virtual Memory**:
    - Implemented 4 KiB page table management including map, unmap, and translate operations.
    - Developed the virtual memory setup process and the CR3 register switch for paging activation.
- **Dynamic Allocation**: implemented a kernel heap allocator for dynamic memory management.

#### I/O & Observability
- **Serial Communication**: 
    - Developed a 16550 UART serial driver for early boot output.
    - Created a `Console` trait and `BootSerialPort` implementation for a standardized I/O interface.
- **Logging Framework**: 
    - implemented a complete kernel logging framework for structured system events.
    - Added a `SerialLogger` that integrates with the early boot console.
- **Diagnostics**:
    - Enhanced the panic handler to include stack trace support for faster debugging.
    - Added kernel-wide assertion and debug macros in `ferrous-core`.

#### CI/CD & Tooling
- **Verification**: Added automated boot verification scripts and QEMU testing documentation.
- **Quality Assurance**: Integrated GitHub CodeQL for static analysis and security scanning.
- **Dependency Management**: Configured Dependabot for automated updates of GitHub Actions and Cargo dependencies.
- **Tooling**: Fixed KVM flag issues in QEMU run scripts to ensure wider compatibility.

#### Documentation
- Updated `ROADMAP.md`, `ARCHITECTURE.md`, and `MEMORY_ARCHITECTURE.md` to reflect the completion of Phases 1.1 through 1.4.
- Standardized ADR (Architecture Decision Record) processes for core design changes.
