use mark_diff::FileStatus;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    model::FileIndex,
    render::{
        style::{diff_base_bg, file_sidebar_style, header_bg},
        text::{fit, fit_padded, fit_with_ellipsis, status_code},
    },
    theme::{
        DiffTheme, FILE_SIDEBAR_DEFAULT_WIDTH, FILE_SIDEBAR_MIN_DIFF_WIDTH, FILE_SIDEBAR_MIN_WIDTH,
    },
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum FileSidebarEntry<'a> {
    Group(&'a str),
    File {
        index: FileIndex,
        file: &'a mark_diff::DiffFile,
        name: &'a str,
    },
}

pub(crate) fn file_sidebar_width(app: &DiffApp, area_width: u16) -> u16 {
    if !app.sidebar.file_sidebar_open {
        return 0;
    }

    let max_width = max_file_sidebar_width(area_width);
    if max_width == 0 {
        return 0;
    }

    app.sidebar
        .file_sidebar_width
        .unwrap_or(FILE_SIDEBAR_DEFAULT_WIDTH)
        .clamp(FILE_SIDEBAR_MIN_WIDTH, max_width)
}

pub(crate) fn max_file_sidebar_width(area_width: u16) -> u16 {
    let max_width = area_width.saturating_sub(FILE_SIDEBAR_MIN_DIFF_WIDTH);
    if max_width < FILE_SIDEBAR_MIN_WIDTH {
        0
    } else {
        max_width
    }
}

pub(crate) fn file_sidebar_entries(app: &DiffApp) -> Vec<FileSidebarEntry<'_>> {
    let mut entries = Vec::new();
    let mut active_group = None;

    for file_index in app.document.model.visible_files().iter().copied() {
        let Some(file) = app.document.changeset.files.get(file_index.get()) else {
            continue;
        };
        let (group, name) = split_sidebar_path(file.display_path());

        if group != active_group {
            active_group = group;
            if let Some(group) = group {
                entries.push(FileSidebarEntry::Group(group));
            }
        }

        entries.push(FileSidebarEntry::File {
            index: file_index,
            file,
            name,
        });
    }

    entries
}

pub(crate) fn file_sidebar_entry_count(app: &DiffApp) -> usize {
    file_sidebar_entries(app).len()
}

pub(crate) fn file_sidebar_file_row(app: &DiffApp, file: FileIndex) -> Option<usize> {
    file_sidebar_entries(app)
        .iter()
        .position(|entry| matches!(entry, FileSidebarEntry::File { index, .. } if *index == file))
}

pub(crate) fn file_sidebar_file_at_row(app: &DiffApp, row: usize) -> Option<FileIndex> {
    match file_sidebar_entries(app).get(row) {
        Some(FileSidebarEntry::File { index, .. }) => Some(*index),
        Some(FileSidebarEntry::Group(_)) | None => None,
    }
}

fn split_sidebar_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((group, name)) if !group.is_empty() => (Some(group), name),
        _ => (None, path),
    }
}

pub(crate) fn draw_file_sidebar(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Paragraph::new(Text::from(file_sidebar_lines(
            app,
            area.width as usize,
            area.height as usize,
        )))
        .style(Style::default().bg(diff_base_bg(app.config.theme))),
        area,
    );
}

pub(crate) fn file_sidebar_lines(app: &DiffApp, width: usize, height: usize) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let theme = app.config.theme;
    let mut lines = Vec::with_capacity(height);
    let visible_files = height;
    let content_width = width.saturating_sub(1);
    let entries = file_sidebar_entries(app);
    for position in app.sidebar.file_sidebar_scroll
        ..app
            .sidebar
            .file_sidebar_scroll
            .saturating_add(visible_files)
    {
        let Some(entry) = entries.get(position) else {
            lines.push(file_sidebar_line(
                "",
                Style::default().bg(diff_base_bg(theme)),
                width,
                theme,
            ));
            continue;
        };

        lines.push(match entry {
            FileSidebarEntry::Group(group) => file_sidebar_group_line(group, content_width, theme),
            FileSidebarEntry::File {
                index, file, name, ..
            } => file_sidebar_entry_line(
                file,
                name,
                *index == app.sidebar.selected_file,
                content_width,
                theme,
            ),
        });
    }

    lines
}

