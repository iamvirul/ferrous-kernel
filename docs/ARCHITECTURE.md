# Ferrous Kernel - System Architecture

**Version:** 0.1  
**Date:** 2026-01-04  
**Status:** Design Phase (Phase 0)

---

## Overview

This document describes the high-level architecture of Ferrous Kernel, a research-grade operating system kernel written in Rust. It defines the core abstractions, system boundaries, and design principles that guide implementation.

**This document is architectural, not implementation-specific.** Detailed design documents will be created for each subsystem (memory management, capability system, IPC, etc.).

For project goals and design principles, see [CHARTER.md](CHARTER.md).  
For development timeline, see [ROADMAP.md](ROADMAP.md).

---

## Architectural Goals

The Ferrous architecture is designed to achieve:

1. **Memory Safety** - Rust's type system eliminates entire bug classes
2. **Isolation by Default** - Component failures do not cascade
3. **Small Trusted Core** - Minimal privileged code (<50k lines of Rust)
4. **Capability-Based Security** - No ambient authority, explicit access control
5. **Observability First** - Every decision is traceable and explainable
6. **Modern Workload Support** - Containers and microservices as primitives

---

## System Architecture Overview

Ferrous uses a **hybrid microkernel-inspired design** with pragmatic performance considerations:

![alt text](res/system_architecture_overview.png)

### Trust Boundaries

- **Privileged Kernel Core**: Runs in kernel mode (ring 0 on x86_64), handles critical resources
- **User-Space Services**: Run in user mode (ring 3), isolated from kernel and each other
- **IPC Boundary**: The only communication path between isolated components

---

## Core Kernel Components

The privileged kernel core consists of six major subsystems:

### 1. Memory Management

**Responsibility**: Physical and virtual memory allocation, page table management, address space isolation.

**Key Design Principles**:
- Higher-half kernel (kernel at high virtual addresses)
- Per-process address spaces with strong isolation
- NUMA-aware allocation from Phase 1
- Zero-copy optimizations where safe

**Core Abstractions**:
- `AddressSpace` - Per-process virtual memory space
- `PageTable` - Hardware page table management
- `PhysicalFrame` - Physical memory frame allocator
- `VirtualRegion` - Contiguous virtual memory regions

**Memory Safety Guarantees**:
- Rust ownership system prevents use-after-free
- Page table modifications are unsafe but wrapped in safe APIs
- All memory access validated by hardware MMU

**Related Documents**: 
- Detailed design: `docs/MEMORY_ARCHITECTURE.md` (Phase 0 deliverable)

---

### 2. Scheduler

**Responsibility**: Task scheduling, context switching, CPU time accounting, energy-aware decisions.

**Key Design Principles**:
- Energy-aware scheduling (consider power domains, NUMA)
- Preemptive multitasking
- Fairness guarantees for resource groups
- Real-time scheduling classes (future)

**Core Abstractions**:
- `Task` - Runnable execution context (process/thread)
- `Scheduler` - Scheduling policy implementation
- `RunQueue` - Per-CPU run queues
- `ResourceGroup` - CPU time accounting and limits

**Scheduling Decisions**:
- All scheduling decisions generate observability events
- Context switches are traceable with causality chains
- CPU time accounting is always enabled

**Energy Considerations**:
- Schedule tasks on cores in appropriate power states
- NUMA-aware scheduling (prefer local memory access)
- Heterogeneous compute support (future: big.LITTLE)

**Related Documents**:
- Detailed design: `docs/SCHEDULER_ARCHITECTURE.md` (Phase 2)

---

### 3. IPC (Inter-Process Communication)

**Responsibility**: Message passing between isolated components, shared memory with capability control.

**Key Design Principles**:
- Message passing is the primary communication mechanism
- Zero-copy transfers where possible
- Synchronous and asynchronous messaging
- Causality tracking for observability

**Core Abstractions**:
- `Endpoint` - Communication endpoint (like a socket)
- `Message` - Typed message structure
- `Channel` - Bidirectional communication channel
- `SharedMemory` - Capability-controlled shared memory regions

**Message Passing Model**:
- Send/receive operations require endpoint capabilities
- Messages can carry capability transfers
- Blocking and non-blocking variants
- Timeouts and cancellation support

**Safety Guarantees**:
- Message buffers are type-safe (Rust types)
- Endpoint capabilities cannot be forged
- Shared memory access controlled by capabilities

**Related Documents**:
- Detailed design: `docs/IPC_ARCHITECTURE.md` (Phase 0 deliverable)

---

### 4. Capability System

**Responsibility**: Access control, resource authorization, capability derivation and revocation.

