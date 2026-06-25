use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use unicode_width::UnicodeWidthStr;

use crate::{
    annotation::{
        ANNOTATION_ADD_BUTTON, ANNOTATION_ADD_BUTTON_WIDTH, ANNOTATION_CLOSE_BUTTON,
        ANNOTATION_CLOSE_BUTTON_WIDTH, ANNOTATION_EDIT_BUTTON, ANNOTATION_EDIT_BUTTON_ASCII,
        ANNOTATION_EDIT_BUTTON_WIDTH, ANNOTATION_SUBMIT_BUTTON, ANNOTATION_SUBMIT_BUTTON_ASCII,
        ANNOTATION_SUBMIT_BUTTON_WIDTH, AnnotationDraft, AnnotationSide,
    },
    controls::INPUT_CURSOR,
    render::style::base_bg,
    render::text::{fit, fit_padded, fit_with_width, skip_display_prefix, spaces},
    theme::DiffTheme,
};

fn annotation_border_style(theme: DiffTheme) -> Style {
    Style::default()
        .fg(theme.hunk)
        .bg(base_bg(theme))
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn append_annotation_add_button(
    line: Line<'static>,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    if width < ANNOTATION_ADD_BUTTON_WIDTH {
        return line;
    }

    let content_width = width.saturating_sub(ANNOTATION_ADD_BUTTON_WIDTH);
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        if used >= content_width {
            break;
        }
        let text = span.content.as_ref();
        let span_width = text.width();
        if used + span_width <= content_width {
            out.push(span);
            used += span_width;
            continue;
        }
        let remaining = content_width.saturating_sub(used);
        out.push(Span::styled(fit_padded(text, remaining), span.style));
        used = content_width;
        break;
    }
    let cursorline_bg = theme.cursor_line_bg;
    if used < content_width {
        out.push(Span::styled(
            spaces(content_width - used),
            Style::default().bg(cursorline_bg),
        ));
    }
    out.push(Span::styled(
        ANNOTATION_ADD_BUTTON.to_owned(),
        Style::default()
            .fg(theme.hunk)
            .bg(cursorline_bg)
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(out)
}

pub(crate) fn append_split_annotation_add_button(
    line: Line<'static>,
    width: usize,
    side: AnnotationSide,
    theme: DiffTheme,
) -> Line<'static> {
    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    match side {
        AnnotationSide::Old if left_width >= ANNOTATION_ADD_BUTTON_WIDTH => {
            append_annotation_add_button_ending_at(line, left_width, theme)
        }
        AnnotationSide::New if right_width >= ANNOTATION_ADD_BUTTON_WIDTH => {
            append_annotation_add_button(line, width, theme)
        }
        _ => line,
    }
}

fn append_annotation_add_button_ending_at(
    line: Line<'static>,
    end_width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    let (prefix, suffix) = split_line_at_width(line.spans, end_width);
    let mut spans = append_annotation_add_button(Line::from(prefix), end_width, theme).spans;
    spans.extend(suffix);
    Line::from(spans)
}

fn split_line_at_width(
    spans: Vec<Span<'static>>,
    width: usize,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    if width == 0 {
        return (Vec::new(), spans);
    }

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut used = 0usize;
    let mut split = false;

    for span in spans {
        if split {
            right.push(span);
            continue;
        }

        let span_width = span.content.as_ref().width();
        if used.saturating_add(span_width) <= width {
            used = used.saturating_add(span_width);
            left.push(span);
            if used == width {
                split = true;
            }
            continue;
        }

        let remaining = width.saturating_sub(used);
        let style = span.style;
        let text = span.content.into_owned();
        let (left_text, left_width, _) = fit_with_width(&text, remaining);
        if !left_text.is_empty() {
            left.push(Span::styled(left_text, style));
        }
        if left_width < remaining {
            left.push(Span::styled(spaces(remaining - left_width), style));
        }
        let (right_text, _) = skip_display_prefix(&text, remaining);
        if !right_text.is_empty() {
            right.push(Span::styled(right_text.to_owned(), style));
        }
        split = true;
    }

    (left, right)
}

