# ADR-0001: Kernel Entry Point Handoff and UEFI Boot Services Exit Strategy

**Status:** Proposed
**Date:** 2026-03-01
**Deciders:** iamvirul
**Tags:** boot, uefi, entry-point, safety, no-std

---

## Context

Transitioning control from UEFI firmware to the Ferrous Kernel requires a well-defined, safe, and auditable handoff mechanism. Without a deliberate design, this transition risks memory corruption, undefined behaviour, and an unrecoverable boot failure.

The key problems to solve:

- UEFI boot services provide memory, protocol, and runtime APIs. After `exit_boot_services()` is called, all of these are gone. The UEFI memory map — the only authoritative record of physical memory — must be captured **before** that call.
- The UEFI firmware-provided stack is reclaimed when boot services exit. Rust code cannot run without a valid stack.
- No Rust allocator exists at this point. All early-boot structures must be statically allocated.
- The kernel and bootloader are separate compilation units. The information contract between them must be explicit and versioned.
- Every `unsafe` operation in this path must have a documented invariant (per `UNSAFE_GUIDELINES.md`).

**Current state:** The UEFI bootloader (`boot/src/main.rs`) compiles and runs as a UEFI application (Task 1.1.1 complete). There is no kernel entry point, no `BootInfo` struct, and no defined ABI boundary between bootloader and kernel.

**Related GitHub Issues:**
- Proposed in: #18 ([ADR Proposal](https://github.com/iamvirul/ferrous-kernel/issues/18))
- Implements: #3 ([Phase 1.1.2: Kernel Entry Point Handoff](https://github.com/iamvirul/ferrous-kernel/issues/3))
- Blocks: #6 (Kernel Stack Setup), #7 (GDT Init), #8 (IDT Config), #10 (Parse UEFI Memory Map)

---

## Decision

> We will use a **two-stage handoff** design where the bootloader captures the UEFI memory map into a statically allocated `BootInfo` buffer, exits boot services, switches to a kernel bootstrap stack, and jumps to a Rust `extern "C"` kernel entry point that validates `BootInfo` and transfers to `kernel_main`.

### Stage 1 — Bootloader Side (`boot/`)

Executed while UEFI boot services are still active:

1. Retrieve the UEFI memory map via `BootServices::memory_map()`.
2. Copy all memory descriptors into a statically allocated `MemoryMapBuffer` embedded in `BootInfo`.
3. Populate the full `BootInfo` struct (memory map, ACPI RSDP pointer, framebuffer info, bootloader name).
4. Call `exit_boot_services()` — **no UEFI calls after this point**.
5. Execute `cli` to disable interrupts.
6. Load the kernel bootstrap stack pointer (statically allocated, defined in linker script).
7. Jump to `kernel_entry` symbol as `extern "C"`, passing `&BootInfo` as the first argument (RDI register per SysV AMD64 ABI).

### Stage 2 — Kernel Side (`kernel/`)

Executed with UEFI gone, interrupts disabled, on the kernel bootstrap stack:

1. Receive `*const BootInfo` in RDI.
2. Validate the `BootInfo` magic value — halt with a serial error if wrong.
3. Zero the BSS section (required for correct Rust static variable initialisation).
4. Call `kernel_main(boot_info: &'static BootInfo)`.

### `BootInfo` ABI Contract

```rust
/// Magic sentinel: detects stale pointers or ABI version mismatches.
pub const BOOT_INFO_MAGIC: u64 = 0xFE220B007_CAFE0001;
pub const BOOT_INFO_VERSION: u32 = 1;

/// Maximum number of UEFI memory descriptors to store.
pub const MEMORY_MAP_MAX_ENTRIES: usize = 256;

#[repr(C)]
pub struct BootInfo {
    pub magic:           u64,
    pub version:         u32,
    pub _pad:            u32,
    pub memory_map:      MemoryMapBuffer,
    pub framebuffer:     Option<FramebufferInfo>,
    pub acpi_rsdp:       Option<u64>, // physical address of RSDP
    pub bootloader_name: [u8; 32],
}

#[repr(C)]
pub struct MemoryMapBuffer {
    pub entries:     [MemoryDescriptor; MEMORY_MAP_MAX_ENTRIES],
    pub entry_count: usize,
    pub map_key:     usize,
    pub desc_size:   usize,
}

#[repr(C)]
pub struct MemoryDescriptor {
    pub ty:            u32,
    pub _pad:          u32,
    pub phys_start:    u64,
    pub virt_start:    u64,
    pub page_count:    u64,
    pub attribute:     u64,
}

#[repr(C)]
pub struct FramebufferInfo {
    pub base:         u64,
    pub size:         u64,
    pub width:        u32,
    pub height:       u32,
    pub stride:       u32,
    pub pixel_format: u32,
}
```

`BootInfo` is placed in a `static mut` in the bootloader, populated before `exit_boot_services()`, and its address is passed to the kernel. The kernel treats the pointer as `&'static BootInfo` after validation.

---

## Rationale

