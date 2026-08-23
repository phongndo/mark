use ratatui::{
    Frame,
    buffer::{Buffer, Cell, CellWidth},
    layout::{Alignment, Rect},
    prelude::{Line, Span, Style},
    widgets::Widget,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    app::DiffApp,
    model::{FileIndex, HunkIndex, UiRow},
    render::{
        grep::{grep_highlight_targets_for_row, highlighted_grep_text_line},
        headers::{
            file_header_line, hunk_header_line_with_focus, hunk_header_line_with_focus_and_metadata,
        },
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
        DiffViewport {
            lines,
            style: Style::default().bg(diff_base_bg(app.config.theme)),
        },
        area,
    );
}

/// Renders lines that Mark has already wrapped and fitted. Using `Paragraph`
/// here would collect every row into temporary styled-grapheme storage solely
/// to apply an unneeded second truncation pass.
struct DiffViewport {
    lines: Vec<Line<'static>>,
    style: Style,
}

impl Widget for DiffViewport {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }

        buffer.set_style(area, self.style);
        for (offset, line) in self.lines.iter().take(area.height as usize).enumerate() {
            let y = area.y + offset as u16;
            let row_start = buffer.index_of(area.x, y);
            let row = &mut buffer.content[row_start..row_start + area.width as usize];
            render_diff_line(line, area.width, row);
        }
    }
}

fn render_diff_line(line: &Line<'_>, width: u16, row: &mut [Cell]) {
    let alignment = line.alignment.unwrap_or(Alignment::Left);
    let line_width = match alignment {
        Alignment::Left => 0,
        Alignment::Center | Alignment::Right => truncated_line_width(line, width),
    };
    let offset = match alignment {
        Alignment::Left => 0,
        Alignment::Center => (width / 2).saturating_sub(line_width / 2),
        Alignment::Right => width.saturating_sub(line_width),
    };
    let mut x = offset as usize;
    visit_truncated_line(line, width, |symbol, style, symbol_width| {
        row[x].set_symbol(symbol).set_style(style);
        x += symbol_width as usize;
    });
}

fn truncated_line_width(line: &Line<'_>, max_width: u16) -> u16 {
    visit_truncated_line(line, max_width, |_, _, _| {})
}

fn visit_truncated_line(
    line: &Line<'_>,
    max_width: u16,
    mut visit: impl FnMut(&str, Style, u16),
) -> u16 {
    let mut used = 0u16;
    let line_style = line.style;

    'spans: for span in &line.spans {
        let style = line_style.patch(span.style);
        let text = span.content.as_ref();
        // Every printable ASCII byte is one grapheme and one cell. Preserve
        // Paragraph's control filtering without invoking Unicode segmentation.
        if text.is_ascii() {
            for (index, byte) in text.bytes().enumerate() {
                if byte.is_ascii_control() {
                    continue;
                }
                if used == max_width {
                    break 'spans;
                }
                visit(&text[index..index + 1], style, 1);
                used += 1;
            }
            continue;
        }

        for symbol in text
            .graphemes(true)
            .filter(|symbol| !symbol.contains(char::is_control))
        {
            let width = symbol.cell_width();
            if width > max_width {
                continue;
            }
            let Some(next_used) = used.checked_add(width).filter(|width| *width <= max_width)
            else {
                break 'spans;
            };
            used = next_used;
            if width > 0 {
                visit(symbol, style, width);
            }
        }
    }

    used
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
            let hunk_diff = &app.document.changeset.files[file].hunks()[hunk];
            if let Some((additions, deletions, fallback_context_line)) =
                app.document.search_index.cached_hunk_header(file, hunk)
            {
                hunk_header_line_with_focus_and_metadata(
                    hunk_diff,
                    width,
                    theme,
                    hunk_focused,
                    fallback_context_line,
                    additions,
                    deletions,
                )
            } else {
                hunk_header_line_with_focus(hunk_diff, width, theme, hunk_focused)
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

#[cfg(test)]
mod viewport_widget_tests {
    use ratatui::{
        buffer::Buffer,
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span, Text},
        widgets::{Paragraph, Widget},
    };

    use super::DiffViewport;

    #[test]
    fn direct_diff_viewport_matches_paragraph_reference() {
        const ATOMS: &[&str] = &[
            "a",
            " ",
            "\t",
            "\0",
            "é",
            "e\u{301}",
            "界",
            "👩‍💻",
            "ｶﾞ",
            "\u{200b}",
            "\n",
            "x",
        ];
        let mut state = 0x8f3c_2a17_d4e5_690bu64;

        for case in 0..256 {
            let line_count = 1 + next_random(&mut state) as usize % 5;
            let mut lines = Vec::with_capacity(line_count);
            for _ in 0..line_count {
                let span_count = 1 + next_random(&mut state) as usize % 4;
                let mut spans = Vec::with_capacity(span_count);
                for _ in 0..span_count {
                    let atom_count = next_random(&mut state) as usize % 9;
                    let mut content = String::new();
                    for _ in 0..atom_count {
                        content.push_str(ATOMS[next_random(&mut state) as usize % ATOMS.len()]);
                    }
                    spans.push(Span::styled(content, random_style(&mut state)));
                }
                let alignment = match next_random(&mut state) % 4 {
                    0 => None,
                    1 => Some(Alignment::Left),
                    2 => Some(Alignment::Center),
                    _ => Some(Alignment::Right),
                };
                lines.push(Line {
                    spans,
                    style: random_style(&mut state),
                    alignment,
                });
            }

            let width = 1 + next_random(&mut state) as u16 % 24;
            let height = 1 + next_random(&mut state) as u16 % 6;
            let outer = Rect::new(2, 3, width + 4, height + 3);
            let area = Rect::new(3, 4, width, height);
            let mut expected = Buffer::empty(outer);
            for cell in &mut expected.content {
                cell.set_symbol("z");
            }
            expected.set_style(outer, Style::default().fg(Color::Yellow).bg(Color::Blue));
            let mut actual = expected.clone();
            let base = random_style(&mut state);

            Widget::render(
                Paragraph::new(Text::from(lines.clone())).style(base),
                area,
                &mut expected,
            );
            Widget::render(DiffViewport { lines, style: base }, area, &mut actual);

            assert_eq!(actual, expected, "render mismatch in generated case {case}");
        }
    }

    fn next_random(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    fn random_style(state: &mut u64) -> Style {
        let foreground = match next_random(state) % 4 {
            0 => None,
            1 => Some(Color::Red),
            2 => Some(Color::Green),
            _ => Some(Color::Indexed(7)),
        };
        let background = match next_random(state) % 3 {
            0 => None,
            1 => Some(Color::Black),
            _ => Some(Color::Indexed(8)),
        };
        let mut style = Style::default();
        if let Some(foreground) = foreground {
            style = style.fg(foreground);
        }
        if let Some(background) = background {
            style = style.bg(background);
        }
        if next_random(state).is_multiple_of(2) {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }
}
