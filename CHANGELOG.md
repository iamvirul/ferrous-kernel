# Changelog

All notable changes to Ferrous Kernel are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

#### Phase 2.1.1 - Task and Process Data Structures
- `TaskId` and `ProcessId` newtypes over `u64`; opaque and unforgeable
- `TaskState` enum (Ready / Running / Blocked / Exiting / Zombie) with
  `const can_transition_to()` and atomic compare-and-swap via
  `try_transition()`; invalid transitions return `TaskStateError`
- `TaskPriority` enum (Idle / Low / Normal / High / RealTime)
- `RegisterState` (`repr(C)`) holding callee-saved registers plus
  rsp / rip / rflags; field offsets documented for the Phase 2.2.3
  context-switch assembly stub
- `TaskControlBlock` (`repr(C)`) with saved register state, kernel stack
  bounds behind safe wrappers, atomic state, priority, time-slice, and
  owner PID (`kernel/src/task/task.rs`)
- `ProcessState` enum (Active / Exiting / Zombie) with validated
  transitions and `ProcessStateError`
- `Process` with fixed-capacity task list (16 slots), atomic state,
  exit code storage, and stubs for address-space and capability-space
  fields to be populated in Phases 2.1.2 and 2.3.1
  (`kernel/src/task/process.rs`)
- `kernel::task::smoke_test()` with 8 test cases covering ID
  round-trips, state machine enforcement, atomic CAS behaviour, task
  registration, capacity overflow rejection, and exit-code storage

#### Phase 2.1.2 - Address Space Management
- `AddressSpace` struct owning an isolated PML4 physical frame and a
  fixed-capacity region list (32 slots) to avoid heap dependency during
  early boot (`kernel/src/memory/address_space.rs`)
- Kernel higher-half entries (PML4[256-511]) copied from the bootstrap
  PML4 at construction so all address spaces share one kernel mapping
- `RegionKind` enum (Code / Data / Stack / Heap) with `page_flags()`
  returning correct x86-64 PTE flags (`PRESENT | USER_ACCESSIBLE`, plus
  `WRITABLE` for non-code kinds)
- `VirtualRegion` with `overlaps()` guard enforced by `map_region`
- `map_4k_into` / `unmap_4k_from` — page-table walkers operating on an
  explicit PML4 physical address; exploit VA==PA identity mapping
  established in Phase 1.3.3
- `AddressSpace::map_region` validates user-space bounds, alignment, and
  no-overlap; rolls back already-mapped pages on partial failure
- `AddressSpace::unmap_region` clears PTEs, frees per-page frames, and
  compacts the region list via swap-with-last
- `AddressSpace::translate` translates a VA in this space without
  touching CR3
- `AddressSpace::switch_to` loads this space's PML4 into CR3 (full TLB
  flush)
- `AddressSpace::destroy` tears down all user regions and frees the PML4
  frame
- `memory::address_space::smoke_test()` with 6 test cases covering
  overlap detection, construction, map+translate, unmap, invalid-input
  rejection, and destroy

#### Phase 2.1.3 - ELF Binary Loader
- `kernel/src/task/elf.rs` module added for statically-linked ELF64 executable parsing and loading.
- Safe `read_struct` parsing to map byte slices into `#[repr(C)]` ELF headers without allocations or external dependencies.
- `load_elf` loads `PT_LOAD` segments into a target `AddressSpace` with page-aligned mappings (`RegionKind::Code` and `RegionKind::Data`), including BSS zero-filling.
- `ParseError` enum provides robust error handling for invalid binaries.
- `elf::smoke_test()` added to test loader parsing and mapping functionality.

#### Phase 2.1.5 - System Call Interface (minimal, ADR-0007)
- New `ferrous-syscall` crate (`lib/syscall`): host-testable ABI with the
  typed `SyscallNumber` (0 reserved; `Exit` = 1, `Yield` = 2, `Log` = 3),
  `SyscallError` (Linux-compatible `EFAULT`/`EINVAL`/`ENOSYS`, returned as
  `-errno`), `encode_result`, and `UserPtr<T>` (overflow, bounds,
  alignment, and null checks). 26 unit tests
- `kernel::syscall::dispatch`: exhaustive `match` on `SyscallNumber`;
  unknown and reserved numbers return `-ENOSYS`; no heap use, and bounded
  time on every path
- `copy_from_user` (`kernel/src/syscall/user_memory.rs`): walks the active
  page tables and requires `PRESENT | USER` at every level before copying.
  This rejects lower-half identity-map (supervisor-only) addresses and
  never page-faults on a bad user pointer
