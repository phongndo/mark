use super::DiffApp;
use crate::model::{ContextSourceKey, FileIndex, UiRow};
use crate::syntax::{
    DiffSide, SyntaxKey, SyntaxPosition, SyntaxPriority, SyntaxRuntime, unified_syntax_side,
};
use crate::theme::{MAX_SYNTAX_RESULTS_PER_FRAME, SyntaxBenchmarkReport};
use mark_syntax::HighlightedLine;
use std::collections::HashSet;

impl DiffApp {
    pub(crate) fn prepare_syntax_for_viewport(&mut self, visible_rows: usize) {
        if visible_rows == 0 || self.config.syntax.is_none() {
            return;
        }
        let mut requested = HashSet::new();
        let mut requested_files = HashSet::new();

        let Some(visible_range) = self.visible_model_range_for_viewport(visible_rows) else {
            return;
        };
        let visible_start = visible_range.start;
        let visible_end = visible_range.end;
        self.prepare_syntax_for_range(
            visible_start,
            visible_end,
            SyntaxPriority::Visible,
            &mut requested,
            &mut requested_files,
        );

        if self.syntax_prefetch_paused() {
            return;
        }

        let prefetch_rows =
            visible_rows.saturating_mul(self.config.syntax_limits.prefetch_viewports);
        let ahead_end = visible_end
            .saturating_add(prefetch_rows)
            .min(self.document.model.len());
        self.prepare_syntax_for_range(
            visible_end,
            ahead_end,
            SyntaxPriority::Prefetch,
            &mut requested,
            &mut requested_files,
        );

        let behind_start = visible_start.saturating_sub(prefetch_rows);
        self.prepare_syntax_for_range(
            behind_start,
            visible_start,
            SyntaxPriority::Prefetch,
            &mut requested,
            &mut requested_files,
        );
    }

    pub(crate) fn prepare_syntax_for_range(
        &mut self,
        start: usize,
        end: usize,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
        requested_files: &mut HashSet<ContextSourceKey>,
    ) {
        for row_index in start..end {
            let Some(row) = self.document.model.row(row_index) else {
                continue;
            };
            self.prepare_syntax_for_row(row, priority, requested, requested_files);
        }
    }

    pub(crate) fn prepare_syntax_for_row(
        &mut self,
        row: UiRow,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
        requested_files: &mut HashSet<ContextSourceKey>,
    ) {
        match row {
            UiRow::FileSeparator => {}
            UiRow::UnifiedLine { file, hunk, line } => {
                let Some(diff_line) = self
                    .document
                    .changeset
                    .files
                    .get(file.get())
                    .and_then(|file_diff| file_diff.hunks().get(hunk.get()))
                    .and_then(|hunk_diff| hunk_diff.lines.get(line.get()))
                else {
                    return;
                };
                if let Some(side) = unified_syntax_side(diff_line.kind()) {
                    self.queue_syntax_hunk(file.get(), hunk.get(), side, priority, requested);
                }
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                if left.is_some() {
                    self.queue_syntax_hunk(
                        file.get(),
                        hunk.get(),
                        DiffSide::Old,
                        priority,
                        requested,
                    );
                }
                if right.is_some() {
                    self.queue_syntax_hunk(
                        file.get(),
                        hunk.get(),
                        DiffSide::New,
                        priority,
                        requested,
                    );
                }
            }
            UiRow::ContextLine { file, .. } => {
                if let Some(side) = self.context_source_side(file.get()) {
                    self.queue_syntax_file(file.get(), side, priority, requested_files);
                }
            }
            UiRow::FileHeader(_)
            | UiRow::FileBodyNotice(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextHide { .. }
            | UiRow::HunkHeader { .. }
            | UiRow::MetaLine { .. } => {}
        }
    }

    pub(crate) fn queue_syntax_hunk(
        &mut self,
        file: usize,
        hunk: usize,
        side: DiffSide,
        priority: SyntaxPriority,
        requested: &mut HashSet<SyntaxPosition>,
    ) {
        let position = SyntaxPosition {
            generation: self.document.generation,
            file,
            hunk,
            side,
        };
        if !requested.insert(position) {
            return;
        }
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.queue_hunk(
                &self.document.options,
                &self.document.changeset,
                position,
                priority,
            );
        }
    }