pub(crate) fn file_sidebar_entry_line(
    file: &mark_diff::DiffFile,
    name: &str,
    selected: bool,
    content_width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let width = content_width.saturating_add(1);
    if width == 0 {
        return Line::default();
    }

    let bg = if selected {
        header_bg(theme)
    } else {
        diff_base_bg(theme)
    };
    let status_style = file_sidebar_status_style(file.status(), bg, theme);
    let body_style = file_sidebar_body_style(selected, bg, theme);

    if file.is_binary() || (file.additions == 0 && file.deletions == 0) {
        let stats = file_sidebar_stats(file);
        let stats_width = stats.width();
        let gap_width = usize::from(!stats.is_empty() && content_width > stats_width);
        let left_width = content_width
            .saturating_sub(stats_width)
            .saturating_sub(gap_width);
        let stats_width = content_width.saturating_sub(left_width + gap_width);

        let mut spans = file_sidebar_left_spans(file, name, left_width, status_style, body_style);
        if gap_width > 0 {
            spans.push(Span::styled(" ", body_style));
        }
        if stats_width > 0 {
            spans.push(Span::styled(fit(&stats, stats_width), body_style));
        }
        let used = spans_width(&spans);
        if used < content_width {
            spans.push(Span::styled(" ".repeat(content_width - used), body_style));
        }
        spans.push(file_sidebar_separator(theme));
        return Line::from(spans);
    }

    let additions = format!("+{}", file.additions);
    let deletions = format!("-{}", file.deletions);
    let stats_width = additions
        .width()
        .saturating_add(1)
        .saturating_add(deletions.width());
    let gap_width = usize::from(content_width > stats_width);
    let left_width = content_width
        .saturating_sub(stats_width)
        .saturating_sub(gap_width);

    let mut spans = Vec::new();
    if left_width > 0 {
        spans.extend(file_sidebar_left_spans(
            file,
            name,
            left_width,
            status_style,
            body_style,
        ));
    }
    if gap_width > 0 {
        spans.push(Span::styled(" ", body_style));
    }

    let mut remaining = content_width.saturating_sub(left_width + gap_width);
    push_sidebar_stat_span(
        &mut spans,
        &additions,
        sidebar_stat_style(theme.addition_fg, selected, bg),
        &mut remaining,
    );
    if remaining > 0 {
        spans.push(Span::styled(" ", body_style));
        remaining -= 1;
    }
    push_sidebar_stat_span(
        &mut spans,
        &deletions,
        sidebar_stat_style(theme.deletion_fg, selected, bg),
        &mut remaining,
    );
    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), body_style));
    }
    spans.push(file_sidebar_separator(theme));

    Line::from(spans)
}

pub(crate) fn push_sidebar_stat_span(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }

    let text = fit(text, *remaining);
    if text.is_empty() {
        return;
    }

    *remaining = (*remaining).saturating_sub(text.width());
    spans.push(Span::styled(text, style));
}

pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.as_ref().width()).sum()
}

pub(crate) fn sidebar_stat_style(color: Color, selected: bool, bg: Color) -> Style {
    let mut style = Style::default().fg(color).bg(bg);
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

pub(crate) fn file_sidebar_status_style(status: FileStatus, bg: Color, theme: DiffTheme) -> Style {
    file_sidebar_style(status, theme)
        .bg(bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn file_sidebar_body_style(selected: bool, bg: Color, theme: DiffTheme) -> Style {
    let mut style = Style::default()
        .fg(if selected {
            theme.header
        } else {
            theme.foreground
        })
        .bg(bg);
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

pub(crate) fn file_sidebar_left_spans(
    file: &mark_diff::DiffFile,
    name: &str,
    width: usize,
    status_style: Style,
    body_style: Style,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let prefix = format!(" {} ", status_code(file.status()));
    let prefix_width = prefix.width();
    if prefix_width >= width {
        return vec![Span::styled(fit_padded(&prefix, width), status_style)];
    }

    let path_width = width - prefix_width;
    vec![
        Span::styled(prefix, status_style),
        Span::styled(
            fit_padded(&fit_with_ellipsis(name, path_width), path_width),
            body_style,
        ),
    ]
}

pub(crate) fn file_sidebar_group_line(
    group: &str,
    content_width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let style = Style::default().fg(theme.muted).bg(diff_base_bg(theme));
    let label = format!(" {group}/");
    let text = fit_padded(&fit_with_ellipsis(&label, content_width), content_width);
    Line::from(vec![
        Span::styled(text, style),
        file_sidebar_separator(theme),
    ])
}

pub(crate) fn file_sidebar_stats(file: &mark_diff::DiffFile) -> String {
    if file.is_binary() {
        "binary".to_owned()
    } else if file.additions == 0 && file.deletions == 0 {
        String::new()
    } else {
        format!("+{} -{}", file.additions, file.deletions)
    }
}

pub(crate) fn file_sidebar_line(
    text: &str,
    style: Style,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    if width == 1 {
        return Line::from(file_sidebar_separator(theme));
    }

    Line::from(vec![
        Span::styled(fit_padded(text, width - 1), style),
        file_sidebar_separator(theme),
    ])
}

pub(crate) fn file_sidebar_separator(theme: DiffTheme) -> Span<'static> {
    let text = if theme.decorations.is_fancy() {
        "│"
    } else {
        " "
    };
    Span::styled(
        text,
        Style::default().fg(theme.muted).bg(diff_base_bg(theme)),
    )
}