**Key Design Principles**:
- **No ambient authority** - All access requires explicit capability
- Capabilities cannot be forged or guessed
- Capabilities can be derived with restrictions
- Revocation is always possible

**Core Abstractions**:
- `Capability` - Opaque capability token (cryptographically secure)
- `CapabilitySpace` - Per-process capability table
- `CapabilityRef` - Reference-counted capability handle
- `Resource` - Resource tagged with capability requirements

**Capability Types**:
- **Object Capabilities**: Grant access to specific resources (files, devices, endpoints)
- **Process Capabilities**: Control process lifecycle (kill, suspend, migrate)
- **System Capabilities**: System-wide operations (create resource groups, allocate memory)
- **Derived Capabilities**: Restricted subsets of parent capabilities

**Security Model**:
- Capability derivation rules are explicit and auditable
- Capability revocation invalidates all derived capabilities
- Capability transfer through IPC is explicit and logged

**Related Documents**:
- Detailed design: `docs/CAPABILITY_SYSTEM.md` (Phase 0 deliverable)

---

### 5. Observability System

**Responsibility**: Structured event tracing, metrics collection, causality tracking, performance diagnostics.

**Key Design Principles**:
- **Observability is built-in, not bolted on**
- Zero overhead when disabled (compiled out)
- Structured events with typed payloads
- Causality tracking through message passing

**Core Abstractions**:
- `Event` - Structured event (syscall, scheduling, IPC, etc.)
- `Trace` - Causality-linked event chain
- `Metric` - Counter/gauge/histogram metrics
- `Observer` - Event sink (serial, in-memory buffer, network export)

**Event Categories**:
- **Syscall Events**: Entry/exit, parameters, return values
- **Scheduling Events**: Task switch, wakeup, blocking
- **IPC Events**: Send/receive, capability transfer
- **Memory Events**: Allocation, page faults, memory pressure
- **Security Events**: Capability grant/revoke, access denied

**Export Formats**:
- Structured binary format (zero-copy)
- OpenTelemetry compatibility (future)
- Human-readable logging (development)

**Performance**:
- Events are generated synchronously (no async overhead)
- Observers can be hot-swapped
- Full tracing overhead target: <5%

**Related Documents**:
- Detailed design: `docs/OBSERVABILITY_ARCHITECTURE.md` (Phase 4)

---

### 6. Boot and Initialization

**Responsibility**: Early boot sequence, hardware initialization, kernel startup, first user-space process.

**Key Design Principles**:
- UEFI boot (modern firmware interface)
- Minimal boot-time dependencies
- Clear initialization order
- Boot failures are explicit and traceable

**Boot Sequence** (high-level):
1. UEFI handoff to kernel entry point
2. Early CPU initialization (GDT, IDT, exception handlers)
3. Memory map parsing and physical memory setup
4. Virtual memory initialization (page tables)
5. Kernel heap allocation setup
6. Interrupt subsystem initialization
7. Core subsystems initialization (scheduler, IPC, capabilities)
8. Create initial kernel task
9. Load and execute init process (first user-space)

**Core Abstractions**:
- `BootInfo` - Boot-time information (memory map, firmware tables)
- `EarlyAllocator` - Bootstrap memory allocator
- `InitProcess` - First user-space process

**Related Documents**:
- Detailed design: `docs/BOOT_ARCHITECTURE.md` (Phase 0 deliverable)

---

## User-Space Interface

### System Call Interface

Ferrous provides a capability-based system call interface:

**Core System Calls**:
- `send(endpoint, message)` - Send message to endpoint
- `receive(endpoint, buffer)` - Receive message from endpoint
- `grant(capability, process)` - Transfer capability to process
- `revoke(capability)` - Revoke capability
- `derive(capability, restrictions)` - Create restricted capability
- `create_endpoint()` - Create new IPC endpoint (requires capability)
- `create_resource_group(caps)` - Create resource group (requires capability)
- `yield()` - Voluntary context switch
- `exit(status)` - Terminate process

**Memory System Calls**:
- `map(region, flags)` - Map memory region (requires capability)
- `unmap(region)` - Unmap memory region
- `share_memory(size, caps)` - Create shared memory region

**Process System Calls**:
- `spawn(executable, caps)` - Create new process (requires capability)
- `wait(process)` - Wait for process termination
- `kill(process, signal)` - Send signal to process (requires capability)

**Design Philosophy**:
- System calls are capability-gated (no ambient authority)
- Return values are always explicit (`Result<T, Error>`)
- System calls generate observability events
- No "root" user - all access through capabilities

---

## Isolation and Fault Containment

### Process Isolation

**Address Space Isolation**:
- Each process has independent virtual address space
- Hardware MMU enforces isolation
- Kernel memory is inaccessible to user-space
- Shared memory is explicit and capability-controlled

