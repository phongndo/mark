use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Span, Style, Text},
    widgets::Paragraph,
};

use crate::{
    annotation::AnnotationKey,
    app::DiffApp,
    model::{FileIndex, HunkIndex, UiRow},
    render::{
        annotation_hints::{AnnotationTargetHint, apply_annotation_target_hint},
        annotation_ranges::{
            annotation_block_geometry, apply_annotation_connector, highlighted_annotation_row,
        },
        annotations::{render_annotation_compose_block, render_annotation_saved_block},
        grep::{grep_highlight_targets_for_row, highlighted_grep_text_line},
        headers::{file_header_line, hunk_header_line, hunk_header_line_with_focus},
        style::diff_base_bg,
        text::fit_padded,
    },
    syntax::unified_syntax_side,
};

mod content;
mod context;
mod empty;
mod split;
mod unified;
pub(crate) use content::{
    ContentSpanRender, append_content_spans_at_scroll, append_gutter_spans, content_span_capacity,
    diff_indicator_span_for_focus, empty_diff_fill_from,
};
#[cfg(test)]
pub(crate) use content::{content_spans_at_scroll, inline_bg, syntax_fg};
use content::{split_gutter_text, unified_gutter_text};
#[cfg(test)]
pub(crate) use context::render_split_context_line_wrapped;
#[cfg(test)]
pub(crate) use context::{context_expand_marker, context_hide_marker};
pub(crate) use context::{
    context_expand_marker_for_theme, context_hide_line, context_hide_marker_for_theme,
    context_show_line, render_context_line, render_context_line_wrapped,
};
use empty::draw_empty_diff;
#[cfg(test)]
pub(crate) use empty::empty_diff_message;
#[cfg(test)]
pub(crate) use split::{SplitCellRender, SplitSide, split_cell_spans_at_scroll};
pub(crate) use split::{
    SplitLineRender, render_split_line_with_focus, render_split_line_wrapped_with_focus,
};
pub(crate) use unified::{
    line_style, render_unified_line_at_scroll_with_focus, render_unified_line_wrapped_with_focus,
};
#[cfg(test)]
pub(crate) use unified::{render_unified_line_at_scroll, row_bg};

pub(crate) fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if app.document.model.is_empty() {
        draw_empty_diff(frame, app, area);
        return;
    }

    let visible_rows = area.height as usize;
    app.prepare_full_file_context_for_viewport(visible_rows);
    app.prepare_syntax_for_viewport(visible_rows);
    let width = area.width as usize;
    let lines = build_diff_viewport_lines(app, width, visible_rows);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().bg(diff_base_bg(app.config.theme))),
        area,
    );
}

pub(crate) fn build_diff_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    app.prepare_full_file_context_for_viewport(visible_rows);
    if app.viewport.line_wrapping {
        return build_wrapped_viewport_lines(app, width, visible_rows);
    }

    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let has_annotation_blocks = draft.is_some() || !app.annotations_state.annotations.is_empty();
    let focused_hunk = app.focused_hunk_for_viewport(visible_rows);
    let mut lines = Vec::with_capacity(visible_rows);

    for offset in 0..visible_rows {
        if lines.len() >= visible_rows {
            break;
        }
        let visual_row = app.viewport.scroll.saturating_add(offset);
        let Some(row) = app.document.model.row(visual_row) else {
            break;
        };
        let mut line = render_row_with_focus(app, visual_row, row, width, focused_hunk);
        let active_side = app.annotation_active_line_side(visual_row, row);
        if app.annotation_cursor_at_visual_scroll(visual_row) || active_side.is_some() {
            line = highlighted_annotation_row(line, row, layout, width, active_side, theme);
        }
        for (hint, scope, side, existing) in
            app.annotation_target_hints_at_visual_scroll(visual_row)
        {
            line = apply_annotation_target_hint(
                line,
                layout,
                width,
                AnnotationTargetHint {
                    scope,
                    side,
                    hint,
                    existing_annotation: existing,
                    uppercase: app.config.syntax_settings.annotations.uppercase_hints,
                },
                theme,
            );
        }
        for (side, starts_range) in app
            .annotation_connectors_at_model_row(visual_row, row)
            .into_iter()
            .flatten()
        {
            line = apply_annotation_connector(line, layout, width, side, starts_range, theme);
        }
        lines.push(line);

        if has_annotation_blocks {
            for key in app.annotation_keys_at_model_row(visual_row, row) {
                let geometry = annotation_block_geometry(layout, width, &key);
                if let Some(draft) = draft
                    .as_ref()
                    .filter(|d| d.model_row_index == visual_row && d.key == key)
                {
                    let label = app.annotation_block_label(&draft.key, true);
                    push_annotation_block(
                        &mut lines,
                        render_annotation_compose_block(
                            draft,
                            width,
                            geometry,
                            theme,
                            Some(&label),
                        ),
                        visible_rows,
                    );
                } else if let Some(text) = app.annotations_state.annotations.get(&key)
                    && draft.as_ref().is_none_or(|d| d.key != key)
                {
                    let label = app.annotation_block_label(&key, false);
                    let block_scroll = annotation_saved_block_scroll(app, &key);
                    push_annotation_block(
                        &mut lines,
                        render_annotation_saved_block(text, width, geometry, theme, Some(&label))
                            .into_iter()
                            .skip(block_scroll),
                        visible_rows,
                    );
                }
            }
        }
    }

    lines.truncate(visible_rows);
    lines
}

