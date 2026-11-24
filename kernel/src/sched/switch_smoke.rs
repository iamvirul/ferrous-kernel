//! Boot self-test for context switching (Phase 2.2.3, ADR-0008 §4).
//!
//! The boot path acts as task 0. It switches into small kernel tasks, which
//! switch among themselves and finally back to boot. The tasks run inside
//! the interrupt-disabled scope that boot opened (ADR-0008 §3), so they use
//! boot's `InterruptGuard` as their proof. They never return (`fn() -> !`)
//! and are simply never resumed once the test ends.
//!
//! Checks:
//! - 20.1 ping-pong: the interleaving is exactly A, B, A, B, …, every switch
//!   points TSS.RSP0 at the incoming task's stack, and same-root switches
//!   perform no CR3 load.
//! - 20.2 callee-saved: rbx, rbp, r12–r15 survive a switch away and back,
//!   in both directions.
//! - 20.3 CR3: two tasks in different address spaces read the same user VA
//!   and each sees its own value; every root change loads CR3 exactly once.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::switch::{cr3_load_count, init_kernel_task_context, switch_to};
use crate::arch::x86_64::context::switch_context_checking_callee_saved;
use crate::arch::x86_64::cpu::{disable_interrupts, irq_disable_depth, InterruptGuard};
use crate::arch::x86_64::gdt;
use crate::memory::address_space::{active_pml4_phys, AddressSpace, RegionKind, VirtualRegion};
use crate::memory::paging::{PageTable, VirtualAddress};
use crate::task::{ProcessId, TaskControlBlock, TaskId, TaskPriority};

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
const TASKS: usize = 6;

static STACKS: TestCell<[TaskStack; TASKS]> =
    TestCell::new([const { TaskStack([0; TASK_STACK_SIZE]) }; TASKS]);
static TCBS: TestCell<[Option<TaskControlBlock>; TASKS]> = TestCell::new([const { None }; TASKS]);

/// Boot's interrupt guard, borrowed by the tasks running inside its scope.
static BOOT_IRQ: TestCell<*const InterruptGuard> = TestCell::new(core::ptr::null());

/// Ping-pong event log ('A' / 'B').
const ROUNDS: usize = 4;
static EVENTS: TestCell<[u8; 2 * ROUNDS]> = TestCell::new([0; 2 * ROUNDS]);
static EVENT_LEN: AtomicUsize = AtomicUsize::new(0);

/// Callee-saved masks reported by each side of the sentinel test.
static SENTINEL_MASKS: TestCell<[u64; 2]> = TestCell::new([u64::MAX; 2]);

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

/// Switch from task `from` to task `to` through `switch_to`, using boot's
/// interrupt scope.
///
/// # Safety
///
/// `from` is the running task; `to` is a created, resumable task; boot's
/// guard is alive (it is, until boot finishes the test).
unsafe fn switch(from: usize, to: usize) {
    assert_ne!(from, to);
    let tcbs = TCBS.0.get() as *mut Option<TaskControlBlock>;
    // SAFETY: distinct indices, so these borrows do not alias; slots are
    // populated by create_task before any switch into them.
    let prev = (*tcbs.add(from)).as_mut().expect("from task exists");
    let next = (*tcbs.add(to)).as_ref().expect("to task exists");
    let irq = &**BOOT_IRQ.0.get();
    switch_to(prev, next, irq);
}

/// Raw register pointers for the sentinel harness (bypasses `switch_to`).
unsafe fn regs(slot: usize) -> *mut crate::task::RegisterState {
    let tcb = (*TCBS.0.get())[slot].as_mut().expect("task exists");
    &mut tcb.registers
}

/// Assert TSS.RSP0 points at task `slot`'s kernel stack top.
unsafe fn assert_tss_rsp0_is(slot: usize) {
    let expected = (*TCBS.0.get())[slot].as_ref().unwrap().stack_top() as u64;
    let actual = gdt::TSS.rsp0;
    assert_eq!(actual, expected, "TSS.RSP0 not updated for task {slot}");
}

fn record(event: u8) {
    let i = EVENT_LEN.fetch_add(1, Ordering::Relaxed);
    // SAFETY: only the running task records (see TestCell).
    unsafe { (*EVENTS.0.get())[i] = event };
}

