# ADR-0007: System Call ABI and Dispatch

**Status:** Proposed  
**Date:** 2026-11-24  
**Deciders:** iamvirul  
**Tags:** phase-2, syscall, abi, security, user-memory

---

## Context

ADR-0002 delivered the SYSCALL/SYSRET transition: user code can enter the kernel and
return. Phase 2.1.5 needs the layer above it: how a syscall is numbered, how its
arguments are validated, how results and errors are encoded, and how the kernel reads
memory that user space hands it.

Three properties of the current system shape the design:

1. **The identity map lives in the lower half.** `[0, 1 GiB)` is mapped VA == PA
   (supervisor-only) and is copied into every user PML4 so the kernel keeps running after
   a CR3 switch. A pointer check that only tests "below the canonical boundary" would let
   user space pass kernel addresses and make the kernel read them (e.g. via `sys_log`).
2. **There is no fault fixup.** A kernel-mode page fault halts the machine, so the kernel
   must never touch an unmapped user address. It has to prove the pages are mapped first.
3. **There is no scheduler yet** (Phase 2.2.2), so `sys_yield` and `sys_exit` cannot yet
   switch to another task.

**Related GitHub Issues:**
- Implements: #70 ([Phase 2.1.5: System Call Interface (minimal)](https://github.com/iamvirul/ferrous-kernel/issues/70))
- Builds on: #68 / ADR-0002 (Kernel/User Mode Transition)
- Future: #78 (Capability-Based Syscalls), #85 (Core Syscalls)

---

## Decision

**Decision:** A shared, host-testable `ferrous-syscall` crate defines the user/kernel
ABI. The kernel dispatches through an exhaustive `match` on a typed `SyscallNumber`, and
every user-memory access goes through `UserPtr<T>` plus a page-table walk that requires
`PRESENT | USER` (and `WRITABLE` for writes) at every level.

### Register convention (x86-64)

| Register | Meaning |
|----------|---------|
| RAX | Syscall number on entry; result on return |
| RDI, RSI, RDX, R10, R8, R9 | Arguments 1–6 (R10 replaces RCX, which SYSCALL clobbers) |
| RCX, R11 | Clobbered by hardware (user RIP / RFLAGS) |
| All others | Preserved |

### Numbers

| Number | Syscall | Arguments | Result |
|--------|---------|-----------|--------|
| 0 | *(reserved, always ENOSYS)* | — | — |
| 1 | `sys_exit` | `code: i32` | does not return |
| 2 | `sys_yield` | — | `0` |
| 3 | `sys_log` | `ptr: *const u8, len: u64` | `0` |

Number 0 is deliberately unassigned, so a zeroed RAX can never trigger a real syscall.

### Return encoding

Success returns a value in `[0, 2^63)`. An error returns `-(errno)` as `u64`, using
Linux-compatible values: `EFAULT = 14`, `EINVAL = 22`, `ENOSYS = 38`. Values in
`[-4095, -1]` are reserved for errors, as on Linux.

### User memory

- `UserPtr<T>::new(addr, count)` is pure and checks for: no `addr + count * size_of::<T>()`
  overflow, an end that is `<= USER_SPACE_END` (`0x0000_8000_0000_0000`), and alignment
  to `align_of::<T>()`. A zero-length range is valid and is never dereferenced. A null
  address with a non-zero length is rejected.
- `copy_from_user(dst, src)` walks the *active* page tables for every page of the range
  and requires `PRESENT | USER_ACCESSIBLE` on all four levels before it copies anything.
  It returns `EFAULT` otherwise, and never page-faults.
- `sys_log` caps `len` at `LOG_MAX_LEN = 512` bytes. That bounds both the page walk
  (≤ 2 pages) and the kernel stack buffer, and gives bounded handler time with no heap use.

### Placeholder semantics until Phase 2.2

- `sys_yield` returns `0` immediately: there is no other runnable task.
- `sys_exit` logs the exit code and halts the CPU. With one task and no scheduler,
  "terminate the caller" is the same as "nothing left to run". It never returns to user
  space.
- `sys_log` validates its arguments in every build. The serial write is compiled only
  when `debug_assertions` are on, so a release kernel emits no user-controlled output.

---

## Rationale

**Key Factors:**
- **Correctness:** a typed enum plus an exhaustive `match` makes an unhandled syscall a
  compile error. Unknown numbers map to `ENOSYS` in exactly one place.
- **Security:** checking the USER bit closes the identity-map information leak. Walking
  the page tables before copying makes "never fault in the kernel" a checked property.
- **Testability:** the kernel crate has no host test harness, so the pure parts (numbers,
  errors, range validation) live in `lib/syscall` and are unit-tested on the host, like
  `lib/boot-info`.