**Why a shared `boot-info` crate?**
Both the bootloader and kernel must agree on the `BootInfo` layout. A dedicated `no_std` crate with `#[repr(C)]` structs is the only safe way to enforce this at compile time. A mismatch would cause silent memory corruption.

**Why a static buffer instead of heap allocation?**
There is no allocator at handoff time. The static buffer approach requires no allocator and has a fixed, predictable footprint. UEFI systems typically have under 256 memory map entries; `MEMORY_MAP_MAX_ENTRIES = 256` is a safe upper bound.

**Why keep assembly to a minimum?**
Rust inline `asm!` + `extern "C"` achieves the same outcome as a pure assembly `_start` with far less surface area. Fewer assembly lines means the handoff is easier to read, audit, and test on future architectures.

**Why a magic field instead of a null check?**
A null pointer dereference is caught by many environments but a stale or shifted pointer is not. The magic constant detects the common case where the pointer is plausible-looking but points at the wrong structure (e.g., after a linker script change).

**Why `cli` before the stack switch?**
An interrupt between `exit_boot_services()` and the stack switch would use the (now-invalid) UEFI stack. Disabling interrupts makes this window safe. Interrupts are re-enabled only after GDT and IDT are installed (Tasks 1.2.2 and 1.2.3).

**Alignment with Charter Principles:**
- *Correctness first* — fail loudly on bad magic, never proceed with a corrupt `BootInfo`
- *Unsafe Rust is explicit and isolated* — every unsafe block in this path will have a `// SAFETY:` comment
- *Simplicity beats features* — two clear stages, no external boot-protocol dependency
- *No silent global state* — `BootInfo` is the single explicit dependency injected at kernel entry
- *Fail fast, fail visibly* — bad magic → serial error → halt

---

## Alternatives Considered

### Alternative 1: Pure Assembly Entry Point (`_start`)

**Description:** Write the entry point entirely in NASM/GAS assembly, manually set up the stack and BSS, then `call` into Rust.

**Pros:**
- Maximum explicit control
- Standard approach for bare-metal C kernels

**Cons:**
- More assembly to maintain and audit
- Diverges from the Rust-first philosophy
- Harder to unit-test assembly logic

**Why Not Chosen:** Rust `extern "C"` + `asm!` for the stack switch achieves identical results with significantly less assembly surface area.

---

### Alternative 2: phil-opp `bootloader` Crate

**Description:** Use the well-known `bootloader` crate which handles UEFI loading, page table setup, and passes a `BootInfo` to the kernel automatically.

**Pros:**
- Well-tested and widely used
- Handles many early-boot details automatically

**Cons:**
- Black-boxes the decisions we explicitly want to understand and control
- Ties the kernel to a third-party abstraction with its own `BootInfo` layout
- Reduces the research and learning value of the project

**Why Not Chosen:** Ferrous Kernel is a research project. Owning the handoff design is a goal, not a burden.

---

### Alternative 3: Limine Boot Protocol

**Description:** Use the Limine bootloader protocol, where the kernel declares requests via special linker sections and Limine fills them at boot time.

**Pros:**
- Modern, feature-rich, widely adopted in hobby-OS community
- Handles memory map, framebuffer, SMP, HHDM automatically

**Cons:**
- Adds a mandatory external bootloader dependency (Limine binary)
- The UEFI → kernel transition is hidden inside Limine
- Makes the boot process less transparent and harder to understand end-to-end

**Why Not Chosen:** Direct UEFI → kernel is the most transparent path and aligns with the project's observability-first principle.

---

## Consequences

### Positive

- A versioned `BootInfo` contract prevents silent ABI drift between bootloader and kernel.
- The kernel has zero UEFI dependency after `kernel_entry` is reached — clean mental model.
- Minimal assembly surface area keeps the handoff auditable and architecture-portable.
- The static bootstrap stack eliminates the allocator chicken-and-egg problem entirely.
- A shared `boot-info` crate makes the ABI boundary a compile-time contract.

### Negative

- We own all complexity that a bootloader crate would otherwise hide.
- `MEMORY_MAP_MAX_ENTRIES = 256` is a compile-time constant; pathological firmware with more entries would silently truncate. Mitigation: assert at runtime and halt if truncation occurs.

### Risks

- **Stack too small:** If the bootstrap stack is undersized, early Rust code will silently corrupt adjacent memory. Mitigation: allocate 16 KiB, place a known guard pattern below it, check in a debug assertion.
- **BSS not zeroed:** If `kernel_entry` fails to zero BSS before calling Rust, any `static` variable with a non-zero initial value will be incorrect. Mitigation: zero BSS unconditionally before calling `kernel_main`, add a test that checks a known static.
- **Magic collision:** The magic constant could theoretically collide with data at a wrong pointer. Mitigation: the constant was chosen to be highly distinctive; the risk is negligible.

### Implementation Notes

- `boot-info` crate must be `no_std` and have no dependencies.
- `BootInfo` must be `#[repr(C)]` to guarantee layout between separately compiled crates.
- The BSS-zeroing loop must use `volatile_write` or `core::ptr::write_volatile` to prevent the optimiser from eliding it.
- After `exit_boot_services()`, no UEFI calls are permitted — add a `#[cfg(debug_assertions)]` flag that panics on any UEFI access post-exit.

