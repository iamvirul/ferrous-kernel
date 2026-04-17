# Ferrous Kernel - Development Roadmap

**Last Updated:** 2026-04-17
**Status:** Phase 1 — Proof of Life (In Progress)

---

## Overview

This roadmap outlines the development path for Ferrous, a next-generation operating system kernel written in Rust. This is a research-grade, long-term project focused on security, isolation, and modern workload support.

**Project Duration Estimate:** Multi-year effort
**Current Phase:** Phase 1 — Proof of Life

---

## Guiding Principles

Every milestone must advance these core goals:

1. **Memory Safety** - Eliminate entire bug classes through Rust
2. **Isolation by Default** - Component failures don't cascade
3. **First-Class Modern Workloads** - Containers/microservices as primitives
4. **Built-In Observability** - Every decision is explainable
5. **Small Trusted Core** - Minimize privileged code
6. **Energy-Aware** - Power-conscious scheduling and resource management

---

## Phase 0: Foundation & Design — COMPLETE (Q1 2026)

**Goal:** Establish project structure, design documents, and development infrastructure.

**Completed:** 2026-01-04

### Deliverables

- [x] Project charter and vision document
- [x] Repository structure
- [x] Development environment setup guide
- [x] Coding standards and unsafe Rust guidelines
- [x] Architecture Decision Records (ADR) template
- [x] Build system (cargo-based workspace)
- [x] CI/CD pipeline skeleton
- [x] Pre-commit formatting hook (`.githooks/pre-commit`)
- [x] Initial design documents:
  - [x] Memory management architecture
  - [x] Capability system design
  - [x] IPC mechanism design
  - [x] Boot process overview

### Success Criteria — MET

- Any contributor can clone, build, and understand the project structure
- Core architectural decisions are documented with rationale
- Unsafe Rust usage policy is clear and enforced

---

## Phase 1: Proof of Life — IN PROGRESS (Q2-Q3 2026)

**Goal:** Boot into kernel space and establish basic runtime environment.

**Started:** 2026-01-04

### Architecture Decisions

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-0001](adr/ADR-0001-kernel-entry-point-handoff.md) | Kernel Entry Point Handoff and UEFI Boot Services Exit Strategy | Approved |

### Milestones

#### 1.1 - Bare Metal Boot