fn annotation_top_border_line(
    width: usize,
    theme: DiffTheme,
    label: Option<&str>,
) -> Line<'static> {
    if width < ANNOTATION_CLOSE_BUTTON_WIDTH {
        return Line::from(Span::styled(
            fit("─", width),
            annotation_border_style(theme),
        ));
    }

    let rule_width = width.saturating_sub(ANNOTATION_CLOSE_BUTTON_WIDTH);
    let mut spans = Vec::with_capacity(2);
    if let Some(label) = label {
        let label = format!("{label} ");
        let label = fit(&label, rule_width);
        let label_width = label.width();
        if label_width > 0 {
            spans.push(Span::styled(
                label,
                Style::default().fg(theme.foreground).bg(base_bg(theme)),
            ));
        }
        let fill_width = rule_width.saturating_sub(label_width);
        if fill_width > 0 {
            spans.push(Span::styled(
                "─".repeat(fill_width),
                annotation_border_style(theme),
            ));
        }
    } else if rule_width > 0 {
        spans.push(Span::styled(
            "─".repeat(rule_width),
            annotation_border_style(theme),
        ));
    }
    spans.push(Span::styled(
        ANNOTATION_CLOSE_BUTTON.to_owned(),
        Style::default()
            .fg(theme.deletion_fg)
            .bg(base_bg(theme))
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationFooterButton {
    None,
    Edit,
    Submit,
}

fn annotation_bottom_border_line(
    width: usize,
    theme: DiffTheme,
    button: AnnotationFooterButton,
) -> Line<'static> {
    let style = annotation_border_style(theme);
    if button == AnnotationFooterButton::None {
        return Line::from(Span::styled("─".repeat(width), style));
    }
    if width == 0 {
        return Line::default();
    }

    let label = annotation_footer_button_label(width, button);
    let label_width = label.width();
    let left = width.saturating_sub(label_width);
    let button_fg = match button {
        AnnotationFooterButton::None => theme.hunk,
        AnnotationFooterButton::Edit => theme.search_match_bg,
        AnnotationFooterButton::Submit => theme.addition_fg,
    };
    Line::from(vec![
        Span::styled("─".repeat(left), style),
        Span::styled(
            label,
            Style::default()
                .fg(button_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn annotation_footer_button_label(width: usize, button: AnnotationFooterButton) -> String {
    match button {
        AnnotationFooterButton::None => String::new(),
        AnnotationFooterButton::Edit => {
            if width >= ANNOTATION_EDIT_BUTTON_WIDTH {
                ANNOTATION_EDIT_BUTTON.to_owned()
            } else {
                fit(ANNOTATION_EDIT_BUTTON_ASCII, width)
            }
        }
        AnnotationFooterButton::Submit => {
            if width >= ANNOTATION_SUBMIT_BUTTON_WIDTH {
                ANNOTATION_SUBMIT_BUTTON.to_owned()
            } else {
                fit(ANNOTATION_SUBMIT_BUTTON_ASCII, width)
            }
        }
    }
}

fn annotation_body_width(width: usize) -> usize {
    width
}

fn annotation_body_line(text: &str, width: usize, theme: DiffTheme, fg: Color) -> Line<'static> {
    let body_width = annotation_body_width(width);
    Line::from(Span::styled(
        fit_padded(text, body_width),
        Style::default().fg(fg).bg(base_bg(theme)),
    ))
}

fn annotation_display_lines(text: &str, width: usize) -> Vec<String> {
    let body_width = annotation_body_width(width);
    wrap_annotation_text(text, body_width)
}

fn wrap_annotation_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        wrap_annotation_paragraph(paragraph, width, &mut lines);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_annotation_paragraph(paragraph: &str, width: usize, lines: &mut Vec<String>) {
    if paragraph.is_empty() {
        lines.push(String::new());
        return;
    }

    let mut rest = paragraph;
    while !rest.is_empty() {
        let (segment, _, complete) = fit_with_width(rest, width);
        if complete {
            lines.push(segment);
            break;
        }

        let break_len = annotation_wrap_boundary(rest, segment.len()).unwrap_or(segment.len());
        if break_len == 0 {
            let Some(character) = rest.chars().next() else {
                break;
            };
            let character_len = character.len_utf8();
            lines.push(rest[..character_len].to_owned());
            rest = &rest[character_len..];
            continue;
        }

        lines.push(rest[..break_len].to_owned());
        rest = &rest[break_len..];
    }
}

fn annotation_wrap_boundary(text: &str, fit_len: usize) -> Option<usize> {
    let mut seen_content = false;
    let mut boundary = None;
    for (index, character) in text[..fit_len].char_indices() {
        if character.is_whitespace() {
            if seen_content {
                boundary = Some(index + character.len_utf8());
            }
        } else {
            seen_content = true;
        }
    }
    boundary
}

pub(crate) fn render_annotation_saved_block(
    text: &str,
    width: usize,
    theme: DiffTheme,
    label: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = vec![annotation_top_border_line(width, theme, label)];
    for line in annotation_display_lines(text, width) {
        lines.push(annotation_body_line(&line, width, theme, theme.muted));
    }
    lines.push(annotation_bottom_border_line(
        width,
        theme,
        AnnotationFooterButton::Edit,
    ));
    lines
}

pub(crate) fn annotation_saved_block_height(text: &str, width: usize) -> usize {
    annotation_display_lines(text, width)
        .len()
        .saturating_add(2)
}

pub(crate) fn render_annotation_compose_block(
    draft: &AnnotationDraft,
    width: usize,
    theme: DiffTheme,
    label: Option<&str>,
) -> Vec<Line<'static>> {
    let display = text_with_cursor(&draft.input, draft.cursor);
    let mut lines = vec![annotation_top_border_line(width, theme, label)];
    for line in annotation_display_lines(&display, width) {
        lines.push(annotation_body_line(&line, width, theme, theme.foreground));
    }
    lines.push(annotation_bottom_border_line(
        width,
        theme,
        AnnotationFooterButton::Submit,
    ));
    lines
}

pub(crate) fn annotation_compose_block_height(draft: &AnnotationDraft, width: usize) -> usize {
    let display = text_with_cursor(&draft.input, draft.cursor);
    annotation_display_lines(&display, width)
        .len()
        .saturating_add(2)
}

fn text_with_cursor(input: &str, cursor: usize) -> String {
    let cursor = cursor.min(input.len());
    if input.is_char_boundary(cursor) {
        format!("{}{}{}", &input[..cursor], INPUT_CURSOR, &input[cursor..])
    } else {
        format!("{input}{INPUT_CURSOR}")
    }
}

pub(crate) fn annotation_hit_at_column(column: u16, width: usize) -> bool {
    let width = width as u16;
    if width < ANNOTATION_ADD_BUTTON_WIDTH as u16 {
        return false;
    }
    let start = width.saturating_sub(ANNOTATION_ADD_BUTTON_WIDTH as u16);
    column >= start
}

pub(crate) fn split_annotation_side_at_column(column: u16, width: usize) -> Option<AnnotationSide> {
    let width = width.min(usize::from(u16::MAX)) as u16;
    if width == 0 || column >= width {
        return None;
    }
    let left_width = width / 2;
    if column < left_width {
        Some(AnnotationSide::Old)
    } else {
        Some(AnnotationSide::New)
    }
}

pub(crate) fn split_annotation_hit_side_at_column(
    column: u16,
    width: usize,
) -> Option<AnnotationSide> {
    let side = split_annotation_side_at_column(column, width)?;
    let width = width.min(usize::from(u16::MAX)) as u16;
    let left_width = width / 2;
    let right_width = width.saturating_sub(left_width);
    let (cell_start, cell_width) = match side {
        AnnotationSide::Old => (0, left_width),
        AnnotationSide::New => (left_width, right_width),
    };
    let button_width = ANNOTATION_ADD_BUTTON_WIDTH as u16;
    if cell_width < button_width {
        return None;
    }
    let start = cell_start.saturating_add(cell_width.saturating_sub(button_width));
    let end = cell_start.saturating_add(cell_width);
    (column >= start && column < end).then_some(side)
}

pub(crate) fn annotation_close_hit_at_column(column: u16, width: usize) -> bool {
    let width = width as u16;
    if width < ANNOTATION_CLOSE_BUTTON_WIDTH as u16 {
        return false;
    }
    let start = width.saturating_sub(ANNOTATION_CLOSE_BUTTON_WIDTH as u16);
    column >= start
}

pub(crate) fn annotation_submit_hit_at_column(column: u16, width: usize) -> bool {
    let width = width as u16;
    if width < ANNOTATION_SUBMIT_BUTTON_WIDTH as u16 {
        return false;
    }
    let start = width.saturating_sub(ANNOTATION_SUBMIT_BUTTON_WIDTH as u16);
    column >= start
}

pub(crate) fn annotation_edit_hit_at_column(column: u16, width: usize) -> bool {
    let width = width as u16;
    if width < ANNOTATION_EDIT_BUTTON_WIDTH as u16 {
        return false;
    }
    let start = width.saturating_sub(ANNOTATION_EDIT_BUTTON_WIDTH as u16);
    column >= start
}