fn build_wrapped_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let has_annotation_blocks = draft.is_some() || !app.annotations_state.annotations.is_empty();
    let focused_hunk = app.focused_hunk_for_viewport(visible_rows);
    let mut lines = Vec::with_capacity(visible_rows);
    let Some((mut row_index, mut row_offset)) = app.model_row_at_scroll(app.viewport.scroll) else {
        return lines;
    };
    let mut visual_row = app.viewport.scroll;
    while lines.len() < visible_rows {
        let Some(row) = app.document.model.row(row_index) else {
            break;
        };
        let remaining = visible_rows.saturating_sub(lines.len());
        let rendered = render_row_wrapped_with_focus(app, row_index, row, width, focused_hunk);
        let wrap_count = rendered.len().saturating_sub(row_offset);
        for (wrap_index, line) in rendered
            .into_iter()
            .skip(row_offset)
            .take(remaining)
            .enumerate()
        {
            let mut line = line;
            let is_last_wrap = wrap_index + 1 == wrap_count.min(remaining);
            let active_side = app.annotation_active_line_side(row_index, row);
            if app.annotation_cursor_at_model_row(row_index) || active_side.is_some() {
                line = highlighted_annotation_row(line, row, layout, width, active_side, theme);
            }
            for (hint, scope, side, existing) in
                app.annotation_target_hints_at_visual_scroll(visual_row)
            {
                line = apply_annotation_target_hint(
                    line,
                    layout,
                    width,
                    AnnotationTargetHint {
                        scope,
                        side,
                        hint,
                        existing_annotation: existing,
                        uppercase: app.config.syntax_settings.annotations.uppercase_hints,
                    },
                    theme,
                );
            }
            for (side, starts_range) in app
                .annotation_connectors_at_model_row(row_index, row)
                .into_iter()
                .flatten()
            {
                line = apply_annotation_connector(
                    line,
                    layout,
                    width,
                    side,
                    starts_range && wrap_index == 0,
                    theme,
                );
            }
            lines.push(line);
            visual_row = visual_row.saturating_add(1);
            if lines.len() >= visible_rows {
                break;
            }
            if is_last_wrap && has_annotation_blocks {
                for key in app.annotation_keys_at_model_row(row_index, row) {
                    let geometry = annotation_block_geometry(layout, width, &key);
                    if let Some(draft) = draft
                        .as_ref()
                        .filter(|d| d.model_row_index == row_index && d.key == key)
                    {
                        let label = app.annotation_block_label(&draft.key, true);
                        push_annotation_block(
                            &mut lines,
                            render_annotation_compose_block(
                                draft,
                                width,
                                geometry,
                                theme,
                                Some(&label),
                            ),
                            visible_rows,
                        );
                    } else if let Some(text) = app.annotations_state.annotations.get(&key)
                        && draft.as_ref().is_none_or(|d| d.key != key)
                    {
                        let label = app.annotation_block_label(&key, false);
                        let block_scroll = annotation_saved_block_scroll(app, &key);
                        push_annotation_block(
                            &mut lines,
                            render_annotation_saved_block(
                                text,
                                width,
                                geometry,
                                theme,
                                Some(&label),
                            )
                            .into_iter()
                            .skip(block_scroll),
                            visible_rows,
                        );
                    }
                }
            }
        }
        row_offset = 0;
        row_index = row_index.saturating_add(1);
    }
    lines.truncate(visible_rows);
    lines
}

fn annotation_saved_block_scroll(app: &DiffApp, key: &AnnotationKey) -> usize {
    app.annotations_state
        .annotation_block_scroll
        .as_ref()
        .filter(|(scroll_key, _)| scroll_key == key)
        .map(|(_, offset)| *offset)
        .unwrap_or_default()
}

fn push_annotation_block(
    lines: &mut Vec<Line<'static>>,
    block: impl IntoIterator<Item = Line<'static>>,
    visible_rows: usize,
) {
    for line in block {
        if lines.len() >= visible_rows {
            break;
        }
        lines.push(line);
    }
}

