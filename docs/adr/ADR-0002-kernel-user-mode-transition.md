# ADR-0002: Kernel and User Mode Transition Mechanism

**Status:** Proposed  
**Date:** 2026-05-27  
**Deciders:** iamvirul  
**Tags:** phase-2, user-mode, syscall, security, performance

---

## Context

To support a secure and isolated operating system environment, Ferrous Kernel must distinguish between privileged kernel execution and unprivileged user-space execution. This is a fundamental requirement for process isolation and system stability. 

Currently, Ferrous Kernel executes entirely in Ring 0 (Kernel Mode). Phase 2.1.4 introduces the concept of user-space processes which must run in Ring 3 (User Mode). We need a mechanism to transition from Ring 0 to Ring 3 (when launching or resuming a process) and a secure, efficient mechanism to transition from Ring 3 back to Ring 0 (for system calls, exceptions, and interrupts).

The mechanism must balance performance for frequent transitions (like system calls) with strict security guarantees to prevent user processes from escalating privileges or corrupting kernel memory.

**Related GitHub Issues:**
- Related to: #68 ([Phase 2.1.4: Kernel/User Mode Transition](https://github.com/iamvirul/ferrous-kernel/issues/68))
- Proposed in: #104 ([PLANNING: Create ADR-0002 for Kernel/User Mode Transition](https://github.com/iamvirul/ferrous-kernel/issues/104))

---

## Decision

> We will utilize x86-64 hardware protection rings (Ring 0 for Kernel, Ring 3 for User) and use the `SYSCALL`/`SYSRET` instructions for fast system call transitions, alongside the standard `IDT` (Interrupt Descriptor Table) and `IRETQ` for handling hardware interrupts and exceptions. A dedicated Task State Segment (TSS) will be maintained per CPU core to securely provide a known good kernel stack pointer (`RSP0`) during transitions.

### 1. Protection Rings and Privilege Levels
- **Ring 0 (Kernel Mode):** Has full access to all memory and hardware instructions.
- **Ring 3 (User Mode):** Restricted access. Cannot execute privileged instructions (e.g., `cli`, `sti`, `hlt`, `invlpg`) and can only access memory pages marked with the `USER` bit in the page tables.

### 2. Transition Mechanism
- **User to Kernel (System Calls):** We will use the `SYSCALL` instruction. This instruction is heavily optimized on x86-64 for fast transitions. It bypasses the IDT, directly loading `RIP` from the `LSTAR` MSR and applying a predefined `RFLAGS` mask (via `SFMASK`). The `SFMASK` MSR will be configured to mask the `IF` (Interrupt Flag, bit 9) to keep interrupts disabled on syscall entry until the kernel stack is ready.
- **Kernel to User (Return from Syscall):** We will use the `SYSRET` instruction.
- **First Userspace Entry & Returns from Interrupts:** The initial entry into user space for a newly created process will be achieved by constructing a fake `IRET` frame (containing `SS`, `RSP`, `RFLAGS`, `CS`, `RIP`) on the kernel stack and executing the `IRETQ` instruction. `IRETQ` will also be used to return to user space after handling hardware interrupts and exceptions.
- **GS Segment Base:** The `SWAPGS` instruction will be used immediately upon entry from Ring 3 to swap the `GS` base register. The kernel `GS` base will point to CPU-local storage containing the current task pointer and the kernel stack pointer.

---

## Rationale

**Why `SYSCALL`/`SYSRET` instead of `INT 0x80`?**
- `SYSCALL` is designed specifically for x86-64 and is significantly faster than traditional software interrupts (`INT 0x80`). Software interrupts require IDT lookup, access checks, and multiple memory writes for pushing the stack frame, making them a bottleneck for modern operating systems.

**Why use a TSS?**
- In x86-64, hardware task switching is deprecated, but the TSS is still strictly required to provide the kernel stack pointer (`RSP0`) when a privilege level change occurs via an interrupt or exception. Without a valid `RSP0` in the TSS, any interrupt in Ring 3 would cause a Double Fault due to the inability to switch to a valid Ring 0 stack.

**Alignment with Charter Principles:**
- *Performance:* Utilizing `SYSCALL` directly aligns with providing a highly performant interface for user processes.
- *Security:* Explicit separation via Ring 3 and robust stack switching prevents user-mode code from corrupting kernel state.

---

## Alternatives Considered

### Alternative 1: Traditional Software Interrupts (`INT 0x80`)

**Description:** Use the traditional 32-bit Linux style system call mechanism via the IDT.

**Pros:**
- Conceptually simpler to implement as it reuses the existing IDT infrastructure.
- Automatically handles stack switching via the TSS without needing manual stack manipulation.

**Cons:**
- Unacceptably slow for modern workloads due to processor microcode overhead for IDT traversal and privilege checks.

**Why Not Chosen:** Performance overhead is too high compared to hardware-optimized `SYSCALL`.

### Alternative 2: Hardware Task Switching

