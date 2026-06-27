use mark_diff::{DiffLine, DiffLineKind};
use mark_syntax::{DiffBackground, HighlightedLine, SyntaxClass};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    annotation::AnnotationKey,
    app::{DiffApp, split_cell_content_width, unified_content_width, wrapped_line_start_columns},
    controls::DiffLayoutMode,
    model::UiRow,
    render::{
        annotations::{
            append_annotation_add_button, render_annotation_compose_block,
            render_annotation_saved_block,
        },
        grep::{
            diff_line_grep_highlight_text, grep_highlight_target_for_columns,
            grep_highlight_targets_for_row, highlighted_grep_text_line,
            highlighted_mouse_diff_content_line, scrolled_text_byte_start,
            split_diff_line_grep_highlight_target, unified_content_start_column,
        },
        headers::{
            file_header_line, file_separator_line, hunk_header_line, hunk_header_line_with_focus,
        },
        style::{base_bg, diff_indicator_span, diff_sign_style, focused_diff_indicator_span},
        text::{
            fit, fit_padded, fit_padded_from, fit_with_width, format_count, skip_display_prefix,
            spaces,
        },
    },
    syntax::{DiffSide, InlineRange, unified_syntax_side},
    theme::{
        DiffTheme, EMPTY_DIFF_FILL, EMPTY_DIFF_FILL_SPACING, GUTTER_WIDTH, UNIFIED_GUTTER_WIDTH,
        line_gutter_bg, line_gutter_fg,
    },
};

mod content;
mod split;
pub(crate) use content::*;
pub(crate) use split::*;

pub(crate) fn draw_diff(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    if app.document.model.is_empty() {
        let message = if app.filters_active() && !app.document.base_changeset.files.is_empty() {
            "No files match filters."
        } else {
            "No changes."
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message,
                Style::default().fg(app.config.theme.muted),
            )))
            .style(Style::default().bg(base_bg(app.config.theme))),
            area,
        );
        return;
    }

    let visible_rows = area.height as usize;
    app.prepare_syntax_for_viewport(visible_rows);
    let width = area.width as usize;
    let lines = build_diff_viewport_lines(app, width, visible_rows);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(base_bg(app.config.theme))),
        area,
    );
}

