# Ferrous Kernel - Development Roadmap

**Last Updated:** 2026-03-07
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
| 1.1.4 Verify Execution on QEMU and Hardware | #5 | Not Started |

**Notes:**
- `lib/boot-info` crate added — shared `#[repr(C)]` `KernelBootInfo` ABI between bootloader and kernel
- UEFI boot services exit, bootstrap stack switch, and `kernel_entry` validated on QEMU (99 memory map entries, ACPI RSDP, framebuffer all passed through correctly)
- `kernel/src/drivers/serial.rs` added — `SerialPort` struct with full 16550 UART init (115200 baud, 8N1); "Hello from Ferrous!" confirmed on QEMU serial console

#### 1.2 - Runtime Setup

| Task | Issue | Status |
|------|-------|--------|
| 1.2.1 Kernel Stack Setup | #6 | Not Started |
| 1.2.2 GDT (Global Descriptor Table) Initialization | #7 | Not Started |
| 1.2.3 IDT (Interrupt Descriptor Table) Configuration | #8 | Not Started |
| 1.2.4 Basic Exception Handlers | #9 | Not Started |

#### 1.3 - Memory Management Foundation

| Task | Issue | Status |
|------|-------|--------|
| 1.3.1 Parse UEFI Memory Map | #10 | Not Started |
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
- [ ] Page fault handler catches and reports violations
- [ ] Clean panic messages with source locations

---

## Phase 2: Core Kernel Services (Q4 2026 - Q2 2027)

**Goal:** Implement fundamental kernel abstractions and user-space transition.

### Milestones

#### 2.1 - Process Abstractions
- [ ] Task/process structures
- [ ] Address space management
- [ ] ELF loader (basic)
- [ ] User/kernel mode switching
- [ ] System call interface (minimal)

#### 2.2 - Scheduler
- [ ] Runnable task queue
- [ ] Basic round-robin scheduler
- [ ] Context switching (assembly + Rust)
- [ ] Timer interrupts (PIT/APIC)
- [ ] Idle task

#### 2.3 - Capability System Foundation
- [ ] Capability data structures
- [ ] Capability derivation rules
- [ ] Capability-based syscalls (grant, revoke, derive)
- [ ] Initial capability space for processes

#### 2.4 - IPC Primitives
- [ ] Message passing interface design
- [ ] Synchronous send/receive
- [ ] Message buffer management
- [ ] Basic endpoint addressing

#### 2.5 - First User-Space Program
- [ ] Minimal init process
- [ ] Syscall: exit, yield, send, receive
- [ ] Load and execute first userspace binary

### Success Criteria

- Can load and run a simple userspace program
- Program can send/receive messages via IPC
- Scheduler switches between multiple tasks
- Capabilities prevent unauthorized access

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
