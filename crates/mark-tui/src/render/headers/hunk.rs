use mark_diff::DiffLineKind;
use ratatui::prelude::{Color, Line, Span, Style};

use crate::{
    render::{
        headers::{
            HeaderSpanPart, HeaderStyles, compact_delta_parts, hunk_header_spans_with_delta,
        },
        style::{diff_indicator_span, focused_diff_indicator_span},
        text::terminal_text,
    },
    theme::{DiffTheme, line_gutter_bg},
};

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
    if width == 0 {
        return Line::default();
    }

    let gutter_bg = line_gutter_bg(DiffLineKind::Meta, theme);
    let content_width = width.saturating_sub(1);
    let mut spans = Vec::new();
    spans.push(if focused {
        focused_diff_indicator_span(DiffLineKind::Meta, theme)
    } else {
        diff_indicator_span(DiffLineKind::Meta, theme)
    });
    if content_width > 0 {
        spans.push(Span::styled(" ", Style::default().bg(gutter_bg)));
        if content_width > 1 {
            spans.extend(hunk_header_spans(hunk, content_width - 1, theme, gutter_bg));
        }
    }

    Line::from(spans)
}

pub(crate) fn hunk_header_spans(
    hunk: &mark_diff::DiffHunk,
    width: usize,
    theme: DiffTheme,
    bg: Color,
) -> Vec<Span<'static>> {
    let (additions, deletions) = hunk_change_counts(hunk);
    hunk_header_spans_with_delta(
        &hunk_header_location_parts(&hunk.header, theme, bg),
        hunk_header_context(&hunk.header),
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

pub(crate) fn hunk_header_context(header: &str) -> &str {
    header
        .splitn(3, "@@")
        .nth(2)
        .map(str::trim)
        .unwrap_or_default()
}

pub(crate) fn normalized_hunk_header_text(header: &str) -> String {
    let mut text = hunk_header_location_text(header);
    let context = hunk_header_context(header);
    if !context.is_empty() {
        text.push(' ');
        text.push_str(context);
    }

    terminal_text(&text)
}

fn hunk_header_location_text(header: &str) -> String {
    match parse_hunk_header_location(header) {
        HunkHeaderLocation::Ranges {
            old_range,
            new_range,
        } => format!("@@ {old_range} {new_range} @@"),
        HunkHeaderLocation::Fallback(text) => text,
    }
}

pub(crate) fn hunk_header_location_parts(
    header: &str,
    theme: DiffTheme,
    bg: Color,
) -> Vec<HeaderSpanPart> {
    match parse_hunk_header_location(header) {
        HunkHeaderLocation::Ranges {
            old_range,
            new_range,
        } => vec![
            HeaderSpanPart {
                text: "@@ ".to_owned(),
                style: Style::default().fg(theme.muted).bg(bg),
            },
            HeaderSpanPart {
                text: old_range.to_owned(),
                style: Style::default().fg(theme.deletion_fg).bg(bg),
            },
            HeaderSpanPart {
                text: " ".to_owned(),
                style: Style::default().fg(theme.muted).bg(bg),
            },
            HeaderSpanPart {
                text: new_range.to_owned(),
                style: Style::default().fg(theme.addition_fg).bg(bg),
            },
            HeaderSpanPart {
                text: " @@".to_owned(),
                style: Style::default().fg(theme.muted).bg(bg),
            },
        ],
        HunkHeaderLocation::Fallback(text) => vec![HeaderSpanPart {
            text,
            style: Style::default().fg(theme.muted).bg(bg),
        }],
    }
}

enum HunkHeaderLocation<'a> {
    Ranges {
        old_range: &'a str,
        new_range: &'a str,
    },
    Fallback(String),
}

fn parse_hunk_header_location(header: &str) -> HunkHeaderLocation<'_> {
    let mut parts = header.splitn(3, "@@");
    let Some("") = parts.next() else {
        return HunkHeaderLocation::Fallback(header.trim().to_owned());
    };
    let Some(location) = parts.next() else {
        return HunkHeaderLocation::Fallback(header.trim().to_owned());
    };

    let mut coordinates = location.split_whitespace();
    let old_range = coordinates.next().unwrap_or_default();
    let new_range = coordinates.next().unwrap_or_default();
    if old_range.is_empty() || new_range.is_empty() {
        return HunkHeaderLocation::Fallback(format!("@@{location}@@"));
    }

    HunkHeaderLocation::Ranges {
        old_range,
        new_range,
    }
}

pub(crate) fn hunk_change_counts(hunk: &mark_diff::DiffHunk) -> (usize, usize) {
    hunk.lines.iter().fold(
        (0usize, 0usize),
        |(additions, deletions), line| match line.kind() {
            DiffLineKind::Addition => (additions + 1, deletions),
            DiffLineKind::Deletion => (additions, deletions + 1),
            DiffLineKind::Context | DiffLineKind::Meta => (additions, deletions),
        },
    )
}
