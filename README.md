# Ferrous Kernel

**A next-generation operating system kernel written in Rust**

> Rethinking kernel design for security, isolation, and modern workloads

---

## What is Ferrous?

Ferrous is a research-grade operating system kernel that addresses fundamental limitations of existing kernels through modern language features, capability-based security, and first-class support for cloud-era workloads.

This is **not** a Linux replacement. It's a long-term research project exploring what becomes possible when you design a kernel from scratch without decades of backward compatibility constraints.

---

## Core Principles

**Memory Safety**
- Eliminate entire classes of bugs through Rust's type system
- Minimal, auditable unsafe code with clear safety boundaries

**Isolation by Default**
- Driver crashes don't bring down the system
- Fault containment is a first-class design constraint
- Sandboxed components where possible

**Modern Workloads First**
- Containers and microservices as kernel primitives, not abstractions
- Resource groups, namespaces, and observability built-in

**Built-In Observability**
- Every kernel decision is explainable
- Tracing, metrics, and causality tracking from day one

**Small Trusted Core**
- Minimize privileged code
- Capability-based security model
- Message passing over shared state

---

## Current Status

**Phase:** Foundation & Design (Phase 0)
**Target Architecture:** x86_64 (ARM64 planned)
**Target Environment:** Server / Cloud / Research

See [ROADMAP.md](docs/ROADMAP.md) for detailed development plan.

---

## Project Structure

```
ferrous-kernel/
├── docs/               # Documentation and design documents
├── kernel/             # Core kernel code
│   ├── src/           # Platform-independent kernel code
│   └── arch/          # Architecture-specific implementations
│       ├── x86_64/    # x86-64 support
│       └── aarch64/   # ARM64 support (future)
├── drivers/           # Driver subsystems
│   ├── block/        # Block device drivers
│   ├── net/          # Network drivers
│   └── char/         # Character device drivers
├── userspace/         # User-space components
│   ├── init/         # Init system
│   └── services/     # System services
├── boot/              # Bootloader and early boot
├── lib/               # Shared libraries
│   ├── core/         # Core utilities
│   └── alloc/        # Allocation primitives
├── tools/             # Development tools
├── tests/             # Testing infrastructure
└── scripts/           # Build and utility scripts
```

---

## Key Design Decisions

### Why Rust?

- **Memory safety without garbage collection** - Critical for kernel performance
- **Zero-cost abstractions** - Safety doesn't mean slow
- **Fearless concurrency** - Compiler-verified thread safety
- **Explicit unsafe boundaries** - Audit surface is minimized

### Why Capabilities?

- **No ambient authority** - Explicit permission model
- **Composable security** - Build complex policies from simple rules
- **Audit-friendly** - Clear authorization chain

### Why Microkernel-Inspired?

- **Fault isolation** - Component failures don't cascade
- **Easier verification** - Smaller trusted computing base
- **Flexibility** - Swap components without kernel changes

*Note: "Microkernel-inspired" means we adopt good ideas (isolation, small TCB) while pragmatically keeping performance-critical paths in kernel space.*

---

## What Makes Ferrous Different?

| Aspect | Traditional Kernels | Ferrous |
|--------|-------------------|---------|
| **Memory Safety** | C/C++, manual memory management | Rust, compiler-verified safety |
| **Driver Crashes** | Often panic entire system | Isolated, restartable services |
| **Containers** | Bolt-on via namespaces/cgroups | First-class kernel primitives |
| **Observability** | Added via external tools | Built-in tracing and metrics |
| **Security Model** | DAC/MAC, ambient authority | Capability-based, explicit grants |
| **Unsafe Code** | Entire kernel | Minimal, isolated, audited |

---

## Getting Started

### Prerequisites

- Rust toolchain (nightly)
- QEMU (for testing)
- Cross-compilation tools for x86_64

*(Detailed setup guide coming in Phase 0)*

### Building

```bash
# Clone the repository
git clone https://github.com/yourusername/ferrous-kernel.git
cd ferrous-kernel

# Build the kernel
cargo build --release

# Run in QEMU
./scripts/run-qemu.sh
```

*(Build system is work-in-progress)*

---

## Documentation

- [ROADMAP.md](docs/ROADMAP.md) - Development roadmap and milestones
- [ARCHITECTURE.md](docs/ARCHITECTURE.md) - System architecture
- [CONTRIBUTING.md](docs/CONTRIBUTING.md) *(coming soon)* - Contribution guidelines
- [UNSAFE_GUIDELINES.md](docs/UNSAFE_GUIDELINES.md) *(coming soon)* - Unsafe Rust policy

---

## Development Phases

### Phase 0: Foundation & Design (Current)
Establish structure, documentation, and development environment.

### Phase 1: Proof of Life (Q2-Q3 2026)
Boot via UEFI, basic memory management, serial output.

### Phase 2: Core Kernel (Q4 2026 - Q2 2027)
Scheduler, IPC, capability system, first user-space program.

### Phase 3: Isolation (Q3-Q4 2027)
User-space drivers, fault containment, service restart.

### Phase 4: Modern Workloads (2028)
Container primitives, resource groups, observability pipeline.

### Phase 5: Validation (2029+)
Performance benchmarks, security audit, formal verification.

See [ROADMAP.md](docs/ROADMAP.md) for complete details.

---

## Contributing

This is a research project with high technical standards. Contributions are welcome, but expect:

- **Rigorous code review** - Especially for unsafe code
- **Design justification** - Major changes need Architecture Decision Records
- **Test coverage** - New code needs tests
- **Documentation** - Code should be understandable

*(Detailed contributing guide coming soon)*

---

## Non-Goals

To keep the project focused, these are **explicitly out of scope**:

- Desktop/GUI support (research phase)
- Full POSIX compatibility
- Running existing Linux binaries
- Supporting legacy hardware
- Backward compatibility with anything

These might be explored later, but not during core development.

---

## License

*(To be determined - likely permissive open source)*

---

## Inspiration & Related Work

This project stands on the shoulders of giants:

- **seL4** - Formal verification, capability model
- **Redox OS** - Rust microkernel
- **Fuchsia** - Capability-based, modern design
- **Linux** - Decades of kernel wisdom (and lessons in what to avoid)
- **MINIX 3** - Fault tolerance and isolation

---

## FAQ

**Q: When will this be usable?**
A: This is a multi-year research project. Don't expect to run production workloads for several years.

**Q: Why not contribute to Redox OS?**
A: Redox is excellent. Ferrous explores different design points (capability model, observability-first, etc.). Multiple experiments are healthy.

**Q: Will this support my hardware?**
A: Initially only QEMU x86_64. Real hardware support comes later. ARM64 is planned.

**Q: Can I use this for my startup/product?**
A: Not yet. This is research-grade software. Stability and completeness are years away.

**Q: How can I help?**
A: Star the repo, follow development, provide feedback on designs. Code contributions welcome once Phase 1 begins.

---

## Contact

*(Project communication channels to be established)*

---

## Project Status

This project is in its infancy. The roadmap is ambitious. Progress will be incremental.

If you're interested in the long-term future of operating systems, kernel security, or Rust systems programming - welcome. Let's build something remarkable.

---

**"Correctness first. Performance follows. Features last."**
