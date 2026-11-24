//! Boot self-test for context switching (Phase 2.2.3, ADR-0008 §4).
//!
//! The boot path acts as task 0. It switches into small kernel tasks, which
//! switch among themselves and finally back to boot. Every switch follows
//! the real protocol (ADR-0008 §3): the switching task opens its own
//! `InterruptGuard` for the switch; the scope is closed on the far side,
//! by a resumed task dropping its guard or by a new task's entry trampoline.
//! The tasks never return (`fn() -> !`) and are simply never resumed once
//! the test ends.
//!
//! Checks:
//! - 20.1 ping-pong: the interleaving is exactly A, B, A, B, …, every switch
//!   points both kernel-entry stacks (TSS.RSP0 and the SYSCALL stack in
//!   CpuLocal) at the incoming task's stack, and same-root switches
//!   perform no CR3 load.
//! - 20.2 callee-saved: rbx, rbp, r12–r15 survive a switch away and back,
//!   in both directions.
//! - 20.3 CR3: two tasks in different address spaces read the same user VA
//!   and each sees its own value; every root change loads CR3 exactly once.
//! - 20.4 x87/SSE: both 64-bit lanes of xmm0–xmm15, MXCSR, and the x87
//!   stack survive a switch in both directions, and a new task starts from
//!   the clean initial FPU state, not the previous task's.
//! - 20.5 interrupt scopes: every task body runs at the test's base
//!   nesting depth (new tasks close the inherited scope), so the per-CPU
//!   count is balanced after every switch.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::switch::{cr3_load_count, init_kernel_task_context, switch_to};
use crate::arch::x86_64::context::{
    switch_context_checking_callee_saved, switch_context_checking_vector_state,
};
use crate::arch::x86_64::cpu::{self, disable_interrupts, interrupts_enabled, irq_disable_depth};
use crate::arch::x86_64::gdt;
use crate::memory::address_space::{active_pml4_phys, AddressSpace, RegionKind, VirtualRegion};
use crate::memory::paging::{PageTable, VirtualAddress};
use crate::task::{FpuState, ProcessId, RegisterState, TaskControlBlock, TaskId, TaskPriority};

// ---------------------------------------------------------------------------
// Test-only shared state
// ---------------------------------------------------------------------------

/// Interior-mutable cell for test-only statics.
struct TestCell<T>(UnsafeCell<T>);

// SAFETY: every access happens on the only CPU, with interrupts disabled,
// from exactly one task at a time. Tasks run strictly one after another (a
// switch is the only way control moves), so accesses never overlap.
unsafe impl<T> Sync for TestCell<T> {}

impl<T> TestCell<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }
}

const TASK_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct TaskStack([u8; TASK_STACK_SIZE]);

/// Task slots (index into `TCBS` and `STACKS`).
/// Boot runs on its own kernel stack (passed in); its STACKS slot is unused.
const BOOT: usize = 0;
const PING: usize = 1;
const PONG: usize = 2;
const SENTINEL: usize = 3;
const CR3_A: usize = 4;
const CR3_B: usize = 5;
const VECTOR: usize = 6;
const TASKS: usize = 7;

static STACKS: TestCell<[TaskStack; TASKS]> =
    TestCell::new([const { TaskStack([0; TASK_STACK_SIZE]) }; TASKS]);
static TCBS: TestCell<[Option<TaskControlBlock>; TASKS]> = TestCell::new([const { None }; TASKS]);

/// Interrupt-nesting depth at test start: every task body must run at it.
static BASE_DEPTH: AtomicUsize = AtomicUsize::new(0);
/// Interrupt state at test start: every task body must run with it.
static BASE_IF: TestCell<bool> = TestCell::new(false);
/// Number of balance checks that passed (reported in 20.5).
static BALANCE_CHECKS: AtomicUsize = AtomicUsize::new(0);

/// Ping-pong event log ('A' / 'B').
const ROUNDS: usize = 4;
static EVENTS: TestCell<[u8; 2 * ROUNDS]> = TestCell::new([0; 2 * ROUNDS]);
static EVENT_LEN: AtomicUsize = AtomicUsize::new(0);

/// Masks reported by the task side of the sentinel tests (u64::MAX = unset).
static SENTINEL_TASK_MASK: TestCell<u64> = TestCell::new(u64::MAX);
static VECTOR_TASK_MASK: TestCell<u64> = TestCell::new(u64::MAX);
/// MXCSR the vector task observed on its first instruction.
static VECTOR_TASK_INITIAL_MXCSR: TestCell<u32> = TestCell::new(0);

