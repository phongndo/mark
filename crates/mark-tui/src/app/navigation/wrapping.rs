use std::{collections::BTreeMap, ops::Range};

use super::super::{
    DiffApp, WrappedVisualLayout, split_cell_content_width, unified_content_width,
    wrapped_line_count,
};
use crate::{
    controls::DiffLayoutMode,
    model::{ContextSourceEntry, ContextSourceKey, FileIndex, UiRow},
    syntax::DiffSide,
};

const MAX_EAGER_WRAPPED_ROWS: usize = 200_000;
const SPARSE_WRAPPED_ROW_START_STRIDE: usize = 256;

impl DiffApp {
    pub(in crate::app) fn invalidate_wrapped_visual_layout(&self) {
        self.viewport.wrapped_visual_layout.borrow_mut().take();
    }

    pub(in crate::app) fn cached_context_line_text(
        &self,
        file: usize,
        old_line: usize,
        new_line: usize,
    ) -> Option<&str> {
        for side in self.context_side_order(file) {
            let key = ContextSourceKey {
                file: FileIndex::new(file),
                side,
            };
            match self.document.context_cache.get(&key) {
                Some(ContextSourceEntry::Lines(lines)) => {
                    let line_number = match side {
                        DiffSide::Old => old_line,
                        DiffSide::New => new_line,
                    };
                    let Some(line_index) = line_number.checked_sub(1) else {
                        return Some("");
                    };
                    return Some(lines.get(line_index).map(String::as_str).unwrap_or(""));
                }
                Some(ContextSourceEntry::Unavailable) => continue,
                None if self.has_context_source(file, side) => return None,
                None => {}
            }
        }
        None
    }

    pub(in crate::app) fn wrapped_visual_height_for_text(&self, text: &str) -> usize {
        match self.viewport.layout {
            DiffLayoutMode::Unified => {
                wrapped_line_count(text, unified_content_width(self.viewport.viewport_width))
            }
            DiffLayoutMode::Split => {
                let left_width = self.viewport.viewport_width / 2;
                let right_width = self.viewport.viewport_width.saturating_sub(left_width);
                wrapped_line_count(text, split_cell_content_width(left_width)).max(
                    wrapped_line_count(text, split_cell_content_width(right_width)),
                )
            }
        }
    }

    pub(in crate::app) fn wrapped_visual_height_for_row(&self, row: UiRow) -> usize {
        match row {
            UiRow::ContextLine {
                file,
                old_line,
                new_line,
            } => self
                .cached_context_line_text(file.get(), old_line, new_line)
                .map(|text| self.wrapped_visual_height_for_text(text))
                .unwrap_or(1),
            UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
                let text =
                    self.document.changeset.files[file].hunks()[hunk].lines[line].text_lossy();
                wrapped_line_count(&text, unified_content_width(self.viewport.viewport_width))
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let lines = &self.document.changeset.files[file].hunks()[hunk].lines;
                let left_width = self.viewport.viewport_width / 2;
                let right_width = self.viewport.viewport_width.saturating_sub(left_width);
                let left_content_width = split_cell_content_width(left_width);
                let right_content_width = split_cell_content_width(right_width);
                let left_rows = left
                    .and_then(|index| lines.get(index.get()))
                    .map(|line| wrapped_line_count(&line.text_lossy(), left_content_width))
                    .unwrap_or(1);
                let right_rows = right
                    .and_then(|index| lines.get(index.get()))
                    .map(|line| wrapped_line_count(&line.text_lossy(), right_content_width))
                    .unwrap_or(1);
                left_rows.max(right_rows).max(1)
            }
            UiRow::FileSeparator
            | UiRow::FileHeader(_)
            | UiRow::FileBodyNotice(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextHide { .. }
            | UiRow::HunkHeader { .. } => 1,
        }
    }

    fn measured_wrapped_visual_height_for_model_row(&self, row_index: usize) -> usize {
        self.document
            .model
            .row(row_index)
            .map(|row| self.wrapped_visual_height_for_row(row).max(1))
            .unwrap_or(1)
    }

