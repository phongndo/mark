use ratatui::prelude::{Line, Modifier, Span, Style};

use super::header::push_fitted_statusline_span;
use crate::{annotation::AnnotationTargetMode, app::DiffApp, render::style::statusline_bg};

pub(super) fn annotation_target_header_line(
    app: &DiffApp,
    mode: &AnnotationTargetMode,
    width: usize,
) -> Line<'static> {
    let bg = statusline_bg(app.config.theme);
    let mut remaining = width;
    let mut spans = Vec::new();
    push_fitted_statusline_span(
        &mut spans,
        " ANNOTATE ",
        Style::default()
            .fg(app.config.theme.search_match_fg)
            .bg(app.config.theme.search_match_bg)
            .add_modifier(Modifier::BOLD),
        &mut remaining,
    );
    let detail = if mode.prefix.is_empty() {
        format!("  {} targets · type hint · Esc", mode.targets.len())
    } else {
        let prefix = if app.config.syntax_settings.annotations.uppercase_hints {
            mode.prefix.to_ascii_uppercase()
        } else {
            mode.prefix.clone()
        };
        format!(
            "  {prefix}… · {} matches · Backspace · Esc",
            mode.matching_target_count()
        )
    };
    push_fitted_statusline_span(
        &mut spans,
        detail,
        Style::default().fg(app.config.theme.statusline_fg).bg(bg),
        &mut remaining,
    );
    if remaining > 0 {
        spans.push(Span::styled(" ".repeat(remaining), Style::default().bg(bg)));
    }
    Line::from(spans)
}