/// MXCSR values with distinct rounding modes (RC = bits 13–14) and no
/// reserved bits: round-down for boot, round-up for the vector task.
const BOOT_MXCSR: u32 = 0x3F80;
const TASK_MXCSR: u32 = 0x5F80;
/// Power-on MXCSR, which a new task must start with.
const INITIAL_MXCSR: u32 = 0x1F80;

/// What each CR3 task read from the shared user VA, in order.
static CR3_READS: TestCell<[u64; 3]> = TestCell::new([0; 3]);

/// The user VA mapped to a different frame in each test address space.
const SHARED_USER_VA: u64 = 0x0000_1000_0000_0000;
const VALUE_IN_A: u64 = 0xC0DE_0000_0000_000A;
const VALUE_IN_B: u64 = 0xC0DE_0000_0000_000B;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create task `slot` on its dedicated test stack, running under `root`.
///
/// # Safety
///
/// Single-threaded boot context; `slot` is not running and is not being
/// accessed elsewhere.
unsafe fn create_task(slot: usize, root: u64, entry: fn() -> !) {
    let stack = &mut (*STACKS.0.get())[slot].0;
    let bottom = stack.as_mut_ptr();
    let top = bottom.add(TASK_STACK_SIZE);
    // SAFETY: the stack is a dedicated static buffer, 16-byte aligned, used
    // only by this task; `root` maps the kernel identically (callers pass
    // the kernel root or an AddressSpace with PML4[0] copied).
    install_task(slot, bottom, top, root, Some(entry));
}

/// Install a TCB for `slot` describing the stack `[bottom, top)`.
///
/// # Safety
///
/// As for `TaskControlBlock::new`, plus the `create_task` conditions.
unsafe fn install_task(
    slot: usize,
    bottom: *mut u8,
    top: *mut u8,
    root: u64,
    entry: Option<fn() -> !>,
) {
    let mut tcb = TaskControlBlock::new(
        TaskId::new(slot as u64),
        ProcessId::new(0),
        top,
        bottom,
        TaskPriority::Normal,
        root,
    );
    if let Some(entry) = entry {
        init_kernel_task_context(&mut tcb, entry);
    }
    (*TCBS.0.get())[slot] = Some(tcb);
}

/// Raw pointer to the TCB in `slot`.
///
/// Raw on purpose: TCBs are accessed by other tasks while their owner is
/// switched out, so no reference may be held across a switch (see
/// `switch_to`). Callers form only short-lived borrows from it.
///
/// # Safety
///
/// The slot must be populated (asserted).
unsafe fn tcb_ptr(slot: usize) -> *mut TaskControlBlock {
    // SAFETY: `slot < TASKS` (indexing below panics otherwise) and the
    // array lives in a static, so the place is valid for the whole test.
    let entry: *mut Option<TaskControlBlock> = &mut (*TCBS.0.get())[slot];
    match &mut *entry {
        Some(tcb) => tcb,
        None => panic!("task {slot} not created"),
    }
}

/// Raw pointers to a task's integer and FPU save areas, for the
/// self-test harnesses that bypass `switch_to`.
///
/// # Safety
///
/// As [`tcb_ptr`]. The pointers are derived with `addr_of_mut!`, so no
/// reference outlives this call.
unsafe fn save_areas(slot: usize) -> (*mut RegisterState, *mut FpuState) {
    let t = tcb_ptr(slot);
    (
        core::ptr::addr_of_mut!((*t).registers),
        core::ptr::addr_of_mut!((*t).fpu),
    )
}

/// Switch from task `from` to task `to` through `switch_to`, opening this
/// switch's interrupt scope like real scheduler code.
///
/// # Safety
///
/// `from` is the running task; `to` is a created, resumable task.
unsafe fn switch(from: usize, to: usize) {
    assert_ne!(from, to);
    let (prev, next) = (tcb_ptr(from), tcb_ptr(to));
    let irq = disable_interrupts();
    // SAFETY: distinct, populated, static TCBs; no reference to either is
    // live (raw pointers only); `next` was created by `create_task` or saved
    // by an earlier switch.
    switch_to(prev, next, &irq);
    // Resumed: dropping our guard closes the scope our switcher opened.
    drop(irq);
    assert_balanced();
}

