use ratatui::prelude::{Color, Style};

use crate::{
    render::{headers::HeaderSpanPart, text::terminal_text},
    theme::DiffTheme,
};

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

pub(super) fn hunk_header_location_parts(
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

fn hunk_header_location_text(header: &str) -> String {
    match parse_hunk_header_location(header) {
        HunkHeaderLocation::Ranges {
            old_range,
            new_range,
        } => format!("@@ {old_range} {new_range} @@"),
        HunkHeaderLocation::Fallback(text) => text,
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
