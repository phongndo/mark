use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{AnnotationMenuItem, DiffApp},
    render::{
        selector_menu::{
            centered_floating_rect, floating_menu_fits_terminal, floating_menu_max_inner_height,
            floating_menu_max_width, selector_empty_line, selector_input_line, selector_menu_block,
            selector_menu_list_rows, selector_menu_outer_height, selector_menu_outer_width,
            selector_row_style, selector_separator_line,
        },
        style::base_bg,
        text::{fit, fit_padded, fit_with_ellipsis, terminal_text},
    },
    theme::DiffTheme,
};

pub(crate) fn draw_annotation_menu(frame: &mut Frame<'_>, app: &DiffApp, area: Rect) {
    let items = app.filtered_annotation_menu_items();
    let Some(menu_area) = annotation_menu_area(app, area, &items) else {
        return;
    };
    let block = annotation_menu_block(app);
    let inner = block.inner(menu_area);
    let selected = app
        .overlays
        .annotation_menu
        .selected
        .min(items.len().saturating_sub(1));
    let content = annotation_menu_lines(app, &items, inner, selected);
    frame.render_widget(Clear, menu_area);
    frame.render_widget(block, menu_area);
    frame.render_widget(
        Paragraph::new(content).style(Style::default().bg(base_bg(app.config.theme))),
        inner,
    );
}

fn annotation_menu_lines(
    app: &DiffApp,
    items: &[AnnotationMenuItem],
    inner: Rect,
    selected: usize,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let width = inner.width as usize;
    let menu = &app.overlays.annotation_menu;
    let total = app.annotation_menu_items().len();
    let checked = app.overlays.annotation_selection.len();
    let mut lines = vec![
        if app.overlays.annotation_menu_filtering || !menu.input.is_empty() {
            selector_input_line(
                &menu.input,
                menu.input_cursor,
                width,
                theme,
                items.len(),
                total,
            )
        } else {
            let count = match checked {
                0 if total == 1 => "1 mark".to_owned(),
                0 => format!("{total} marks"),
                n => format!("{n} of {total} selected"),
            };
            hint_line(" / filter", &count, width, theme)
        },
    ];
    lines.push(selector_separator_line(width, theme));

    let visible_items = annotation_menu_visible_items(inner.height).max(1);
    if items.is_empty() {
        lines.push(selector_empty_line(" no matching marks", width, theme));
    }
    for (index, item) in items
        .iter()
        .enumerate()
        .skip(menu.scroll)
        .take(visible_items)
    {
        lines.extend(annotation_menu_item_lines(
            item,
            width,
            theme,
            index == selected,
            app.annotations_state
                .annotations
                .has_human(&item.key)
                .then(|| app.overlays.annotation_selection.contains(&item.key)),
        ));
    }
    lines.push(selector_separator_line(width, theme));
    let footer = if app.overlays.annotation_menu_filtering {
        " type to filter  ⏎ done  esc clear".to_owned()
    } else {
        let send = if checked == 0 {
            "send"
        } else {
            "send selected"
        };
        format!(" space select  a all  ⏎ edit  d delete  ^S {send}  Q send all  esc close")
    };
    lines.push(Line::from(Span::styled(
        fit_padded(&footer, width),
        Style::default().fg(theme.muted).bg(base_bg(theme)),
    )));
    lines
}

fn hint_line(left: &str, right: &str, width: usize, theme: DiffTheme) -> Line<'static> {
    let style = Style::default().fg(theme.muted).bg(base_bg(theme));
    let left_width = width.saturating_sub(right.width());
    Line::from(vec![
        Span::styled(fit_padded(left, left_width), style),
        Span::styled(fit(right, width.saturating_sub(left_width)), style),
    ])
}

pub(crate) fn annotation_menu_area(
    app: &DiffApp,
    area: Rect,
    items: &[AnnotationMenuItem],
) -> Option<Rect> {
    if !app.overlays.annotation_menu_is_open() || !floating_menu_fits_terminal(area) {
        return None;
    }
    let list_cap = annotation_menu_visible_items(floating_menu_max_inner_height(area));
    let list_rows = items.len().max(1).min(list_cap).saturating_mul(2);
    let width = floating_menu_max_width(area, annotation_menu_width(app));
    let height = selector_menu_outer_height(area, list_rows, 2);
    if width == 0 || height == 0 {
        return None;
    }
    Some(centered_floating_rect(area, width, height))
}

pub(crate) fn annotation_menu_list_visible_rows(app: &DiffApp, area: Rect) -> Option<usize> {
    let items = app.filtered_annotation_menu_items();
    let menu_area = annotation_menu_area(app, area, &items)?;
    let inner = annotation_menu_block(app).inner(menu_area);
    Some(annotation_menu_visible_items(inner.height))
}

fn annotation_menu_visible_items(inner_height: u16) -> usize {
    selector_menu_list_rows(inner_height, 2)
        .saturating_div(2)
        .max(1)
}

pub(crate) fn annotation_menu_block(app: &DiffApp) -> Block<'static> {
    selector_menu_block(app.config.theme, " Marks ", base_bg(app.config.theme))
}

fn annotation_menu_width(app: &DiffApp) -> u16 {
    let rows = app
        .annotation_menu_items()
        .iter()
        .map(|item| {
            let label = terminal_text(&item.label).width();
            let body = terminal_text(&item.preview).width();
            label.max(body).saturating_add(8)
        })
        .max()
        .unwrap_or(0);
    selector_menu_outer_width(rows.clamp(84, 110))
}

fn annotation_menu_item_lines(
    item: &AnnotationMenuItem,
    width: usize,
    theme: DiffTheme,
    highlighted: bool,
    checked: Option<bool>,
) -> [Line<'static>; 2] {
    let style = selector_row_style(theme, highlighted);
    let pointer = if highlighted { "▸ " } else { "  " };
    let pointer_style = style.fg(theme.addition_fg);
    let (mark, mark_style) = match checked {
        Some(true) => (
            "● ",
            style.fg(theme.addition_fg).add_modifier(Modifier::BOLD),
        ),
        Some(false) => ("○ ", style.fg(theme.muted)),
        None => ("  ", style),
    };
    let label = terminal_text(&item.label);
    let header_width = width.saturating_sub(4);
    let body = flattened_note(&item.text);
    let body_style = if highlighted {
        style.remove_modifier(Modifier::BOLD).fg(theme.foreground)
    } else {
        style.fg(theme.muted)
    };
    [
        Line::from(vec![
            Span::styled(pointer, pointer_style),
            Span::styled(mark, mark_style),
            Span::styled(fit_padded(&label, header_width), style),
        ]),
        Line::from(vec![
            Span::styled("    ", style),
            Span::styled(
                fit_padded(&fit_with_ellipsis(&body, header_width), header_width),
                body_style,
            ),
        ]),
    ]
}

// One line per item keeps rows aligned; multi-line notes read as one sentence.
fn flattened_note(text: &str) -> String {
    let words = terminal_text(&text.split_whitespace().collect::<Vec<_>>().join(" "));
    if words.is_empty() {
        "(empty mark)".to_owned()
    } else {
        words
    }
}