---

## Safety and Security Considerations

**Unsafe code required in this path:**

| Location | Operation | Invariant |
|----------|-----------|-----------|
| `boot/src/main.rs` | `exit_boot_services()` | Memory map must already be copied; no UEFI calls will follow |
| `boot/src/main.rs` | Inline `asm!` stack switch | Bootstrap stack is valid, interrupts are disabled |
| `kernel/arch/x86_64/entry.rs` | Dereference `*const BootInfo` | Pointer is non-null and points to the statically allocated `BootInfo` in bootloader memory |
| `kernel/arch/x86_64/entry.rs` | BSS zero loop (`write_volatile`) | Range covers exactly the BSS section as defined by linker symbols |

Each block will carry a `// SAFETY:` comment explaining these invariants per `UNSAFE_GUIDELINES.md`.

**Security implications:**
- The `BootInfo` magic check prevents the kernel from operating with a garbage pointer, which could otherwise be used to forge memory maps and corrupt the physical frame allocator.
- Boot-time memory map is the root of all physical memory knowledge in the kernel. Validation is mandatory, not optional.

---

## Performance Considerations

The handoff is a **one-time boot path**. Performance is irrelevant relative to correctness. The static allocation approach has zero runtime overhead. The BSS zeroing loop is O(BSS size) and runs once.

---

## Dependencies

**Depends on:**
- Task 1.1.1: UEFI Bootloader Integration (complete)
- Linker script (`kernel.ld`) defining `.bss` start/end symbols and the bootstrap stack section

**Blocks:**
- #3 Phase 1.1.2: Kernel Entry Point Handoff (direct implementation)
- #6 Phase 1.2.1: Kernel Stack Setup (permanent stack replaces bootstrap stack)
- #7 Phase 1.2.2: GDT Initialization (first thing after `kernel_main` entry)
- #8 Phase 1.2.3: IDT Configuration
- #10 Phase 1.3.1: Parse UEFI Memory Map (consumes `BootInfo.memory_map`)

---

## Implementation Plan

- [ ] Create `lib/boot-info/` crate with `BootInfo`, `MemoryMapBuffer`, `FramebufferInfo`, `MemoryDescriptor` structs
- [ ] Add `boot-info` as a dependency in both `boot/Cargo.toml` and `kernel/Cargo.toml`
- [ ] Update `boot/src/main.rs`: populate `BootInfo`, copy memory map, call `exit_boot_services()`, disable interrupts, switch stack, jump to kernel
- [ ] Add bootstrap stack section to `kernel.ld` (16 KiB, aligned to 16 bytes)
- [ ] Create `kernel/src/arch/x86_64/entry.rs`: validate magic, zero BSS, call `kernel_main`
- [ ] Create `kernel/src/main.rs`: `kernel_main(boot_info: &'static BootInfo)` stub with serial "Ferrous Kernel booted" output
- [ ] Add debug guard pattern below bootstrap stack
- [ ] Write unit tests for `BootInfo` magic/version validation logic
- [ ] Update `docs/BOOT_ARCHITECTURE.md` to reference this ADR

---

## References

**Project Documents:**
- [BOOT_ARCHITECTURE.md](../BOOT_ARCHITECTURE.md) — Boot process design
- [CHARTER.md](../CHARTER.md) — Design principles
- [UNSAFE_GUIDELINES.md](../UNSAFE_GUIDELINES.md) — Unsafe Rust policy
- [ADR README](README.md) — ADR guidelines and process

**GitHub Issues:**
- Proposal: #18 ([ADR Proposal](https://github.com/iamvirul/ferrous-kernel/issues/18))
- Implementation: #3 ([Phase 1.1.2: Kernel Entry Point Handoff](https://github.com/iamvirul/ferrous-kernel/issues/3))

**External References:**
- [UEFI Specification 2.10](https://uefi.org/specifications) — `EFI_BOOT_SERVICES.ExitBootServices()`
- [System V AMD64 ABI](https://refspecs.linuxbase.org/elf/x86_64-abi-0.99.pdf) — Calling convention (RDI = first argument)
- [uefi-rs crate documentation](https://docs.rs/uefi) — Rust UEFI bindings used in `boot/`

---

## Notes

- `MEMORY_MAP_MAX_ENTRIES = 256` should be reviewed against real hardware before Phase 1 ships. OVMF (QEMU's UEFI firmware) typically produces ~20–40 entries.
- Future work: once the kernel heap is available, `BootInfo` can be copied into a heap-allocated structure and the static buffer reclaimed.
- If we later support a second architecture (aarch64), Stage 2 will need an arch-specific `entry.rs`. The `BootInfo` ABI (being `#[repr(C)]`) will remain unchanged.

---

## Status History

- 2026-03-01: Created (Status: Proposed)

---

## Approval

- **Proposed by:** iamvirul on 2026-03-01
