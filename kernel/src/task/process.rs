//! Process structure and related types (Phase 2.1.1).

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering};

use super::TaskId;

/// Maximum number of tasks a single process may own simultaneously.
///
/// Sized to avoid heap allocation in the fixed array while covering all
/// practical kernel workloads in Phase 2. A slab allocator (Phase 2.2) will
/// relax this constraint.
pub const MAX_TASKS_PER_PROCESS: usize = 16;

// ---------------------------------------------------------------------------
// ProcessId
// ---------------------------------------------------------------------------

/// Opaque, unforgeable identifier for a [`Process`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ProcessId(u64);

impl ProcessId {
    /// Construct a `ProcessId` from a raw value.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the underlying raw value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for ProcessId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "pid({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// ProcessState
// ---------------------------------------------------------------------------

/// Lifecycle state of a [`Process`].
///
/// # State machine
///
/// ```text
///   Active ──(exit called)──► Exiting ──(resources freed)──► Zombie
/// ```
///
/// A `Zombie` process retains its PID and exit code until the parent collects
/// them (Phase 2.1.5 syscall interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessState {
    /// Normally running; may have one or more active tasks.
    Active = 0,
    /// Exit in progress; tasks being torn down, resources being freed.
    Exiting = 1,
    /// All resources freed; exit code available for parent collection.
    Zombie = 2,
}

impl ProcessState {
    /// Returns `true` if the `self → next` transition is permitted.
    pub const fn can_transition_to(self, next: ProcessState) -> bool {
        matches!(
            (self, next),
            (ProcessState::Active, ProcessState::Exiting)
                | (ProcessState::Exiting, ProcessState::Zombie)
        )
    }

    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0 => ProcessState::Active,
            1 => ProcessState::Exiting,
            2 => ProcessState::Zombie,
            _ => panic!("ProcessState: unrecognised discriminant {}", v),
        }
    }
}

// ---------------------------------------------------------------------------
// ProcessStateError
// ---------------------------------------------------------------------------

/// Error returned by [`Process::try_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStateError {
    /// The requested transition is not permitted by the state machine.
    InvalidTransition {
        from: ProcessState,
        to: ProcessState,
    },
    /// CAS failed: actual state differed from expected.
    UnexpectedState {
        expected: ProcessState,
        actual: ProcessState,
    },
}

// ---------------------------------------------------------------------------
// ProcessTaskError
// ---------------------------------------------------------------------------

/// Error returned by [`Process::add_task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTaskError {
    /// The process's task list is full ([`MAX_TASKS_PER_PROCESS`] reached).
    TaskListFull,
}

// ---------------------------------------------------------------------------
// Process
// ---------------------------------------------------------------------------

/// Address-space owner that groups one or more tasks.
///
/// A `Process` holds:
/// - A unique [`ProcessId`].
/// - An atomic [`ProcessState`].
/// - A fixed-capacity list of owned [`TaskId`]s.
/// - An exit code (meaningful only in `Exiting`/`Zombie` states).
/// - Stubs for the address space (Phase 2.1.2) and capability space
///   (Phase 2.3.1) that will be populated in those phases.
///
/// # Synchronization
///
/// `state` and `exit_code` use atomic operations.  The `task_ids` array is
/// protected by `task_count` acting as a watermark: slots `0..task_count` are
/// written once and then only read, so concurrent reads are safe once a slot
/// is visible (ensured by the `Release` store on `task_count`).  Concurrent
/// writes to the task list require an external lock (to be added with the
/// scheduler in Phase 2.2).
pub struct Process {
    /// Unique process identifier.
    pub id: ProcessId,

    /// Current lifecycle state.
    state: AtomicU8,

    /// Flat list of tasks owned by this process.
    ///
    /// Slots `0..task_count` are valid.  Slots `task_count..MAX_TASKS_PER_PROCESS`
    /// are uninitialised and must not be read.
    task_ids: [TaskId; MAX_TASKS_PER_PROCESS],

    /// Number of valid entries in `task_ids`.
    task_count: AtomicUsize,

    /// Exit code value.
    exit_code: AtomicI32,

