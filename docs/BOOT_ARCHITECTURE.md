# Ferrous Kernel - Boot Process Overview

**Version:** 0.1  
**Date:** 2026-01-04  
**Status:** Design Phase (Phase 0)

---

## Overview

This document describes the boot process and initialization sequence for Ferrous Kernel. The boot process transitions from firmware (UEFI) to kernel execution and sets up the runtime environment.

**Related Documents:**
- [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture overview
- [MEMORY_ARCHITECTURE.md](MEMORY_ARCHITECTURE.md) - Memory management (boot-time setup)
- [ROADMAP.md](ROADMAP.md) - Development phases (Phase 1 boot milestones)

---

## Design Goals

### Primary Goals

1. **UEFI Boot** - Modern firmware interface (UEFI, not legacy BIOS)
2. **Clear Initialization Order** - Deterministic boot sequence
3. **Explicit Errors** - Boot failures are visible and traceable
4. **Minimal Dependencies** - Early boot has minimal requirements
5. **Safety** - Boot code is as safe as possible (Rust benefits)

### Constraints

- Must work with UEFI firmware
- Boot code runs in `no_std` environment
- Limited stack/heap during early boot
- Must handle hardware initialization carefully

---

## Boot Sequence Overview

![alt text](res/boot_sequence_overview.png)

---

## Boot Stages

### Stage 1: UEFI Handoff

**Context**: UEFI firmware has loaded the kernel image and calls the entry point.

**Responsibilities**:
- Save UEFI boot services context
- Get UEFI memory map
- Exit boot services (reclaim memory)
- Set up initial stack
- Transfer control to Rust code

**Key Structures**:
```rust
/// Boot-time information passed from UEFI
pub struct BootInfo {
    /// UEFI memory map
    memory_map: MemoryMap,
    
    /// Framebuffer information (if available)
    framebuffer: Option<FramebufferInfo>,
    
    /// ACPI tables
    acpi_tables: AcpiTables,
    
    /// Bootloader revision (if provided)
    bootloader_revision: Option<u64>,
}
```

**UEFI Integration**:
- Kernel is compiled as UEFI application
- Entry point: `efi_main()` (standard UEFI entry)
- Uses `uefi-rs` crate (or equivalent) for UEFI bindings
- Exit boot services before kernel initialization

### Stage 2: Kernel Entry Point (Assembly)

**Purpose**: Low-level CPU setup that cannot be done in Rust.

**Tasks**:
- Save CPU state (registers)
- Set up initial stack (statically allocated)
- Clear BSS section (zero-initialize static data)
- Jump to Rust entry point

**Assembly Code (x86_64, conceptual)**:
```asm
; Kernel entry point (UEFI calls this)
global kernel_entry
kernel_entry:
    ; Save UEFI context
    mov [uefi_context], rcx  ; ImageHandle
    mov [uefi_system_table], rdx  ; SystemTable
    
    ; Set up stack
    mov rsp, kernel_stack_top
    
    ; Clear BSS
    mov rdi, bss_start
    mov rcx, bss_size
    xor rax, rax
    rep stosb
    
    ; Jump to Rust entry point
    call rust_entry_point
    ; Never returns
```

### Stage 3: Early CPU Setup

**Purpose**: Initialize CPU features and structures.

**Tasks**:
- Initialize GDT (Global Descriptor Table)
- Set up IDT (Interrupt Descriptor Table)
- Configure CPU features (SSE, NX bit, etc.)
- Set up exception handlers (basic handlers)

**Rust Code (conceptual)**:
```rust
fn early_cpu_setup() -> Result<(), BootError> {
    // Initialize GDT
    gdt::init()?;
    
    // Set up IDT
    idt::init()?;
    
    // Configure CPU features
    cpu::enable_nx_bit()?;  // No-execute bit
    cpu::enable_sse()?;     // SSE instructions
    
    // Set up basic exception handlers
    exceptions::init_basic_handlers()?;
    
    Ok(())
}
```

**Safety Considerations**:
- GDT/IDT setup is unsafe (hardware register access)
- All unsafe operations require safety comments
- Failures are explicit (return errors, don't panic silently)

### Stage 4: Memory Map Parsing

**Purpose**: Understand system memory layout.

**Tasks**:
- Parse UEFI memory map
- Identify usable memory regions
- Mark reserved regions (ACPI tables, bootloader data, etc.)
- Detect NUMA topology (from ACPI SRAT)

**Memory Map Structure**:
```rust
pub struct MemoryMap {
    regions: Vec<MemoryRegion>,
    total_memory: usize,
    usable_memory: usize,
}

pub struct MemoryRegion {
    start: PhysicalAddress,
    size: usize,
    region_type: MemoryRegionType,
    attributes: MemoryAttributes,
}

pub enum MemoryRegionType {
    Usable,              // Available for allocation
    Reserved,            // Reserved by firmware/hardware
    ACPIReclaimable,     // Can be reclaimed after reading ACPI
    ACPINonVolatile,     // ACPI NVS
    BadMemory,           // Memory errors
    // ...
}
```

**NUMA Detection**:
- Parse ACPI SRAT (System Resource Affinity Table)
- Build NUMA node topology
- Map physical memory to NUMA nodes

### Stage 5: Physical Memory Setup

**Purpose**: Initialize physical memory allocator.

**Tasks**:
- Initialize frame allocator from memory map
- Reserve frames for kernel code/data
- Reserve frames for kernel data structures
- Set up per-NUMA-node allocators

**Initialization**:
```rust
fn setup_physical_memory(boot_info: &BootInfo) -> Result<FrameAllocator, BootError> {
    // Parse memory map
    let memory_map = parse_uefi_memory_map(&boot_info.memory_map)?;
    
    // Create frame allocator
    let mut allocator = FrameAllocator::new(memory_map)?;
    
    // Reserve kernel memory
    reserve_kernel_memory(&mut allocator)?;
    
    // Set up NUMA allocators
    setup_numa_allocators(&mut allocator, &boot_info.numa_topology)?;
    
    Ok(allocator)
}
```

### Stage 6: Virtual Memory Setup

**Purpose**: Set up paging and virtual memory.

**Tasks**:
- Create kernel page tables
- Identity map physical memory (temporary, for transition)
- Map kernel code/data sections
- Enable paging (CR0.PG = 1)

**Temporary Identity Mapping**:
- Initially, physical and virtual addresses are the same
- Allows code to continue executing during paging setup
- Removed after higher-half kernel transition

**Rust Code (conceptual)**:
```rust
fn setup_virtual_memory(allocator: &mut FrameAllocator) -> Result<PageTable, BootError> {
    // Create kernel page table
    let mut page_table = PageTable::new(allocator)?;
    
    // Identity map all physical memory (temporary)
    identity_map_physical_memory(&mut page_table, allocator)?;
    
    // Map kernel code/data sections
    map_kernel_sections(&mut page_table, allocator)?;
    
    // Enable paging
    unsafe {
        cpu::enable_paging(&page_table)?;
    }
    
    Ok(page_table)
}
```

### Stage 7: Higher-Half Kernel Transition

**Purpose**: Move kernel to high virtual addresses.

**Tasks**:
- Remap kernel to high virtual addresses (0xFFFF_8000_0000_0000+)
- Remove low-memory identity mapping
- Update all kernel pointers
- Continue execution from high addresses

**Transition Process**:
1. Create new page table with kernel at high addresses
2. Map kernel code/data to high addresses
3. Set up temporary mapping for transition code
4. Jump to high-address code
5. Switch to new page table
6. Remove low-memory mappings
7. Continue execution from high addresses

**Critical**: Transition must be carefully done to avoid page faults during the switch.

### Stage 8: Kernel Heap Setup

**Purpose**: Enable dynamic memory allocation.

**Tasks**:
- Allocate initial heap frames
- Initialize heap allocator data structures
- Set global allocator
- Enable `alloc` crate usage

**Initialization**:
```rust
fn setup_kernel_heap(allocator: &mut FrameAllocator) -> Result<(), BootError> {
    // Allocate initial heap frames (e.g., 4MB)
    let heap_frames = allocator.allocate_frames(HEAP_SIZE / PAGE_SIZE)?;
    
    // Initialize heap allocator
    let heap_allocator = HeapAllocator::new(heap_frames.start_address(), HEAP_SIZE)?;
    
    // Set as global allocator
    set_global_allocator(heap_allocator);
    
    Ok(())
}
```

**After This Stage**: Kernel can use `Box`, `Vec`, `String`, etc.

### Stage 9: Interrupt Subsystem

**Purpose**: Enable interrupts and set up interrupt handling.

**Tasks**:
- Initialize APIC (Advanced Programmable Interrupt Controller)
- Set up timer interrupts (HPET or APIC timer)
- Configure interrupt routing
- Enable interrupts (STI instruction)

**Timer Setup**:
- Configure timer for scheduling ticks (e.g., 1ms)
- Set up timer interrupt handler
- Timer interrupts enable preemptive multitasking

**Rust Code (conceptual)**:
```rust
fn setup_interrupts() -> Result<(), BootError> {
    // Initialize APIC
    apic::init()?;
    
    // Set up timer
    timer::init(1000)?;  // 1ms tick
    
    // Enable interrupts
    unsafe {
        cpu::enable_interrupts();
    }
    
    Ok(())
}
```

### Stage 10: Core Subsystems

**Purpose**: Initialize kernel subsystems.

**Tasks**:
- Initialize scheduler
- Initialize IPC subsystem
- Initialize capability system
- Initialize observability system (if enabled)

**Initialization Order**:
1. Observability (other subsystems generate events)
2. Capability system (IPC needs capabilities)
3. IPC subsystem (depends on capabilities)
4. Scheduler (depends on IPC for task communication)

**Rust Code (conceptual)**:
```rust
fn init_core_subsystems() -> Result<(), BootError> {
    // Observability first (other subsystems use it)
    observability::init()?;
    
    // Capability system
    capability::init()?;
    
    // IPC subsystem
    ipc::init()?;
    
    // Scheduler
    scheduler::init()?;
    
    Ok(())
}
```

### Stage 11: Initial Kernel Task

**Purpose**: Create initial kernel execution context.

**Tasks**:
- Create kernel task structure
- Set up kernel stack
- Register task with scheduler
- Set as current task

**Kernel Task**:
- Special task representing kernel execution
- Runs in kernel mode (Ring 0)
- Used for kernel threads (future)
- Idle task runs when no user tasks are runnable

### Stage 12: Load Init Process

**Purpose**: Load and prepare the first user-space process.

**Tasks**:
- Load init binary from filesystem (or embedded)
- Parse ELF executable
- Create process structure
- Create address space for init
- Set up stack and heap for init
- Grant initial capabilities to init
- Set up initial IPC endpoints

**Init Process Capabilities**:
- Create endpoint capability
- Create resource group capability
- Allocate memory capability (limited)
- Spawn process capability

**Rust Code (conceptual)**:
```rust
fn load_init_process() -> Result<Process, BootError> {
    // Load init binary
    let init_binary = load_binary("init")?;
    
    // Parse ELF
    let elf = parse_elf(&init_binary)?;
    
    // Create process
    let mut process = Process::new(elf.entry_point)?;
    
    // Create address space
    let address_space = create_init_address_space(&elf)?;
    process.set_address_space(address_space);
    
    // Grant initial capabilities
    grant_initial_capabilities(&mut process)?;
    
    // Set up initial IPC endpoints
    setup_init_endpoints(&mut process)?;
    
    Ok(process)
}
```

### Stage 13: Jump to User-Space

**Purpose**: Transfer control to user-space.

**Tasks**:
- Switch to init process's address space
- Set up user-space stack
- Set up CPU state (registers) for user-space
- Enter user mode (Ring 3)
- Jump to init process entry point

**Context Switch**:
- Save kernel context
- Load init process context
- Switch page table (CR3)
- Set up user-space stack pointer
- Set CPU privilege level (Ring 3)
- Jump to entry point (IRET or SYSEXIT)

**After This Stage**: Kernel is running, init process executes in user-space.

---

## Boot Information Structure

### BootInfo

Information passed from bootloader/UEFI to kernel.

```rust
pub struct BootInfo {
    /// Memory map from UEFI
    pub memory_map: MemoryMap,
    
    /// Framebuffer information (if available)
    pub framebuffer: Option<FramebufferInfo>,
    
    /// ACPI tables
    pub acpi_tables: AcpiTables,
    
    /// Bootloader information
    pub bootloader_info: BootloaderInfo,
    
    /// Kernel command line (if provided)
    pub cmdline: Option<String>,
}

pub struct FramebufferInfo {
    pub base_address: PhysicalAddress,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub pixel_format: PixelFormat,
}

pub struct BootloaderInfo {
    pub name: &'static str,
    pub version: u64,
}
```

---

## Error Handling

### Boot Errors

```rust
#[derive(Debug)]
pub enum BootError {
    UefiError(UefiError),
    MemoryMapParseError,
    InvalidMemoryMap,
    FrameAllocationFailed,
    PageTableSetupFailed,
    InterruptSetupFailed,
    SubsystemInitFailed(&'static str),
    InitProcessLoadFailed,
    InvalidElfFormat,
}
```

**Error Handling Philosophy**:
- Boot failures are **fatal** (cannot continue)
- Errors are logged clearly (serial console)
- Panic with informative message
- No silent failures during boot

---

## Observability

### Boot Events

**Event Types**:
- **Boot Started**: Boot sequence begins
- **Stage Completed**: Each boot stage completion
- **Stage Failed**: Boot stage failure
- **Memory Allocated**: Memory allocations during boot
- **Subsystem Initialized**: Each subsystem initialization

**Boot Logging**:
- All boot events logged to serial console
- Structured logging (if observability enabled)
- Boot time measurement per stage

---

## Safety Considerations

### Unsafe Code in Boot

**Unsafe Operations**:
- CPU register access (CR0, CR3, CR4)
- Page table manipulation
- Interrupt controller access
- UEFI service calls

**Safety Guarantees**:
- All unsafe blocks require safety comments
- Boot code is carefully reviewed
- Failures are explicit (no silent corruption)
- Boot failures are fatal (prevent unsafe execution)

---

## Implementation Phases

### Phase 1: Basic Boot

**Deliverables**:
- UEFI entry point and handoff
- Basic CPU setup (GDT, IDT)
- Memory map parsing
- Physical memory allocator
- Virtual memory setup (identity mapping)
- Kernel heap setup
- Basic serial output

**Success Criteria**:
- Kernel boots on QEMU x86_64
- Can print "Hello from Ferrous!" to serial console
- Paging is enabled
- Kernel heap allocation works

### Phase 2: Full Boot Sequence

**Deliverables**:
- Higher-half kernel transition
- Interrupt subsystem
- Core subsystems initialization
- Init process loading
- User-space transition

**Success Criteria**:
- Kernel boots completely
- Init process loads and executes
- User-space code runs in Ring 3
- System is ready for user-space execution

---

## Future Considerations

### Potential Enhancements

1. **Multi-Boot Support**: Support GRUB multiboot protocol (optional)
2. **Secure Boot**: UEFI Secure Boot support
3. **Boot Time Optimization**: Measure and optimize boot time
4. **Recovery Mode**: Boot to recovery/safe mode
5. **Kernel Modules**: Load kernel modules at boot (future)

---

## Related Documents

- [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture overview
- [MEMORY_ARCHITECTURE.md](MEMORY_ARCHITECTURE.md) - Memory management (boot-time setup)
- [ROADMAP.md](ROADMAP.md) - Phase 1 boot milestones

---

**Document Status**: This is a living document. As implementation progresses, details will be refined and documented in ADRs.