    pub(in crate::app) fn ensure_wrapped_visual_layout(&self) {
        if self
            .viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .is_some_and(|layout| layout.matches(self))
        {
            return;
        }

        let model_rows = self.document.model.len();
        if model_rows > MAX_EAGER_WRAPPED_ROWS {
            // Measure sparse layouts one bounded block at a time. Unknown blocks
            // retain the old large-model fast path of one visual row per model
            // row until navigation or rendering reaches them.
            let row_start_stride = SPARSE_WRAPPED_ROW_START_STRIDE;
            let first_block_end = row_start_stride.min(model_rows);
            let first_block_starts = self.measured_wrapped_row_starts(0, first_block_end);
            let first_block_rows = first_block_end;
            let first_block_height = first_block_starts.last().copied().unwrap_or_default();
            let mut sparse_row_starts = BTreeMap::new();
            if first_block_rows > 0 {
                sparse_row_starts.insert(0, first_block_starts);
            }
            *self.viewport.wrapped_visual_layout.borrow_mut() = Some(WrappedVisualLayout {
                layout: self.viewport.layout,
                viewport_width: self.viewport.viewport_width,
                model_rows,
                model_rows_ptr: self.document.model.cache_key(),
                row_starts: Vec::new(),
                row_start_stride,
                sparse_row_starts,
                total_rows: model_rows
                    .saturating_add(first_block_height.saturating_sub(first_block_rows)),
            });
            return;
        }

        let mut row_starts = Vec::with_capacity(model_rows.saturating_add(1));
        row_starts.push(0);
        let mut total_rows = 0usize;
        for row_index in 0..model_rows {
            total_rows = total_rows
                .saturating_add(self.measured_wrapped_visual_height_for_model_row(row_index));
            row_starts.push(total_rows);
        }

        *self.viewport.wrapped_visual_layout.borrow_mut() = Some(WrappedVisualLayout {
            layout: self.viewport.layout,
            viewport_width: self.viewport.viewport_width,
            model_rows,
            model_rows_ptr: self.document.model.cache_key(),
            row_starts,
            row_start_stride: 1,
            sparse_row_starts: BTreeMap::new(),
            total_rows,
        });
    }

    fn measured_wrapped_row_starts(&self, start: usize, end: usize) -> Vec<usize> {
        let mut row_starts = Vec::with_capacity(end.saturating_sub(start).saturating_add(1));
        row_starts.push(0);
        let mut total_rows = 0usize;
        for row_index in start..end {
            total_rows = total_rows
                .saturating_add(self.measured_wrapped_visual_height_for_model_row(row_index));
            row_starts.push(total_rows);
        }
        row_starts
    }

    fn ensure_sparse_wrapped_block(&self, row_index: usize) {
        let block = {
            let layout = self.viewport.wrapped_visual_layout.borrow();
            let Some(layout) = layout.as_ref() else {
                return;
            };
            if layout.row_start_stride == 1 || layout.model_rows == 0 {
                return;
            }
            let row_index = row_index.min(layout.model_rows.saturating_sub(1));
            let block = row_index / layout.row_start_stride;
            if layout.sparse_row_starts.contains_key(&block) {
                return;
            }
            block
        };

        let (stride, model_rows) = {
            let layout = self.viewport.wrapped_visual_layout.borrow();
            let Some(layout) = layout.as_ref() else {
                return;
            };
            (layout.row_start_stride, layout.model_rows)
        };
        let start = block.saturating_mul(stride);
        let end = start.saturating_add(stride).min(model_rows);
        let row_starts = self.measured_wrapped_row_starts(start, end);
        let block_height = row_starts.last().copied().unwrap_or_default();
        let block_rows = end.saturating_sub(start);

        let mut layout = self.viewport.wrapped_visual_layout.borrow_mut();
        let Some(layout) = layout.as_mut() else {
            return;
        };
        if layout.row_start_stride != stride
            || layout.model_rows != model_rows
            || layout.sparse_row_starts.contains_key(&block)
        {
            return;
        }
        layout.total_rows = layout
            .total_rows
            .saturating_add(block_height.saturating_sub(block_rows));
        layout.sparse_row_starts.insert(block, row_starts);
    }

    fn sparse_visual_scroll_for_model_row(layout: &WrappedVisualLayout, row_index: usize) -> usize {
        if row_index >= layout.model_rows {
            return layout.total_rows;
        }
        let stride = layout.row_start_stride;
        let target_block = row_index / stride;
        let mut visual_start = row_index;
        for (&block, row_starts) in &layout.sparse_row_starts {
            if block > target_block {
                break;
            }
            let block_start = block.saturating_mul(stride);
            let block_rows = row_starts.len().saturating_sub(1);
            if block < target_block {
                let block_height = row_starts.last().copied().unwrap_or_default();
                visual_start = visual_start.saturating_add(block_height.saturating_sub(block_rows));
            } else {
                let offset = row_index.saturating_sub(block_start).min(block_rows);
                let local_start = row_starts.get(offset).copied().unwrap_or(offset);
                visual_start = visual_start.saturating_add(local_start.saturating_sub(offset));
            }
        }
        visual_start
    }

    pub(in crate::app) fn wrapped_visual_row_count(&self) -> usize {
        self.ensure_wrapped_visual_layout();
        self.viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .map(|layout| layout.total_rows)
            .unwrap_or_default()
    }

    pub(crate) fn wrapped_visual_scroll_for_model_row(&self, row_index: usize) -> usize {
        self.ensure_wrapped_visual_layout();
        let sparse = self
            .viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .is_some_and(|layout| layout.row_start_stride > 1);
        if sparse {
            self.ensure_sparse_wrapped_block(row_index);
        }

        self.viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .map(|layout| {
                let row_index = row_index.min(layout.model_rows);
                if layout.row_start_stride > 1 {
                    Self::sparse_visual_scroll_for_model_row(layout, row_index)
                } else {
                    layout
                        .row_starts
                        .get(row_index)
                        .copied()
                        .unwrap_or_default()
                }
            })
            .unwrap_or_default()
    }