**Capability Isolation**:
- Processes cannot access resources without capabilities
- Capabilities cannot be forged (cryptographically secure)
- Cross-process capability transfer is explicit and audited

### Service Isolation

**User-Space Services** (drivers, filesystems, etc.):
- Run in separate address spaces
- Communicate only via IPC
- Cannot directly access hardware without kernel-granted capabilities
- Failures do not affect kernel or other services

**Fault Containment**:
- Service crash → process termination, no kernel panic
- Kernel can restart service (Phase 3)
- Dependent services receive error notifications via IPC
- State recovery mechanisms (Phase 3)

**Restart Infrastructure** (Phase 3):
- Service monitoring framework
- Automatic restart policies
- Dependency tracking
- State recovery protocols

---

## Security Architecture

### Capability-Based Security Model

**No Ambient Authority**:
- No process has default access to anything
- All access requires explicit capability
- No "root" user or superuser privileges
- Capabilities are the only authorization mechanism

**Capability Properties**:
- **Unforgeable**: Cryptographic tokens, cannot be guessed
- **Delegatable**: Can be transferred or derived
- **Revocable**: Can be invalidated by issuer
- **Auditable**: All operations are logged

**Capability Derivation**:
- Parent capabilities can be restricted to create child capabilities
- Example: Full filesystem capability → read-only capability for specific directory
- Derived capabilities are tracked (revocation cascades)

**Capability Revocation**:
- Issuer can revoke capability at any time
- All derived capabilities are invalidated
- In-use capabilities are marked for revocation (lazy cleanup)
- Observability events generated for all revocations

### Security Boundaries

**Kernel Boundary**:
- Kernel code runs in privileged mode (ring 0)
- User-space cannot execute privileged instructions
- System call interface is the only entry point
- All kernel entry points validate capabilities

**Process Boundary**:
- Processes cannot access each other's memory
- Process isolation enforced by hardware MMU
- IPC is the only cross-process communication mechanism

**Service Boundary**:
- Services run in user-space
- Services cannot access hardware directly
- Hardware access requires kernel-granted capabilities
- Service failures are contained

---

## Resource Management

### Resource Groups

**Concept**: Resource groups are first-class kernel primitives for container-like isolation (Phase 4).

**Resource Types**:
- **CPU**: Time limits, CPU affinity, scheduling priority
- **Memory**: Limits, accounting, pressure notifications
- **I/O**: Bandwidth limits, I/O priority
- **Network**: Bandwidth limits, network namespaces

**Capability Model**:
- Creating resource groups requires system capability
- Processes are assigned to resource groups
- Resource usage is attributed to groups
- Observability events track resource consumption

### Memory Management

**Physical Memory**:
- NUMA-aware allocation
- Frame allocator (buddy system or bitmap)
- Memory pressure detection and notification

**Virtual Memory**:
- Per-process address spaces
- Higher-half kernel mapping
- Memory mapping capabilities control access
- Page fault handling with observability

**Memory Safety**:
- Rust ownership prevents use-after-free
- All memory access validated by hardware
- Kernel heap allocator is safe (when used correctly)

---

## Concurrency and Synchronization

### Kernel Concurrency Model

**Multi-Core Support**:
- Per-CPU data structures (run queues, local allocators)
- Lock-free algorithms where possible
- Spinlocks for short critical sections
- No global kernel lock

**Synchronization Primitives**:
- `Mutex<T>` - Mutex with Rust ownership
- `SpinLock<T>` - Spinlock for interrupt contexts
- `Atomic<T>` - Atomic operations
- `Channel` - Async message passing

**Safety Guarantees**:
- Rust's type system prevents data races in safe code
- Unsafe synchronization code is isolated and audited
- Deadlock detection in debug builds (future)

### User-Space Concurrency

**Threads** (future):
- Threads share address space and capabilities
- Thread-local storage supported
- Synchronization via kernel-provided primitives

**Processes**:
- Processes are isolated (separate address spaces)
- Communication via IPC only
- No shared memory without explicit capabilities

---

## Hardware Abstraction

### Architecture Support

**Primary Target**: x86_64 (amd64)
- UEFI boot
- APIC (Advanced Programmable Interrupt Controller)
- HPET or APIC timer
- Modern CPU features (SMEP, SMAP, NX bit)

**Future Targets**:
- ARM64 (aarch64) - Planned
- RISC-V - Exploratory

### Device Access

**MMIO (Memory-Mapped I/O)**:
- User-space drivers access MMIO via capabilities
- Kernel validates MMIO addresses
- Observability events for all MMIO access

**PIO (Port I/O)**:
- x86_64 specific
- User-space drivers access PIO via capabilities
- Kernel validates port ranges

