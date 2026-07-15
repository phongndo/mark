use std::sync::Arc;

use super::{
    HighlightedLineRef, SyntaxPriority,
    runtime::SyntaxRuntime,
    source::SyntaxJobFailure,
    types::{
        DiffSide, SyntaxKey, SyntaxPosition, SyntaxSkipReason, SyntaxSourceId, SyntaxSourceKind,
    },
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyntaxDrain {
    pub(crate) changed: bool,
    pub(crate) changed_keys: Vec<SyntaxKey>,
}

impl SyntaxRuntime {
    pub(crate) fn drain(&mut self, generation: u64, max_results: usize) -> SyntaxDrain {
        let mut drain = SyntaxDrain::default();
        for _ in 0..max_results {
            let Ok(result) = self.result_rx.try_recv() else {
                break;
            };
            self.pending.remove(&result.key);
            if result.key.generation() != generation {
                self.stats.stale_results = self.stats.stale_results.saturating_add(1);
                continue;
            }

            record_latency(
                &mut self.stats,
                &result.language,
                result.source_kind,
                result.priority,
                result.queue_latency_micros,
                result.run_latency_micros,
            );

            match result.side {
                Ok(success) => {
                    let memory_bytes = success.side.memory_bytes();
                    self.cache
                        .insert_with_weight(result.key, Arc::new(success.side), memory_bytes);
                    self.stats.jobs_completed = self.stats.jobs_completed.saturating_add(1);
                    if let Some(source_bytes) = success.source_bytes {
                        self.stats.source_bytes_queued =
                            self.stats.source_bytes_queued.saturating_add(source_bytes);
                    }
                    if let Some(source_lines) = success.source_lines {
                        self.stats.source_lines_queued =
                            self.stats.source_lines_queued.saturating_add(source_lines);
                    }
                    self.stats.cache_entries_peak =
                        self.stats.cache_entries_peak.max(self.cache.len());
                    // Weighted peak tracking stays O(1) on the interactive
                    // drain path. The final benchmark snapshot separately
                    // deduplicates shared scope tables.
                    self.stats.estimated_memory_peak_bytes = self
                        .stats
                        .estimated_memory_peak_bytes
                        .max(self.cache.total_weight() as u64);
                    drain.changed = true;
                    drain.changed_keys.push(result.key);
                }
                Err(SyntaxJobFailure::Unavailable) => {
                    self.handle_unavailable_source(result.key);
                    self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
                    drain.changed = true;
                    drain.changed_keys.push(result.key);
                }
                Err(SyntaxJobFailure::HighlightError) => {
                    self.failed.insert(result.key);
                    let positions = self.positions_for_key(result.key);
                    for position in positions {
                        self.skipped
                            .insert(position, SyntaxSkipReason::HighlightError);
                    }
                    self.stats.jobs_failed = self.stats.jobs_failed.saturating_add(1);
                }
            }
        }
        drain
    }

    pub(crate) fn handle_unavailable_source(&mut self, key: SyntaxKey) {
        self.source_keys.remove(&key.source);
        if matches!(key.source.kind, SyntaxSourceKind::FullFile) {
            self.unavailable_full_files.insert(key);
        } else {
            let positions = self.positions_for_key(key);
            for position in positions {
                self.skipped.insert(position, SyntaxSkipReason::NoSource);
            }
        }

        let positions = self.positions_for_key(key);
        for position in positions {
            self.position_keys.remove(&position);
            self.line_maps.remove(&position);
        }
    }

    pub(crate) fn positions_for_key(&self, key: SyntaxKey) -> Vec<SyntaxPosition> {
        self.position_keys
            .iter()
            .filter_map(|(position, position_key)| (*position_key == key).then_some(*position))
            .collect()
    }

    pub(crate) fn line(
        &mut self,
        position: SyntaxPosition,
        line: usize,
    ) -> Option<HighlightedLineRef> {
        let highlighted = self.position_keys.get(&position).copied().and_then(|key| {
            let source_line = self
                .line_maps
                .get(&position)
                .and_then(|line_map| line_map.get(line))
                .and_then(|source_line| *source_line)?;
            let side = Arc::clone(self.cache.get(&key)?);
            HighlightedLineRef::new(side, source_line)
        });
        if highlighted.is_some() {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
        }
        highlighted
    }

    pub(crate) fn full_file_line(
        &mut self,
        generation: u64,
        file: usize,
        side: DiffSide,
        line_number: usize,
    ) -> Option<HighlightedLineRef> {
        let source_line = line_number.checked_sub(1)?;
        let source_id = SyntaxSourceId {
            generation,
            file,
            side,
            kind: SyntaxSourceKind::FullFile,
        };
        let key = self.source_keys.get(&source_id).copied();
        let highlighted = key
            .and_then(|key| self.cache.get(&key).map(Arc::clone))
            .and_then(|side| HighlightedLineRef::new(side, source_line));
        if highlighted.is_some() {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
        }
        highlighted
    }
}

fn record_latency(
    stats: &mut crate::theme::SyntaxBenchmarkReport,
    language: &str,
    source_kind: SyntaxSourceKind,
    priority: SyntaxPriority,
    queue_latency_micros: u128,
    run_latency_micros: u128,
) {
    let first_visible_latency = queue_latency_micros.saturating_add(run_latency_micros);
    if priority == SyntaxPriority::Visible && stats.first_visible_latency_micros.is_none() {
        stats.first_visible_latency_micros = Some(first_visible_latency);
    }

    let source_kind = source_kind_label(source_kind);
    if let Some(bucket) = stats
        .latency_buckets
        .iter_mut()
        .find(|bucket| bucket.language == language && bucket.source_kind == source_kind)
    {
        bucket.jobs = bucket.jobs.saturating_add(1);
        bucket.queue_latency_total_micros = bucket
            .queue_latency_total_micros
            .saturating_add(queue_latency_micros);
        bucket.queue_latency_max_micros = bucket.queue_latency_max_micros.max(queue_latency_micros);
        bucket.run_latency_total_micros = bucket
            .run_latency_total_micros
            .saturating_add(run_latency_micros);
        bucket.run_latency_max_micros = bucket.run_latency_max_micros.max(run_latency_micros);
        return;
    }

    stats
        .latency_buckets
        .push(crate::theme::SyntaxLatencyBucket {
            language: language.to_owned(),
            source_kind: source_kind.to_owned(),
            jobs: 1,
            queue_latency_total_micros: queue_latency_micros,
            queue_latency_max_micros: queue_latency_micros,
            run_latency_total_micros: run_latency_micros,
            run_latency_max_micros: run_latency_micros,
        });
}

fn source_kind_label(kind: SyntaxSourceKind) -> &'static str {
    match kind {
        SyntaxSourceKind::HunkSide { .. } => "hunk",
        SyntaxSourceKind::FullFile => "full_file",
    }
}