/// A callee-saved sentinel switch from `from` to `to`, using the same
/// interrupt-scope protocol as [`switch`].
unsafe fn sentinel_switch(from: usize, to: usize, base: u64) -> u64 {
    let (prev, prev_fpu) = save_areas(from);
    let (next, next_fpu) = save_areas(to);
    let irq = disable_interrupts();
    let mask = switch_context_checking_callee_saved(prev, next, prev_fpu, next_fpu, base);
    drop(irq);
    assert_balanced();
    mask
}

/// A vector-state sentinel switch from `from` to `to` (see [`switch`]).
unsafe fn vector_switch(from: usize, to: usize, base: u64, mxcsr: u32) -> u64 {
    let (prev, prev_fpu) = save_areas(from);
    let (next, next_fpu) = save_areas(to);
    let irq = disable_interrupts();
    let mask = switch_context_checking_vector_state(prev, next, prev_fpu, next_fpu, base, mxcsr);
    drop(irq);
    assert_balanced();
    mask
}

/// 20.5: a task body (outside any switch) must run at the test's base
/// interrupt-nesting depth, with the test's base interrupt state.
fn assert_balanced() {
    let depth = irq_disable_depth();
    let base = BASE_DEPTH.load(Ordering::Relaxed);
    assert_eq!(depth, base, "interrupt scope unbalanced after a switch");
    // SAFETY: written once by boot before the first switch.
    let base_if = unsafe { *BASE_IF.0.get() };
    assert_eq!(
        interrupts_enabled(),
        base_if,
        "interrupt state changed across a switch"
    );
    BALANCE_CHECKS.fetch_add(1, Ordering::Relaxed);
}

/// Assert both kernel-entry stacks point at task `slot`'s stack:
/// TSS.RSP0 (interrupts and exceptions from ring 3) and
/// CpuLocal.kernel_stack (the SYSCALL stub's `gs:[0]`).
///
/// # Safety
///
/// As [`tcb_ptr`]; called between switches only.
unsafe fn assert_kernel_entry_stacks_are(slot: usize) {
    // SAFETY: short-lived borrow, ends within this statement.
    let expected = (*tcb_ptr(slot)).stack_top() as u64;
    let rsp0 = gdt::TSS.rsp0;
    assert_eq!(rsp0, expected, "TSS.RSP0 not updated for task {slot}");
    // SAFETY: by-value read of a `static mut` field; no reference taken.
    let syscall_stack = core::ptr::addr_of!(cpu::CPU_LOCAL.kernel_stack).read();
    assert_eq!(
        syscall_stack,
        expected & !0xF,
        "CpuLocal.kernel_stack (SYSCALL stack) not updated for task {slot}"
    );
}

fn record(event: u8) {
    let i = EVENT_LEN.fetch_add(1, Ordering::Relaxed);
    // SAFETY: only the running task records (see TestCell).
    unsafe { (*EVENTS.0.get())[i] = event };
}

fn current_mxcsr() -> u32 {
    let mut value: u32 = 0;
    // SAFETY: STMXCSR stores 4 bytes to a valid, aligned local.
    unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &mut value, options(nostack)) };
    value
}

// ---------------------------------------------------------------------------
// Task bodies
// ---------------------------------------------------------------------------

fn ping_entry() -> ! {
    assert_balanced();
    // SAFETY (whole body): runs only inside the test, between switches.
    unsafe {
        for _ in 0..ROUNDS {
            assert_kernel_entry_stacks_are(PING);
            record(b'A');
            switch(PING, PONG);
        }
        switch(PING, BOOT);
    }
    unreachable!("ping resumed after the test finished");
}

fn pong_entry() -> ! {
    assert_balanced();
    // SAFETY: as in ping_entry.
    unsafe {
        loop {
            assert_kernel_entry_stacks_are(PONG);
            record(b'B');
            switch(PONG, PING);
        }
    }
}

fn sentinel_entry() -> ! {
    assert_balanced();
    // SAFETY: as in ping_entry.
    unsafe {
        // Clobber every callee-saved register with a different pattern on
        // the way back to boot, then check ours once boot resumes us.
        *SENTINEL_TASK_MASK.0.get() = sentinel_switch(SENTINEL, BOOT, 0x2222_0000);
        sentinel_switch(SENTINEL, BOOT, 0x4444_0000);
    }
    unreachable!("sentinel task resumed after the test finished");
}