**Alignment with Charter Principles:**
- "Correctness first": no syscall path relies on user input being well-formed.
- "Fail fast, fail visibly": every rejection is a typed error, never a panic or a silent
  truncation.
- "No silent global state": handlers receive their arguments explicitly. The only
  implicit input is the active CR3, which defines the caller's address space.

---

## Alternatives Considered

### Alternative 1: Bounds check only (no page walk)

**Description:** Accept any range below `USER_SPACE_END` and dereference it directly.

**Pros:** Fastest; simplest.

**Cons:** Leaks kernel memory through the lower-half identity map, and turns a
user-supplied unmapped address into a kernel page fault (a machine halt today).

**Why Not Chosen:** It violates the isolation goal outright.

### Alternative 2: Exception-table fault fixup (Linux style)

**Description:** Copy optimistically and recover from #PF via a table of fixup addresses.

**Pros:** O(1) validation regardless of length; standard on mature kernels.

**Cons:** Needs a recoverable #PF handler, which the kernel does not have (the boot
exception handler halts). It is also more unsafe code and harder to review.

**Why Not Chosen:** Premature. Revisit when large copies (IPC buffers) make page walks
expensive. Phase 2.4+.

### Alternative 3: Function-pointer dispatch table indexed by number

**Description:** `static TABLE: [fn(&Args) -> u64; N]`.

**Pros:** O(1) dispatch with many syscalls.

**Cons:** Loses exhaustiveness checking and per-handler typed signatures, and needs
bounds checks on the index.

**Why Not Chosen:** `match` compiles to a jump table anyway at this size, and stays
type-safe.

---

## Consequences

### Positive
- Each user-memory access has one audited path (`copy_from_user`).
- The ABI crate can be reused by user-space runtimes later (same numbers, same errors).

### Negative
- The page walk costs O(pages) per copy. That is negligible at a 512-byte cap, but it
  matters for future bulk transfers.

### Risks
- SMAP is not enabled. Once it is (CR4.SMAP), `copy_from_user` must wrap the access in
  `stac`/`clac`. The single copy path makes this a one-line change.
- The check happens before the copy (TOCTOU). It is safe while the kernel is
  single-threaded with interrupts disabled during syscalls. SMP or preemptible syscalls
  will need the address space locked across the check and the copy.

### Implementation Notes
- `kernel/src/syscall/`: dispatch and handlers. `kernel/src/syscall/user_memory.rs`:
  `copy_from_user`.
- `kernel/src/memory/address_space.rs`: `is_user_accessible_in()` page-table walk.
- `copy_to_user` is deferred until the first syscall that returns data through memory.
  Adding it now would be dead code.

---

## Safety and Security Considerations

**Unsafe Code:**
- `copy_from_user` copies from a user virtual address with `core::ptr::copy_nonoverlapping`
  only after the page walk has proven every page present and user-accessible under the
  active CR3. Its safety comment documents this.
- The page walk reads page tables through the identity map (VA == PA invariant), like the
  existing `translate_in`.

**Security Implications:**
- Kernel addresses in the lower half (identity map) and the upper half are both rejected
  with `EFAULT`.
- Lengths are capped before any walk, so there is no unbounded work driven by user input.

---

## Performance Considerations

At most 2 page walks per `sys_log` (512 bytes), each 4 dependent loads. No heap
allocation, and no loops whose bounds come from unchecked user input. Syscall
latency targets (ROADMAP) are measured in later phases.

---

## Dependencies

**Depends on:** ADR-0002 (entry/exit path), Phase 2.1.2 (address spaces).

**Blocks:** #78 (capability syscalls), #85 (core syscalls with a real scheduler).

---

## Implementation Plan

1. `lib/syscall` crate: `SyscallNumber`, `SyscallError`, return encoding, `UserPtr<T>`,
   with host unit tests.
2. Kernel page walk `is_user_accessible_in()` and `copy_from_user()`.
3. Dispatcher and `sys_exit` / `sys_yield` / `sys_log` handlers.
4. A boot smoke test: a ring-3 program that exercises each syscall and each error path, then
   exits with code 0 only if every result matched.

---

## References

**Project Documents:**
- [ADR-0002](ADR-0002-kernel-user-mode-transition.md)
- [CHARTER.md](../CHARTER.md)
- [ARCHITECTURE.md](../ARCHITECTURE.md)

**GitHub Issues:**
- #70, #68, #78, #85

**External References:**
- Intel SDM Vol. 3A §4.6 (access rights, U/S bit), §5.8.8 (SYSCALL/SYSRET)
- Linux `include/uapi/asm-generic/errno-base.h` (errno values)

---

## Status History

- 2026-11-24: Created (Status: Proposed); to be accepted before the Phase 2.1.5 implementation (#70) lands
