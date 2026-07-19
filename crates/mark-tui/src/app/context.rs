use super::{
    AsyncJob, DiffApp, HunkFocusModelBehavior, HunkFocusScrollBehavior, MAX_LIVE_GREP_MATCHES,
    TrailingContextWorker,
};
use crate::model::{
    ContextKey, ContextSourceEntry, ContextSourceKey, FileIndex, HunkIndex, UiRow,
    context_expands_up, line_after_hunk, normalized_hunk_start,
};
use crate::render::text::display_width;
use crate::runtime;
use crate::search::grep_match_rows;
use crate::syntax::{
    DiffSide, available_context_lines, full_file_source, full_file_source_size,
    load_full_file_source, split_context_source_lines,
};
use mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::oneshot;

fn file_has_full_file_source(
    repo: &std::path::Path,
    options: &mark_diff::DiffOptions,
    file: &mark_diff::DiffFile,
) -> bool {
    !file.is_binary()
        && !file.has_no_textual_changes()
        && [DiffSide::New, DiffSide::Old]
            .into_iter()
            .any(|side| full_file_source(repo, options, file, side).is_some())
}

pub(crate) fn full_file_mode_available(
    changeset: &mark_diff::Changeset,
    options: &mark_diff::DiffOptions,
) -> bool {
    changeset
        .files
        .iter()
        .any(|file| file_has_full_file_source(&changeset.repo, options, file))
}

pub(crate) fn full_file_context_expansions(
    changeset: &mark_diff::Changeset,
    options: &mark_diff::DiffOptions,
    trailing_context_lines: &HashMap<ContextKey, usize>,
) -> HashMap<ContextKey, usize> {
    let mut expansions = HashMap::new();

    for (file, file_diff) in changeset.files.iter().enumerate() {
        if !file_has_full_file_source(&changeset.repo, options, file_diff) {
            continue;
        }

        let file = FileIndex::new(file);
        let mut next_old_line = 1;
        let mut next_new_line = 1;
        for (hunk, diff_hunk) in file_diff.hunks().iter().enumerate() {
            let old_start = normalized_hunk_start(diff_hunk.old_start(), diff_hunk.old_count());
            let new_start = normalized_hunk_start(diff_hunk.new_start(), diff_hunk.new_count());
            let hidden_lines = old_start
                .saturating_sub(next_old_line)
                .min(new_start.saturating_sub(next_new_line));
            if hidden_lines > 0 {
                expansions.insert(
                    ContextKey {
                        file,
                        hunk: HunkIndex::new(hunk),
                    },
                    hidden_lines,
                );
            }
            next_old_line = line_after_hunk(diff_hunk.old_start(), diff_hunk.old_count());
            next_new_line = line_after_hunk(diff_hunk.new_start(), diff_hunk.new_count());
        }

        let trailing_key = ContextKey {
            file,
            hunk: HunkIndex::new(file_diff.hunks().len()),
        };
        if let Some(lines) = trailing_context_lines
            .get(&trailing_key)
            .copied()
            .filter(|lines| *lines > 0)
        {
            expansions.insert(trailing_key, lines);
        }
    }

    expansions
}

#[derive(Debug, Clone, Copy)]
struct FullFileViewportAnchor {
    row: UiRow,
    row_visual_offset: usize,
    viewport_row: usize,
}

fn line_distance_to_range(line: usize, start: usize, count: usize) -> usize {
    let start = normalized_hunk_start(start, count).max(1);
    let end = start.saturating_add(count.saturating_sub(1));
    if line < start {
        start - line
    } else {
        line.saturating_sub(end)
    }
}

impl DiffApp {
    pub(crate) fn full_file_mode_active(&self) -> bool {
        self.viewport.full_file
            && full_file_mode_available(&self.document.changeset, &self.document.options)
    }

    fn full_file_viewport_anchor(&self) -> Option<FullFileViewportAnchor> {
        let focused_hunk = self.focused_hunk_for_viewport(self.viewport.viewport_rows);
        self.full_file_viewport_anchor_for_hunk(focused_hunk)
    }