- `address_space::is_user_readable_in()` and `active_pml4_phys()`.
  `USER_SPACE_END` now has a single definition in the ABI crate
- Handlers: `sys_exit` (logs the code, then halts until the scheduler
  exists), `sys_yield` (returns 0 until Phase 2.2.2), and `sys_log` (≤ 512
  bytes, UTF-8 validated; arguments are checked in every build, output
  only in debug builds)
- Boot Step 18: an assembled, position-independent ring-3 program runs 13
  syscall checks (success, ENOSYS, EFAULT for kernel/unmapped/overflowing/
  page-crossing pointers, EINVAL for length and UTF-8) and exits with code
  0 only if all pass; `verify-boot.sh` requires that exit code

#### Phase 2.2.1 - Runnable Task Queue
- New `ferrous-sched` crate (`lib/sched`, `unsafe` forbidden): generic
  `RunQueue<T, N>`, a fixed-capacity FIFO ring buffer with no heap.
  `enqueue` rejects duplicates (`AlreadyQueued`) and overflow (`Full`).
  The scheduler must still never enqueue the running task: the queue
  cannot see it, so it keeps only half of the queued-XOR-running
  invariant. `remove` takes a
  task from any position and keeps the others in order. All operations
  run in bounded time. 20 unit tests, including remove-from-middle across
  the wraparound and a 10,000-operation model check against a `Vec`
- `kernel::sync::IrqSpinLock`: the kernel's first lock. It disables
  interrupts, then spins on an atomic. The guard releases the lock
  *before* its interrupt scope ends, and guards of different locks may be
  dropped in any order
- `arch::x86_64::cpu`: `interrupts_enabled`, `disable_interrupts`, which
  returns an RAII `InterruptGuard`, and `irq_disable_depth`. A per-CPU
  nesting count re-enables interrupts only when the *last* guard drops,
  and only if they were on before the first one, so an out-of-order
  release can never re-enable them while a lock is held
- `ferrous_core::irq::IrqNesting`: that counting logic as a pure,
  host-tested state machine (8 tests, including out-of-order release)
- `kernel::sched`: `RunQueue` (`TaskId`, capacity 256) and the
  `SharedRunQueue` lock alias. Boot smoke test 19.1–19.5 covers the lock
  (including out-of-order release of two locks), plus FIFO,
  remove-from-middle, duplicate and full rejection, and drain
- Known gap: restoring IF=1 is not exercised at boot, because the kernel
  runs with interrupts off and the PIC/APIC are not yet masked (Phase
  2.2.4)

#### Phase 2.2.3 - Context Switching (ADR-0008)
- `arch::x86_64::context`: `switch_context`, a `global_asm!` routine that
  saves rbx, rbp, r12–r15, `rsp`, and a resume `rip` into the outgoing
  task's `RegisterState`, restores the incoming task's, and resumes it with
  `jmp [rip]`. Offsets come from `offset_of!` and the layout size is
  asserted at compile time. The new-task entry trampoline needs no forged
  stack frames
- `sched::switch::switch_to(prev, next, &InterruptGuard)`: sets TSS.RSP0
  to the incoming task's kernel stack on every switch, and loads CR3 only
  when the root changes (counted by `cr3_load_count`). The guard parameter
  proves interrupts are off. It asserts that `next` has a resumable
  context; debug builds also check `next.rsp` bounds and that the stack is
  identity-mapped under `next`'s root
- `sched::switch::init_kernel_task_context`: builds a new task's initial
  context (trampoline `rip`, entry in `r12`, ABI-aligned `rsp`)
- `TaskControlBlock` gains `page_table_root` (constructor parameter and
  accessor); `address_space::load_page_table_root`
- Boot smoke test 20.1–20.3: two-task ping-pong (9 switches, strict A/B
  alternation, TSS.RSP0 checked on every switch, no CR3 load on the same
  root), callee-saved sentinels in both directions, and CR3 between two
  address spaces (each task sees its own value; 4 root changes, 4 loads).
  Mutation-tested: dropping one register restore, skipping the TSS update,
  or skipping the CR3 load each fails the boot
- Eager x87/MMX/SSE switching: each TCB holds a 16-byte-aligned
  `FpuState` (`FXSAVE` format). `switch_context` runs `fxsave64` first and
  `fxrstor64` before the stack switch, so no compiler-generated SSE can run
  in between. New tasks start from `FpuState::initial()` (FNINIT control
  word, power-on MXCSR). Eager rather than lazy (CR0.TS) avoids LazyFP
  (CVE-2018-3665)
