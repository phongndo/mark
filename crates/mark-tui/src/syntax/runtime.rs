use std::{
    collections::{HashMap, HashSet},
    thread,
};

use mark_core::MarkResult;
use mark_syntax::{SyntaxLanguageSet, SyntaxLimits, SyntaxSettings};
use tokio::sync::mpsc::{self, Receiver};

use crate::theme::SyntaxBenchmarkReport;

use super::source::{SyntaxResult, run_syntax_worker};
use super::{LruCache, SyntaxWorkerQueue};

use super::types::{HighlightedSide, SyntaxKey, SyntaxPosition, SyntaxSkipReason, SyntaxSourceId};

#[derive(Debug)]
pub(crate) struct SyntaxRuntime {
    pub(crate) languages: SyntaxLanguageSet,
    pub(crate) limits: SyntaxLimits,
    pub(crate) result_rx: Receiver<SyntaxResult>,
    pub(crate) queue: SyntaxWorkerQueue,
    pub(crate) cache: LruCache<SyntaxKey, HighlightedSide>,
    pub(crate) pending: HashSet<SyntaxKey>,
    pub(crate) source_keys: HashMap<SyntaxSourceId, SyntaxKey>,
    pub(crate) position_keys: HashMap<SyntaxPosition, SyntaxKey>,
    pub(crate) line_maps: HashMap<SyntaxPosition, Vec<Option<usize>>>,
    pub(crate) skipped: HashMap<SyntaxPosition, SyntaxSkipReason>,
    pub(crate) skipped_sources: HashSet<SyntaxSourceId>,
    pub(crate) unavailable_full_files: HashSet<SyntaxKey>,
    pub(crate) failed: HashSet<SyntaxKey>,
    pub(crate) stats: SyntaxBenchmarkReport,
    pub(crate) workers: Vec<thread::JoinHandle<()>>,
}

impl SyntaxRuntime {
    pub(crate) fn start(settings: &SyntaxSettings) -> MarkResult<Option<Self>> {
        let languages = SyntaxLanguageSet::load_with_mode(settings.mode)?;
        Ok(Self::start_with_language_set(languages, settings.limits))
    }

    pub(crate) fn start_with_language_set(
        languages: SyntaxLanguageSet,
        limits: SyntaxLimits,
    ) -> Option<Self> {
        if languages.is_empty() {
            return None;
        }

        let (result_tx, result_rx) = mpsc::channel(limits.queue_entries.max(1));
        let queue = SyntaxWorkerQueue::new(limits.queue_entries, 0, limits.queue_bytes);
        let worker_count = limits.worker_threads.max(1);
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let worker_queue = queue.clone();
            let result_tx = result_tx.clone();
            workers.push(thread::spawn(move || {
                run_syntax_worker(worker_queue, result_tx)
            }));
        }
        drop(result_tx);

        Some(Self {
            languages,
            limits,
            result_rx,
            queue,
            cache: LruCache::new_weighted(limits.cache_entries, limits.cache_bytes),
            pending: HashSet::new(),
            source_keys: HashMap::new(),
            position_keys: HashMap::new(),
            line_maps: HashMap::new(),
            skipped: HashMap::new(),
            skipped_sources: HashSet::new(),
            unavailable_full_files: HashSet::new(),
            failed: HashSet::new(),
            stats: SyntaxBenchmarkReport::default(),
            workers,
        })
    }

    pub(crate) fn start_with_languages(
        languages: Vec<String>,
        limits: SyntaxLimits,
    ) -> Option<Self> {
        let languages = SyntaxLanguageSet::from_enabled_languages(&languages);
        Self::start_with_language_set(languages, limits)
    }

    pub(crate) fn clear(&mut self, generation: u64) {
        self.cache.clear();
        self.pending.clear();
        self.source_keys.clear();
        self.position_keys.clear();
        self.line_maps.clear();
        self.skipped.clear();
        self.skipped_sources.clear();
        self.unavailable_full_files.clear();
        self.failed.clear();
        self.queue.set_generation(generation);
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.queue.len() == 0
    }

    pub(crate) fn stats(&self) -> SyntaxBenchmarkReport {
        self.stats.clone()
    }

    pub(crate) fn estimated_memory_bytes(&self) -> usize {
        self.cache.total_weight()
    }

    pub(crate) fn scope_table_stats(&self) -> (usize, usize, u64, u64) {
        self.cache.values().fold(
            (0usize, 0usize, 0u64, 0u64),
            |(stacks, bytes, hits, misses), side| {
                let current = side.scope_table_stats();
                (
                    stacks.saturating_add(current.0),
                    bytes.saturating_add(current.1),
                    hits.saturating_add(current.2),
                    misses.saturating_add(current.3),
                )
            },
        )
    }
}

impl Drop for SyntaxRuntime {
    fn drop(&mut self) {
        self.result_rx.close();
        while self.result_rx.try_recv().is_ok() {}
        self.queue.close();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
