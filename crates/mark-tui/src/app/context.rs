use super::{DiffApp, HunkFocusModelBehavior, HunkFocusScrollBehavior, MAX_LIVE_GREP_MATCHES};
use crate::model::{
    ContextKey, FileIndex, HunkIndex, UiRow, context_expands_up, line_after_hunk,
    normalized_hunk_start,
};
use crate::search::grep_match_rows;
use crate::syntax::{DiffSide, available_context_lines, full_file_source};
use std::collections::HashMap;

mod loading;
mod trailing;

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

    pub(super) fn sync_full_file_context_expansions(&mut self) {
        self.document.context_expansions = full_file_context_expansions(
            &self.document.changeset,
            &self.document.options,
            &self.document.trailing_context_lines,
        );
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
            self.cancel_context_load_worker();
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
}