fn vector_entry() -> ! {
    // A new task must start from the clean FPU image, not boot's state.
    // SAFETY: test-only static, see TestCell.
    unsafe { *VECTOR_TASK_INITIAL_MXCSR.0.get() = current_mxcsr() };
    assert_balanced();
    // SAFETY: as in ping_entry.
    unsafe {
        *VECTOR_TASK_MASK.0.get() = vector_switch(VECTOR, BOOT, 0x6666_0000, TASK_MXCSR);
        vector_switch(VECTOR, BOOT, 0x7777_0000, TASK_MXCSR);
    }
    unreachable!("vector task resumed after the test finished");
}

/// Read the shared user VA through the active address space.
fn read_shared_va() -> u64 {
    // SAFETY: both test address spaces map SHARED_USER_VA as a present data
    // page; SMAP is off, so CPL 0 may read it. Only called while one of them
    // is active.
    unsafe { core::ptr::read_volatile(SHARED_USER_VA as *const u64) }
}

fn cr3_a_entry() -> ! {
    assert_balanced();
    // SAFETY: as in ping_entry.
    unsafe {
        (*CR3_READS.0.get())[0] = read_shared_va();
        switch(CR3_A, CR3_B);
        (*CR3_READS.0.get())[2] = read_shared_va();
        switch(CR3_A, BOOT);
    }
    unreachable!("cr3 task A resumed after the test finished");
}