pub(crate) fn render_row(
    app: &mut DiffApp,
    row_index: usize,
    row: UiRow,
    width: usize,
) -> Line<'static> {
    render_row_with_focus(app, row_index, row, width, None)
}

pub(crate) fn render_row_wrapped_with_focus(
    app: &mut DiffApp,
    row_index: usize,
    row: UiRow,
    width: usize,
    focused_hunk: Option<(FileIndex, HunkIndex)>,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let hunk_focused = row
        .typed_hunk_key()
        .is_some_and(|hunk_key| Some(hunk_key) == focused_hunk);

    match row {
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => render_context_line_wrapped(app, file.get(), old_line, new_line, row_index, width),
        UiRow::UnifiedLine { file, hunk, line } => {
            let kind = app.document.changeset.files[file].hunks()[hunk].lines[line].kind();
            let syntax = unified_syntax_side(kind)
                .and_then(|side| app.syntax_line(file.get(), hunk.get(), line.get(), side));
            let inline = app.inline_ranges(file.get(), hunk.get(), line.get());
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_wrapped_with_focus(
                diff_line,
                syntax.as_deref(),
                &inline,
                width,
                theme,
                hunk_focused,
                &app.filters.grep_filter,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_wrapped_with_focus(
                diff_line,
                None,
                &[],
                width,
                theme,
                hunk_focused,
                &app.filters.grep_filter,
            )
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line_wrapped_with_focus(
            app,
            SplitLineRender {
                file: file.get(),
                hunk: hunk.get(),
                left: left.get().map(|line| line.get()),
                right: right.get().map(|line| line.get()),
                row_index,
                width,
                focused: hunk_focused,
            },
        ),
        _ => vec![render_row_with_focus(
            app,
            row_index,
            row,
            width,
            focused_hunk,
        )],
    }
}

pub(crate) fn render_row_with_focus(
    app: &mut DiffApp,
    row_index: usize,
    row: UiRow,
    width: usize,
    focused_hunk: Option<(FileIndex, HunkIndex)>,
) -> Line<'static> {
    let theme = app.config.theme;
    let horizontal_scroll = app.viewport.horizontal_scroll;
    let hunk_focused = row
        .typed_hunk_key()
        .is_some_and(|hunk_key| Some(hunk_key) == focused_hunk);
    let mut line = match row {
        UiRow::FileHeader(file_index) => {
            let file = &app.document.changeset.files[file_index];
            file_header_line(file, width, theme)
        }
        UiRow::FileBodyNotice(file_index) => {
            let file = &app.document.changeset.files[file_index];
            let message = if file.is_binary() {
                "binary file"
            } else {
                "no textual changes"
            };
            Line::from(Span::styled(
                fit_padded(&format!("  {message}"), width),
                Style::default().fg(theme.muted),
            ))
        }
        UiRow::Collapsed {
            hunk,
            lines,
            expanded,
            ..
        } => context_show_line(
            lines as usize,
            expanded > 0,
            context_expand_marker_for_theme(hunk.get(), theme),
            width,
            theme,
        ),
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => render_context_line(app, file.get(), old_line, new_line, row_index, width),
        UiRow::ContextHide { hunk, lines, .. } => context_hide_line(
            lines,
            context_hide_marker_for_theme(hunk.get(), theme),
            width,
            theme,
        ),
        UiRow::HunkHeader { file, hunk } => {
            let hunk = &app.document.changeset.files[file].hunks()[hunk];
            if hunk_focused {
                hunk_header_line_with_focus(hunk, width, theme, true)
            } else {
                hunk_header_line(hunk, width, theme)
            }
        }
        UiRow::UnifiedLine { file, hunk, line } => {
            let kind = app.document.changeset.files[file].hunks()[hunk].lines[line].kind();
            let syntax = unified_syntax_side(kind)
                .and_then(|side| app.syntax_line(file.get(), hunk.get(), line.get(), side));
            let inline = app.inline_ranges(file.get(), hunk.get(), line.get());
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_at_scroll_with_focus(
                diff_line,
                syntax.as_deref(),
                &inline,
                width,
                theme,
                horizontal_scroll,
                hunk_focused,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_at_scroll_with_focus(
                diff_line,
                None,
                &[],
                width,
                theme,
                horizontal_scroll,
                hunk_focused,
            )
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line_with_focus(
            app,
            SplitLineRender {
                file: file.get(),
                hunk: hunk.get(),
                left: left.get().map(|line| line.get()),
                right: right.get().map(|line| line.get()),
                row_index,
                width,
                focused: hunk_focused,
            },
        ),
    };

    if !app.filters.grep_filter.is_empty() {
        let targets = grep_highlight_targets_for_row(app, row, &line, width);
        line = highlighted_grep_text_line(line, &app.filters.grep_filter, targets, theme);
    }
    line
}
