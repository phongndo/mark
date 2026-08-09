use std::{
    alloc::{GlobalAlloc, Layout},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PROFILE_LOCK: AtomicBool = AtomicBool::new(false);

/// A global allocator wrapper used by instrumented benchmark binaries.
///
/// Production binaries should keep using their allocator directly. The wrapper
/// deliberately counts every allocation with atomics, which makes its totals
/// useful for profiling but perturbs latency measurements.
pub struct ProfilingAllocator<A> {
    inner: A,
}

impl<A> ProfilingAllocator<A> {
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }

    /// Enables allocation-profile collection for this process.
    ///
    /// Call this once at process entry after installing this allocator as the
    /// global allocator.
    pub fn enable(&self) {
        ENABLED.store(true, Ordering::Release);
    }
}

// SAFETY: every operation is delegated to `inner` with the original pointer
// and layout. Counters are updated only after a successful allocation.
unsafe impl<A: GlobalAlloc> GlobalAlloc for ProfilingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = lock_profile();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s layout contract.
        let pointer = unsafe { self.inner.alloc(layout) };
        if !pointer.is_null() {
            record_allocation_locked(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _guard = lock_profile();
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        let pointer = unsafe { self.inner.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation_locked(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _guard = lock_profile();
        // SAFETY: the caller guarantees that `pointer` and `layout` identify an
        // allocation currently owned by `inner`.
        unsafe { self.inner.dealloc(pointer, layout) };
        record_deallocation_locked(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _guard = lock_profile();
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s pointer, layout,
        // and non-zero size requirements.
        let new_pointer = unsafe { self.inner.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            record_reallocation_locked(layout.size(), new_size);
        }
        new_pointer
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocationSnapshot {
    pub allocation_calls: u64,
    pub reallocation_calls: u64,
    pub deallocation_calls: u64,
    pub allocated_bytes: u64,
    pub deallocated_bytes: u64,
    pub live_bytes: u64,
    pub peak_live_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocationDelta {
    pub allocation_calls: u64,
    pub reallocation_calls: u64,
    pub deallocation_calls: u64,
    pub allocated_bytes: u64,
    pub deallocated_bytes: u64,
    pub live_bytes_delta: i128,
}

impl AllocationSnapshot {
    pub fn delta_since(self, earlier: Self) -> AllocationDelta {
        AllocationDelta {
            allocation_calls: self
                .allocation_calls
                .saturating_sub(earlier.allocation_calls),
            reallocation_calls: self
                .reallocation_calls
                .saturating_sub(earlier.reallocation_calls),
            deallocation_calls: self
                .deallocation_calls
                .saturating_sub(earlier.deallocation_calls),
            allocated_bytes: self.allocated_bytes.saturating_sub(earlier.allocated_bytes),
            deallocated_bytes: self
                .deallocated_bytes
                .saturating_sub(earlier.deallocated_bytes),
            live_bytes_delta: i128::from(self.live_bytes) - i128::from(earlier.live_bytes),
        }
    }
}

/// Returns whether allocation profiling has been explicitly enabled.
pub fn allocation_profiler_active() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Takes a process-wide allocation counter snapshot.
pub fn allocation_snapshot() -> AllocationSnapshot {
    let _guard = lock_profile();
    allocation_snapshot_locked()
}

/// Resets the recorded peak to the currently live byte count and returns the
/// snapshot at that exact boundary.
///
/// This is intended for single-coordinator benchmark setup. Concurrent worker
/// allocations and deallocations are serialized on either side of the reset.
pub fn reset_allocation_peak() -> AllocationSnapshot {
    let _guard = lock_profile();
    PEAK_LIVE_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
    allocation_snapshot_locked()
}

fn allocation_snapshot_locked() -> AllocationSnapshot {
    AllocationSnapshot {
        allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
        reallocation_calls: REALLOCATION_CALLS.load(Ordering::Relaxed),
        deallocation_calls: DEALLOCATION_CALLS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
        peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed),
    }
}

fn record_allocation_locked(size: usize) {
    let size = size as u64;
    ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
    add_live_bytes_locked(size);
}

fn record_deallocation_locked(size: usize) {
    let size = size as u64;
    DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    DEALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
    subtract_live_bytes_locked(size);
}

fn record_reallocation_locked(old_size: usize, new_size: usize) {
    let old_size = old_size as u64;
    let new_size = new_size as u64;
    REALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
    DEALLOCATED_BYTES.fetch_add(old_size, Ordering::Relaxed);
    match new_size.cmp(&old_size) {
        std::cmp::Ordering::Greater => add_live_bytes_locked(new_size - old_size),
        std::cmp::Ordering::Less => subtract_live_bytes_locked(old_size - new_size),
        std::cmp::Ordering::Equal => {}
    }
}

fn add_live_bytes_locked(size: u64) {
    let live = LIVE_BYTES
        .fetch_add(size, Ordering::Relaxed)
        .saturating_add(size);
    PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
}

fn subtract_live_bytes_locked(size: u64) {
    let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

fn lock_profile() -> ProfileLockGuard {
    while PROFILE_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
    ProfileLockGuard
}

struct ProfileLockGuard;

impl Drop for ProfileLockGuard {
    fn drop(&mut self) {
        PROFILE_LOCK.store(false, Ordering::Release);
    }
}
