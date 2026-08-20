use ratatui::prelude::{Line, Style};

use crate::{controls::DiffLayoutMode, theme::DiffTheme};

use super::{
    ranges::highlighted_line_in_ranges,
    target::{split_content_start_column, unified_content_start_column},
};

pub(crate) fn highlighted_cursor_diff_content_line(
    line: Line<'static>,
    layout: DiffLayoutMode,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    highlighted_cursor_line_in_ranges(line, diff_content_column_ranges(layout, width), theme)
}

pub(crate) fn highlighted_cursor_meta_line(
    line: Line<'static>,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    highlighted_cursor_line_in_ranges(line, vec![(1.min(width), width)], theme)
}

pub(crate) fn highlighted_cursor_full_line(
    line: Line<'static>,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    highlighted_cursor_line_in_ranges(line, vec![(0, width)], theme)
}

pub(crate) fn highlight_saved_annotation_block(
    lines: Vec<Line<'static>>,
    width: usize,
    theme: DiffTheme,
    focused: bool,
) -> Vec<Line<'static>> {
    if !focused {
        return lines;
    }
    lines
        .into_iter()
        .map(|line| highlighted_cursor_full_line(line, width, theme))
        .collect()
}

pub(crate) fn highlighted_cursor_line_in_ranges(
    line: Line<'static>,
    column_ranges: Vec<(usize, usize)>,
    theme: DiffTheme,
) -> Line<'static> {
    highlighted_line_in_ranges(line, column_ranges, cursor_diff_content_line_style(theme))
}

fn diff_content_column_ranges(layout: DiffLayoutMode, width: usize) -> Vec<(usize, usize)> {
    match layout {
        DiffLayoutMode::Unified => vec![(unified_content_start_column(width), width)],
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let right_width = width.saturating_sub(left_width);
            vec![
                (split_content_start_column(left_width), left_width),
                (
                    left_width.saturating_add(split_content_start_column(right_width)),
                    width,
                ),
            ]
        }
    }
}

fn cursor_diff_content_line_style(theme: DiffTheme) -> Style {
    Style::default().bg(theme.cursor_line_bg)
}