    pub(crate) fn queue_syntax_file(
        &mut self,
        file: usize,
        side: DiffSide,
        priority: SyntaxPriority,
        requested: &mut HashSet<ContextSourceKey>,
    ) {
        if !requested.insert(ContextSourceKey {
            file: FileIndex::new(file),
            side,
        }) {
            return;
        }
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.queue_full_file(
                &self.document.options,
                &self.document.changeset,
                self.document.generation,
                file,
                side,
                priority,
            );
        }
    }

    pub(crate) fn drain_syntax(&mut self) {
        let Some(syntax) = self.config.syntax.as_mut() else {
            return;
        };
        let drain = syntax.drain(self.document.generation, MAX_SYNTAX_RESULTS_PER_FRAME);
        if drain.changed
            && drain
                .changed_keys
                .iter()
                .any(|key| self.syntax_key_affects_viewport(*key))
        {
            self.runtime.dirty = true;
        }
    }

    fn syntax_key_affects_viewport(&self, key: SyntaxKey) -> bool {
        if key.generation() != self.document.generation {
            return false;
        }
        let Some(visible_range) =
            self.visible_model_range_for_viewport(self.viewport.viewport_rows)
        else {
            return false;
        };

        visible_range
            .filter_map(|row_index| self.document.model.row(row_index))
            .any(|row| self.syntax_key_affects_row(key, row))
    }

    fn syntax_key_affects_row(&self, key: SyntaxKey, row: UiRow) -> bool {
        match row {
            UiRow::UnifiedLine { file, hunk, line } => {
                let Some(diff_line) = self
                    .document
                    .changeset
                    .files
                    .get(file.get())
                    .and_then(|file_diff| file_diff.hunks().get(hunk.get()))
                    .and_then(|hunk_diff| hunk_diff.lines.get(line.get()))
                else {
                    return false;
                };
                unified_syntax_side(diff_line.kind()).is_some_and(|side| {
                    key.source.generation == self.document.generation
                        && key.source.file == file.get()
                        && key.source.side == side
                })
            }
            UiRow::SplitLine {
                file,
                hunk: _,
                left,
                right,
            } => {
                key.source.generation == self.document.generation
                    && key.source.file == file.get()
                    && ((left.is_some() && key.source.side == DiffSide::Old)
                        || (right.is_some() && key.source.side == DiffSide::New))
            }
            UiRow::ContextLine { file, .. } => {
                key.source.generation == self.document.generation && key.source.file == file.get()
            }
            UiRow::FileSeparator
            | UiRow::FileHeader(_)
            | UiRow::FileBodyNotice(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextHide { .. }
            | UiRow::HunkHeader { .. }
            | UiRow::MetaLine { .. } => false,
        }
    }

    pub(crate) fn syntax_stats(&self) -> SyntaxBenchmarkReport {
        self.config
            .syntax
            .as_ref()
            .map(SyntaxRuntime::stats)
            .unwrap_or_default()
    }

    pub(crate) fn syntax_prefetch_paused(&self) -> bool {
        self.filters.input_open()
    }

    pub(crate) fn syntax_line(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
        side: DiffSide,
    ) -> Option<HighlightedLine> {
        self.config.syntax.as_mut().and_then(|syntax| {
            syntax.line(
                SyntaxPosition {
                    generation: self.document.generation,
                    file,
                    hunk,
                    side,
                },
                line,
            )
        })
    }

    pub(crate) fn syntax_file_line(
        &mut self,
        file: usize,
        side: DiffSide,
        line_number: usize,
    ) -> Option<HighlightedLine> {
        self.config.syntax.as_mut().and_then(|syntax| {
            syntax.full_file_line(self.document.generation, file, side, line_number)
        })
    }
}
