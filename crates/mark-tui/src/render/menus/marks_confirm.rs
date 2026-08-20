use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    prelude::{Line, Modifier, Span, Style, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::DiffApp,
    render::{
        selector_menu::{
            centered_floating_rect, floating_menu_fits_terminal, floating_menu_max_height,
            floating_menu_max_width, selector_border_color, selector_count_color,
            selector_menu_outer_width, selector_title_color,
        },
        style::base_bg,
        text::fit_padded,
    },
    theme::DiffTheme,
};

const TITLE: &str = " Clear all marks? ";
const BODY: &str = "This removes every mark in the review.";
const CONFIRM_HINT: &str = "Enter confirm · Esc cancel";

pub(crate) fn draw_marks_confirm(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let Some(menu_area) = marks_confirm_area(app, area) else {
        return;
    };
    let theme = app.config.theme;
    let bg = base_bg(theme);
    let block = marks_confirm_block(theme);
    let inner = block.inner(menu_area);
    let width = inner.width as usize;
    let lines = vec![
        Line::from(Span::styled(
            fit_padded(BODY, width),
            Style::default().fg(theme.foreground).bg(bg),
        )),
        Line::from(Span::styled(
            fit_padded(CONFIRM_HINT, width),
            Style::default().fg(selector_count_color(theme)).bg(bg),
        )),
    ];
    frame.render_widget(Clear, menu_area);
    frame.render_widget(block, menu_area);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(bg)),
        inner,
    );
}

fn marks_confirm_area(app: &DiffApp, area: Rect) -> Option<Rect> {
    if !app.overlays.marks_confirm_is_open() || !floating_menu_fits_terminal(area) {
        return None;
    }
    let content_width = BODY.width().max(CONFIRM_HINT.width());
    let width = floating_menu_max_width(area, selector_menu_outer_width(content_width.max(28)));
    let height = floating_menu_max_height(area, 4);
    if width == 0 || height == 0 {
        return None;
    }
    Some(centered_floating_rect(area, width, height))
}

fn marks_confirm_block(theme: DiffTheme) -> Block<'static> {
    let bg = base_bg(theme);
    if !theme.decorations.show_borders() {
        return Block::default()
            .style(Style::default().bg(bg))
            .padding(Padding::horizontal(1));
    }
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(selector_border_color(theme)).bg(bg))
        .style(Style::default().bg(bg))
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(
            TITLE,
            Style::default()
                .fg(selector_title_color(theme))
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )))
        .title_alignment(Alignment::Center)
}