    fn full_file_viewport_anchor_for_hunk(
        &self,
        focused_hunk: Option<(FileIndex, HunkIndex)>,
    ) -> Option<FullFileViewportAnchor> {
        let focus_viewport_row = self.rendered_viewport_focus_row(self.viewport.viewport_rows);
        let mut focused_line_anchor: Option<(usize, FullFileViewportAnchor)> = None;
        let mut focused_structural_anchor: Option<(usize, FullFileViewportAnchor)> = None;
        let mut line_fallback = None;
        let mut stable_fallback = None;
        let mut structural_fallback = None;
        for rendered in self.rendered_diff_rows_for_viewport(self.viewport.viewport_rows.max(1)) {
            let row = self.document.model.row(rendered.model_row)?;
            let anchor = FullFileViewportAnchor {
                row,
                row_visual_offset: rendered
                    .visual_scroll
                    .saturating_sub(self.scroll_for_model_row(rendered.model_row)),
                viewport_row: rendered.viewport_row,
            };
            let is_stable_line = matches!(
                row,
                UiRow::UnifiedLine { .. } | UiRow::MetaLine { .. } | UiRow::SplitLine { .. }
            );
            // Prefer a stable line in the focused hunk over the first generic
            // line so adding or removing hunk chrome cannot move focus.
            if focused_hunk.is_some() && row.typed_hunk_key() == focused_hunk {
                let distance = rendered.viewport_row.abs_diff(focus_viewport_row);
                let focused_anchor = if is_stable_line {
                    &mut focused_line_anchor
                } else {
                    &mut focused_structural_anchor
                };
                if focused_anchor.is_none_or(|(known_distance, _)| distance < known_distance) {
                    *focused_anchor = Some((distance, anchor));
                }
            }
            if is_stable_line && line_fallback.is_none() {
                line_fallback = Some(anchor);
            }
            if matches!(row, UiRow::FileHeader(_) | UiRow::FileBodyNotice(_))
                && stable_fallback.is_none()
            {
                stable_fallback = Some(anchor);
            } else if !matches!(row, UiRow::FileSeparator) && structural_fallback.is_none() {
                structural_fallback = Some(anchor);
            }
        }
        focused_line_anchor
            .or(focused_structural_anchor)
            .map(|(_, anchor)| anchor)
            .or(line_fallback)
            .or(stable_fallback)
            .or(structural_fallback)
    }

    fn model_row_for_full_file_anchor(&self, anchor: FullFileViewportAnchor) -> Option<usize> {
        let row = match anchor.row {
            UiRow::FileSeparator => return None,
            UiRow::FileHeader(file) => self.document.model.file_start_row(file.get()),
            UiRow::FileBodyNotice(file) => self
                .document
                .model
                .file_start_row(file.get())
                .map(|row| row.saturating_add(1)),
            UiRow::HunkHeader { file, hunk } | UiRow::ContextHide { file, hunk, .. } => {
                self.model_row_for_nearest_hunk(file, hunk.get())
            }
            UiRow::Collapsed { file, hunk, .. } => self
                .matching_model_row(anchor.row)
                .or_else(|| self.model_row_for_nearest_hunk(file, hunk.get())),
            UiRow::ContextLine {
                file,
                old_line,
                new_line,
            } => self
                .document
                .model
                .context_line_row(file, new_line)
                .filter(|row| self.document.model.row(row.get()) == Some(anchor.row))
                .map(|row| row.get())
                .or_else(|| self.model_row_for_nearest_context_hunk(file, old_line, new_line)),
            UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => self
                .document
                .model
                .diff_line_row(file, hunk, line)
                .map(|row| row.get()),
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => right.or(left).and_then(|line| {
                self.document
                    .model
                    .diff_line_row(file, hunk, line)
                    .map(|row| row.get())
            }),
        }?;
        Some(row)
    }

    fn matching_model_row(&self, expected: UiRow) -> Option<usize> {
        (0..self.document.model.len()).find(|row| self.document.model.row(*row) == Some(expected))
    }

    fn model_row_for_nearest_hunk(&self, file: FileIndex, hunk: usize) -> Option<usize> {
        let hunk_count = self.document.changeset.files.get(file.get())?.hunks().len();
        let hunk = hunk.min(hunk_count.checked_sub(1)?);
        self.document.model.hunk_start_row(file.get(), hunk)
    }

    fn model_row_for_nearest_context_hunk(
        &self,
        file: FileIndex,
        old_line: usize,
        new_line: usize,
    ) -> Option<usize> {
        let file_diff = self.document.changeset.files.get(file.get())?;
        let hunk = file_diff
            .hunks()
            .iter()
            .enumerate()
            .min_by_key(|(_, hunk)| {
                line_distance_to_range(old_line, hunk.old_start(), hunk.old_count()).min(
                    line_distance_to_range(new_line, hunk.new_start(), hunk.new_count()),
                )
            })
            .map(|(hunk, _)| hunk)?;
        self.document.model.hunk_start_row(file.get(), hunk)
    }