pub(crate) fn build_diff_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    if app.viewport.line_wrapping {
        return build_wrapped_viewport_lines(app, width, visible_rows);
    }

    let mouse_highlight = mouse_highlight_for_viewport(app);
    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let annotations = app.annotations_state.annotations.clone();
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
        if mouse_highlight.is_some_and(|(_, highlight_row)| highlight_row == visual_row)
            && row_has_diff_code_content(row)
            && draft.is_none()
        {
            line = highlighted_mouse_diff_content_line(line, layout, width, theme);
            if row_has_annotation_target(app, row) {
                line = append_annotation_add_button(line, width, theme);
            }
        }
        lines.push(line);

        for key in AnnotationKey::candidates_from_ui_row(&app.document.changeset, row) {
            if let Some(draft) = draft
                .as_ref()
                .filter(|d| d.model_row_index == visual_row && d.key == key)
            {
                let label = app.annotation_label(&draft.key);
                push_annotation_block(
                    &mut lines,
                    render_annotation_compose_block(draft, width, theme, label.as_deref()),
                    visible_rows,
                );
            } else if let Some(text) = annotations.get(&key)
                && draft.as_ref().is_none_or(|d| d.key != key)
            {
                let label = app.annotation_label(&key);
                push_annotation_block(
                    &mut lines,
                    render_annotation_saved_block(text, width, theme, label.as_deref()),
                    visible_rows,
                );
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
    let mouse_highlight = mouse_highlight_for_viewport(app);
    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let annotations = app.annotations_state.annotations.clone();
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
            let anchor_visual = app.annotation_anchor_visual_scroll(row_index);
            if mouse_highlight.is_some_and(|(_, highlight_row)| highlight_row == visual_row)
                && row_has_diff_code_content(row)
                && draft.is_none()
            {
                line = highlighted_mouse_diff_content_line(line, layout, width, theme);
                if visual_row == anchor_visual && row_has_annotation_target(app, row) {
                    line = append_annotation_add_button(line, width, theme);
                }
            }
            lines.push(line);
            visual_row = visual_row.saturating_add(1);
            if lines.len() >= visible_rows {
                break;
            }
            if is_last_wrap {
                for key in AnnotationKey::candidates_from_ui_row(&app.document.changeset, row) {
                    if let Some(draft) = draft
                        .as_ref()
                        .filter(|d| d.model_row_index == row_index && d.key == key)
                    {
                        let label = app.annotation_label(&draft.key);
                        push_annotation_block(
                            &mut lines,
                            render_annotation_compose_block(draft, width, theme, label.as_deref()),
                            visible_rows,
                        );
                    } else if let Some(text) = annotations.get(&key)
                        && draft.as_ref().is_none_or(|d| d.key != key)
                    {
                        let label = app.annotation_label(&key);
                        push_annotation_block(
                            &mut lines,
                            render_annotation_saved_block(text, width, theme, label.as_deref()),
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

fn push_annotation_block(
    lines: &mut Vec<Line<'static>>,
    block: Vec<Line<'static>>,
    visible_rows: usize,
) {
    for line in block {
        if lines.len() >= visible_rows {
            break;
        }
        lines.push(line);
    }
}

fn mouse_highlight_for_viewport(app: &DiffApp) -> Option<(u16, usize)> {
    if app.diff_modal_blocks_mouse_hover() {
        return None;
    }
    let (column, _) = app.viewport.mouse_hover?;
    app.diff_mouse_highlight_visual_row()
        .map(|visual_row| (column, visual_row))
}

fn row_has_annotation_target(app: &DiffApp, row: UiRow) -> bool {
    AnnotationKey::from_ui_row(&app.document.changeset, row).is_some()
}

fn row_has_diff_code_content(row: UiRow) -> bool {
    matches!(
        row,
        UiRow::UnifiedLine { .. } | UiRow::SplitLine { .. } | UiRow::ContextLine { .. }
    )
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
    focused_hunk: Option<(usize, usize)>,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let hunk_focused = row
        .hunk_key()
        .is_some_and(|hunk_key| Some(hunk_key) == focused_hunk);

    match row {
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => render_context_line_wrapped(app, file, old_line, new_line, row_index, width),
        UiRow::UnifiedLine { file, hunk, line } => {
            let kind = app.document.changeset.files[file].hunks[hunk].lines[line].kind;
            let syntax =
                unified_syntax_side(kind).and_then(|side| app.syntax_line(file, hunk, line, side));
            let inline = app.inline_ranges(file, hunk, line);
            let diff_line = &app.document.changeset.files[file].hunks[hunk].lines[line];
            render_unified_line_wrapped_with_focus(
                diff_line,
                syntax.as_ref(),
                &inline,
                width,
                theme,
                hunk_focused,
                &app.filters.grep_filter,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = &app.document.changeset.files[file].hunks[hunk].lines[line];
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
                file,
                hunk,
                left,
                right,
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
    focused_hunk: Option<(usize, usize)>,
) -> Line<'static> {
    let theme = app.config.theme;
    let horizontal_scroll = app.viewport.horizontal_scroll;
    let hunk_focused = row
        .hunk_key()
        .is_some_and(|hunk_key| Some(hunk_key) == focused_hunk);
    let mut line = match row {
        UiRow::FileSeparator => file_separator_line(app.viewport.layout, width, theme),
        UiRow::FileHeader(file_index) => {
            let file = &app.document.changeset.files[file_index];
            file_header_line(file, width, theme)
        }
        UiRow::BinaryFile(file_index) => {
            let file = &app.document.changeset.files[file_index];
            let message = if file.is_binary {
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
            lines,
            expanded > 0,
            context_expand_marker(hunk),
            width,
            theme,
        ),
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => render_context_line(app, file, old_line, new_line, row_index, width),
        UiRow::ContextHide { hunk, lines, .. } => {
            context_hide_line(lines, context_hide_marker(hunk), width, theme)
        }
        UiRow::HunkHeader { file, hunk } => {
            let hunk = &app.document.changeset.files[file].hunks[hunk];
            if hunk_focused {
                hunk_header_line_with_focus(hunk, width, theme, true)
            } else {
                hunk_header_line(hunk, width, theme)
            }
        }
        UiRow::UnifiedLine { file, hunk, line } => {
            let kind = app.document.changeset.files[file].hunks[hunk].lines[line].kind;
            let syntax =
                unified_syntax_side(kind).and_then(|side| app.syntax_line(file, hunk, line, side));
            let inline = app.inline_ranges(file, hunk, line);
            let diff_line = &app.document.changeset.files[file].hunks[hunk].lines[line];
            render_unified_line_at_scroll_with_focus(
                diff_line,
                syntax.as_ref(),
                &inline,
                width,
                theme,
                horizontal_scroll,
                hunk_focused,
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = &app.document.changeset.files[file].hunks[hunk].lines[line];
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
                file,
                hunk,
                left,
                right,
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

pub(crate) fn context_show_line(
    lines: usize,
    more: bool,
    marker: &str,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let suffix = if lines == 1 { "line" } else { "lines" };
    let label = if more {
        format!(
            " {marker} show {} more unchanged {suffix}",
            format_count(lines)
        )
    } else {
        format!(" {marker} show {} unchanged {suffix}", format_count(lines))
    };
    context_action_line(&label, width, theme, theme.muted)
}

pub(crate) fn context_hide_line(
    lines: usize,
    marker: &str,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let suffix = if lines == 1 { "line" } else { "lines" };
    context_action_line(
        &format!(" {marker} hide {} unchanged {suffix}", format_count(lines)),
        width,
        theme,
        theme.muted,
    )
}

pub(crate) fn context_expand_marker(hunk: usize) -> &'static str {
    if hunk == 0 { "▴" } else { "▾" }
}

pub(crate) fn context_hide_marker(hunk: usize) -> &'static str {
    if hunk == 0 { "▾" } else { "▴" }
}

pub(crate) fn context_action_line(
    label: &str,
    width: usize,
    theme: DiffTheme,
    text_color: Color,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let bg = base_bg(theme);
    let mut spans = Vec::new();
    let indicator_width = 1.min(width);
    if indicator_width > 0 {
        spans.push(diff_indicator_span(DiffLineKind::Meta, theme));
    }
    let content_width = width.saturating_sub(indicator_width);
    if content_width > 0 {
        spans.push(Span::styled(
            fit_padded(label, content_width),
            Style::default().fg(text_color).bg(bg),
        ));
    }
    Line::from(spans)
}

pub(crate) fn render_context_line(
    app: &mut DiffApp,
    file: usize,
    old_line: usize,
    new_line: usize,
    row_index: usize,
    width: usize,
) -> Line<'static> {
    let theme = app.config.theme;
    let horizontal_scroll = app.viewport.horizontal_scroll;
    let side = app.context_source_side(file);
    let syntax = side.and_then(|side| {
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        app.syntax_file_line(file, side, line_number)
    });
    let diff_line = DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(old_line),
        new_line: Some(new_line),
        text: app.context_line_text(file, old_line, new_line),
    };

    match app.viewport.layout {
        DiffLayoutMode::Unified => render_unified_line_at_scroll(
            &diff_line,
            syntax.as_ref(),
            &[],
            row_index,
            width,
            theme,
            horizontal_scroll,
        ),
        DiffLayoutMode::Split => render_split_context_line(
            &diff_line,
            syntax.as_ref(),
            row_index,
            width,
            theme,
            horizontal_scroll,
        ),
    }
}

pub(crate) fn render_context_line_wrapped(
    app: &mut DiffApp,
    file: usize,
    old_line: usize,
    new_line: usize,
    row_index: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let side = app.context_source_side(file);
    let syntax = side.and_then(|side| {
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        app.syntax_file_line(file, side, line_number)
    });
    let diff_line = DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(old_line),
        new_line: Some(new_line),
        text: app.context_line_text(file, old_line, new_line),
    };

    match app.viewport.layout {
        DiffLayoutMode::Unified => render_unified_line_wrapped_with_focus(
            &diff_line,
            syntax.as_ref(),
            &[],
            width,
            theme,
            false,
            &app.filters.grep_filter,
        ),
        DiffLayoutMode::Split => {
            let visual_row_start = app.wrapped_visual_scroll_for_model_row(row_index);
            render_split_context_line_wrapped(
                &diff_line,
                syntax.as_ref(),
                visual_row_start,
                width,
                theme,
                &app.filters.grep_filter,
            )
        }
    }
}

pub(crate) fn render_split_context_line(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    row_index: usize,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut spans = split_cell_spans_at_scroll(
        Some(line),
        syntax,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index,
            width: left_width,
            theme,
        },
        horizontal_scroll,
    );
    spans.extend(split_cell_spans_at_scroll(
        Some(line),
        syntax,
        &[],
        SplitCellRender {
            side: SplitSide::New,
            row_index,
            width: right_width,
            theme,
        },
        horizontal_scroll,
    ));
    Line::from(spans)
}

pub(crate) fn render_split_context_line_wrapped(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    row_index: usize,
    width: usize,
    theme: DiffTheme,
    grep_filter: &str,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let left_content_width = split_cell_content_width(left_width);
    let right_content_width = split_cell_content_width(right_width);
    let left_scrolls = wrapped_line_start_columns(&line.text, left_content_width);
    let right_scrolls = wrapped_line_start_columns(&line.text, right_content_width);
    let text_width = line.text.width();
    let rows = left_scrolls.len().max(right_scrolls.len());
    let mut lines = Vec::with_capacity(rows);
    for wrap_index in 0..rows {
        let left_scroll = wrapped_segment_scroll(&left_scrolls, text_width, wrap_index);
        let right_scroll = wrapped_segment_scroll(&right_scrolls, text_width, wrap_index);
        let visual_row = row_index.saturating_add(wrap_index);
        let mut spans = split_cell_spans_at_scroll_with_focus_and_continuation(
            Some(line),
            syntax,
            &[],
            SplitCellRender {
                side: SplitSide::Old,
                row_index: visual_row,
                width: left_width,
                theme,
            },
            left_scroll,
            false,
            wrap_index > 0,
        );
        spans.extend(split_cell_spans_at_scroll_with_focus_and_continuation(
            Some(line),
            syntax,
            &[],
            SplitCellRender {
                side: SplitSide::New,
                row_index: visual_row,
                width: right_width,
                theme,
            },
            right_scroll,
            false,
            wrap_index > 0,
        ));
        lines.push(highlight_wrapped_split_grep_line(
            Line::from(spans),
            Some(line),
            Some(line),
            SplitGrepRender {
                query: grep_filter,
                width,
                left_scroll,
                right_scroll,
                theme,
            },
        ));
    }
    lines
}

pub(crate) fn render_unified_line_at_scroll(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    _row_index: usize,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Line<'static> {
    render_unified_line_at_scroll_with_focus(
        line,
        syntax,
        inline,
        width,
        theme,
        horizontal_scroll,
        false,
    )
}

pub(crate) fn render_unified_line_at_scroll_with_focus(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
    focused: bool,
) -> Line<'static> {
    render_unified_line_segment_with_focus(
        line,
        syntax,
        inline,
        UnifiedLineRender {
            width,
            theme,
            horizontal_scroll,
            focused,
            continuation: false,
        },
    )
}

pub(crate) fn render_unified_line_wrapped_with_focus(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    width: usize,
    theme: DiffTheme,
    focused: bool,
    grep_filter: &str,
) -> Vec<Line<'static>> {
    let content_width = unified_content_width(width);
    let scrolls = wrapped_line_start_columns(&line.text, content_width);
    let mut lines = Vec::with_capacity(scrolls.len());
    for (wrap_index, horizontal_scroll) in scrolls.iter().copied().enumerate() {
        let rendered = render_unified_line_segment_with_focus(
            line,
            syntax,
            inline,
            UnifiedLineRender {
                width,
                theme,
                horizontal_scroll,
                focused,
                continuation: wrap_index > 0,
            },
        );
        lines.push(highlight_wrapped_unified_grep_line(
            rendered,
            line,
            grep_filter,
            width,
            horizontal_scroll,
            theme,
        ));
    }
    lines
}

#[derive(Debug, Clone, Copy)]
struct UnifiedLineRender {
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
    focused: bool,
    continuation: bool,
}

fn render_unified_line_segment_with_focus(
    line: &DiffLine,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: UnifiedLineRender,
) -> Line<'static> {
    let UnifiedLineRender {
        width,
        theme,
        horizontal_scroll,
        focused,
        continuation,
    } = render;

    if width == 0 {
        return Line::default();
    }

    let sign = if continuation {
        " "
    } else {
        match line.kind {
            DiffLineKind::Context => " ",
            DiffLineKind::Addition => "+",
            DiffLineKind::Deletion => "-",
            DiffLineKind::Meta => " ",
        }
    };
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = unified_content_width(width);
    let gutter = if continuation {
        spaces(UNIFIED_GUTTER_WIDTH.saturating_sub(1)).into_owned()
    } else {
        unified_gutter_text(line.old_line, line.new_line)
    };
    let mut spans = Vec::new();
    if indicator_width > 0 {
        spans.push(diff_indicator_span_for_focus(line.kind, theme, focused));
    }
    if gutter_width > 0 {
        spans.extend(gutter_spans(&gutter, sign, gutter_width, line.kind, theme));
    }
    spans.extend(content_spans_at_scroll(
        &line.text,
        syntax,
        inline,
        line.kind,
        content_width,
        theme,
        horizontal_scroll,
    ));
    Line::from(spans)
}

fn highlight_wrapped_unified_grep_line(
    rendered: Line<'static>,
    line: &DiffLine,
    query: &str,
    width: usize,
    horizontal_scroll: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if query.is_empty() {
        return rendered;
    }

    let targets = grep_highlight_target_for_columns(
        diff_line_grep_highlight_text(line),
        &rendered.spans,
        unified_content_start_column(width),
        width,
        1 + scrolled_text_byte_start(&line.text, horizontal_scroll),
    )
    .into_iter()
    .collect();
    highlighted_grep_text_line(rendered, query, targets, theme)
}

pub(crate) fn row_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    if theme.transparent_background {
        return Color::Reset;
    }

    match (theme.diff.line_background, kind) {
        (DiffBackground::None, _) => theme.background,
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (DiffBackground::Strong, DiffLineKind::Addition) => theme.addition_inline_bg,
        (DiffBackground::Strong, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => theme.background,
    }
}

pub(crate) fn line_style(kind: DiffLineKind, theme: DiffTheme) -> Style {
    match kind {
        DiffLineKind::Addition => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Deletion => Style::default()
            .fg(theme.foreground)
            .bg(row_bg(kind, theme)),
        DiffLineKind::Meta => Style::default().fg(theme.muted).bg(base_bg(theme)),
        DiffLineKind::Context => Style::default().fg(theme.foreground).bg(base_bg(theme)),
    }
}
