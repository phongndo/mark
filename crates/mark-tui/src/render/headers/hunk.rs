use mark_diff::DiffLineKind;
use ratatui::prelude::{Color, Line, Span, Style};

use crate::{
    render::{
        headers::{HeaderStyles, compact_delta_parts, hunk_header_spans_with_delta},
        style::{diff_base_bg, diff_indicator_span, focused_diff_indicator_span},
    },
    theme::DiffTheme,
};

mod location;

use location::hunk_header_location_parts;
pub(crate) use location::{hunk_header_context, normalized_hunk_header_text};

#[cfg(test)]
pub(crate) fn hunk_header_line(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    hunk_header_line_with_focus(hunk, width, theme, false)
}

pub(crate) fn hunk_header_line_with_focus(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    focused: bool,
) -> Line<'static> {
    let (additions, deletions) = hunk_change_counts(hunk);
    let display_context = hunk_header_display_context(hunk);
    hunk_header_line_with_focus_and_context(
        hunk,
        width,
        theme,
        focused,
        display_context,
        additions,
        deletions,
    )
}

pub(crate) fn hunk_header_line_with_focus_and_metadata(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    focused: bool,
    fallback_context_line: Option<usize>,
    additions: usize,
    deletions: usize,
) -> Line<'static> {
    let display_context =
        hunk_header_display_context_with_fallback_line(hunk, fallback_context_line);
    hunk_header_line_with_focus_and_context(
        hunk,
        width,
        theme,
        focused,
        display_context,
        additions,
        deletions,
    )
}

fn hunk_header_line_with_focus_and_context(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    focused: bool,
    display_context: &str,
    additions: usize,
    deletions: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let content_bg = diff_base_bg(theme);
    let content_width = width.saturating_sub(1);
    let mut spans = Vec::new();
    spans.push(if focused {
        focused_diff_indicator_span(DiffLineKind::Meta, theme)
    } else {
        diff_indicator_span(DiffLineKind::Meta, theme)
    });
    if content_width > 0 {
        spans.push(Span::styled(" ", Style::default().bg(content_bg)));
        if content_width > 1 {
            spans.extend(hunk_header_spans_with_metadata(
                hunk,
                content_width - 1,
                theme,
                content_bg,
                display_context,
                additions,
                deletions,
            ));
        }
    }

    Line::from(spans)
}

#[cfg(test)]
pub(crate) fn hunk_header_spans(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    bg: Color,
) -> Vec<Span<'static>> {
    let (additions, deletions) = hunk_change_counts(hunk);
    let display_context = hunk_header_display_context(hunk);
    hunk_header_spans_with_metadata(
        hunk,
        width,
        theme,
        bg,
        display_context,
        additions,
        deletions,
    )
}

fn hunk_header_spans_with_metadata(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    bg: Color,
    display_context: &str,
    additions: usize,
    deletions: usize,
) -> Vec<Span<'static>> {
    hunk_header_spans_with_delta(
        &hunk_header_location_parts(&hunk.header, theme, bg),
        display_context,
        &compact_delta_parts(additions, deletions),
        width,
        HeaderStyles {
            prefix: Style::default().fg(theme.muted).bg(bg),
            body: Style::default().fg(theme.foreground).bg(bg),
            fill: Style::default().bg(bg),
            addition: Style::default().fg(theme.addition_fg).bg(bg),
            deletion: Style::default().fg(theme.deletion_fg).bg(bg),
        },
    )
}

fn hunk_header_display_context(hunk: &mark_diff::DiffHunk) -> &str {
    let context = hunk_header_context(&hunk.header);
    if !context.is_empty() {
        return context;
    }

    hunk.lines
        .iter()
        .filter(|line| line.kind() != DiffLineKind::Meta)
        .map(|line| line.text().trim())
        .find(|text| !text.is_empty())
        .unwrap_or_default()
}

fn hunk_header_display_context_with_fallback_line(
    hunk: &mark_diff::DiffHunk,
    fallback_context_line: Option<usize>,
) -> &str {
    let context = hunk_header_context(&hunk.header);
    if !context.is_empty() {
        return context;
    }
    fallback_context_line
        .and_then(|line| hunk.lines.get(line))
        .map(|line| line.text().trim())
        .unwrap_or_default()
}
fn hunk_change_counts(hunk: &mark_diff::DiffHunk) -> (usize, usize) {
    hunk.lines.iter().fold(
        (0usize, 0usize),
        |(additions, deletions), line| match line.kind() {
            DiffLineKind::Addition => (additions + 1, deletions),
            DiffLineKind::Deletion => (additions, deletions + 1),
            DiffLineKind::Context | DiffLineKind::Meta => (additions, deletions),
        },
    )
}