    fn hunk_is_rendered(&self, file: FileIndex, hunk: HunkIndex) -> bool {
        self.rendered_diff_rows_for_viewport(self.viewport.viewport_rows)
            .into_iter()
            .any(|rendered| {
                self.document
                    .model
                    .row(rendered.model_row)
                    .is_some_and(|row| row.is_hunk_row(file.get(), hunk.get()))
            })
    }

    fn hunk_focus_for_full_file_transition(&self) -> Option<(FileIndex, HunkIndex)> {
        // Explicit hunk and grep navigation take precedence over focus inferred
        // from the viewport's sliding center.
        self.viewport
            .manual_hunk_focus
            .filter(|(file, hunk)| self.hunk_is_rendered(*file, *hunk))
            .or_else(|| {
                self.selected_grep_match_row()
                    .and_then(|row| self.document.model.row(row))
                    .and_then(UiRow::typed_hunk_key)
                    .filter(|(file, hunk)| self.hunk_is_rendered(*file, *hunk))
            })
            .or_else(|| self.focused_hunk_for_viewport(self.viewport.viewport_rows))
    }

    fn restore_hunk_focus_after_full_file_transition(
        &mut self,
        focused_hunk: Option<(FileIndex, HunkIndex)>,
    ) {
        let Some((file, hunk)) = focused_hunk.filter(|(file, hunk)| {
            self.document
                .model
                .hunk_start_row(file.get(), hunk.get())
                .is_some()
        }) else {
            return;
        };

        if !self.hunk_is_rendered(file, hunk) {
            self.set_scroll_focused_on_hunk(file.get(), hunk.get());
        }
        if !self.hunk_is_rendered(file, hunk) {
            return;
        }

        self.viewport.manual_hunk_focus = Some((file, hunk));
        self.sidebar.selected_file = file;
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
        self.runtime.dirty = true;
    }

