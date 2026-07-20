use super::super::{AsyncJob, DiffApp, HunkFocusScrollBehavior, TrailingContextWorker};
use crate::model::{
    ContextKey, ContextSourceEntry, ContextSourceKey, FileIndex, HunkIndex, UiRow, line_after_hunk,
};
use crate::runtime;
use crate::syntax::{
    DiffSide, available_context_lines, context_source_byte_limit,
    count_context_source_lines_cancellable, full_file_source,
    load_full_file_source_limited_cancellable,
};
use mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES;
use std::sync::{Arc, atomic::AtomicBool};
use tokio::sync::oneshot;

impl DiffApp {
    pub(in crate::app) fn retry_unresolved_trailing_context(&mut self) {
        // A bounded discovery started in hunk mode may have skipped a large
        // source. Remove those sentinels so full-file discovery retries them
        // without the display-time cap.
        self.jobs.trailing_context_worker = None;
        let unresolved = self
            .document
            .trailing_context_lines
            .iter()
            .filter_map(|(key, lines)| {
                (*lines == 0 && !self.document.trailing_context_sides.contains_key(key))
                    .then_some(*key)
            })
            .collect::<Vec<_>>();
        for key in unresolved {
            if [DiffSide::New, DiffSide::Old]
                .into_iter()
                .any(|side| self.has_context_source(key.file.get(), side))
            {
                self.document.trailing_context_lines.remove(&key);
            }
        }
    }

    pub(crate) fn discover_trailing_context_for_viewport(&mut self) -> bool {
        if self.jobs.trailing_context_worker.is_some() {
            return false;
        }

        let mut files = Vec::new();
        for rendered_row in self.rendered_diff_rows_for_viewport(self.viewport.viewport_rows.max(1))
        {
            let Some(file) = self.document.model.file_at_row(rendered_row.model_row) else {
                continue;
            };
            if !files.contains(&file) {
                files.push(file);
            }
        }

        let discovery_byte_limit = (!self.full_file_mode_active()).then(|| {
            self.config
                .syntax_limits
                .max_source_bytes
                .min(DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES)
        });
        let mut requests = Vec::new();
        for file in files {
            let Some(file_diff) = self.document.changeset.files.get(file) else {
                continue;
            };
            let hunk_count = file_diff.hunks().len();
            let Some(last_hunk) = file_diff.hunks().last() else {
                continue;
            };
            let key = ContextKey {
                file: FileIndex::new(file),
                hunk: HunkIndex::new(hunk_count),
            };
            if self.document.trailing_context_lines.contains_key(&key) {
                continue;
            }

            let old_start = line_after_hunk(last_hunk.old_start(), last_hunk.old_count());
            let new_start = line_after_hunk(last_hunk.new_start(), last_hunk.new_count());
            let mut candidates = Vec::new();
            let mut source_loading = false;
            for side in [DiffSide::New, DiffSide::Old] {
                let source_key = ContextSourceKey {
                    file: FileIndex::new(file),
                    side,
                };
                match self.document.context_cache.get(&source_key) {
                    Some(ContextSourceEntry::Lines(lines)) => {
                        candidates.push((side, Some(lines.len()), None));
                    }
                    Some(ContextSourceEntry::Loading) => {
                        source_loading = true;
                        continue;
                    }
                    Some(ContextSourceEntry::Unavailable) => continue,
                    None => {
                        let source = full_file_source(
                            &self.document.changeset.repo,
                            &self.document.options,
                            file_diff,
                            side,
                        );
                        if let Some(source) = source {
                            candidates.push((side, None, Some(source)));
                        }
                    }
                }
            }

            if candidates.is_empty() {
                if !source_loading {
                    self.document.trailing_context_lines.insert(key, 0);
                }
                continue;
            }
            requests.push((key, old_start, new_start, candidates));
        }

        if requests.is_empty() {
            return false;
        }

        let generation = self.document.generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (tx, rx) = oneshot::channel();
        drop(runtime::spawn_blocking(move || {
            let results = requests
                .into_iter()
                .map(|(key, old_start, new_start, candidates)| {
                    let source =
                        candidates
                            .into_iter()
                            .find_map(|(side, cached_line_count, source)| {
                                if let Some(line_count) = cached_line_count {
                                    return Some((side, line_count));
                                }
                                let source = source?;
                                let source_limit =
                                    discovery_byte_limit.unwrap_or_else(context_source_byte_limit);
                                let text = match load_full_file_source_limited_cancellable(
                                    &source,
                                    source_limit,
                                    &worker_cancelled,
                                ) {
                                    Ok(text) => text,
                                    Err(_) => return None,
                                };
                                if discovery_byte_limit.is_some_and(|limit| text.len() > limit) {
                                    return None;
                                }
                                let line_count = count_context_source_lines_cancellable(
                                    &text,
                                    &worker_cancelled,
                                )
                                .ok()?;
                                Some((side, line_count))
                            });
                    let (available, source_side) = match source {
                        Some((side, line_count)) => {
                            let source_start = match side {
                                DiffSide::Old => old_start,
                                DiffSide::New => new_start,
                            };
                            (
                                available_context_lines(source_start, usize::MAX, line_count),
                                Some(side),
                            )
                        }
                        None => (0, None),
                    };
                    (key, available, source_side)
                })
                .collect();
            let _ = tx.send(results);
        }));
        self.jobs.trailing_context_worker = Some(TrailingContextWorker {
            generation,
            cancelled,
            job: AsyncJob::new(rx),
        });
        true
    }

