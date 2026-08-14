use std::{borrow::Cow, path::Path};

use mark_diff::{DiffSource, PatchSource};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use super::{
    annotation_target::annotation_target_header_line, review_lifecycle::push_review_lifecycle_spans,
};
use crate::{
    app::DiffApp,
    controls::BranchMenu,
    keymap::GlobalAction,
    render::{
        menus::{diff_comparison_label_for_theme, diff_selector_text},
        style::statusline_bg,
        text::{
            display_char_supports_partial_render, display_width, fit, fit_with_ellipsis,
            format_count, progress_label,
        },
    },
    theme::STATUSLINE_SELECTOR_GAP,
};

pub(crate) fn draw_header(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let line = statusline_header_line(app, area.width as usize);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(statusline_bg(app.config.theme))),
        area,
    );
}

pub(crate) fn statusline_header_line(app: &DiffApp, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if let Some(mode) = app.annotations_state.annotation_target_mode.as_ref() {
        return annotation_target_header_line(app, mode, width);
    }

    let right_max_width = statusline_right_max_width(width);
    let right = statusline_file_spans(app, right_max_width);
    let right_width = right.iter().map(|span| span.content.width()).sum::<usize>();
    let mut left_width = width.saturating_sub(right_width);
    let mut spans = Vec::new();

    push_statusline_left_spans(&mut spans, app, &mut left_width);
    let left_used = width.saturating_sub(right_width).saturating_sub(left_width);
    let gap = width.saturating_sub(left_used).saturating_sub(right_width);
    if gap > 0 {
        spans.push(Span::styled(
            " ".repeat(gap),
            Style::default().bg(statusline_bg(app.config.theme)),
        ));
    }
    spans.extend(right);

    Line::from(spans)
}