**Description:** Use x86 hardware task gates in the IDT and multiple TSS segments.

**Pros:**
- Fully hardware-managed context switching.

**Cons:**
- Not supported in x86-64 (Long Mode). The architecture explicitly removed hardware task switching in 64-bit mode.

**Why Not Chosen:** Architecturally impossible in 64-bit mode.

---

## Consequences

### Positive
- High-performance boundary between user applications and the kernel.
- Strict hardware-enforced isolation.
- Standardized approach matching modern x86-64 OS design (Linux, Windows, macOS).

### Negative
- Complexity in the transition code (assembly stubs).
- Handling `SYSCALL` safely requires meticulously saving and restoring user state, as the hardware only saves `RIP` and `RFLAGS` into `RCX` and `R11`. It does *not* automatically switch the stack pointer (`RSP`).

### Risks
- **Stack Corruption during SYSCALL:** Because `SYSCALL` does not switch the stack pointer, the kernel entry stub executes momentarily on the *user stack*. An interrupt firing at this exact moment could corrupt kernel state or lead to a security vulnerability.
  - *Mitigation:* `SYSCALL` must be configured to mask interrupts (via `IA32_FMASK` MSR) upon entry. The kernel must quickly swap to a secure kernel stack before re-enabling interrupts.
- **SWAPGS vulnerabilities:** Incorrect use of `SWAPGS` can lead to speculative execution vulnerabilities (like CVE-2019-1125).
  - *Mitigation:* Carefully audit the entry and exit paths to ensure `SWAPGS` is used symmetrically and exactly once per transition.

---

## Safety and Security Considerations

**Unsafe Code:**
- The assembly stubs for `SYSCALL` entry and `IRETQ`/`SYSRET` exits are inherently unsafe and bypass Rust's type system.
- MSR manipulation (setting up `LSTAR`, `STAR`, `FMASK`) requires unsafe blocks.

**Security Implications:**
- The boundary between Ring 3 and Ring 0 is the primary defense perimeter of the OS.
- User-provided pointers during system calls must be thoroughly validated against the user's address space boundaries to prevent confused deputy attacks (where the kernel is tricked into reading/writing kernel memory on behalf of the user).

---

## Performance Considerations

- The `SYSCALL` path is the hottest path in an OS. The assembly stub must be kept as short as possible.
- State saving (pushing registers) should be minimized to what is strictly necessary per the SysV ABI, though for process suspension, a full register state save is required.

---

## Interaction with Address Space Management and Paging

- User processes will execute in the lower half of the virtual address space, while the kernel resides in the higher half.
- The user page tables must map the kernel (higher half) as *Supervisor-only* (User bit cleared) so Ring 3 code cannot access it.
- **Page Table Isolation (PTI):** Currently, we will rely on supervisor bits for isolation. If Kernel Page Table Isolation (KPTI) is needed for mitigation of side-channel attacks (like Meltdown), it will require a separate ADR, as it significantly complicates the transition mechanism by requiring CR3 swaps on every entry/exit. For Phase 2.1.4, we assume a shared address space with supervisor protection.

---

## Dependencies

**Depends on:**
- Phase 2.1.1: Task/Process Data Structures (needed to store CPU context).
- Phase 2.1.2: Address Space Management (needed to create Ring 3 page tables).
- Global Descriptor Table (GDT) must be updated to include Ring 3 Code and Data segments, and the TSS descriptor.

**Blocks:**
- Phase 2.2: System Call Interface.
- Phase 2.3: User Space Environment.

---

## Implementation Plan

- [ ] Update the GDT to include Ring 3 Code (`0x18`) and Ring 3 Data (`0x20`) segments.
- [ ] Implement a TSS and configure the GDT to load it using the `LTR` instruction.
- [ ] Update the Page Table allocator to allow setting the `USER` bit for specific mappings.
- [ ] Initialize the `STAR`, `LSTAR`, and `FMASK` MSRs for the `SYSCALL` instruction.
- [ ] Write the `SYSCALL` entry assembly stub to safely save user state, perform `SWAPGS`, switch to the kernel stack, and call a Rust handler.
- [ ] Write the `SYSRET` and `IRETQ` exit paths to restore user state and return to Ring 3.

---

## References

**Project Documents:**
- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [CHARTER.md](../CHARTER.md)

**GitHub Issues:**
- #68 [Phase 2.1.4: Kernel/User Mode Transition](https://github.com/iamvirul/ferrous-kernel/issues/68)
- #104 [PLANNING: Create ADR-0002 for Kernel/User Mode Transition](https://github.com/iamvirul/ferrous-kernel/issues/104)

**External References:**
- Intel 64 and IA-32 Architectures Software Developer's Manual (Volume 3, Chapter 5: Protection)
- AMD64 Architecture Programmer's Manual (Volume 2, Chapter 4: Segmented Virtual Memory)

---

## Status History

- 2026-05-27: Created (Status: Proposed)

---

## Approval

- **Proposed by:** iamvirul on 2026-05-27