    pub(crate) fn wrapped_visual_height_for_model_row(&self, row_index: usize) -> usize {
        self.ensure_wrapped_visual_layout();
        let sparse = self
            .viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .is_some_and(|layout| layout.row_start_stride > 1);
        if sparse {
            self.ensure_sparse_wrapped_block(row_index);
        }

        self.viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .and_then(|layout| {
                let row_index = row_index.min(layout.model_rows);
                if layout.row_start_stride == 1 {
                    return layout
                        .row_starts
                        .get(row_index)
                        .zip(layout.row_starts.get(row_index.saturating_add(1)))
                        .map(|(start, end)| end.saturating_sub(*start));
                }
                if row_index >= layout.model_rows {
                    return None;
                }
                let block = row_index / layout.row_start_stride;
                let block_start = block.saturating_mul(layout.row_start_stride);
                let offset = row_index.saturating_sub(block_start);
                layout
                    .sparse_row_starts
                    .get(&block)
                    .and_then(|starts| starts.get(offset).zip(starts.get(offset.saturating_add(1))))
                    .map(|(start, end)| end.saturating_sub(*start))
            })
            .unwrap_or(1)
    }

    pub(crate) fn model_row_at_scroll(&self, scroll: usize) -> Option<(usize, usize)> {
        if !self.viewport.line_wrapping {
            return self.document.model.row(scroll).map(|_| (scroll, 0));
        }

        self.ensure_wrapped_visual_layout();
        let sparse = self
            .viewport
            .wrapped_visual_layout
            .borrow()
            .as_ref()
            .is_some_and(|layout| layout.row_start_stride > 1);
        if !sparse {
            let layout = self.viewport.wrapped_visual_layout.borrow();
            let layout = layout.as_ref()?;
            if scroll >= layout.total_rows {
                return None;
            }
            let row_index = layout
                .row_starts
                .partition_point(|row_start| *row_start <= scroll)
                .saturating_sub(1);
            let row_start = layout
                .row_starts
                .get(row_index)
                .copied()
                .unwrap_or_default();
            return Some((row_index, scroll.saturating_sub(row_start)));
        }

        loop {
            let mut block_to_measure = None;
            let result = {
                let layout = self.viewport.wrapped_visual_layout.borrow();
                let layout = layout.as_ref()?;
                if scroll >= layout.total_rows {
                    return None;
                }

                let stride = layout.row_start_stride;
                let mut measured_extra = 0usize;
                let mut found = None;
                for (&block, row_starts) in &layout.sparse_row_starts {
                    let block_model_start = block.saturating_mul(stride);
                    let block_visual_start = block_model_start.saturating_add(measured_extra);
                    if scroll < block_visual_start {
                        let model_row = scroll.saturating_sub(measured_extra);
                        block_to_measure = Some(model_row);
                        break;
                    }

                    let block_height = row_starts.last().copied().unwrap_or_default();
                    let block_visual_end = block_visual_start.saturating_add(block_height);
                    if scroll < block_visual_end {
                        let local_scroll = scroll.saturating_sub(block_visual_start);
                        let offset = row_starts
                            .partition_point(|row_start| *row_start <= local_scroll)
                            .saturating_sub(1)
                            .min(row_starts.len().saturating_sub(2));
                        let row_start = row_starts.get(offset).copied().unwrap_or_default();
                        found = Some((
                            block_model_start.saturating_add(offset),
                            local_scroll.saturating_sub(row_start),
                        ));
                        break;
                    }

                    let block_rows = row_starts.len().saturating_sub(1);
                    measured_extra =
                        measured_extra.saturating_add(block_height.saturating_sub(block_rows));
                }

                if found.is_none() && block_to_measure.is_none() {
                    let model_row = scroll.saturating_sub(measured_extra);
                    if model_row >= layout.model_rows {
                        return None;
                    }
                    block_to_measure = Some(model_row);
                }
                found
            };

            if let Some(result) = result {
                return Some(result);
            }
            self.ensure_sparse_wrapped_block(block_to_measure?);
        }
    }

    pub(in crate::app) fn visible_model_range_for_viewport(
        &self,
        visible_rows: usize,
    ) -> Option<Range<usize>> {
        if visible_rows == 0 || self.document.model.is_empty() {
            return None;
        }

        if !self.viewport.line_wrapping {
            let visible_start = self.viewport.scroll.min(self.document.model.len());
            let visible_end = visible_start
                .saturating_add(visible_rows)
                .min(self.document.model.len());
            return (visible_start < visible_end).then_some(visible_start..visible_end);
        }

        let visible_start = self
            .model_row_at_scroll(self.viewport.scroll)
            .map(|(row, _)| row)?;
        let visible_end = self
            .model_row_at_scroll(self.viewport.scroll.saturating_add(visible_rows - 1))
            .map(|(row, _)| row.saturating_add(1))
            .unwrap_or_else(|| self.document.model.len());

        (visible_start < visible_end).then_some(visible_start..visible_end)
    }
}
