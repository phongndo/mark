use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use unicode_width::UnicodeWidthStr;

use crate::{
    annotation::{
        ANNOTATION_ADD_BUTTON, ANNOTATION_ADD_BUTTON_WIDTH, ANNOTATION_CLOSE_BUTTON,
        ANNOTATION_CLOSE_BUTTON_WIDTH, ANNOTATION_EDIT_BUTTON, ANNOTATION_EDIT_BUTTON_ASCII,
        ANNOTATION_EDIT_BUTTON_WIDTH, ANNOTATION_SUBMIT_BUTTON, ANNOTATION_SUBMIT_BUTTON_ASCII,
        ANNOTATION_SUBMIT_BUTTON_WIDTH, AnnotationDraft,
    },
    controls::INPUT_CURSOR,
    render::style::{base_bg, input_cursor_style, spans_with_input_cursor},
    render::text::{fit, fit_byte_prefix_with_width, fit_padded, spaces, terminal_text},
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

fn annotation_top_border_line(
    width: usize,
    theme: DiffTheme,
    label: Option<&str>,
) -> Line<'static> {
    if width < ANNOTATION_CLOSE_BUTTON_WIDTH {
        return Line::from(Span::styled(
            fit(theme.decorations.horizontal_rule(), width),
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
                annotation_rule(fill_width, theme),
                annotation_border_style(theme),
            ));
        }
    } else if rule_width > 0 {
        spans.push(Span::styled(
            annotation_rule(rule_width, theme),
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
        return Line::from(Span::styled(annotation_rule(width, theme), style));
    }
    if width == 0 {
        return Line::default();
    }

    let label = annotation_footer_button_label(width, button, theme);
    let label_width = label.width();
    let left = width.saturating_sub(label_width);
    let button_fg = match button {
        AnnotationFooterButton::None => theme.hunk,
        AnnotationFooterButton::Edit => theme.search_match_bg,
        AnnotationFooterButton::Submit => theme.addition_fg,
    };
    Line::from(vec![
        Span::styled(annotation_rule(left, theme), style),
        Span::styled(
            label,
            Style::default()
                .fg(button_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn annotation_rule(width: usize, theme: DiffTheme) -> String {
    if theme.decorations.is_fancy() {
        theme.decorations.horizontal_rule().repeat(width)
    } else {
        spaces(width).into_owned()
    }
}

fn annotation_footer_button_label(
    width: usize,
    button: AnnotationFooterButton,
    theme: DiffTheme,
) -> String {
    match button {
        AnnotationFooterButton::None => String::new(),
        AnnotationFooterButton::Edit => {
            if width >= ANNOTATION_EDIT_BUTTON_WIDTH && theme.decorations.is_fancy() {
                ANNOTATION_EDIT_BUTTON.to_owned()
            } else {
                fit(ANNOTATION_EDIT_BUTTON_ASCII, width)
            }
        }
        AnnotationFooterButton::Submit => {
            if width >= ANNOTATION_SUBMIT_BUTTON_WIDTH && theme.decorations.is_fancy() {
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
    let bg = base_bg(theme);
    let display = fit_padded(text, body_width);
    if display.contains(INPUT_CURSOR) {
        let text_style = Style::default().fg(fg).bg(bg);
        return Line::from(spans_with_input_cursor(
            &display,
            text_style,
            input_cursor_style(theme, bg),
            theme.decorations.input_cursor(),
        ));
    }
    Line::from(Span::styled(display, Style::default().fg(fg).bg(bg)))
}

fn annotation_display_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    visit_annotation_display_lines(text, annotation_body_width(width), |line| {
        lines.push(line.to_owned());
    });
    lines
}

fn annotation_display_line_count(text: &str, width: usize) -> usize {
    visit_annotation_display_lines(text, annotation_body_width(width), |_| {})
}

fn visit_annotation_display_lines(text: &str, width: usize, mut visit: impl FnMut(&str)) -> usize {
    if width == 0 {
        visit("");
        return 1;
    }

    let mut line_count = 0usize;
    for paragraph in text.split('\n') {
        // Wrap terminal-safe text so expanded tabs/control escapes can be
        // split across visual line boundaries without re-rendering bytes.
        let display_paragraph = terminal_text(paragraph);
        visit_annotation_paragraph(&display_paragraph, width, &mut |line| {
            line_count = line_count.saturating_add(1);
            visit(line);
        });
    }
    if line_count == 0 {
        visit("");
        return 1;
    }
    line_count
}

fn visit_annotation_paragraph(paragraph: &str, width: usize, visit: &mut impl FnMut(&str)) {
    if paragraph.is_empty() {
        visit("");
        return;
    }

    let mut rest = paragraph;
    while !rest.is_empty() {
        let (fit_len, _, complete) = fit_byte_prefix_with_width(rest, width);
        if complete {
            visit(rest);
            break;
        }

        let break_len = annotation_wrap_boundary(rest, fit_len).unwrap_or(fit_len);
        if break_len == 0 {
            let Some(character) = rest.chars().next() else {
                break;
            };
            let character_len = character.len_utf8();
            visit(&rest[..character_len]);
            rest = &rest[character_len..];
            continue;
        }

        visit(&rest[..break_len]);
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
    annotation_display_line_count(text, width).saturating_add(2)
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
    annotation_display_line_count(&display, width).saturating_add(2)
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

#[cfg(test)]
mod tests {
    use super::{annotation_display_line_count, annotation_display_lines};

    #[test]
    fn count_only_annotation_wrapping_matches_rendered_lines() {
        for text in [
            "",
            "one two three four",
            "first\n\nthird",
            "wide 👩‍💻 text",
            "tab\there and control \u{1b}[31m",
        ] {
            for width in [0, 1, 4, 8, 40] {
                assert_eq!(
                    annotation_display_line_count(text, width),
                    annotation_display_lines(text, width).len(),
                    "text={text:?}, width={width}",
                );
            }
        }
    }
}