| Task | Issue | Status |
|------|-------|--------|
| 1.1.1 UEFI Bootloader Integration | — | Complete (PR #15) |
| 1.1.2 Kernel Entry Point Handoff | #3 | Complete (PR #56) |
| 1.1.3 Basic Serial Output | #4 | Complete (PR #58) |
| 1.1.4 Verify Execution on QEMU and Hardware | #5 | Complete (PR #59) |

**Notes:**
- `lib/boot-info` crate added — shared `#[repr(C)]` `KernelBootInfo` ABI between bootloader and kernel
- UEFI boot services exit, bootstrap stack switch, and `kernel_entry` validated on QEMU (99 memory map entries, ACPI RSDP, framebuffer all passed through correctly)
- `kernel/src/drivers/serial.rs` added — `SerialPort` struct with full 16550 UART init (115200 baud, 8N1); "Hello from Ferrous!" confirmed on QEMU serial console
- `scripts/verify-boot.sh` added — automated boot verification for CI; `docs/QEMU_TESTING.md` documents expected output and troubleshooting
- `kernel/src/arch/x86_64/stack.rs` added — `KernelStack<N>` type with `top()`/`bottom()` and guard-region constants; 64 KiB primary stack active in `kernel_main`, bounds printed to serial
- `kernel/src/arch/x86_64/gdt.rs` added — minimal 3-entry GDT (null, kernel-code 0x08, kernel-data 0x10); loaded via `LGDT`, CS reloaded via far-return (`RETFQ`), data segments reloaded; verified active in QEMU serial output
- `kernel/src/arch/x86_64/idt.rs` added — 256-entry IDT with `IdtEntry`, `IdtPointer`, `ExceptionFrame` types and `unsafe load()`; 32 exception stubs (vectors 0-31) + generic IRQ stub (32-255) via `global_asm!`; `LIDT` loaded, interrupts remain disabled; verified active in QEMU serial output
- Exception stubs upgraded — two `global_asm!` macro variants: `isr_stub` (no error code: RDI=vector, RSI=0, RDX=frame ptr) and `isr_stub_ec` (error code popped into RSI, RDX=frame ptr); `exception_handler()` prints vector name, error code (for vectors 8,10-14,17,21,29,30), faulting RIP+RFLAGS+RSP from the CPU-pushed `ExceptionFrame`, and CR2 for #PF (vector 14); boot verification passes
- `MemoryMap`, `MemoryRegionKind`, `MemoryStats`, `ParseError` added to `ferrous-boot-info` — parses `KernelMemoryMap` from boot info, classifies UEFI memory types into kernel-relevant buckets, computes usable/reclaimable/total byte statistics; 45 host-side tests pass
- `kernel/src/memory/mod.rs` added — global `MemoryMap` storage with `init()` / `get()` API backed by `MaybeUninit` + `AtomicBool`; Phase 1.3.2 physical allocator consumes this
- `boot/src/main.rs` kernel_main Step 5 — prints full memory region table (base, end, size, type) and RAM summary to serial on every boot

#### 1.2 - Runtime Setup

| Task | Issue | Status |
|------|-------|--------|
| 1.2.1 Kernel Stack Setup | #6 | Complete (PR #60) |
| 1.2.2 GDT (Global Descriptor Table) Initialization | #7 | Complete (PR #61) |
| 1.2.3 IDT (Interrupt Descriptor Table) Configuration | #8 | Complete (PR #62) |
| 1.2.4 Basic Exception Handlers | #9 | Complete (PR #63) |

#### 1.3 - Memory Management Foundation

| Task | Issue | Status |
|------|-------|--------|
| 1.3.1 Parse UEFI Memory Map | #10 | Complete (PR #64) |
| 1.3.2 Physical Memory Allocator | #13 | Not Started |
| 1.3.3 Virtual Memory Setup | #14 | Not Started |
| 1.3.4 Page Table Management | #19 | Not Started |
| 1.3.5 Kernel Heap Allocator | #20 | Not Started |

#### 1.4 - Core Infrastructure

| Task | Issue | Status |
|------|-------|--------|
| 1.4.1 Logging Framework | #21 | Not Started |
| 1.4.2 Panic Handler with Stack Traces | #22 | Not Started |
| 1.4.3 Basic Assertions and Debug Macros | #23 | Not Started |
| 1.4.4 Serial Console Driver | #24 | Not Started |

### Success Criteria

- [x] Kernel boots on QEMU x86_64
- [x] Can print "Hello from Ferrous!" to serial console
- [x] Page fault handler catches and reports violations
- [ ] Clean panic messages with source locations

---

## Phase 2: Core Kernel Services (Q4 2026 - Q2 2027)

**Goal:** Implement fundamental kernel abstractions and user-space transition.

### Architecture Decisions

| ADR | Title | Status |
|-----|-------|--------|
| ADR-0002 | Process Model and Task Representation | Proposed |
| ADR-0003 | Scheduler Algorithm Selection | Proposed |
| ADR-0004 | Capability System Data Structures | Proposed |
| ADR-0005 | IPC Mechanism Design | Proposed |

### Milestones

#### 2.1 - Process Abstractions

| Task | Issue | Status | Priority |
|------|-------|--------|----------|
| 2.1.1 Task and Process Data Structures | — | Not Started | Critical |
| 2.1.2 Address Space Management | — | Not Started | Critical |
| 2.1.3 ELF Binary Loader | — | Not Started | High |
| 2.1.4 Kernel/User Mode Transition | — | Not Started | Critical |
| 2.1.5 System Call Interface (minimal) | — | Not Started | Critical |

**Dependencies:** Phase 1.3 (memory management) must be complete.

#### 2.2 - Scheduler

| Task | Issue | Status | Priority |
|------|-------|--------|----------|
| 2.2.1 Runnable Task Queue | — | Not Started | Critical |
| 2.2.2 Basic Round-Robin Scheduler | — | Not Started | Critical |
| 2.2.3 Context Switching (assembly + Rust) | — | Not Started | Critical |
| 2.2.4 Timer Interrupts (PIT/APIC) | — | Not Started | High |
| 2.2.5 Idle Task | — | Not Started | High |

**Dependencies:** 2.1.1 (task structures) must be complete.

#### 2.3 - Capability System Foundation

| Task | Issue | Status | Priority |
|------|-------|--------|----------|
| 2.3.1 Capability Data Structures | — | Not Started | Critical |
| 2.3.2 Capability Derivation and Delegation | — | Not Started | Critical |
| 2.3.3 Capability-Based Syscalls (grant, revoke, derive) | — | Not Started | High |
| 2.3.4 Initial Capability Space for Processes | — | Not Started | High |

**Dependencies:** 2.1.5 (syscall interface) must be complete.

#### 2.4 - IPC Primitives

| Task | Issue | Status | Priority |
|------|-------|--------|----------|
| 2.4.1 Endpoint and Channel Data Structures | — | Not Started | Critical |
| 2.4.2 Synchronous Send/Receive | — | Not Started | Critical |
| 2.4.3 Message Buffer Management | — | Not Started | High |
| 2.4.4 IPC Capability Integration | — | Not Started | High |

**Dependencies:** 2.2 (scheduler) and 2.3.1 (capability structures) must be complete.

#### 2.5 - First User-Space Program

| Task | Issue | Status | Priority |
|------|-------|--------|----------|
| 2.5.1 Minimal Init Process | — | Not Started | Critical |
| 2.5.2 Core Syscalls (exit, yield, send, receive) | — | Not Started | Critical |
| 2.5.3 Load and Execute First Userspace Binary | — | Not Started | Critical |

**Dependencies:** 2.1 through 2.4 must be complete.

### Success Criteria

- [ ] Can load and run a simple userspace program
- [ ] Program can send/receive messages via IPC
- [ ] Scheduler switches between multiple tasks
- [ ] Capabilities prevent unauthorized access
- [ ] Kernel remains stable when a user-space task crashes

---

## Phase 3: Isolation & Fault Containment (Q3-Q4 2027)

**Goal:** Move components out of kernel, prove isolation works.

### Milestones

#### 3.1 - User-Space Driver Framework
- [ ] Driver capability model
- [ ] MMIO/PIO access control via capabilities
- [ ] Interrupt routing to user-space
- [ ] DMA buffer management (safe)

#### 3.2 - First User-Space Driver
- [ ] Serial driver moved to userspace
- [ ] Kernel acts as intermediary (temporary)
- [ ] Verify crash doesn't panic kernel

#### 3.3 - Service Restart Infrastructure
- [ ] Service monitoring framework
- [ ] Automatic restart policies
- [ ] State recovery mechanisms
- [ ] Dependency tracking

#### 3.4 - Fault Injection Testing
- [ ] Deliberate driver crashes
- [ ] Measure recovery time
- [ ] Verify no kernel memory corruption
- [ ] Test dependency cascades

### Success Criteria

- Kill a user-space driver; system continues running
- Driver can be restarted without reboot
- Kernel remains stable under driver faults
- Clear fault isolation boundaries

---

## Phase 4: Modern Workload Primitives (2028)

**Goal:** Native support for container-like isolation and resource management.

### Milestones

#### 4.1 - Resource Groups
- [ ] Namespace-like isolation (PID, mount, network)
- [ ] Resource limits (CPU, memory, I/O)
- [ ] Cgroup-style hierarchies
- [ ] Capability-based group management

#### 4.2 - Container Runtime Interface
- [ ] Create/destroy isolated groups
- [ ] Bind capabilities to groups
- [ ] Process migration between groups
- [ ] Resource accounting per group

#### 4.3 - Observability Pipeline
- [ ] Structured event tracing
- [ ] Per-task/per-group metrics
- [ ] Causality tracking (request tracing)
- [ ] Export to standard formats (OpenTelemetry)

#### 4.4 - Network Stack (Basic)
- [ ] Ethernet driver (user-space)
- [ ] TCP/IP stack (user-space or kernel)
- [ ] Socket-like IPC abstractions
- [ ] Network namespacing

### Success Criteria

- Can run isolated "containers" without external tools
- Resource usage is observable and attributable
- Network communication works within/across groups

---

## Phase 5: Performance & Validation (2029+)

**Goal:** Prove the design works at scale and under scrutiny.

### Milestones

#### 5.1 - Benchmarking
- [ ] Syscall latency vs Linux
- [ ] IPC throughput vs Linux, seL4
- [ ] Context switch overhead
- [ ] Driver isolation overhead
- [ ] Container creation time

#### 5.2 - Security Audit
- [ ] External security review
- [ ] Unsafe Rust audit
- [ ] Capability model verification
- [ ] Fuzzing infrastructure
- [ ] Exploit mitigation analysis

#### 5.3 - Formal Verification (Exploratory)
- [ ] Identify critical subsystems for proofs
- [ ] Memory safety proofs for core allocators
- [ ] IPC correctness properties
- [ ] Capability invariants

#### 5.4 - Real-World Workloads
- [ ] Run web server (user-space)
- [ ] Run database (SQLite, embedded)
- [ ] Run container orchestrator
- [ ] Measure energy efficiency

### Success Criteria

- Performance within 2x of Linux for target workloads
- Zero critical security findings
- Can run non-trivial applications
- Design tradeoffs are well-understood and documented

---

## Future Considerations (Beyond 2029)

These are **not commitments**, but areas for exploration:

- ARM64 (aarch64) support
- RISC-V port
- GPU driver framework
- Persistent memory support
- Distributed tracing across nodes
- Real-time scheduling guarantees
- Power management (ACPI, device PM)
- POSIX compatibility layer (optional)
- GUI/Wayland support (research)

---

## Key Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Scope creep | High | Strict phase gates, no features outside roadmap |
| Performance too slow | Medium | Early benchmarking, profile-guided optimization |
| Capability model too complex | Medium | Prototype early, iterate on usability |
| Unsafe Rust bugs | High | Mandatory code review, fuzzing, audit |
| Contributor burnout | High | Clear milestones, celebrate small wins |
| Hardware compatibility | Medium | Focus on QEMU first, real hardware later |

---

## Decision Framework

When facing design choices, prioritize in this order:

1. **Correctness** - Does it work as specified?
2. **Safety** - Can it cause undefined behavior or crashes?
3. **Simplicity** - Is this the simplest solution?
4. **Performance** - Is it fast enough?
5. **Features** - Does it enable new capabilities?

If a feature conflicts with safety or simplicity, **reject it**.

---

## How to Use This Roadmap

- **Phases are sequential** - Don't skip to Phase 3 before Phase 2 is solid
- **Milestones within a phase can overlap** - Parallelize where possible
- **Success criteria are gates** - Don't move forward until met
- **Update this document** - As we learn, the roadmap will evolve
- **Link to ADRs** - Major decisions get Architecture Decision Records

---

## Contributing to the Roadmap

If you want to propose changes:

1. Open an issue describing the change and rationale
2. Discuss tradeoffs with the team
3. Create an ADR if it affects architecture
4. Update this roadmap via pull request

---

## Conclusion

This roadmap is ambitious. That's intentional.

Ferrous is not a weekend project. It's a long-term research effort to explore what a modern kernel can be when freed from decades of legacy constraints.

Success means:
- Building something **correct and understandable**
- Proving that **isolation and performance can coexist**
- Creating a platform for **future OS research**

Let's build it right.