- Inherited interrupt scope hand-off: `cpu::end_inherited_interrupt_scope`,
  called by the new-task trampoline, closes the scope the switching task
  opened, so the per-CPU nesting count balances across every switch
- Boot checks 20.4 (xmm0–15 and MXCSR preserved in both directions; new
  task starts with the clean MXCSR) and 20.5 (nesting depth balanced at
  every task body, 23 checks). Mutation-tested: dropping `fxsave` or
  `fxrstor` loses all 18 checked states (mask `0x3ffff`: 16 XMM registers,
  MXCSR, x87); dropping the hand-off unbalances the nesting count
- Fixes from an independent second review, each mutation-tested:
  - `switch_to` also updates `CpuLocal.kernel_stack`, the stack the SYSCALL
    stub loads. Previously only TSS.RSP0 was updated, so a switched-in task
    making a syscall would have run on another task's stack
  - `switch_to(prev: *mut, next: *const, &InterruptGuard)` takes raw
    pointers, so no `&mut` / `&` to a TCB is held across the suspension
    (that would be aliasing UB once the scheduler reads `prev` after
    resuming)
  - The depth-1 contract: debug builds assert that the switch guard is the
    only open interrupt scope (holding a lock across a switch would
    deadlock the new task)
  - The new-task trampoline checks rsp ≡ 8 (mod 16) and traps (`ud2`) on a
    mis-built frame
  - The debug identity-map check now covers both stacks, both TCBs and the
    switch code
  - The self-test checks both 64-bit lanes of every XMM register and the
    x87 stack (a dropped `fxrstor` now reports mask `0x3ffff`)
- Fixes from the CodeRabbit review:
  - AMD x87 pointer leak: before every `fxrstor64`, `fnclex; emms; fild`
    from a kernel constant overwrites FIP/FDP/FOP. AMD does not restore
    them from an image with no pending exception, so the previous task's
    values would leak (CVE-2006-1056 class)
  - The identity-map debug check also probes `next`'s saved `rsp`
  - Each XMM lane gets a distinct sentinel, so a lane swap is detected
  - Documented contracts: the low-level switch needs a depth-1 interrupt
    scope, not merely IF = 0; a cached `page_table_root` must outlive its
    tasks (`TaskControlBlock::new`, `AddressSpace::destroy`)
- ADR-0008 amended for all of the above. ISR/NMI FPU preservation is
  recorded as a requirement for #75
