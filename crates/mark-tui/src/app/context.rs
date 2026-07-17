use super::{
    AsyncJob, DiffApp, HunkFocusModelBehavior, HunkFocusScrollBehavior, MAX_LIVE_GREP_MATCHES,
    TrailingContextWorker,
};
use crate::model::{
    ContextKey, ContextSourceEntry, ContextSourceKey, FileIndex, HunkIndex, UiRow,
    context_expands_up,
};
use crate::render::text::display_width;
use crate::runtime;
use crate::search::grep_match_rows;
use crate::syntax::{
    DiffSide, available_context_lines, full_file_source, full_file_source_size,
    load_full_file_source, split_context_source_lines,
};
use mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES;
use std::sync::Arc;
use tokio::sync::oneshot;

impl DiffApp {
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

        let discovery_byte_limit = self
            .config
            .syntax_limits
            .max_source_bytes
            .min(DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES);
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

            let old_start = last_hunk.old_start().saturating_add(last_hunk.old_count());
            let new_start = last_hunk.new_start().saturating_add(last_hunk.new_count());
            let mut candidates = Vec::new();
            for side in [DiffSide::New, DiffSide::Old] {
                let source_key = ContextSourceKey {
                    file: FileIndex::new(file),
                    side,
                };
                match self.document.context_cache.get(&source_key) {
                    Some(ContextSourceEntry::Lines(lines)) => {
                        candidates.push((side, Some(lines.len()), None));
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
                self.document.trailing_context_lines.insert(key, 0);
                continue;
            }
            requests.push((key, old_start, new_start, candidates));
        }

        if requests.is_empty() {
            return false;
        }

        let generation = self.document.generation;
        let (tx, rx) = oneshot::channel();
        drop(runtime::spawn_blocking(move || {
            let max_source_bytes_u64 = u64::try_from(discovery_byte_limit).unwrap_or(u64::MAX);
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
                                let source_bytes = match full_file_source_size(&source) {
                                    Ok(source_bytes) => source_bytes,
                                    Err(_) => return None,
                                };
                                if source_bytes > max_source_bytes_u64 {
                                    return None;
                                }
                                let text = match load_full_file_source(&source) {
                                    Ok(text) => text,
                                    Err(_) => return None,
                                };
                                if text.len() > discovery_byte_limit {
                                    return None;
                                }
                                Some((side, text.lines().count()))
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

    fn context_side_order(&self, file: usize) -> [DiffSide; 2] {
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

    pub(super) fn context_source_line_count(&self, file: usize) -> Option<(DiffSide, usize)> {
        for side in self.context_side_order(file) {
            let key = ContextSourceKey {
                file: FileIndex::new(file),
                side,
            };
            match self.document.context_cache.get(&key) {
                Some(ContextSourceEntry::Lines(lines)) => return Some((side, lines.len())),
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

    pub(crate) fn handle_context_at_row(&mut self, row_index: usize) -> bool {
        match self.document.model.row(row_index) {
            Some(UiRow::Collapsed { .. }) => self.expand_context_at_row(row_index),
            Some(UiRow::ContextHide { file, hunk, .. }) => {
                self.hide_context(file.get(), hunk.get())
            }
            _ => false,
        }
    }

    pub(crate) fn expand_context_around_focused_hunk(&mut self, direction: isize) -> bool {
        let Some((file, hunk)) = self.focused_hunk_for_viewport(self.viewport.viewport_rows) else {
            return false;
        };

        let target_hunk = if direction < 0 {
            hunk
        } else {
            let Some(next_hunk) = hunk.get().checked_add(1).map(crate::model::HunkIndex::new)
            else {
                return false;
            };
            let Some(hunk_count) = self
                .document
                .changeset
                .files
                .get(file.get())
                .map(|file_diff| file_diff.hunks().len())
            else {
                return false;
            };
            if next_hunk.get() == hunk_count {
                return self.expand_trailing_context_for_key(file.get(), next_hunk.get());
            }
            if next_hunk.get() > hunk_count {
                return false;
            }
            next_hunk
        };

        self.expand_context_for_key(file.get(), target_hunk.get())
    }

    pub(crate) fn expand_trailing_context_for_key(&mut self, file: usize, hunk: usize) -> bool {
        let Some(file_diff) = self.document.changeset.files.get(file) else {
            return false;
        };
        if hunk != file_diff.hunks().len() {
            return false;
        }
        let Some(last_hunk) = hunk
            .checked_sub(1)
            .and_then(|hunk| file_diff.hunks().get(hunk))
        else {
            return false;
        };
        let old_start = last_hunk.old_start().saturating_add(last_hunk.old_count());
        let new_start = last_hunk.new_start().saturating_add(last_hunk.new_count());
        let key = ContextKey {
            file: FileIndex::new(file),
            hunk: HunkIndex::new(hunk),
        };

        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            self.set_notice("context unavailable for this diff");
            return true;
        };

        let source_start = match side {
            DiffSide::Old => old_start,
            DiffSide::New => new_start,
        };
        let available = available_context_lines(source_start, usize::MAX, source_lines.len());
        self.document.trailing_context_lines.insert(key, available);
        self.document.trailing_context_sides.insert(key, side);
        let current = self
            .document
            .context_expansions
            .get(&key)
            .copied()
            .unwrap_or_default()
            .min(available);
        if current == available {
            self.set_notice("no more context");
            return true;
        }

        self.update_max_line_width_for_expanded_context(
            &source_lines,
            source_start,
            current..available,
        );
        self.document.context_expansions.insert(key, available);
        self.rebuild_model_after_context_visibility_change();
        true
    }

    pub(crate) fn expand_context_for_key(&mut self, file: usize, hunk: usize) -> bool {
        let Some(row_index) = (0..self.document.model.len()).find(|row_index| {
            matches!(
                self.document.model.row(*row_index),
                Some(UiRow::Collapsed {
                    file: row_file,
                    hunk: row_hunk,
                    ..
                }) if row_file.get() == file && row_hunk.get() == hunk
            )
        }) else {
            return false;
        };

        self.expand_context_at_row(row_index)
    }

    pub(crate) fn expand_context_at_row(&mut self, row_index: usize) -> bool {
        let Some(UiRow::Collapsed {
            file,
            hunk,
            old_start,
            new_start,
            lines,
            expanded,
        }) = self.document.model.row(row_index)
        else {
            return false;
        };

        let Some((side, source_lines)) = self.ensure_context_lines(file.get()) else {
            self.set_warning_notice("context unavailable for this diff");
            return true;
        };

        let total = lines.saturating_add(expanded);
        let expands_up = context_expands_up(hunk);
        let source_start = match (side, expands_up) {
            (DiffSide::Old, true) => old_start,
            (DiffSide::Old, false) => old_start.saturating_sub(expanded),
            (DiffSide::New, true) => new_start,
            (DiffSide::New, false) => new_start.saturating_sub(expanded),
        } as usize;
        let total = total as usize;
        let expanded = expanded as usize;
        let available = available_context_lines(source_start, total, source_lines.len());
        let current = expanded.min(available);
        let remaining = available.saturating_sub(current);
        if remaining == 0 {
            self.set_warning_notice("no more context");
            return true;
        }

        let next = available;
        let newly_visible = if expands_up {
            total.saturating_sub(next)..total.saturating_sub(current)
        } else {
            current..next
        };
        self.update_max_line_width_for_expanded_context(&source_lines, source_start, newly_visible);
        self.document
            .context_expansions
            .insert(ContextKey { file, hunk }, next);
        self.rebuild_model_after_context_visibility_change();
        true
    }

    pub(crate) fn hide_context(&mut self, file: usize, hunk: usize) -> bool {
        if self
            .document
            .context_expansions
            .remove(&ContextKey {
                file: FileIndex::new(file),
                hunk: HunkIndex::new(hunk),
            })
            .is_none()
        {
            return false;
        }

        self.rebuild_model_after_context_visibility_change();
        true
    }

    pub(crate) fn collapse_all_context(&mut self) -> bool {
        if self.document.context_expansions.is_empty() {
            return false;
        }

        self.document.context_expansions.clear();
        self.rebuild_model_after_context_visibility_change();
        true
    }

    pub(super) fn rebuild_model_after_context_visibility_change(&mut self) {
        let search_result = self.document.search_index.search_with_grep_match_limit(
            &self.document.changeset,
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_model(
            &search_result.visible_files,
            HunkFocusModelBehavior::PreserveIfValid,
        );
        self.filters.grep_matches =
            grep_match_rows(&self.document.model, &search_result.grep_matches);
        self.filters.grep_matches_truncated = search_result.grep_matches_truncated;
        self.filters.selected_grep_match = None;
        self.set_scroll_with_grep_sync(
            self.viewport.scroll,
            true,
            HunkFocusScrollBehavior::Preserve,
        );
        self.sync_grep_match_selection_to_scroll();
        self.set_horizontal_scroll(self.viewport.horizontal_scroll);
        self.runtime.dirty = true;
    }

    pub(crate) fn ensure_context_lines(
        &mut self,
        file: usize,
    ) -> Option<(DiffSide, Arc<Vec<String>>)> {
        for side in self.context_side_order(file) {
            if !self.has_context_source(file, side) {
                continue;
            }
            if let Some(lines) = self.context_lines(file, side) {
                return Some((side, lines));
            }
        }
        None
    }

    pub(crate) fn has_context_source(&self, file: usize, side: DiffSide) -> bool {
        self.document
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| {
                full_file_source(
                    &self.document.changeset.repo,
                    &self.document.options,
                    file_diff,
                    side,
                )
            })
            .is_some()
    }

    pub(crate) fn context_source_side(&self, file: usize) -> Option<DiffSide> {
        for side in self.context_side_order(file) {
            match self.document.context_cache.get(&ContextSourceKey {
                file: FileIndex::new(file),
                side,
            }) {
                Some(ContextSourceEntry::Lines(_)) => return Some(side),
                Some(ContextSourceEntry::Unavailable) => continue,
                None if self.has_context_source(file, side) => return Some(side),
                None => {}
            }
        }
        None
    }

    pub(crate) fn context_lines(
        &mut self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<Vec<String>>> {
        let key = ContextSourceKey {
            file: FileIndex::new(file),
            side,
        };
        if !self.document.context_cache.contains_key(&key) {
            let entry = self
                .load_context_lines(file, side)
                .map(ContextSourceEntry::Lines)
                .unwrap_or(ContextSourceEntry::Unavailable);
            self.document.context_cache.insert(key, entry);
            self.invalidate_wrapped_visual_layout();
        }

        match self.document.context_cache.get(&key) {
            Some(ContextSourceEntry::Lines(lines)) => Some(Arc::clone(lines)),
            Some(ContextSourceEntry::Unavailable) | None => None,
        }
    }

    pub(crate) fn load_context_lines(
        &self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<Vec<String>>> {
        let file_diff = self.document.changeset.files.get(file)?;
        let source = full_file_source(
            &self.document.changeset.repo,
            &self.document.options,
            file_diff,
            side,
        )?;
        let text = load_full_file_source(&source).ok()?;
        Some(Arc::new(split_context_source_lines(&text)))
    }

    pub(crate) fn context_line_text(
        &mut self,
        file: usize,
        old_line: usize,
        new_line: usize,
    ) -> String {
        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            return "context unavailable".to_owned();
        };
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        let Some(line_index) = line_number.checked_sub(1) else {
            return String::new();
        };
        source_lines.get(line_index).cloned().unwrap_or_default()
    }

    pub(crate) fn update_max_line_width_for_expanded_context(
        &mut self,
        source_lines: &[String],
        source_start: usize,
        offsets: std::ops::Range<usize>,
    ) {
        let Some(source_index_start) = source_start.checked_sub(1) else {
            return;
        };
        for offset in offsets {
            let Some(text) = source_lines.get(source_index_start + offset) else {
                continue;
            };
            self.document.max_line_width = self.document.max_line_width.max(display_width(text));
        }
    }
}