pub(crate) fn push_statusline_left_spans(
    spans: &mut Vec<Span<'static>>,
    app: &DiffApp,
    remaining: &mut usize,
) {
    let selector_text = Cow::Owned(diff_selector_text(&app.document.options));
    push_fitted_statusline_span(spans, selector_text, statusline_mode_style(app), remaining);
    let info_bg = app.config.theme.statusline_info_bg;
    push_fitted_statusline_span(
        spans,
        STATUSLINE_SELECTOR_GAP,
        Style::default().bg(info_bg),
        remaining,
    );
    if app.is_show_diff()
        && let Some(commit) = app.commit_selector_text()
    {
        push_fitted_statusline_span(spans, commit, statusline_info_style(app), remaining);
    } else if app.is_branch_diff()
        && let (Some(head), Some(base)) = (
            app.branch_selector_text(BranchMenu::Head),
            app.branch_selector_text(BranchMenu::Base),
        )
    {
        push_fitted_statusline_span(spans, head, statusline_info_style(app), remaining);
        push_fitted_statusline_span(
            spans,
            app.config.theme.decorations.comparison_separator(),
            Style::default().fg(app.config.theme.muted).bg(info_bg),
            remaining,
        );
        push_fitted_statusline_span(spans, base, statusline_info_style(app), remaining);
    } else {
        let source = match &app.document.options.source {
            DiffSource::Worktree => Cow::Borrowed("HEAD"),
            DiffSource::Patch(PatchSource::Review { label, .. }) => Cow::Owned(
                label
                    .as_str()
                    .strip_prefix("review ")
                    .unwrap_or(label.as_str())
                    .to_owned(),
            ),
            _ => Cow::Owned(diff_comparison_label_for_theme(
                &app.document.options,
                app.config.theme,
            )),
        };
        push_fitted_statusline_span(spans, source, statusline_info_style(app), remaining);
    }
    push_fitted_statusline_span(spans, "  ", Style::default().bg(info_bg), remaining);
    push_fitted_statusline_span(
        spans,
        format!("+{}", format_count(app.document.stats.additions)),
        Style::default()
            .fg(app.config.theme.addition_fg)
            .bg(info_bg)
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    push_fitted_statusline_span(spans, " ", Style::default().bg(info_bg), remaining);
    push_fitted_statusline_span(
        spans,
        format!("-{}", format_count(app.document.stats.deletions)),
        Style::default()
            .fg(app.config.theme.deletion_fg)
            .bg(info_bg)
            .add_modifier(Modifier::BOLD),
        remaining,
    );
    if app.jobs.source_changed {
        let reload_key = app.config.keymap.global_action_label(GlobalAction::Reload);
        push_fitted_statusline_span(spans, "  ", Style::default().bg(info_bg), remaining);
        push_fitted_statusline_span(
            spans,
            format!("source changed · {reload_key} reload"),
            Style::default()
                .fg(app.config.theme.notice)
                .bg(info_bg)
                .add_modifier(Modifier::BOLD),
            remaining,
        );
    }
    push_review_lifecycle_spans(spans, app, remaining);
    let annotation_count = app.annotations_state.annotations.len();
    if annotation_count > 0 {
        push_fitted_statusline_span(spans, "  ", Style::default().bg(info_bg), remaining);
        push_fitted_statusline_span(
            spans,
            format!(
                "{} {}",
                format_count(annotation_count),
                if annotation_count == 1 {
                    "note"
                } else {
                    "notes"
                }
            ),
            Style::default()
                .fg(app.config.theme.notice)
                .bg(info_bg)
                .add_modifier(Modifier::BOLD),
            remaining,
        );
    }
    push_fitted_statusline_span(spans, " ", Style::default().bg(info_bg), remaining);
}

pub(crate) fn push_fitted_statusline_span(
    spans: &mut Vec<Span<'static>>,
    text: impl Into<Cow<'static, str>>,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }

    let text = text.into();
    let text_width = display_width(text.as_ref());
    if text_width <= *remaining && !text.chars().any(display_char_supports_partial_render) {
        *remaining -= text_width;
        if !text.is_empty() {
            spans.push(Span::styled(text, style));
        }
        return;
    }

    let text = fit(text.as_ref(), *remaining);
    if !text.is_empty() {
        *remaining = (*remaining).saturating_sub(text.width());
        spans.push(Span::styled(text, style));
    }
}

pub(crate) fn statusline_right_max_width(width: usize) -> usize {
    if width <= 24 {
        width
    } else {
        (width / 2).max(24).min(width)
    }
}

fn statusline_mode_style(app: &DiffApp) -> Style {
    Style::default()
        .fg(app.config.theme.statusline_accent_fg)
        .bg(app.config.theme.statusline_accent_bg)
        .add_modifier(Modifier::BOLD)
}

fn statusline_info_style(app: &DiffApp) -> Style {
    Style::default()
        .fg(app.config.theme.statusline_info_fg)
        .bg(app.config.theme.statusline_info_bg)
        .add_modifier(Modifier::BOLD)
}

fn statusline_file_spans(app: &DiffApp, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }

    let file_index = app
        .annotation_cursor_target()
        .and_then(|target| app.document.model.file_at_row(target.model_row_index))
        .unwrap_or(app.sidebar.selected_file.get());
    let file_name = app
        .document
        .changeset
        .files
        .get(file_index)
        .map(|file| file.display_path())
        .and_then(|path| Path::new(path).file_name().and_then(|name| name.to_str()))
        .unwrap_or("No file");
    let file = if max_width >= 2 {
        format!(
            " {} ",
            fit_with_ellipsis(file_name, max_width.saturating_sub(2))
        )
    } else {
        fit(file_name, max_width)
    };
    let remaining = max_width.saturating_sub(file.width());
    let progress = format!(
        " {} ",
        progress_label(app.viewport.scroll, app.max_scroll())
    );

    let mut spans = Vec::with_capacity(2);
    if progress.width() <= remaining {
        spans.push(Span::styled(progress, statusline_info_style(app)));
    }
    if !file.is_empty() {
        spans.push(Span::styled(file, statusline_mode_style(app)));
    }
    spans
}
