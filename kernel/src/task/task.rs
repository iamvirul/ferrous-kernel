//! Task Control Block and related types (Phase 2.1.1).

use core::sync::atomic::{AtomicU8, Ordering};

use super::ProcessId;

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

/// Opaque, unforgeable identifier for a kernel task (thread).
///
/// A newtype over `u64` prevents accidental comparison or arithmetic with
/// unrelated integer values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct TaskId(u64);

impl TaskId {
    /// Construct a `TaskId` from a raw value.
    ///
    /// Callers must ensure uniqueness; the kernel task table is the
    /// authoritative allocator (Phase 2.2).
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the underlying raw value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for TaskId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "tid({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// TaskState
// ---------------------------------------------------------------------------

/// Execution state of a [`TaskControlBlock`].
///
/// # State machine
///
/// ```text
///   Ready ──(scheduled)──► Running ──(yield/preempt)──► Ready
///                             │
///                        (wait/block)
///                             │
///                             ▼
///                          Blocked ──(wake)──► Ready
///
///   Ready / Running / Blocked ──(exit)──► Exiting ──(cleanup)──► Zombie
/// ```
///
/// Use [`TaskState::can_transition_to`] to validate a transition before
/// applying it, or call [`TaskControlBlock::try_transition`] for an atomic
/// compare-and-swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    /// Eligible to run; waiting for the scheduler to dispatch it.
    Ready = 0,
    /// Currently executing on a CPU core.
    Running = 1,
    /// Waiting for an event (I/O, IPC, timer). Not eligible for dispatch.
    Blocked = 2,
    /// Cleanup in progress; no longer scheduled.
    Exiting = 3,
    /// All resources freed; waiting for the parent process to collect status.
    Zombie = 4,
}

impl TaskState {
    /// Returns `true` if the `self → next` transition is permitted.
    pub const fn can_transition_to(self, next: TaskState) -> bool {
        matches!(
            (self, next),
            (TaskState::Ready,   TaskState::Running) // scheduler dispatch
            | (TaskState::Running, TaskState::Ready)   // preempted / yield
            | (TaskState::Running, TaskState::Blocked) // blocking wait
            | (TaskState::Blocked, TaskState::Ready)   // event wake-up
            | (TaskState::Ready,   TaskState::Exiting) // killed while ready
            | (TaskState::Running, TaskState::Exiting) // voluntary exit
            | (TaskState::Blocked, TaskState::Exiting) // killed while blocked
            | (TaskState::Exiting, TaskState::Zombie) // cleanup complete
        )
    }

    /// Decode a raw `u8` produced by [`AtomicU8`] load.
    ///
    /// Panics on unrecognised values — this indicates memory corruption.
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0 => TaskState::Ready,
            1 => TaskState::Running,
            2 => TaskState::Blocked,
            3 => TaskState::Exiting,
            4 => TaskState::Zombie,
            _ => panic!("TaskState: unrecognised discriminant {}", v),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskStateError
// ---------------------------------------------------------------------------

/// Error returned by [`TaskControlBlock::try_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStateError {
    /// The requested `from → to` transition is not permitted by the state
    /// machine (regardless of current state).
    InvalidTransition { from: TaskState, to: TaskState },
    /// The CAS failed: the task's state was `actual`, not the expected `from`.
    UnexpectedState {
        expected: TaskState,
        actual: TaskState,
    },
}

// ---------------------------------------------------------------------------
// TaskPriority
// ---------------------------------------------------------------------------

/// Scheduling priority for a task.
///
/// Higher numeric values indicate higher priority. The scheduler (Phase 2.2)
/// will use these to order the run queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TaskPriority {
    /// Runs only when no other task is ready (idle task).
    Idle = 0,
    /// Background work; may be starved under load.
    Low = 1,
    /// General-purpose kernel tasks.
    Normal = 2,
    /// Latency-sensitive tasks.
    High = 3,
    /// Soft real-time; preempts all lower-priority tasks immediately.
    RealTime = 4,
}

// ---------------------------------------------------------------------------
// RegisterState
// ---------------------------------------------------------------------------

/// Saved CPU register state used for context switching.
///
/// Only the callee-saved registers (System V AMD64 ABI) plus `rsp`, `rip`,
/// and `rflags` are preserved here — the caller-saved registers are either
/// on the stack or do not need to survive a context switch.
///
/// # `repr(C)` requirement
///
/// The context-switch assembly stub (Phase 2.2.3) will load and store fields
/// by their byte offset from the struct base.  **Do not reorder fields**
/// without updating the assembly accordingly.
///
/// # Field offsets (for reference)
///
/// | Field    | Offset |
/// |----------|--------|
/// | `rbx`    | 0x00   |
/// | `rbp`    | 0x08   |
/// | `r12`    | 0x10   |
/// | `r13`    | 0x18   |
/// | `r14`    | 0x20   |
/// | `r15`    | 0x28   |
/// | `rsp`    | 0x30   |
/// | `rip`    | 0x38   |
/// | `rflags` | 0x40   |
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct RegisterState {
    /// Callee-saved: `rbx`
    pub rbx: u64,
    /// Callee-saved: `rbp` (frame pointer)
    pub rbp: u64,
    /// Callee-saved: `r12`
    pub r12: u64,
    /// Callee-saved: `r13`
    pub r13: u64,
    /// Callee-saved: `r14`
    pub r14: u64,
    /// Callee-saved: `r15`
    pub r15: u64,
    /// Stack pointer — restored by the switch routine.
    pub rsp: u64,
    /// Instruction pointer — the address to resume execution at.
    pub rip: u64,
    /// CPU flags register.
    pub rflags: u64,
}

