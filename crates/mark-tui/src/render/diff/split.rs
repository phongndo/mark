use mark_diff::{DiffLine, DiffLineKind};
use mark_syntax::HighlightedLine;
use ratatui::prelude::{Line, Span, Style};

use crate::{
    app::{DiffApp, split_cell_content_width, wrapped_line_start_columns},
    render::{
        grep::{highlighted_grep_text_line, split_diff_line_grep_highlight_target},
        style::diff_base_bg,
        text::{display_width, spaces},
    },
    syntax::{DiffSide, InlineRange},
    theme::{DiffTheme, GUTTER_WIDTH, line_gutter_bg},
};

use super::{
    ContentSpanRender, append_content_spans_at_scroll, append_gutter_spans, content_span_capacity,
    diff_indicator_span_for_focus, empty_diff_fill_from, split_gutter_text,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SplitLineRender {
    pub(crate) file: usize,
    pub(crate) hunk: usize,
    pub(crate) left: Option<usize>,
    pub(crate) right: Option<usize>,
    pub(crate) row_index: usize,
    pub(crate) width: usize,
    pub(crate) focused: bool,
}

pub(crate) fn render_split_line_with_focus(
    app: &mut DiffApp,
    render: SplitLineRender,
) -> Line<'static> {
    let SplitLineRender {
        file,
        hunk,
        left,
        right,
        row_index,
        width,
        focused,
    } = render;
    if width == 0 {
        return Line::default();
    }
    let theme = app.config.theme;
    let horizontal_scroll = app.viewport.horizontal_scroll;

    let left_syntax = left.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::Old));
    let right_syntax = right.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::New));
    let left_inline = left
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();
    let right_inline = right
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let lines = &app.document.changeset.files[file].hunks()[hunk].lines;
    let left_line = left.and_then(|index| lines.get(index));
    let right_line = right.and_then(|index| lines.get(index));
    let mut spans = Vec::with_capacity(
        split_cell_span_capacity(
            left_line,
            left_syntax.as_deref(),
            left_inline.len(),
            left_width,
        )
        .saturating_add(split_cell_span_capacity(
            right_line,
            right_syntax.as_deref(),
            right_inline.len(),
            right_width,
        )),
    );
    append_split_cell_spans_at_scroll_with_focus_and_continuation(
        &mut spans,
        left_line,
        left_syntax.as_deref(),
        &left_inline,
        SplitCellSpanRender {
            cell: SplitCellRender {
                side: SplitSide::Old,
                row_index,
                width: left_width,
                theme,
            },
            horizontal_scroll,
            focused,
            continuation: false,
        },
    );
    append_split_cell_spans_at_scroll_with_focus_and_continuation(
        &mut spans,
        right_line,
        right_syntax.as_deref(),
        &right_inline,
        SplitCellSpanRender {
            cell: SplitCellRender {
                side: SplitSide::New,
                row_index,
                width: right_width,
                theme,
            },
            horizontal_scroll,
            focused,
            continuation: false,
        },
    );
    Line::from(spans)
}

