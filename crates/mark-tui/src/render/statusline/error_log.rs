use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style, Text},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    keymap::GlobalAction,
    render::{style::diff_base_bg, text::fit},
    theme::DiffTheme,
};

pub(crate) fn draw_error_log(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let Some(error_log) = app.notifications.error_log.as_deref() else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }

    let bg = diff_base_bg(app.config.theme);
    frame.render_widget(
        Paragraph::new(error_log_header_line(app, area.width as usize))
            .style(Style::default().bg(bg)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    let body_area = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    if body_area.height == 0 {
        return;
    }

    frame.render_widget(
        Paragraph::new(Text::from(error_log.to_owned()))
            .style(Style::default().fg(app.config.theme.foreground).bg(bg))
            .wrap(Wrap { trim: false }),
        body_area,
    );
}

pub(crate) fn error_log_header_line(app: &DiffApp, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let bg = diff_base_bg(app.config.theme);
    let title = "error ";
    let title_width = title.width();
    let rule_style = Style::default()
        .fg(app.config.theme.deletion_fg)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    if width <= title_width {
        return Line::from(Span::styled(fit(title, width), rule_style));
    }

    let copy_label = error_log_copy_label(app);
    let copy_width = copy_label.width();
    if copy_width == 0 || title_width.saturating_add(copy_width) >= width {
        return Line::from(Span::styled(
            error_log_separator_for_theme(width, app.config.theme),
            rule_style,
        ));
    }

    let rule_width = width.saturating_sub(title_width).saturating_sub(copy_width);
    Line::from(vec![
        Span::styled(title.to_owned(), rule_style),
        Span::styled(error_log_rule(rule_width, app.config.theme), rule_style),
        Span::styled(copy_label, rule_style),
    ])
}

fn error_log_copy_label(app: &DiffApp) -> String {
    let key = app
        .config
        .keymap
        .global_action_label(GlobalAction::CopyErrorLog);
    if key == "unbound" {
        String::new()
    } else {
        format!(" [Copy All ({key})]")
    }
}

#[cfg(test)]
pub(crate) fn error_log_separator(width: usize) -> String {
    error_log_separator_with_rule(width, "─")
}

pub(crate) fn error_log_separator_for_theme(width: usize, theme: DiffTheme) -> String {
    error_log_separator_with_rule(width, theme.decorations.horizontal_rule())
}

fn error_log_separator_with_rule(width: usize, rule: &str) -> String {
    let title = "error ";
    if width == 0 {
        return String::new();
    }
    if width <= title.width() {
        return fit(title, width);
    }
    let right = width.saturating_sub(title.width());
    format!("{title}{}", rule.repeat(right))
}

fn error_log_rule(width: usize, theme: DiffTheme) -> String {
    theme.decorations.horizontal_rule().repeat(width)
}

#[cfg(test)]
pub(crate) fn error_log_height(app: &DiffApp, available_height: u16) -> u16 {
    if app.notifications.error_log.is_none() || available_height == 0 {
        return 0;
    }

    app.notifications
        .error_log_height
        .clamp(
            crate::app::ERROR_LOG_MIN_HEIGHT,
            crate::app::ERROR_LOG_MAX_HEIGHT,
        )
        .min(available_height)
}
