use std::{
    collections::{HashMap, hash_map::Entry},
    ops::Range,
    sync::Arc,
};

use ratatui::prelude::{Line, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{AppEffect, DiffApp, wrapped_line_start_columns};
use crate::{
    controls::DiffLayoutMode,
    model::UiRow,
    render::{
        grep::{
            highlighted_line_in_ranges, split_content_start_column, unified_content_start_column,
        },
        text::{display_width, fit_with_width_from},
        viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CodeSelectionPane {
    Unified,
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeSelectionRegion {
    pane: CodeSelectionPane,
    columns: Range<usize>,
}

#[derive(Debug)]
struct CodeSelectionSource {
    text: Arc<str>,
    wrapped_line_starts: Vec<usize>,
    display_width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedCodeSelectionRow {
    text: String,
    regions: Vec<CodeSelectionRegion>,
    model_row: Option<usize>,
    visual_scroll: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeSelectionCopyPiece {
    viewport_row: usize,
    model_row: usize,
    visual_scroll: usize,
    pane: CodeSelectionPane,
    columns: Range<usize>,
    content_width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeSelectionPoint {
    column: usize,
    row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeSelection {
    anchor: CodeSelectionPoint,
    head: CodeSelectionPoint,
    pane: CodeSelectionPane,
    mouse_down: bool,
    dragged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeSelectionSnapshotKey {
    layout: DiffLayoutMode,
    scroll: usize,
    horizontal_scroll: usize,
    line_wrapping: bool,
    width: usize,
    visible_rows: usize,
    document_generation: u64,
}

#[derive(Debug, Default)]
pub(crate) struct CodeSelectionState {
    selection: Option<CodeSelection>,
    rows: Vec<RenderedCodeSelectionRow>,
    snapshot_key: Option<CodeSelectionSnapshotKey>,
}

impl CodeSelectionState {
    fn clear(&mut self) -> bool {
        self.selection.take().is_some()
    }

    fn clear_render_snapshot(&mut self) -> bool {
        let changed = self.clear();
        self.rows.clear();
        self.snapshot_key = None;
        changed
    }

    fn replace_render_snapshot(
        &mut self,
        key: CodeSelectionSnapshotKey,
        rows: Vec<RenderedCodeSelectionRow>,
    ) {
        if self
            .snapshot_key
            .as_ref()
            .is_some_and(|previous| previous != &key)
        {
            self.selection = None;
        }
        self.snapshot_key = Some(key);
        self.rows = rows;
    }

    fn begin(&mut self, column: usize, row: usize) -> bool {
        let Some(rendered_row) = self.rows.get(row) else {
            self.selection = None;
            return false;
        };
        let Some(region) = rendered_row
            .regions
            .iter()
            .find(|region| region.columns.contains(&column))
        else {
            self.selection = None;
            return false;
        };

        let point = CodeSelectionPoint { column, row };
        self.selection = Some(CodeSelection {
            anchor: point,
            head: point,
            pane: region.pane,
            mouse_down: true,
            dragged: false,
        });
        true
    }

    fn mouse_down(&self) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| selection.mouse_down)
    }

    fn update(&mut self, column: usize, row: usize) -> bool {
        let Some(selection) = self
            .selection
            .as_mut()
            .filter(|selection| selection.mouse_down)
        else {
            return false;
        };
        let Some(last_row) = self.rows.len().checked_sub(1) else {
            return false;
        };
        let point = CodeSelectionPoint {
            column,
            row: row.min(last_row),
        };
        selection.head = point;
        selection.dragged |= point != selection.anchor;
        true
    }

    fn finish(&mut self, column: usize, row: usize) -> Option<Vec<CodeSelectionCopyPiece>> {
        if !self.update(column, row) {
            return None;
        }
        let selection = self.selection.as_mut()?;
        selection.mouse_down = false;
        let dragged = selection.dragged;
        if !dragged {
            self.selection = None;
            return None;
        }
        self.selected_copy_pieces()
    }

    fn selected_copy_pieces(&self) -> Option<Vec<CodeSelectionCopyPiece>> {
        let selection = self
            .selection
            .as_ref()
            .filter(|selection| selection.dragged)?;
        let (start, end) = ordered_points(selection.anchor, selection.head);
        let mut pieces = Vec::new();

        for row_index in start.row..=end.row.min(self.rows.len().saturating_sub(1)) {
            let row = &self.rows[row_index];
            let Some(region) = row
                .regions
                .iter()
                .find(|region| region.pane == selection.pane)
            else {
                continue;
            };
            let start_column = if row_index == start.row {
                start.column.max(region.columns.start)
            } else {
                region.columns.start
            };
            let end_column = if row_index == end.row {
                end.column.saturating_add(1).min(region.columns.end)
            } else {
                region.columns.end
            };
            if start_column >= end_column {
                continue;
            }
            let (Some(model_row), Some(visual_scroll)) = (row.model_row, row.visual_scroll) else {
                continue;
            };

            pieces.push(CodeSelectionCopyPiece {
                viewport_row: row_index,
                model_row,
                visual_scroll,
                pane: selection.pane,
                columns: start_column.saturating_sub(region.columns.start)
                    ..end_column.saturating_sub(region.columns.start),
                content_width: region.columns.end.saturating_sub(region.columns.start),
            });
        }

        (!pieces.is_empty()).then_some(pieces)
    }

    fn highlighted_ranges(&self) -> Vec<Option<Range<usize>>> {
        let mut ranges = vec![None; self.rows.len()];
        let Some(selection) = self
            .selection
            .as_ref()
            .filter(|selection| selection.dragged)
        else {
            return ranges;
        };
        let (start, end) = ordered_points(selection.anchor, selection.head);

        let last_row = end.row.min(self.rows.len().saturating_sub(1));
        for (row_index, (range, row)) in ranges
            .iter_mut()
            .zip(&self.rows)
            .enumerate()
            .take(last_row.saturating_add(1))
            .skip(start.row)
        {
            let Some(region) = row
                .regions
                .iter()
                .find(|region| region.pane == selection.pane)
            else {
                continue;
            };
            let start_column = if row_index == start.row {
                start.column.max(region.columns.start)
            } else {
                region.columns.start
            };
            let end_column = if row_index == end.row {
                end.column.saturating_add(1).min(region.columns.end)
            } else {
                region.columns.end
            };
            if start_column < end_column {
                *range = Some(expand_column_range_to_graphemes(
                    &row.text,
                    start_column..end_column,
                ));
            }
        }
        ranges
    }
}

impl DiffApp {
    pub(crate) fn refresh_code_selection_render(
        &mut self,
        lines: &mut [Line<'static>],
        width: usize,
        visible_rows: usize,
    ) {
        let plans = plan_diff_viewport_rows(self, visible_rows);
        let rows = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let (regions, model_row, visual_scroll) = plans.get(index).map_or_else(
                    || (Vec::new(), None, None),
                    |slot| match slot.kind {
                        ViewportSlotKind::DiffVisual {
                            visual_scroll,
                            model_row,
                        } => {
                            let regions = self
                                .document
                                .model
                                .row(model_row)
                                .map(|row| selection_regions(row, self.viewport.layout, width))
                                .unwrap_or_default();
                            (regions, Some(model_row), Some(visual_scroll))
                        }
                        ViewportSlotKind::AnnotationCompose { .. }
                        | ViewportSlotKind::AnnotationSaved { .. } => (Vec::new(), None, None),
                    },
                );
                RenderedCodeSelectionRow {
                    text: rendered_line_text(line),
                    regions,
                    model_row,
                    visual_scroll,
                }
            })
            .collect();
        let key = CodeSelectionSnapshotKey {
            layout: self.viewport.layout,
            scroll: self.viewport.scroll,
            horizontal_scroll: self.viewport.horizontal_scroll,
            line_wrapping: self.viewport.line_wrapping,
            width,
            visible_rows,
            document_generation: self.document.generation,
        };
        self.input.code_selection.replace_render_snapshot(key, rows);

        let ranges = self.input.code_selection.highlighted_ranges();
        let highlight = Style::default().add_modifier(Modifier::REVERSED);
        for (line, range) in lines.iter_mut().zip(ranges) {
            if let Some(range) = range.filter(|range| range.start < range.end) {
                *line = highlighted_line_in_ranges(
                    std::mem::take(line),
                    vec![(range.start, range.end)],
                    highlight,
                );
            }
        }
    }

    pub(crate) fn clear_code_selection_render(&mut self) {
        if self.input.code_selection.clear_render_snapshot() {
            self.runtime.mark_dirty();
        }
    }

    pub(crate) fn begin_code_selection(&mut self, column: u16, row: u16) -> bool {
        let began = self
            .input
            .code_selection
            .begin(usize::from(column), usize::from(row));
        if began {
            self.runtime.mark_dirty();
        }
        began
    }

    pub(crate) fn code_selection_mouse_down(&self) -> bool {
        self.input.code_selection.mouse_down()
    }

    pub(crate) fn update_code_selection(&mut self, column: u16, row: u16) -> bool {
        let updated = self
            .input
            .code_selection
            .update(usize::from(column), usize::from(row));
        if updated {
            self.runtime.mark_dirty();
        }
        updated
    }

    pub(crate) fn finish_code_selection(&mut self, column: u16, row: u16) -> bool {
        if !self.input.code_selection.mouse_down() {
            return false;
        }
        let pieces = self
            .input
            .code_selection
            .finish(usize::from(column), usize::from(row));
        let text = pieces.and_then(|pieces| selected_source_text(self, pieces));
        self.runtime.mark_dirty();
        if let Some(text) = text {
            self.queue_effect(AppEffect::CopyToClipboard {
                text,
                success_message: "copied selected code".to_owned(),
                error_prefix: "could not copy selected code".to_owned(),
            });
        }
        true
    }

    pub(crate) fn clear_code_selection(&mut self) -> bool {
        let cleared = self.input.code_selection.clear();
        if cleared {
            self.runtime.mark_dirty();
        }
        cleared
    }
}

fn ordered_points(
    left: CodeSelectionPoint,
    right: CodeSelectionPoint,
) -> (CodeSelectionPoint, CodeSelectionPoint) {
    if (left.row, left.column) <= (right.row, right.column) {
        (left, right)
    } else {
        (right, left)
    }
}

fn selection_regions(row: UiRow, layout: DiffLayoutMode, width: usize) -> Vec<CodeSelectionRegion> {
    match (layout, row) {
        (DiffLayoutMode::Unified, UiRow::ContextLine { .. } | UiRow::UnifiedLine { .. }) => region(
            CodeSelectionPane::Unified,
            unified_content_start_column(width),
            width,
        )
        .into_iter()
        .collect(),
        (DiffLayoutMode::Split, UiRow::ContextLine { .. }) => split_regions(width, true, true),
        (DiffLayoutMode::Split, UiRow::SplitLine { left, right, .. }) => {
            split_regions(width, left.is_some(), right.is_some())
        }
        _ => Vec::new(),
    }
}

fn split_regions(width: usize, left: bool, right: bool) -> Vec<CodeSelectionRegion> {
    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut regions = Vec::with_capacity(2);
    if left
        && let Some(region) = region(
            CodeSelectionPane::Old,
            split_content_start_column(left_width),
            left_width,
        )
    {
        regions.push(region);
    }
    if right
        && let Some(region) = region(
            CodeSelectionPane::New,
            left_width.saturating_add(split_content_start_column(right_width)),
            width,
        )
    {
        regions.push(region);
    }
    regions
}

fn region(pane: CodeSelectionPane, start: usize, end: usize) -> Option<CodeSelectionRegion> {
    (start < end).then_some(CodeSelectionRegion {
        pane,
        columns: start..end,
    })
}

fn selected_source_text(app: &DiffApp, pieces: Vec<CodeSelectionCopyPiece>) -> Option<String> {
    let mut source_cache = HashMap::new();
    let mut output = String::new();
    let mut previous: Option<(usize, usize, Option<usize>)> = None;

    for piece in pieces {
        let source = code_selection_source(app, &piece, &mut source_cache);
        let requested_source_column_start = if app.viewport.line_wrapping {
            let wrap_index = piece
                .visual_scroll
                .saturating_sub(app.wrapped_visual_scroll_for_model_row(piece.model_row));
            source
                .wrapped_line_starts
                .get(wrap_index)
                .copied()
                .unwrap_or(source.display_width)
        } else {
            app.viewport.horizontal_scroll
        };
        let (_, source_display_column_start, source_display_width, _) = fit_with_width_from(
            &source.text,
            requested_source_column_start,
            piece.content_width,
        );
        let source_columns = source_display_column_start
            .saturating_add(piece.columns.start.min(source_display_width))
            ..source_display_column_start
                .saturating_add(piece.columns.end.min(source_display_width));
        let source_range = source_byte_range_in_display_columns(&source.text, source_columns);
        let mut consumed_source_end =
            (source_range.start < source_range.end).then_some(source_range.end);

        if let Some((previous_row, previous_model_row, previous_source_end)) = previous {
            let wrapped_continuation = piece.viewport_row == previous_row.saturating_add(1)
                && piece.model_row == previous_model_row;
            if wrapped_continuation {
                let append_start = previous_source_end
                    .map_or(source_range.start, |end| source_range.start.max(end));
                if append_start < source_range.end {
                    output.push_str(&source.text[append_start..source_range.end]);
                }
                consumed_source_end = match (previous_source_end, consumed_source_end) {
                    (Some(previous), Some(current)) => Some(previous.max(current)),
                    (previous, current) => previous.or(current),
                };
            } else {
                output.push('\n');
                output.push_str(&source.text[source_range]);
            }
        } else {
            output.push_str(&source.text[source_range]);
        }

        previous = Some((piece.viewport_row, piece.model_row, consumed_source_end));
    }

    (!output.is_empty()).then_some(output)
}

fn code_selection_source(
    app: &DiffApp,
    piece: &CodeSelectionCopyPiece,
    source_cache: &mut HashMap<(usize, CodeSelectionPane), Arc<CodeSelectionSource>>,
) -> Arc<CodeSelectionSource> {
    let key = (piece.model_row, piece.pane);
    match source_cache.entry(key) {
        Entry::Occupied(entry) => Arc::clone(entry.get()),
        Entry::Vacant(entry) => {
            let text =
                Arc::<str>::from(code_selection_source_text(app, piece.model_row, piece.pane));
            let (wrapped_line_starts, source_display_width) = if app.viewport.line_wrapping {
                (
                    wrapped_line_start_columns(&text, piece.content_width),
                    display_width(&text),
                )
            } else {
                (Vec::new(), 0)
            };
            let source = Arc::new(CodeSelectionSource {
                text,
                wrapped_line_starts,
                display_width: source_display_width,
            });
            entry.insert(Arc::clone(&source));
            source
        }
    }
}

fn code_selection_source_text(app: &DiffApp, model_row: usize, pane: CodeSelectionPane) -> String {
    let row = app
        .document
        .model
        .row(model_row)
        .expect("selectable model row should still exist");
    match (app.viewport.layout, row, pane) {
        (
            DiffLayoutMode::Unified,
            UiRow::ContextLine {
                file,
                old_line,
                new_line,
            },
            CodeSelectionPane::Unified,
        )
        | (
            DiffLayoutMode::Split,
            UiRow::ContextLine {
                file,
                old_line,
                new_line,
            },
            CodeSelectionPane::Old | CodeSelectionPane::New,
        ) => app.rendered_context_line_text(file.get(), old_line, new_line),
        (
            DiffLayoutMode::Unified,
            UiRow::UnifiedLine { file, hunk, line },
            CodeSelectionPane::Unified,
        ) => app.document.changeset.files[file].hunks()[hunk].lines[line]
            .text_lossy()
            .into_owned(),
        (
            DiffLayoutMode::Split,
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            },
            pane @ (CodeSelectionPane::Old | CodeSelectionPane::New),
        ) => {
            let line = match pane {
                CodeSelectionPane::Old => left.get(),
                CodeSelectionPane::New => right.get(),
                CodeSelectionPane::Unified => None,
            }
            .expect("selectable split pane should still have a source line");
            app.document.changeset.files[file].hunks()[hunk].lines[line]
                .text_lossy()
                .into_owned()
        }
        _ => unreachable!("selection snapshot should only contain selectable source rows"),
    }
}

fn rendered_line_text(line: &Line<'_>) -> String {
    let capacity = line.spans.iter().map(|span| span.content.len()).sum();
    let mut text = String::with_capacity(capacity);
    for span in &line.spans {
        text.push_str(span.content.as_ref());
    }
    text
}

fn source_byte_range_in_display_columns(text: &str, range: Range<usize>) -> Range<usize> {
    let mut column = 0usize;
    let mut byte_start = None;
    let mut byte_end = 0usize;

    for (byte_index, grapheme) in text.grapheme_indices(true) {
        let next_column = column.saturating_add(display_width(grapheme));
        let next_byte = byte_index.saturating_add(grapheme.len());
        if next_column > range.start && column < range.end {
            byte_start.get_or_insert(byte_index);
            byte_end = next_byte;
        }
        column = next_column;
        if column >= range.end {
            break;
        }
    }

    let byte_start = byte_start.unwrap_or(text.len());
    byte_start..byte_end.max(byte_start)
}

fn expand_column_range_to_graphemes(text: &str, range: Range<usize>) -> Range<usize> {
    let mut column = 0usize;
    let mut start = range.start;
    let mut end = range.end;
    for grapheme in text.graphemes(true) {
        let next = column.saturating_add(grapheme.width());
        if column < range.start && range.start < next {
            start = column;
        }
        if column < range.end && range.end < next {
            end = next;
            break;
        }
        column = next;
        if column >= range.end {
            break;
        }
    }
    start..end
}