pub(crate) fn render_split_line_wrapped_with_focus(
    app: &mut DiffApp,
    render: SplitLineRender,
) -> Vec<Line<'static>> {
    let SplitLineRender {
        file,
        hunk,
        left,
        right,
        row_index,
        width,
        focused,
    } = render;
    if width == 0 {
        return vec![Line::default()];
    }
    let theme = app.config.theme;

    let left_syntax = left.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::Old));
    let right_syntax = right.and_then(|index| app.syntax_line(file, hunk, index, DiffSide::New));
    let left_inline = left
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();
    let right_inline = right
        .map(|index| app.inline_ranges(file, hunk, index))
        .unwrap_or_default();

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let lines = &app.document.changeset.files[file].hunks()[hunk].lines;
    let left_line = left.and_then(|index| lines.get(index));
    let right_line = right.and_then(|index| lines.get(index));
    let left_content_width = split_cell_content_width(left_width);
    let right_content_width = split_cell_content_width(right_width);
    let left_text = left_line.map(DiffLine::text_lossy);
    let right_text = right_line.map(DiffLine::text_lossy);
    let left_scrolls = left_text
        .as_ref()
        .map(|text| wrapped_line_start_columns(text, left_content_width))
        .unwrap_or_else(|| vec![0]);
    let right_scrolls = right_text
        .as_ref()
        .map(|text| wrapped_line_start_columns(text, right_content_width))
        .unwrap_or_else(|| vec![0]);
    let left_text_width = left_text
        .as_ref()
        .map(|text| display_width(text))
        .unwrap_or(0);
    let right_text_width = right_text
        .as_ref()
        .map(|text| display_width(text))
        .unwrap_or(0);
    let rows = left_scrolls.len().max(right_scrolls.len()).max(1);
    let visual_row_start = app.wrapped_visual_scroll_for_model_row(row_index);
    let mut rendered_lines = Vec::with_capacity(rows);
    for wrap_index in 0..rows {
        let left_scroll = wrapped_segment_scroll(&left_scrolls, left_text_width, wrap_index);
        let right_scroll = wrapped_segment_scroll(&right_scrolls, right_text_width, wrap_index);
        let visual_row = visual_row_start.saturating_add(wrap_index);
        let mut spans = Vec::with_capacity(
            split_cell_span_capacity(
                left_line,
                left_syntax.as_deref(),
                left_inline.len(),
                left_width,
            )
            .saturating_add(split_cell_span_capacity(
                right_line,
                right_syntax.as_deref(),
                right_inline.len(),
                right_width,
            )),
        );
        append_split_cell_spans_at_scroll_with_focus_and_continuation(
            &mut spans,
            left_line,
            left_syntax.as_deref(),
            &left_inline,
            SplitCellSpanRender {
                cell: SplitCellRender {
                    side: SplitSide::Old,
                    row_index: visual_row,
                    width: left_width,
                    theme,
                },
                horizontal_scroll: left_scroll,
                focused,
                continuation: wrap_index > 0,
            },
        );
        append_split_cell_spans_at_scroll_with_focus_and_continuation(
            &mut spans,
            right_line,
            right_syntax.as_deref(),
            &right_inline,
            SplitCellSpanRender {
                cell: SplitCellRender {
                    side: SplitSide::New,
                    row_index: visual_row,
                    width: right_width,
                    theme,
                },
                horizontal_scroll: right_scroll,
                focused,
                continuation: wrap_index > 0,
            },
        );
        let line = Line::from(spans);
        rendered_lines.push(highlight_wrapped_split_grep_line(
            line,
            left_line,
            right_line,
            SplitGrepRender {
                query: &app.filters.grep_filter,
                width,
                left_scroll,
                right_scroll,
                theme,
            },
        ));
    }
    rendered_lines
}

pub(super) fn wrapped_segment_scroll(
    starts: &[usize],
    text_width: usize,
    wrap_index: usize,
) -> usize {
    starts.get(wrap_index).copied().unwrap_or(text_width)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SplitGrepRender<'a> {
    pub(super) query: &'a str,
    pub(super) width: usize,
    pub(super) left_scroll: usize,
    pub(super) right_scroll: usize,
    pub(super) theme: DiffTheme,
}

pub(super) fn highlight_wrapped_split_grep_line(
    rendered: Line<'static>,
    left_line: Option<&DiffLine>,
    right_line: Option<&DiffLine>,
    render: SplitGrepRender<'_>,
) -> Line<'static> {
    let SplitGrepRender {
        query,
        width,
        left_scroll,
        right_scroll,
        theme,
    } = render;

    if query.is_empty() {
        return rendered;
    }

    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let mut targets = Vec::with_capacity(2);
    if let Some(target) = left_line.and_then(|line| {
        split_diff_line_grep_highlight_target(line, &rendered.spans, 0, left_width, left_scroll)
    }) {
        targets.push(target);
    }
    if let Some(target) = right_line.and_then(|line| {
        split_diff_line_grep_highlight_target(
            line,
            &rendered.spans,
            left_width,
            right_width,
            right_scroll,
        )
    }) {
        targets.push(target);
    }

    highlighted_grep_text_line(rendered, query, targets, theme)
}

