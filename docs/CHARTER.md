# Ferrous Kernel - Project Charter

**Version:** 1.0
**Date:** 2026-01-04
**Status:** Active

---

## Vision

Design and build a **modern operating system kernel written primarily in Rust** that directly addresses the structural limitations of existing kernels, rather than layering workarounds on top of decades of legacy decisions.

The kernel prioritizes **security, isolation, observability, and cloud-era workloads** without sacrificing performance.

This is a **research-grade, long-term project**, not a Linux replacement.

---

## Mission Statement

Create a kernel that proves modern language features, capability-based security, and isolation-first design can coexist with high performance and practical usability.

Success means demonstrating that:
- Memory safety eliminates entire bug classes
- Component isolation prevents cascading failures
- Modern workloads deserve first-class kernel support
- Observability enables understanding, not just instrumentation

---

## Core Problems to Solve

### 1. Memory Safety

**The Problem:**
- Traditional kernels written in C/C++ are plagued by memory safety bugs
- Use-after-free, buffer overflows, and data races cause crashes and security vulnerabilities
- Manual memory management scales poorly with kernel complexity

**Our Approach:**
- Eliminate entire classes of kernel bugs through Rust's ownership system
- Unsafe code must be minimal, isolated, and auditable
- All drivers and subsystems must define explicit safety boundaries
- Compiler-verified memory safety without garbage collection overhead

**Success Metric:**
- Zero memory safety CVEs in safe Rust code
- All unsafe code has documented safety invariants
- Unsafe code represents <5% of total codebase

---

### 2. Isolation by Default

**The Problem:**
- Driver bugs crash entire systems
- One faulty component can corrupt kernel memory
- Debugging requires reboots and post-mortem analysis

**Our Approach:**
- No component should be able to crash the whole system
- Drivers, filesystems, and networking stacks should be sandboxed where possible
- Fault containment is a first-class design constraint
- Services can be restarted without system reboot

**Success Metric:**
- Driver crash does not panic kernel
- Services can be killed and restarted in <100ms
- Component failures are isolated to their fault domains

---

### 3. First-Class Modern Workloads

**The Problem:**
- Containers, microservices, and ephemeral workloads are bolt-on abstractions
- Namespaces and cgroups were retrofitted onto existing kernel architecture
- Resource isolation has high overhead

**Our Approach:**
- Containers and microservices are core kernel primitives, not afterthoughts
- Resource groups, namespaces, and capabilities designed-in from day one
- Scheduling, memory, and I/O APIs reflect cloud-native reality
- Fast creation/destruction of isolated execution contexts

**Success Metric:**
- Container creation in <10ms
- Resource accounting with <1% overhead
- Native support for namespace-style isolation

---

### 4. Built-In Observability

**The Problem:**
- Kernel behavior is a black box
- Performance issues require attaching external tools (perf, eBPF)
- Debugging relies on printf/printk and prayer
- Tracing is fragmented across multiple systems

**Our Approach:**
- Every kernel decision should be explainable
- Tracing, metrics, and causality tracking are not optional tools
- Observability is designed in, not bolted on
- Structured events with zero-copy export

**Success Metric:**
- All syscalls, scheduling decisions, and resource allocations are traceable
- <5% overhead for full tracing
- Export to standard formats (OpenTelemetry, etc.)

---

### 5. Small Trusted Core

**The Problem:**
- Monolithic kernels have millions of lines of privileged code
- Large attack surface
- Difficult to audit or verify

**Our Approach:**
- Minimize the amount of code running with full privileges
- Favor message passing and capability-based access
- Move functionality to user-space where practical
- Keep the kernel auditable and reason-able

**Success Metric:**
- Core kernel (privileged code) under 50k lines of Rust
- 90%+ of drivers in user-space
- Clear trust boundaries with documented security assumptions

---

### 6. Energy-Aware and Hardware-Conscious

**The Problem:**
- Power consumption ignored in traditional scheduling
- NUMA awareness is an afterthought
- Heterogeneous systems (big.LITTLE) poorly supported

**Our Approach:**
- Scheduling and resource management consider power usage
- Designed for modern CPUs with multiple power domains
- NUMA-aware memory allocation and scheduling
- First-class support for heterogeneous compute

**Success Metric:**
- Measurable energy savings vs Linux on identical workloads
- NUMA-aware allocation reduces remote memory access
- Per-core power state management

---

## Design Principles (Non-Negotiable)

These principles guide every design decision:

1. **Rust is the primary language**
   - C compatibility only at well-defined boundaries
   - Inline assembly only when absolutely necessary

2. **Unsafe Rust is explicit, reviewed, and isolated**
   - Every unsafe block has a safety comment
   - Unsafe code undergoes additional review
   - Abstraction boundaries hide unsafety

3. **One clear abstraction per subsystem**
   - No overlapping or competing abstractions
   - Each concept has a canonical representation

4. **No silent global state**
   - All dependencies are explicit
   - No hidden ambient authority

5. **Compatibility is optional, correctness is not**
   - We will not sacrifice correctness for compatibility
   - POSIX compliance is a non-goal
   - Design for the future, not the past

6. **Simplicity beats features**
   - When in doubt, choose the simpler design
   - Fewer features, better implemented
   - Complexity requires extraordinary justification

7. **Performance through design, not tricks**
   - Avoid clever hacks and micro-optimizations
   - Let the compiler and hardware do their job
   - Measure before optimizing

