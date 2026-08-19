use ratatui::prelude::{Modifier, Span, Style};

use super::header::push_fitted_statusline_span;
use crate::{app::DiffApp, render::text::format_count, review::VerdictKind};

pub(super) fn push_review_lifecycle_spans(
    spans: &mut Vec<Span<'static>>,
    app: &DiffApp,
    remaining: &mut usize,
) {
    let lifecycle = &app.annotations_state.lifecycle;
    let info_bg = app.config.theme.statusline_info_bg;
    let reviewed = lifecycle
        .reviewed_files
        .len()
        .saturating_add(lifecycle.reviewed_hunks.len());
    if reviewed > 0 {
        push_fitted_statusline_span(spans, "  ", Style::default().bg(info_bg), remaining);
        push_fitted_statusline_span(
            spans,
            format!("{} reviewed", format_count(reviewed)),
            Style::default().fg(app.config.theme.muted).bg(info_bg),
            remaining,
        );
    }
    if let Some(verdict) = lifecycle.verdict.as_ref() {
        let verdict = match verdict.kind {
            VerdictKind::Approve => "approve",
            VerdictKind::RequestChanges => "request changes",
            VerdictKind::Comment => "comment",
        };
        push_fitted_statusline_span(spans, "  ", Style::default().bg(info_bg), remaining);
        push_fitted_statusline_span(
            spans,
            format!("verdict: {verdict}"),
            Style::default()
                .fg(app.config.theme.notice)
                .bg(info_bg)
                .add_modifier(Modifier::BOLD),
            remaining,
        );
    }
}