fn split_cell_span_capacity(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline_range_count: usize,
    width: usize,
) -> usize {
    if width == 0 {
        0
    } else if line.is_some() {
        3usize.saturating_add(content_span_capacity(syntax, inline_range_count))
    } else {
        3
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SplitSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SplitCellRender {
    pub(crate) side: SplitSide,
    pub(crate) row_index: usize,
    pub(crate) width: usize,
    pub(crate) theme: DiffTheme,
}

#[derive(Debug, Clone, Copy)]
struct SplitCellSpanRender {
    cell: SplitCellRender,
    horizontal_scroll: usize,
    focused: bool,
    continuation: bool,
}

pub(crate) fn split_cell_spans_at_scroll(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellRender,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    split_cell_spans_at_scroll_with_focus(line, syntax, inline, render, horizontal_scroll, false)
}

pub(crate) fn split_cell_spans_at_scroll_with_focus(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellRender,
    horizontal_scroll: usize,
    focused: bool,
) -> Vec<Span<'static>> {
    split_cell_spans_at_scroll_with_focus_and_continuation(
        line,
        syntax,
        inline,
        render,
        horizontal_scroll,
        focused,
        false,
    )
}

pub(super) fn split_cell_spans_at_scroll_with_focus_and_continuation(
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellRender,
    horizontal_scroll: usize,
    focused: bool,
    continuation: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(6);
    append_split_cell_spans_at_scroll_with_focus_and_continuation(
        &mut spans,
        line,
        syntax,
        inline,
        SplitCellSpanRender {
            cell: render,
            horizontal_scroll,
            focused,
            continuation,
        },
    );
    spans
}

fn append_split_cell_spans_at_scroll_with_focus_and_continuation(
    spans: &mut Vec<Span<'static>>,
    line: Option<&DiffLine>,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    render: SplitCellSpanRender,
) {
    let SplitCellSpanRender {
        cell,
        horizontal_scroll,
        focused,
        continuation,
    } = render;
    let SplitCellRender {
        side,
        row_index,
        width,
        theme,
    } = cell;

    if width == 0 {
        return;
    }

    let Some(line) = line else {
        let empty_kind = DiffLineKind::Context;
        let indicator_width = 1.min(width);
        let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
        let content_width = split_cell_content_width(width);
        if indicator_width > 0 {
            spans.push(diff_indicator_span_for_focus(empty_kind, theme, focused));
        }
        if gutter_width > 0 {
            spans.push(Span::styled(
                spaces(gutter_width),
                Style::default().bg(line_gutter_bg(empty_kind, theme)),
            ));
        }
        if content_width > 0 {
            spans.push(Span::styled(
                empty_diff_fill_from(
                    content_width,
                    row_index,
                    indicator_width + gutter_width + horizontal_scroll,
                    theme.decorations.show_empty_fill(),
                ),
                Style::default()
                    .fg(theme.empty_diff)
                    .bg(diff_base_bg(theme)),
            ));
        }
        return;
    };

    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    let content_width = split_cell_content_width(width);
    let line_number = if continuation {
        None
    } else {
        match side {
            SplitSide::Old => line.old_line(),
            SplitSide::New => line.new_line(),
        }
    };
    let sign = if continuation {
        " "
    } else {
        match (side, line.kind()) {
            (SplitSide::Old, DiffLineKind::Deletion) => "-",
            (SplitSide::New, DiffLineKind::Addition) => "+",
            _ => " ",
        }
    };

    if indicator_width > 0 {
        spans.push(diff_indicator_span_for_focus(line.kind(), theme, focused));
    }
    if gutter_width > 0 {
        append_gutter_spans(
            spans,
            split_gutter_text(line_number, sign.trim().is_empty()),
            sign,
            gutter_width,
            line.kind(),
            theme,
        );
    }
    let text = line.text_lossy();
    append_content_spans_at_scroll(
        spans,
        &text,
        ContentSpanRender {
            syntax,
            inline,
            kind: line.kind(),
            width: content_width,
            theme,
            horizontal_scroll,
        },
    );
}
