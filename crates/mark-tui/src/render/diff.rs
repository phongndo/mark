use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Span, Style, Text},
    widgets::Paragraph,
};

use crate::{
    app::DiffApp,
    model::{FileIndex, HunkIndex, UiRow},
    render::{
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
mod sticky;
mod unified;
mod viewport;
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
pub(crate) use viewport::build_diff_viewport_lines;

pub(crate) fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if app.document.model.is_empty() {
        app.clear_code_selection_render();
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