- Still outstanding: AVX/XSAVE state (not enabled), and system-wide
  interrupt enabling (needs PIC/APIC setup, #75)

#### Documentation and Tooling
- README badges: CI, Release, CodeQL, latest release version, license,
  Rust language, and architecture
- ROADMAP.md: current phase updated to Phase 2; tasks 2.1.1 and 2.1.2
  marked complete
- ARCHITECTURE.md: Scheduler abstractions section updated with
  implemented types and file locations; boot sequence annotated with
  v0.1.0 completion status

### Fixed

#### Phase 2.1.4 - Kernel/User Mode Transition (ring 3 round trip now validated in QEMU)
- Boot: switch to `KERNEL_STACK` in `kernel_entry` before calling
  `kernel_main`, instead of `mov rsp` inside `kernel_main`'s body. The old
  switch left `kernel_main`'s ~34 KiB debug frame addressed above the stack
  top, corrupting the IDT and kernel page tables; this was the real cause of
  the Phase 2.1.2 "hang" (actually a triple fault)
- `BitmapFrameAllocator` never hands out PFN 0: physical address 0 is
  indistinguishable from null under VA == PA, and `AddressSpace::new` was
  receiving a PML4 at PA 0
- `syscall::init` sets `EFER.SCE`; without it ring-3 `syscall` raised #UD
- `kernel_entry`, `kernel_main`, `exception_handler`, and
  `syscall_dispatcher` are `extern "sysv64"`: on `x86_64-unknown-uefi`,
  `extern "C"` is Win64 and read arguments from the wrong registers (every
  exception report printed a garbage vector)
- Exception common stub realigns RSP to 16 bytes before calling the handler
- User smoke-test stub spins with `pause` after SYSRET instead of `hlt`,
  which is privileged and faults at CPL 3

---

## [0.1.0] - 2026-05-16

First tagged release. The kernel boots from UEFI firmware, initialises physical
and virtual memory, enters the kernel proper, and produces observable structured
output over COM1 serial before halting cleanly. Foundation milestone only; see
Known Limitations below.

### Added

#### Phase 1.1 - Bare Metal Boot
- UEFI bootloader (`ferrous-boot`) targeting `x86_64-unknown-uefi`
- Kernel entry point receives typed `BootInfo` from the bootloader
- Private 64 KiB kernel stack with guard page established before kernel entry
- GDT and IDT loaded; 32 CPU exception handlers installed
- Serial output available from the first instruction after UEFI handoff
- Boot identity message: `=== Ferrous Kernel ===` followed by `Hello from Ferrous!`

#### Phase 1.2 - Memory Map
- Physical memory map parsed from UEFI memory descriptors
- Usable RAM regions identified, deduplicated, and reported at boot
- Memory map passed to the kernel via `BootInfo`

#### Phase 1.3.2 - Physical Frame Allocator
- Bitmap-based physical frame allocator covering all usable RAM
- 4 KiB frame allocation and deallocation
- Reports free / total frame counts at initialisation

#### Phase 1.3.3 - Virtual Memory
- Kernel page tables (PML4) built from scratch after `exit_boot_services`
- CR3 loaded with new tables; higher-half kernel alias verified at boot
- 1 GiB identity map and direct-physical map established

#### Phase 1.3.4 - Page Table Management
- `map_4k` and `unmap_4k` for individual 4 KiB page mappings
- Guard page support: unmapped addresses translate to `None`
- Smoke tests verify translate, map, unmap, and guard page behaviour

#### Phase 1.3.5 - Heap Allocator
- Linked-list heap allocator backed by physical frames
- `alloc` crate available in kernel code (`Vec`, `Box`, `String`, `BTreeMap`)
- Smoke tests verify push/index/deref/starts_with on heap-allocated types

#### Phase 1.4.1 - Logging Framework
- `log` crate integration with `SerialLogger` backend
- Log levels ERROR, WARN, INFO, DEBUG routed to COM1
- Format: `[LEVEL] crate: message`

#### Phase 1.4.2 - Panic Handler
- `#[panic_handler]` emits ASCII banner, source location, and message to COM1
- RBP-chain stack trace walker prints raw frame return addresses
- Smoke test verifies handler survives normal execution

#### Phase 1.4.3 - Assertion and Debug Macros (`ferrous-core`)
- `kassert!`, `kassert_eq!`, `kassert_ne!` - panic with expression context on failure
- `kdebug_assert!`, `kdebug_assert_eq!`, `kdebug_assert_ne!` - elided in release builds
- `kunreachable!` and `ktodo!` for path-impossible markers
- Smoke tests verify all nine macro variants

#### Phase 1.4.4 - Serial Console Driver
- `kernel/src/drivers/serial.rs`: production `SerialPort` with configurable baud
  rate (`BaudRate`), framing (`DataBits`, `Parity`, `StopBits`), multi-port
  support (`ComPort`), and typed `SerialError`
- `kernel/src/drivers/console.rs`: `Console` trait (supertrait of `fmt::Write`)
  and `SerialConsole` implementation
- `boot/src/serial.rs`: `BootSerialPort` for the UEFI boot phase; zero-cost
  `Copy` type; mirrors the hardware model without a kernel-crate dependency
- Module-level free functions (`serial_init`, `serial_write_str`, etc.) preserve
  the existing API used by the panic handler and stack-trace walker
- Smoke tests cover `fmt::Write`, non-blocking receive (`try_read_byte`), and
  RX FIFO status (`data_available`)

#### Tooling
- `scripts/make-image.sh`: produces a 64 MiB FAT32 bootable disk image
  (`EFI/BOOT/BOOTX64.EFI`); supports macOS (`hdiutil` + `newfs_msdos`) and
  Linux (`mtools`)
- `scripts/verify-boot.sh`: headless QEMU boot verification; checks all expected
  serial output strings within a configurable timeout; suitable for CI
- GitHub Actions CI: build and boot-verify on every push and pull request
- GitHub CodeQL: static analysis and security scanning

### Known Limitations

- Single core only; no SMP
- No interrupt-driven I/O; all serial I/O is polled
- No keyboard input or display / framebuffer output
- No filesystem or persistent storage
- No networking
- No user-space processes or syscall interface

---

[Unreleased]: https://github.com/iamvirul/ferrous-kernel/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/iamvirul/ferrous-kernel/releases/tag/v0.1.0