// ---------------------------------------------------------------------------
// TaskControlBlock
// ---------------------------------------------------------------------------

/// Per-task execution context.
///
/// The TCB holds everything the scheduler needs to suspend and resume a task:
/// saved register state, kernel stack bounds, scheduling metadata, and the
/// owning process ID.
///
/// # Safety invariants
///
/// - `stack_top` and `stack_bottom` delimit the kernel stack allocated for
///   this task.  They are stored as raw pointers because the allocator that
///   owns the stack may outlive the borrow; the kernel stack allocator
///   (Phase 2.2) is responsible for their lifetime.
/// - `registers.rsp` must lie within `[stack_bottom, stack_top)` whenever
///   the task is not in the `Running` state.
/// - The `state` field is an `AtomicU8`; all state reads and writes go
///   through the provided accessors to ensure ordering.
/// - `repr(C)` is required for stable field offsets used by the context-switch
///   assembly (Phase 2.2.3).
#[repr(C)]
pub struct TaskControlBlock {
    /// Unique task identifier.
    pub id: TaskId,
    /// Saved register state — valid when the task is not `Running`.
    pub registers: RegisterState,
    /// Top of the kernel stack (highest address; stack grows downward).
    stack_top: *mut u8,
    /// Bottom of the kernel stack (lowest valid address).
    stack_bottom: *mut u8,
    /// Current execution state (see [`TaskState`]).
    state: AtomicU8,
    /// Scheduling priority.
    pub priority: TaskPriority,
    /// Remaining time-slice in scheduler ticks (decremented by the timer ISR).
    pub time_slice: u32,
    /// ID of the [`Process`](super::Process) that owns this task.
    pub owner_pid: ProcessId,
}

// SAFETY: `stack_top` and `stack_bottom` are never dereferenced through the
// TCB itself — they exist solely for bounds checking.  No aliased mutable
// access is possible through these pointers from the TCB type alone.
unsafe impl Send for TaskControlBlock {}
unsafe impl Sync for TaskControlBlock {}

impl TaskControlBlock {
    /// Default time-slice in scheduler ticks for a new `Normal`-priority task.
    pub const DEFAULT_TIME_SLICE: u32 = 10;

    /// Construct a new `TaskControlBlock` in the `Ready` state.
    ///
    /// # Safety
    ///
    /// - `stack_top` must be the highest address of a valid, allocated kernel
    ///   stack region (i.e. `stack_top > stack_bottom`).
    /// - `stack_bottom` must be the lowest valid address of that region.
    /// - Both pointers must remain valid for the entire lifetime of this TCB.
    /// - The stack must be at least 8-byte aligned.
    pub unsafe fn new(
        id: TaskId,
        owner_pid: ProcessId,
        stack_top: *mut u8,
        stack_bottom: *mut u8,
        priority: TaskPriority,
    ) -> Self {
        debug_assert!(
            stack_top > stack_bottom,
            "stack_top must be above stack_bottom"
        );
        debug_assert!(
            (stack_top as usize) % 8 == 0,
            "stack_top must be 8-byte aligned"
        );

        Self {
            id,
            registers: RegisterState::default(),
            stack_top,
            stack_bottom,
            state: AtomicU8::new(TaskState::Ready as u8),
            priority,
            time_slice: Self::DEFAULT_TIME_SLICE,
            owner_pid,
        }
    }

    // -----------------------------------------------------------------------
    // State accessors
    // -----------------------------------------------------------------------

    /// Load the current [`TaskState`] with `Acquire` ordering.
    pub fn state(&self) -> TaskState {
        TaskState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Atomically transition from `from` to `to` using compare-and-swap.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err(TaskStateError::InvalidTransition)` if the transition is
    /// not permitted by the state machine.
    /// Returns `Err(TaskStateError::UnexpectedState)` if the CAS fails because
    /// the task's actual state differs from `from`.
    pub fn try_transition(&self, from: TaskState, to: TaskState) -> Result<(), TaskStateError> {
        if !from.can_transition_to(to) {
            return Err(TaskStateError::InvalidTransition { from, to });
        }
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|actual| TaskStateError::UnexpectedState {
                expected: from,
                actual: TaskState::from_u8(actual),
            })
    }

    // -----------------------------------------------------------------------
    // Stack accessors
    // -----------------------------------------------------------------------

    /// Return the top of the kernel stack (highest address).
    pub fn stack_top(&self) -> *mut u8 {
        self.stack_top
    }

    /// Return the bottom of the kernel stack (lowest valid address).
    pub fn stack_bottom(&self) -> *mut u8 {
        self.stack_bottom
    }

    /// Return the stack size in bytes.
    pub fn stack_size(&self) -> usize {
        // SAFETY: both pointers come from the same allocation (invariant on
        // `new`); the subtraction cannot wrap.
        (self.stack_top as usize).saturating_sub(self.stack_bottom as usize)
    }

    /// Return `true` if `addr` falls within this task's kernel stack.
    pub fn stack_contains(&self, addr: *const u8) -> bool {
        let a = addr as usize;
        a >= self.stack_bottom as usize && a < self.stack_top as usize
    }
}

impl core::fmt::Debug for TaskControlBlock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TaskControlBlock")
            .field("id", &self.id)
            .field("state", &self.state())
            .field("priority", &self.priority)
            .field("time_slice", &self.time_slice)
            .field("owner_pid", &self.owner_pid)
            .field("stack_size", &self.stack_size())
            .finish()
    }
}