8. **Fail fast, fail visibly**
   - Panics are better than silent corruption
   - Errors are explicit, never ignored
   - Debug builds are paranoid

---

## Architectural Direction

### Hybrid Microkernel-Inspired Design

- **Minimal privileged core**
  - Memory management
  - Scheduling
  - IPC
  - Capability management

- **User-space services for drivers and subsystems**
  - Device drivers
  - File systems
  - Network stacks
  - System services

- **High-performance IPC**
  - Zero-copy message passing
  - Shared memory with capability control
  - Fast cross-domain calls

### Capability-Based Security Model

- **No ambient authority**
  - All access requires explicit capability
  - No global "root" user
  - Capabilities cannot be forged

- **Explicit resource ownership**
  - Clear ownership chains
  - Capabilities can be delegated with restrictions
  - Revocation is possible

### Message-Passing Over Shared State

- **Predictable behavior**
  - Explicit communication points
  - No hidden data sharing

- **Easier reasoning and debugging**
  - Message logs show system behavior
  - Causality tracking through messages

---

## Target Scope (Initial)

### Hardware
- **Architecture:** x86_64 (amd64)
- **Future:** ARM64 (aarch64), RISC-V (exploratory)
- **Platform:** Modern server hardware, UEFI boot
- **Emulation:** QEMU (primary development platform)

### Environment
- **Target:** Server / Cloud / Research
- **Not targeting:** Desktop, embedded (initially)
- **Use cases:**
  - Container runtime
  - Microservice orchestration
  - Research platform for OS concepts

### Compatibility
- **No GUI** (research phase)
- **No legacy POSIX guarantee**
  - Compatibility layers possible later
  - Native API is primary interface
- **No Linux binary compatibility**
  - Applications must be built for Ferrous

---

## Long-Term Milestones

### Phase 1: Proof of Life (Q2-Q3 2026)
Boot via UEFI, Rust kernel entry, memory setup, logging to serial.

### Phase 2: Core Kernel (Q4 2026 - Q2 2027)
Scheduler, IPC, capability system, user/kernel separation.

### Phase 3: Isolation (Q3-Q4 2027)
User-space drivers, fault containment, restartable services.

### Phase 4: Modern Workloads (2028)
Native container-like abstractions, resource groups as primitives, observability pipeline.

### Phase 5: Research & Validation (2029+)
Benchmarks vs Linux in narrow cases, security audits, formal reasoning where feasible.

See [ROADMAP.md](ROADMAP.md) for detailed breakdown.

---

## Success Criteria

This kernel is successful if:

1. **A single driver crash does not bring down the system**
   - Isolation prevents cascading failures
   - Services can be restarted gracefully

2. **Memory safety bugs are structurally rare**
   - Safe Rust code has zero memory safety issues
   - Unsafe code is minimal and audited

3. **Kernel behavior is observable and explainable**
   - Every decision can be traced
   - Performance issues can be diagnosed

4. **New ideas can be tested without destabilizing the core**
   - Clean abstractions enable experimentation
   - Faults are contained

5. **Performance is competitive for target workloads**
   - Within 2x of Linux for cloud workloads
   - Isolation overhead is acceptable

6. **Code is understandable and maintainable**
   - New contributors can understand the design
   - Documentation reflects reality

---

## What This Is NOT

To maintain focus, we explicitly state what Ferrous is **not trying to be**:

- **Not a Linux replacement**
  - Different goals, different tradeoffs
  - Not targeting existing Linux users or workloads

- **Not a hobby OS with endless features**
  - Focused research goals
  - Feature discipline is critical

- **Not bound by backward compatibility**
  - Free to make breaking changes
  - Learn and iterate without legacy constraints

- **Not production-ready (yet)**
  - Research and validation first
  - Stability comes after correctness

- **Not trying to support all hardware**
  - Focused on modern server platforms
  - Legacy hardware is not a goal

- **Not a desktop/gaming OS**
  - No GUI in research phase
  - Server and cloud workloads first

---

## Governance and Decision-Making

### Design Decisions

Major architectural decisions require:
1. Written proposal (Architecture Decision Record)
2. Discussion with core team
3. Alignment with charter principles
4. Documentation of tradeoffs

### Code Review

All code must be reviewed, with extra scrutiny for:
- Unsafe Rust code
- Security-critical paths
- Performance-sensitive code
- Public API changes

### Philosophy on Change

- **Principles are stable** - This charter changes rarely
- **Design evolves** - We learn as we build
- **Code is malleable** - Refactor when better approaches emerge
- **APIs are versioned** - Breaking changes are okay if justified

---

## Commitment to Research Values

This project embraces research principles:

- **Publish findings** - Share what we learn
- **Open development** - Design documents and discussions are public
- **Honest assessment** - Document failures, not just successes
- **Cite inspiration** - Credit ideas and prior work
- **Collaborate** - Engage with academic and industry researchers

---

## Conclusion

Ferrous is an exploration of what becomes possible when you rethink kernel design with modern tools and constraints.

We're not building the fastest kernel, or the most compatible kernel, or the most feature-complete kernel.

We're building a **correct, understandable, and secure** kernel that demonstrates new approaches to old problems.

This charter is our commitment to that vision.

---

**Approved by:** [Project Maintainers]
**Last Reviewed:** 2026-01-04
**Next Review:** 2027-01-04
