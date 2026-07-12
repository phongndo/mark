use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

use super::{SyntaxKey, source::SyntaxJob};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxPriority {
    Visible,
    Prefetch,
}

#[derive(Debug, Clone)]
pub(crate) struct SyntaxWorkerQueue {
    pub(crate) inner: Arc<SyntaxWorkerQueueInner>,
}

#[derive(Debug)]
pub(crate) struct SyntaxWorkerQueueInner {
    pub(crate) state: Mutex<SyntaxWorkerQueueState>,
    pub(crate) ready: Condvar,
    pub(crate) capacity: usize,
    pub(crate) capacity_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct SyntaxWorkerQueueState {
    pub(crate) generation: u64,
    pub(crate) visible: VecDeque<SyntaxJob>,
    pub(crate) prefetch: VecDeque<SyntaxJob>,
    pub(crate) queued_bytes: u64,
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxQueuePush {
    pub(crate) dropped: Option<SyntaxKey>,
    pub(crate) dropped_more: Vec<SyntaxKey>,
    pub(crate) dropped_overflow: bool,
    pub(crate) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxQueueError {
    Full,
    Closed,
    Stale,
}

impl SyntaxWorkerQueue {
    pub(crate) fn new(capacity: usize, generation: u64, capacity_bytes: usize) -> Self {
        Self {
            inner: Arc::new(SyntaxWorkerQueueInner {
                state: Mutex::new(SyntaxWorkerQueueState {
                    generation,
                    visible: VecDeque::new(),
                    prefetch: VecDeque::new(),
                    queued_bytes: 0,
                    closed: false,
                }),
                ready: Condvar::new(),
                capacity,
                capacity_bytes: capacity_bytes as u64,
            }),
        }
    }

    pub(crate) fn try_push(
        &self,
        job: SyntaxJob,
        priority: SyntaxPriority,
    ) -> Result<SyntaxQueuePush, SyntaxQueueError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| SyntaxQueueError::Closed)?;
        if state.closed {
            return Err(SyntaxQueueError::Closed);
        }
        if job.key.generation() != state.generation {
            return Err(SyntaxQueueError::Stale);
        }
        if self.inner.capacity == 0 {
            return Err(SyntaxQueueError::Full);
        }

        let mut dropped = None;
        let mut dropped_more = Vec::new();
        while state.len() >= self.inner.capacity
            || state.queued_bytes.saturating_add(job.queued_source_bytes)
                > self.inner.capacity_bytes
        {
            match priority {
                SyntaxPriority::Visible => {
                    let Some(evicted) = state.prefetch.pop_back() else {
                        return Err(SyntaxQueueError::Full);
                    };
                    state.queued_bytes = state
                        .queued_bytes
                        .saturating_sub(evicted.queued_source_bytes);
                    if dropped.is_none() {
                        dropped = Some(evicted.key);
                    } else {
                        dropped_more.push(evicted.key);
                    }
                }
                SyntaxPriority::Prefetch => return Err(SyntaxQueueError::Full),
            }
        }

        match priority {
            SyntaxPriority::Visible => state.visible.push_back(job),
            SyntaxPriority::Prefetch => state.prefetch.push_back(job),
        }
        let queued = match priority {
            SyntaxPriority::Visible => state.visible.back(),
            SyntaxPriority::Prefetch => state.prefetch.back(),
        }
        .map(|job| job.queued_source_bytes)
        .unwrap_or_default();
        state.queued_bytes = state.queued_bytes.saturating_add(queued);
        let depth = state.len();
        self.inner.ready.notify_one();
        Ok(SyntaxQueuePush {
            dropped,
            dropped_more,
            dropped_overflow: false,
            depth,
        })
    }

    pub(crate) fn promote(&self, key: SyntaxKey) -> bool {
        let Ok(mut state) = self.inner.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }

        let Some(index) = state.prefetch.iter().position(|job| job.key == key) else {
            return false;
        };
        let Some(job) = state.prefetch.remove(index) else {
            return false;
        };
        state.visible.push_back(job);
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn set_generation(&self, generation: u64) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.generation = generation;
        state
            .visible
            .retain(|job| job.key.generation() == generation);
        state
            .prefetch
            .retain(|job| job.key.generation() == generation);
        state.queued_bytes = state
            .visible
            .iter()
            .chain(state.prefetch.iter())
            .map(|job| job.queued_source_bytes)
            .sum();
        self.inner.ready.notify_all();
    }

    pub(crate) fn pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }

            let job = state
                .visible
                .pop_front()
                .or_else(|| state.prefetch.pop_front());
            if let Some(job) = job {
                state.queued_bytes = state.queued_bytes.saturating_sub(job.queued_source_bytes);
                if job.key.generation() == state.generation {
                    return Some(job);
                }
                continue;
            }

            state = self.inner.ready.wait(state).ok()?;
        }
    }

    pub(crate) fn close(&self) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.closed = true;
        state.visible.clear();
        state.prefetch.clear();
        state.queued_bytes = 0;
        self.inner.ready.notify_all();
    }

    pub(crate) fn len(&self) -> usize {
        let Ok(state) = self.inner.state.lock() else {
            return 0;
        };
        state.len()
    }

    #[cfg(test)]
    pub(crate) fn try_pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        state
            .visible
            .pop_front()
            .or_else(|| state.prefetch.pop_front())
    }
}

impl SyntaxWorkerQueueState {
    pub(crate) fn len(&self) -> usize {
        self.visible.len() + self.prefetch.len()
    }
}