**DMA (Direct Memory Access)**:
- DMA buffers are capability-controlled
- Kernel manages DMA buffer allocation
- Safety guarantees: Rust ownership + capability checks

**Interrupts**:
- Interrupts routed to user-space drivers via IPC
- Kernel acts as interrupt dispatcher
- Interrupt handlers are capability-gated

---

## Observability Integration

### Event Generation

**All Kernel Operations Generate Events**:
- System call entry/exit
- Scheduling decisions
- IPC operations
- Memory allocations
- Capability operations
- Hardware interrupts

**Event Structure**:
- Timestamp (high-resolution)
- Event type (typed enum)
- Process/thread context
- Causality chain (linked to previous events)
- Payload (typed, structured data)

### Causality Tracking

**Message Passing Enables Causality**:
- Each IPC message carries causality context
- Events linked through message chains
- Request tracing across service boundaries
- Distributed tracing support (future)

**Use Cases**:
- Debug performance issues across components
- Security audit trails
- Resource usage attribution
- Failure analysis

---

## Implementation Constraints

### Rust-Specific Considerations

**no_std Environment**:
- Kernel runs in `no_std` (no standard library)
- Custom allocator required
- Core library (`lib/core`) provides essential utilities

**Unsafe Code Guidelines**:
- Unsafe code <5% of codebase
- Every unsafe block requires safety comment
- Unsafe code isolated behind safe abstractions
- Public APIs are safe

**Memory Safety**:
- Safe Rust code has zero memory safety bugs
- Unsafe code is minimal and audited
- All unsafe invariants documented

### Performance Constraints

**Performance Targets** (later phases):
- Syscall latency: Competitive with Linux
- IPC throughput: Competitive with seL4
- Context switch overhead: <1μs
- Observability overhead: <5% when enabled

**Early Phases Priority**:
- Correctness first
- Performance follows
- Features last

---

## Design Evolution

### Phase 0 (Current): Foundation & Design
- Architecture document (this document)
- Detailed subsystem designs
- ADR template and initial ADRs

### Phase 1: Proof of Life
- Basic boot and memory management
- No user-space yet
- Foundation for all subsystems

### Phase 2: Core Kernel
- Scheduler, IPC, capabilities
- First user-space process
- Basic isolation working

### Phase 3: Isolation
- User-space drivers
- Fault containment proven
- Service restart infrastructure

### Phase 4: Modern Workloads
- Resource groups as primitives
- Observability pipeline
- Container-like abstractions

### Phase 5: Validation
- Performance benchmarks
- Security audits
- Formal verification (exploratory)

---

## Related Documents

**Must Read Before Implementation**:
- [CHARTER.md](CHARTER.md) - Project goals and principles
- [ROADMAP.md](ROADMAP.md) - Development timeline

**Detailed Design Documents** (Phase 0 deliverables):
- `docs/MEMORY_ARCHITECTURE.md` - Memory management design
- `docs/CAPABILITY_SYSTEM.md` - Capability system design
- `docs/IPC_ARCHITECTURE.md` - IPC mechanism design
- `docs/BOOT_ARCHITECTURE.md` - Boot process design

**Future Documents**:
- `docs/SCHEDULER_ARCHITECTURE.md` - Scheduler design (Phase 2)
- `docs/OBSERVABILITY_ARCHITECTURE.md` - Observability design (Phase 4)

**Policy Documents**:
- `docs/UNSAFE_GUIDELINES.md` - Unsafe Rust policy
- `docs/adr/` - Architecture Decision Records

---

## Architecture Decision Records (ADRs)

Major architectural decisions are documented in ADRs:

- ADR location: `docs/adr/`
- ADR template: `docs/adr/template.md` (Phase 0 deliverable)
- All core subsystem designs require ADRs
- ADRs link to this architecture document

---

## Summary

Ferrous Kernel is designed as a **hybrid microkernel-inspired system** with:

- **Small trusted core** (<50k lines of Rust) handling memory, scheduling, IPC, and capabilities
- **User-space services** for drivers, filesystems, and system services
- **Capability-based security** with no ambient authority
- **Built-in observability** making every decision traceable
- **Fault isolation** preventing cascading failures
- **Modern workload primitives** for container-like isolation

This architecture prioritizes **correctness and safety** over features, enabling a research platform for exploring what modern kernel design can achieve.

---

**Next Steps**: 
1. Create detailed design documents for each subsystem (Phase 0)
2. Begin implementation of boot and memory management (Phase 1)
3. Iterate on architecture based on implementation experience

---

**Document Status**: This is a living document. As implementation progresses, the architecture will evolve. All changes should be reflected in ADRs and updated design documents.

