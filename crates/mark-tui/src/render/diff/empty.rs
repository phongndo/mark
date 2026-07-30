use mark_diff::{DiffSource, PatchSource};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    prelude::{Line, Span, Style},
    widgets::{Block, Paragraph},
};

use crate::{app::DiffApp, keymap::GlobalAction, render::style::diff_base_bg};

pub(crate) fn draw_empty_diff(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let background = Style::default().bg(diff_base_bg(app.config.theme));
    frame.render_widget(Block::default().style(background), area);

    let (title, hint) = empty_diff_message(app);
    let mut lines = vec![Line::from(Span::styled(
        title,
        Style::default()
            .fg(app.config.theme.foreground)
            .bg(diff_base_bg(app.config.theme)),
    ))];
    if !hint.is_empty() && area.height > 1 {
        lines.push(Line::from(Span::styled(
            hint,
            Style::default()
                .fg(app.config.theme.muted)
                .bg(diff_base_bg(app.config.theme)),
        )));
    }

    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let message_area = Rect {
        x: area.x,
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width: area.width,
        height,
    };
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(background),
        message_area,
    );
}

pub(crate) fn empty_diff_message(app: &DiffApp) -> (String, String) {
    if app.filters.active() && !app.document.base_changeset.files.is_empty() {
        return (
            "No files match the active filters.".to_owned(),
            action_hints(app, &[(GlobalAction::ClearFilters, "clear filters")]),
        );
    }

    let title = match &app.document.options.source {
        DiffSource::Worktree => "Working tree is clean.",
        DiffSource::Show(_) => "This revision contains no changes.",
        DiffSource::Base(_) | DiffSource::Branch { .. } | DiffSource::Range { .. } => {
            "No changes between these revisions."
        }
        DiffSource::Difftool { .. } => "The compared files are identical.",
        DiffSource::Patch(PatchSource::Review { .. }) => "This review contains no changes.",
        DiffSource::Patch(_) => "This patch contains no changes.",
    };
    (
        title.to_owned(),
        action_hints(
            app,
            &[
                (GlobalAction::DiffMenu, "choose source"),
                (GlobalAction::Help, "help"),
            ],
        ),
    )
}

fn action_hints(app: &DiffApp, actions: &[(GlobalAction, &str)]) -> String {
    actions
        .iter()
        .filter_map(|(action, description)| {
            let label = app.config.keymap.global_action_label(*action);
            (!label.is_empty()).then(|| format!("{label} {description}"))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