    pub(super) fn retry_unresolved_trailing_context(&mut self) {
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

    pub(super) fn sync_full_file_context_expansions(&mut self) {
        self.document.context_expansions = full_file_context_expansions(
            &self.document.changeset,
            &self.document.options,
            &self.document.trailing_context_lines,
        );
    }

    pub(super) fn full_file_context_files_for_viewport(
        &self,
        visible_files: &[FileIndex],
        visible_rows: usize,
    ) -> Vec<FileIndex> {
        let visible_files = visible_files.iter().copied().collect::<HashSet<_>>();
        let mut files = Vec::new();
        for rendered in self.rendered_diff_rows_for_viewport(visible_rows.max(1)) {
            let Some(file) = self
                .document
                .model
                .file_at_row(rendered.model_row)
                .map(FileIndex::new)
            else {
                continue;
            };
            if visible_files.contains(&file) && !files.contains(&file) {
                files.push(file);
            }
        }

        if files.is_empty() {
            if visible_files.contains(&self.sidebar.selected_file) {
                files.push(self.sidebar.selected_file);
            } else if let Some(file) = visible_files.iter().copied().min() {
                files.push(file);
            }
        }
        files
    }

    pub(super) fn load_full_file_context_for_files(&mut self, files: &[FileIndex]) -> bool {
        let expanded_files = self
            .document
            .context_expansions
            .keys()
            .map(|key| key.file)
            .collect::<HashSet<_>>();
        let mut cache_changed = false;
        for file in files
            .iter()
            .copied()
            .filter(|file| expanded_files.contains(file))
        {
            let previous_entries = self.document.context_cache.len();
            let _ = self.ensure_context_lines(file.get());
            cache_changed |= self.document.context_cache.len() != previous_entries;
        }
        cache_changed
    }

    pub(super) fn prepare_full_file_context_layout(&mut self, visible_files: &[FileIndex]) {
        let context_needed_before_layout = self.viewport.line_wrapping
            || self.viewport.horizontal_scroll > self.max_horizontal_scroll();
        if context_needed_before_layout {
            let layout_files = self
                .full_file_context_files_for_viewport(visible_files, self.viewport.viewport_rows);
            self.load_full_file_context_for_files(&layout_files);
        }
        self.update_max_line_width_from_cached_context(visible_files);
    }

    pub(crate) fn prepare_full_file_context_for_viewport(&mut self, visible_rows: usize) -> bool {
        if !self.full_file_mode_active() {
            return false;
        }

        let visible_files = self.document.model.visible_files().to_vec();
        let files = self.full_file_context_files_for_viewport(&visible_files, visible_rows);
        if files.is_empty() {
            return false;
        }

        let anchor = self
            .viewport
            .line_wrapping
            .then(|| self.full_file_viewport_anchor())
            .flatten();
        let cache_changed = self.load_full_file_context_for_files(&files);
        if !cache_changed {
            return false;
        }
        self.update_max_line_width_from_cached_context(&visible_files);

        if let Some((anchor, model_row)) = anchor.and_then(|anchor| {
            self.model_row_for_full_file_anchor(anchor)
                .map(|row| (anchor, row))
        }) {
            let scroll = self.scroll_for_model_row_offset_at_viewport_row(
                model_row,
                anchor.row_visual_offset,
                anchor.viewport_row,
            );
            self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
            self.sync_grep_match_selection_to_scroll();
        }
        self.runtime.dirty = true;
        true
    }

    pub(super) fn update_max_line_width_from_cached_context(
        &mut self,
        visible_files: &[FileIndex],
    ) {
        let visible_file_set = visible_files.iter().copied().collect::<HashSet<_>>();
        let width = self
            .document
            .context_cache
            .iter()
            .filter(|(key, _)| visible_file_set.contains(&key.file))
            .filter_map(|(_, entry)| match entry {
                ContextSourceEntry::Lines(lines) => {
                    lines.iter().map(|line| display_width(line)).max()
                }
                ContextSourceEntry::Unavailable => None,
            })
            .max();
        if let Some(width) = width {
            self.document.max_line_width = self.document.max_line_width.max(width);
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
            let max_source_bytes_u64 =
                discovery_byte_limit.map(|limit| u64::try_from(limit).unwrap_or(u64::MAX));
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
                                if max_source_bytes_u64.is_some_and(|limit| source_bytes > limit) {
                                    return None;
                                }
                                let text = match load_full_file_source(&source) {
                                    Ok(text) => text,
                                    Err(_) => return None,
                                };
                                if discovery_byte_limit.is_some_and(|limit| text.len() > limit) {
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

    pub(super) fn refresh_trailing_context_for_file(&mut self, file: usize) -> bool {
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
        let old_start = line_after_hunk(last_hunk.old_start(), last_hunk.old_count());
        let new_start = line_after_hunk(last_hunk.new_start(), last_hunk.new_count());
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

    pub(crate) fn toggle_full_file(&mut self) {
        self.set_full_file(!self.viewport.full_file);
    }

    pub(crate) fn set_full_file(&mut self, enabled: bool) {
        self.overlays.options_menu_draft.full_file = enabled;
        if self.viewport.full_file == enabled {
            self.runtime.dirty = true;
            return;
        }

        let was_active = self.full_file_mode_active();
        let focused_hunk = self.hunk_focus_for_full_file_transition();
        let viewport_anchor = self.full_file_viewport_anchor_for_hunk(focused_hunk);
        let visible_files = self.document.model.visible_files().to_vec();
        self.viewport.full_file = enabled;
        let is_active = self.full_file_mode_active();
        if is_active {
            self.retry_unresolved_trailing_context();
            self.sync_full_file_context_expansions();
            self.update_max_line_width_from_cached_context(&visible_files);
        } else if was_active {
            self.document.context_expansions.clear();
            self.document.max_line_width = self
                .document
                .search_index
                .max_line_width_for_files(&visible_files);
        }
        self.rebuild_model_after_context_visibility_change();
        if let Some((anchor, model_row)) = viewport_anchor.and_then(|anchor| {
            self.model_row_for_full_file_anchor(anchor)
                .map(|row| (anchor, row))
        }) {
            let scroll = self.scroll_for_model_row_offset_at_viewport_row(
                model_row,
                anchor.row_visual_offset,
                anchor.viewport_row,
            );
            self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
        }
        if was_active != is_active {
            self.restore_hunk_focus_after_full_file_transition(focused_hunk);
        }
        self.sync_grep_match_selection_to_scroll();
    }

    pub(crate) fn collapse_all_context(&mut self) -> bool {
        if self.full_file_mode_active() {
            self.set_full_file(false);
            return true;
        }
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

    pub(super) fn restore_full_file_trailing_context_for_line(&mut self, file: usize, line: usize) {
        if !self.full_file_mode_active() {
            return;
        }

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
            if self.full_file_mode_active()
                && let ContextSourceEntry::Lines(lines) = &entry
                && let Some(width) = lines.iter().map(|line| display_width(line)).max()
            {
                self.document.max_line_width = self.document.max_line_width.max(width);
            }
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