    /// Whether [`set_exit_code`](Self::set_exit_code) has been called.
    exit_code_set: AtomicBool,
    // -----------------------------------------------------------------------
    // Stubs — populated in later phases
    // -----------------------------------------------------------------------
    //
    // address_space: AddressSpaceHandle  (Phase 2.1.2)
    // capability_space: CapSpaceHandle   (Phase 2.3.1)
}

impl Process {
    /// Construct a new `Process` in the `Active` state with no tasks.
    pub fn new(id: ProcessId) -> Self {
        Self {
            id,
            state: AtomicU8::new(ProcessState::Active as u8),
            task_ids: [TaskId::new(0); MAX_TASKS_PER_PROCESS],
            task_count: AtomicUsize::new(0),
            exit_code: AtomicI32::new(0),
            exit_code_set: AtomicBool::new(false),
        }
    }

    // -----------------------------------------------------------------------
    // State accessors
    // -----------------------------------------------------------------------

    /// Load the current [`ProcessState`] with `Acquire` ordering.
    pub fn state(&self) -> ProcessState {
        ProcessState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Atomically transition from `from` to `to`.
    ///
    /// Returns `Ok(())` on success, or a [`ProcessStateError`] if the
    /// transition is invalid or the CAS fails.
    pub fn try_transition(
        &self,
        from: ProcessState,
        to: ProcessState,
    ) -> Result<(), ProcessStateError> {
        if !from.can_transition_to(to) {
            return Err(ProcessStateError::InvalidTransition { from, to });
        }
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|actual| ProcessStateError::UnexpectedState {
                expected: from,
                actual: ProcessState::from_u8(actual),
            })
    }

    // -----------------------------------------------------------------------
    // Task list
    // -----------------------------------------------------------------------

    /// Register a task as owned by this process.
    ///
    /// Returns `Err(ProcessTaskError::TaskListFull)` if the process already
    /// owns [`MAX_TASKS_PER_PROCESS`] tasks.
    ///
    /// # Concurrency
    ///
    /// Safe for single-threaded use.  Concurrent calls require an external
    /// lock (to be added with the scheduler in Phase 2.2).
    pub fn add_task(&mut self, id: TaskId) -> Result<(), ProcessTaskError> {
        let count = self.task_count.load(Ordering::Relaxed);
        if count >= MAX_TASKS_PER_PROCESS {
            return Err(ProcessTaskError::TaskListFull);
        }
        self.task_ids[count] = id;
        // Release so the written slot is visible before count is bumped.
        self.task_count.store(count + 1, Ordering::Release);
        Ok(())
    }

    /// Return the number of tasks currently registered with this process.
    pub fn task_count(&self) -> usize {
        self.task_count.load(Ordering::Acquire)
    }

    /// Return `true` if `id` is in this process's task list.
    pub fn has_task(&self, id: TaskId) -> bool {
        let count = self.task_count.load(Ordering::Acquire);
        self.task_ids[..count].contains(&id)
    }

    /// Return a slice of the registered task IDs.
    pub fn tasks(&self) -> &[TaskId] {
        let count = self.task_count.load(Ordering::Acquire);
        &self.task_ids[..count]
    }

    // -----------------------------------------------------------------------
    // Exit code
    // -----------------------------------------------------------------------

    /// Store the process exit code.
    ///
    /// Only accepts writes when the process is in the `Exiting` or `Zombie` state.
    /// Returns `Ok(())` on success, or `Err(())` if the process is in the `Active` state.
    pub fn set_exit_code(&self, code: i32) -> Result<(), ()> {
        let state = self.state();
        if state == ProcessState::Active {
            return Err(());
        }
        self.exit_code.store(code, Ordering::Release);
        self.exit_code_set.store(true, Ordering::Release);
        Ok(())
    }

    /// Return the exit code if [`set_exit_code`](Self::set_exit_code) has been
    /// called, or `None` if the process has not exited yet.
    pub fn exit_code(&self) -> Option<i32> {
        if self.exit_code_set.load(Ordering::Acquire) {
            Some(self.exit_code.load(Ordering::Acquire))
        } else {
            None
        }
    }
}

impl core::fmt::Debug for Process {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Process")
            .field("id", &self.id)
            .field("state", &self.state())
            .field("task_count", &self.task_count())
            .field("exit_code", &self.exit_code())
            .finish()
    }
}