fn cr3_b_entry() -> ! {
    assert_balanced();
    // SAFETY: as in ping_entry.
    unsafe {
        (*CR3_READS.0.get())[1] = read_shared_va();
        switch(CR3_B, CR3_A);
    }
    unreachable!("cr3 task B resumed after the test finished");
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the Phase 2.2.3 context-switch self-test.
///
/// `boot_stack_bottom .. boot_stack_top` is the kernel stack the caller is
/// running on. It becomes task 0's stack, so switching back into boot is
/// checked against the right bounds.
///
/// # Safety
///
/// - Boot context: single CPU, kernel page tables active, frame allocator
///   initialised (the address spaces in 20.3 need frames).
/// - The caller's RSP must lie within the given boot stack bounds.
/// - Must be called at most once (the task slots are static).
///
/// # Panics
///
/// On any failed check.
pub unsafe fn smoke_test(boot_stack_bottom: *mut u8, boot_stack_top: *mut u8) {
    log::info!("Context switch smoke test (Phase 2.2.3)");

    // FXSAVE/FXRSTOR fault unless the OS has enabled them (CR4.OSFXSR,
    // bit 9). UEFI must set it on x86-64; fail loudly if it did not.
    let cr4: u64;
    // SAFETY: reading CR4 at CPL 0 has no side effects and touches no memory.
    core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    assert!(cr4 & (1 << 9) != 0, "CR4.OSFXSR is clear: FXSAVE unusable");

    let kernel_root = active_pml4_phys();
    BASE_DEPTH.store(irq_disable_depth(), Ordering::Relaxed);
    *BASE_IF.0.get() = interrupts_enabled();

    install_task(BOOT, boot_stack_bottom, boot_stack_top, kernel_root, None);

    // 20.1: ping-pong through switch_to.
    create_task(PING, kernel_root, ping_entry);
    create_task(PONG, kernel_root, pong_entry);
    let loads_before = cr3_load_count();
    switch(BOOT, PING);
    assert_kernel_entry_stacks_are(BOOT);
    let events = &*EVENTS.0.get();
    assert_eq!(EVENT_LEN.load(Ordering::Relaxed), 2 * ROUNDS);
    assert_eq!(events, b"ABABABAB", "tasks did not alternate");
    assert_eq!(
        cr3_load_count(),
        loads_before,
        "same-root switches must not load CR3"
    );
    log::info!(
        "[OK] 20.1) ping-pong: {} switches alternated A/B, TSS.RSP0 and SYSCALL stack updated, no CR3 load on same root",
        2 * ROUNDS + 1
    );

    // 20.2: callee-saved registers, both directions.
    create_task(SENTINEL, kernel_root, sentinel_entry);
    let mask = sentinel_switch(BOOT, SENTINEL, 0x1111_0000);
    assert_eq!(
        mask, 0,
        "boot side lost callee-saved registers (mask {mask:#b})"
    );
    let mask = sentinel_switch(BOOT, SENTINEL, 0x3333_0000);
    assert_eq!(
        mask, 0,
        "boot side lost callee-saved registers (mask {mask:#b})"
    );
    let task_mask = *SENTINEL_TASK_MASK.0.get();
    assert_eq!(
        task_mask, 0,
        "task side lost callee-saved registers (mask {task_mask:#b})"
    );
    log::info!(
        "[OK] 20.2) callee-saved: rbx, rbp, r12-r15 preserved across switches (both directions)"
    );

    // 20.3: CR3 switching between two address spaces.
    let mut space_a = test_address_space(VALUE_IN_A);
    let mut space_b = test_address_space(VALUE_IN_B);
    create_task(CR3_A, space_a.pml4_phys(), cr3_a_entry);
    create_task(CR3_B, space_b.pml4_phys(), cr3_b_entry);
    let loads_before = cr3_load_count();
    switch(BOOT, CR3_A);
    assert_eq!(
        active_pml4_phys(),
        kernel_root,
        "boot must resume under its own root"
    );
    let reads = *CR3_READS.0.get();
    assert_eq!(
        reads,
        [VALUE_IN_A, VALUE_IN_B, VALUE_IN_A],
        "tasks saw the wrong address space"
    );
    // boot->A, A->B, B->A, A->boot: every switch changed the root.
    assert_eq!(
        cr3_load_count() - loads_before,
        4,
        "expected one CR3 load per root change"
    );
    space_a.destroy();
    space_b.destroy();
    log::info!("[OK] 20.3) CR3: each task saw its own address space; 4 root changes, 4 CR3 loads");

    // 20.4: x87/SSE state, both directions, and a clean start for new tasks.
    create_task(VECTOR, kernel_root, vector_entry);
    let mask = vector_switch(BOOT, VECTOR, 0x5555_0000, BOOT_MXCSR);
    assert_eq!(
        mask, 0,
        "boot side lost vector state (mask {mask:#x}; bit 16 = MXCSR, bit 17 = x87)"
    );
    let initial = *VECTOR_TASK_INITIAL_MXCSR.0.get();
    assert_eq!(
        initial, INITIAL_MXCSR,
        "new task inherited MXCSR {initial:#x}"
    );
    let mask = vector_switch(BOOT, VECTOR, 0x8888_0000, BOOT_MXCSR);
    assert_eq!(
        mask, 0,
        "boot side lost vector state (mask {mask:#x}; bit 16 = MXCSR, bit 17 = x87)"
    );
    let task_mask = *VECTOR_TASK_MASK.0.get();
    assert_eq!(
        task_mask, 0,
        "task side lost vector state (mask {task_mask:#x})"
    );
    log::info!("[OK] 20.4) x87/SSE: xmm0-15 (both lanes), MXCSR, x87 stack preserved both ways; new task starts clean");

    // 20.5: every task body (and boot, after every switch) ran at the base
    // interrupt-nesting depth, so no scope leaked across any switch.
    assert_balanced();
    log::info!(
        "[OK] 20.5) interrupt scopes balanced across all switches ({} checks); new tasks close the inherited scope",
        BALANCE_CHECKS.load(Ordering::Relaxed)
    );

    log::info!("Context switch smoke test complete");
}

/// An address space whose SHARED_USER_VA page holds `value`, sharing the
/// kernel identity map (PML4[0]) with the current root.
unsafe fn test_address_space(value: u64) -> AddressSpace {
    let mut space = AddressSpace::new().expect("AddressSpace::new");
    // Kernel code, stacks, and TCBs live in the [0, 1 GiB) identity map: copy
    // PML4[0] so they stay mapped after the CR3 switch (as Step 17 does).
    let current = &*(active_pml4_phys() as *const PageTable);
    let new = &mut *(space.pml4_phys() as *mut PageTable);
    *new.entry_mut(0) = *current.entry(0);

    let va = VirtualAddress::try_new(SHARED_USER_VA).expect("canonical");
    space
        .map_region(VirtualRegion {
            base: va,
            size: 4096,
            kind: RegionKind::Data,
        })
        .expect("map test page");
    let pa = space.translate(va).expect("test page mapped");
    // SAFETY: `pa` is a frame just mapped for this page, identity-mapped.
    core::ptr::write_volatile(pa as *mut u64, value);
    space
}