    pub(crate) fn drain_trailing_context_worker(&mut self) -> bool {
        let Some(outcome) = self
            .jobs
            .trailing_context_worker
            .as_mut()
            .and_then(|worker| match worker.job.try_recv() {
                Ok(results) => Some(Some(results)),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(None),
            })
        else {
            return false;
        };
        let Some(worker) = self.jobs.trailing_context_worker.take() else {
            return false;
        };

        let anchor = self
            .model_row_at_scroll(self.viewport.scroll)
            .and_then(|(row, _)| {
                if matches!(self.document.model.row(row), Some(UiRow::FileSeparator)) {
                    self.document.model.file_at_row(row.saturating_add(1))
                } else {
                    self.document.model.file_at_row(row)
                }
            })
            .map(|file| (file, self.relative_scroll_from_file_start(file)));
        let mut model_changed = false;
        if worker.generation == self.document.generation
            && let Some(results) = outcome
        {
            for (key, available, source_side) in results {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.document.trailing_context_lines.entry(key)
                {
                    entry.insert(available);
                    if let Some(side) = source_side {
                        self.document.trailing_context_sides.insert(key, side);
                    }
                    model_changed |= available > 0;
                }
            }
        }

        // Trigger another discovery pass even when this batch found no context:
        // the viewport may have moved while the worker was busy.
        self.runtime.dirty = true;
        if !model_changed {
            return false;
        }

        self.rebuild_model_after_context_visibility_change();
        if let Some((file, relative_scroll)) = anchor
            && let Some(start) = self.document.model.file_start_row(file)
        {
            let scroll = self
                .scroll_for_model_row(start)
                .saturating_add(relative_scroll);
            self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
        }
        true
    }

    pub(in crate::app) fn context_side_order(&self, file: usize) -> [DiffSide; 2] {
        let preferred = self
            .document
            .changeset
            .files
            .get(file)
            .map(|file_diff| ContextKey {
                file: FileIndex::new(file),
                hunk: HunkIndex::new(file_diff.hunks().len()),
            })
            .and_then(|key| self.document.trailing_context_sides.get(&key).copied());
        match preferred {
            Some(DiffSide::Old) => [DiffSide::Old, DiffSide::New],
            Some(DiffSide::New) | None => [DiffSide::New, DiffSide::Old],
        }
    }

    pub(in crate::app) fn context_source_line_count(
        &self,
        file: usize,
    ) -> Option<(DiffSide, usize)> {
        for side in self.context_side_order(file) {
            let key = ContextSourceKey {
                file: FileIndex::new(file),
                side,
            };
            match self.document.context_cache.get(&key) {
                Some(ContextSourceEntry::Lines(lines)) => return Some((side, lines.len())),
                Some(ContextSourceEntry::Loading) => return None,
                Some(ContextSourceEntry::Unavailable) => continue,
                None => {
                    if let Some(lines) = self.load_context_lines(file, side) {
                        return Some((side, lines.len()));
                    }
                }
            }
        }
        None
    }

    pub(in crate::app) fn refresh_trailing_context_for_file(&mut self, file: usize) -> bool {
        let Some((hunk_count, old_start, new_start)) = self
            .document
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| {
                let last_hunk = file_diff.hunks().last()?;
                Some((
                    file_diff.hunks().len(),
                    line_after_hunk(last_hunk.old_start(), last_hunk.old_count()),
                    line_after_hunk(last_hunk.new_start(), last_hunk.new_count()),
                ))
            })
        else {
            return false;
        };
        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            return false;
        };
        let source_start = match side {
            DiffSide::Old => old_start,
            DiffSide::New => new_start,
        };
        let available = available_context_lines(source_start, usize::MAX, source_lines.len());
        let key = ContextKey {
            file: FileIndex::new(file),
            hunk: HunkIndex::new(hunk_count),
        };
        self.document.trailing_context_lines.insert(key, available);
        self.document.trailing_context_sides.insert(key, side);
        true
    }

    pub(in crate::app) fn restore_full_file_trailing_context_for_line(
        &mut self,
        file: usize,
        line: usize,
    ) {
        if !self.full_file_mode_active() {
            return;
        }
        self.load_full_file_context_for_files_sync(&[FileIndex::new(file)]);

        let Some(file_diff) = self.document.changeset.files.get(file) else {
            return;
        };
        let hunk_count = file_diff.hunks().len();
        let Some(last_hunk) = file_diff.hunks().last() else {
            return;
        };
        let old_start = line_after_hunk(last_hunk.old_start(), last_hunk.old_count());
        let new_start = line_after_hunk(last_hunk.new_start(), last_hunk.new_count());
        if line < new_start {
            return;
        }

        let key = ContextKey {
            file: FileIndex::new(file),
            hunk: HunkIndex::new(hunk_count),
        };
        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            return;
        };
        let source_start = match side {
            DiffSide::Old => old_start,
            DiffSide::New => new_start,
        };
        let available = available_context_lines(source_start, usize::MAX, source_lines.len());
        self.document.trailing_context_lines.insert(key, available);
        self.document.trailing_context_sides.insert(key, side);
        if available == 0 {
            return;
        }

        self.document.context_expansions.insert(key, available);
        self.update_max_line_width_for_expanded_context(&source_lines, source_start, 0..available);
        self.rebuild_model_after_context_visibility_change();
    }
}