// ---------------------------------------------------------------------------
// Task bodies
// ---------------------------------------------------------------------------

fn ping_entry() -> ! {
    // SAFETY (whole body): runs only while boot is switched out inside the
    // test, with boot's guard alive.
    unsafe {
        for _ in 0..ROUNDS {
            assert_tss_rsp0_is(PING);
            record(b'A');
            switch(PING, PONG);
        }
        switch(PING, BOOT);
    }
    unreachable!("ping resumed after the test finished");
}

fn pong_entry() -> ! {
    // SAFETY: as in ping_entry.
    unsafe {
        loop {
            assert_tss_rsp0_is(PONG);
            record(b'B');
            switch(PONG, PING);
        }
    }
}

fn sentinel_entry() -> ! {
    // SAFETY: raw switches between this task and boot only; both contexts
    // are valid and interrupts are disabled by boot's guard.
    unsafe {
        // Clobber every callee-saved register with a different pattern on
        // the way back to boot, then check ours once boot resumes us.
        let mask = switch_context_checking_callee_saved(regs(SENTINEL), regs(BOOT), 0x2222_0000);
        (*SENTINEL_MASKS.0.get())[1] = mask;
        switch_context_checking_callee_saved(regs(SENTINEL), regs(BOOT), 0x4444_0000);
    }
    unreachable!("sentinel task resumed after the test finished");
}

/// Read the shared user VA through the active address space.
fn read_shared_va() -> u64 {
    // SAFETY: both test address spaces map SHARED_USER_VA as a present data
    // page; SMAP is off, so CPL 0 may read it. Only called while one of them
    // is active.
    unsafe { core::ptr::read_volatile(SHARED_USER_VA as *const u64) }
}

fn cr3_a_entry() -> ! {
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

    let kernel_root = active_pml4_phys();
    let depth_before = irq_disable_depth();
    let irq = disable_interrupts();
    *BOOT_IRQ.0.get() = &irq;

    install_task(BOOT, boot_stack_bottom, boot_stack_top, kernel_root, None);

    // 20.1 — ping-pong through switch_to.
    create_task(PING, kernel_root, ping_entry);
    create_task(PONG, kernel_root, pong_entry);
    let loads_before = cr3_load_count();
    switch(BOOT, PING);
    assert_tss_rsp0_is(BOOT);
    let events = &*EVENTS.0.get();
    assert_eq!(EVENT_LEN.load(Ordering::Relaxed), 2 * ROUNDS);
    assert_eq!(events, b"ABABABAB", "tasks did not alternate");
    assert_eq!(
        cr3_load_count(),
        loads_before,
        "same-root switches must not load CR3"
    );
    log::info!(
        "[OK] 20.1) ping-pong: {} switches alternated A/B, TSS.RSP0 updated, no CR3 load on same root",
        2 * ROUNDS + 1
    );

    // 20.2 — callee-saved registers, both directions.
    create_task(SENTINEL, kernel_root, sentinel_entry);
    let mask = switch_context_checking_callee_saved(regs(BOOT), regs(SENTINEL), 0x1111_0000);
    assert_eq!(
        mask, 0,
        "boot side lost callee-saved registers (mask {mask:#b})"
    );
    let mask = switch_context_checking_callee_saved(regs(BOOT), regs(SENTINEL), 0x3333_0000);
    assert_eq!(
        mask, 0,
        "boot side lost callee-saved registers (mask {mask:#b})"
    );
    let task_mask = (*SENTINEL_MASKS.0.get())[1];
    assert_eq!(
        task_mask, 0,
        "task side lost callee-saved registers (mask {task_mask:#b})"
    );
    log::info!(
        "[OK] 20.2) callee-saved: rbx, rbp, r12-r15 preserved across switches (both directions)"
    );

    // 20.3 — CR3 switching between two address spaces.
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

    // The interrupt scope is unchanged: tasks used boot's, opened no others.
    assert_eq!(irq_disable_depth(), depth_before + 1);
    drop(irq);
    assert_eq!(irq_disable_depth(), depth_before);

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
