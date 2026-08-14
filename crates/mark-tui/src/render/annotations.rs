use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use unicode_width::UnicodeWidthStr;

use crate::{
    annotation::{
        ANNOTATION_CLOSE_BUTTON, ANNOTATION_CLOSE_BUTTON_WIDTH, ANNOTATION_EDIT_BUTTON,
        ANNOTATION_EDIT_BUTTON_ASCII, ANNOTATION_EDIT_BUTTON_WIDTH, ANNOTATION_SUBMIT_BUTTON,
        ANNOTATION_SUBMIT_BUTTON_ASCII, ANNOTATION_SUBMIT_BUTTON_WIDTH, AnnotationDraft,
    },
    controls::INPUT_CURSOR,
    render::{
        style::{base_bg, input_cursor_style, spans_with_input_cursor},
        text::{fit, fit_byte_prefix_with_width, fit_padded, spaces, terminal_text},
    },
    theme::DiffTheme,
};

fn annotation_border_style(theme: DiffTheme) -> Style {
    Style::default()
        .fg(theme.hunk)
        .bg(base_bg(theme))
        .add_modifier(Modifier::BOLD)
}

fn annotation_top_border_line(
    width: usize,
    theme: DiffTheme,
    label: Option<&str>,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if width == 1 {
        return Line::from(Span::styled(
            annotation_rule(1, theme),
            annotation_border_style(theme),
        ));
    }

    let inner_width = annotation_body_width(width);
    let show_close = inner_width >= ANNOTATION_CLOSE_BUTTON_WIDTH;
    let rule_width = inner_width.saturating_sub(if show_close {
        ANNOTATION_CLOSE_BUTTON_WIDTH
    } else {
        0
    });
    let mut spans = Vec::with_capacity(5);
    spans.push(annotation_border_span("┌", theme));
    if let Some(label) = label {
        let label = terminal_text(&format!("{label} "));
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
    if show_close {
        spans.push(Span::styled(
            ANNOTATION_CLOSE_BUTTON.to_owned(),
            Style::default()
                .fg(theme.deletion_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(annotation_border_span("┐", theme));
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
    if width == 0 {
        return Line::default();
    }
    if width == 1 {
        return Line::from(Span::styled(
            annotation_rule(1, theme),
            annotation_border_style(theme),
        ));
    }

    let style = annotation_border_style(theme);
    let inner_width = annotation_body_width(width);
    let button_width = match button {
        AnnotationFooterButton::None => 0,
        AnnotationFooterButton::Edit => ANNOTATION_EDIT_BUTTON_WIDTH,
        AnnotationFooterButton::Submit => ANNOTATION_SUBMIT_BUTTON_WIDTH,
    };
    let show_button = button_width > 0 && inner_width >= button_width;
    let label = if show_button {
        annotation_footer_button_label(inner_width, button, theme)
    } else {
        String::new()
    };
    let left = inner_width.saturating_sub(label.width());
    let button_fg = match button {
        AnnotationFooterButton::None => theme.hunk,
        AnnotationFooterButton::Edit => theme.search_match_bg,
        AnnotationFooterButton::Submit => theme.addition_fg,
    };
    let mut spans = vec![
        annotation_border_span("└", theme),
        Span::styled(annotation_rule(left, theme), style),
    ];
    if show_button {
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(button_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(annotation_border_span("┘", theme));
    Line::from(spans)
}

fn annotation_border_span(glyph: &'static str, theme: DiffTheme) -> Span<'static> {
    Span::styled(
        if theme.decorations.show_borders() {
            glyph
        } else {
            " "
        },
        annotation_border_style(theme),
    )
}

fn annotation_rule(width: usize, theme: DiffTheme) -> String {
    if theme.decorations.show_borders() {
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
    width.saturating_sub(2)
}

fn annotation_body_line(text: &str, width: usize, theme: DiffTheme, fg: Color) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if width == 1 {
        return Line::from(annotation_border_span("│", theme));
    }

    let bg = base_bg(theme);
    let display = fit_padded(text, annotation_body_width(width));
    let text_style = Style::default().fg(fg).bg(bg);
    let mut spans = vec![annotation_border_span("│", theme)];
    if display.contains(INPUT_CURSOR) {
        spans.extend(spans_with_input_cursor(
            &display,
            text_style,
            input_cursor_style(theme, bg),
            theme.decorations.input_cursor(),
        ));
    } else {
        spans.push(Span::styled(display, text_style));
    }
    spans.push(annotation_border_span("│", theme));
    Line::from(spans)
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
    editable_human: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![annotation_top_border_line(width, theme, label)];
    for line in annotation_display_lines(text, width) {
        lines.push(annotation_body_line(&line, width, theme, theme.muted));
    }
    lines.push(annotation_bottom_border_line(
        width,
        theme,
        if editable_human {
            AnnotationFooterButton::Edit
        } else {
            AnnotationFooterButton::None
        },
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

pub(crate) fn annotation_close_hit_at_column(column: u16, columns: (usize, usize)) -> bool {
    annotation_button_hit_at_column(column, columns, ANNOTATION_CLOSE_BUTTON_WIDTH)
}

pub(crate) fn annotation_submit_hit_at_column(column: u16, columns: (usize, usize)) -> bool {
    annotation_button_hit_at_column(column, columns, ANNOTATION_SUBMIT_BUTTON_WIDTH)
}

pub(crate) fn annotation_edit_hit_at_column(column: u16, columns: (usize, usize)) -> bool {
    annotation_button_hit_at_column(column, columns, ANNOTATION_EDIT_BUTTON_WIDTH)
}

fn annotation_button_hit_at_column(
    column: u16,
    (start, end): (usize, usize),
    button_width: usize,
) -> bool {
    let card_width = end.saturating_sub(start);
    if card_width < button_width.saturating_add(2) {
        return false;
    }
    let end = end.min(usize::from(u16::MAX)) as u16;
    let button_end = end.saturating_sub(1);
    column >= button_end.saturating_sub(button_width as u16) && column < button_end
}

#[cfg(test)]
mod tests {
    use crate::theme::DiffTheme;

    use super::{
        annotation_button_hit_at_column, annotation_display_line_count, annotation_display_lines,
        annotation_top_border_line, render_annotation_saved_block,
    };

    #[test]
    fn agent_and_mixed_cards_do_not_render_an_edit_button() {
        let lines = render_annotation_saved_block(
            "Agent: explanation",
            32,
            DiffTheme::default(),
            Some("Agent"),
            false,
        );
        let footer = lines.last().unwrap().to_string();

        assert!(!footer.contains("[↻]"));
        assert!(!footer.contains("[e]"));
    }

    #[test]
    fn annotation_cards_are_enclosed() {
        let lines =
            render_annotation_saved_block("note", 12, DiffTheme::default(), Some("Line"), true);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();

        assert_eq!(text, ["┌Line ──[x]┐", "│note      │", "└───────[↻]┘"]);
    }

    #[test]
    fn annotation_button_hits_use_the_trailing_inner_columns() {
        assert!(!annotation_button_hit_at_column(5, (0, 10), 3));
        assert!(annotation_button_hit_at_column(6, (0, 10), 3));
        assert!(annotation_button_hit_at_column(8, (0, 10), 3));
        assert!(!annotation_button_hit_at_column(9, (0, 10), 3));
        assert!(!annotation_button_hit_at_column(10, (0, 10), 3));
    }

    #[test]
    fn annotation_titles_escape_terminal_controls() {
        let line = annotation_top_border_line(
            120,
            DiffTheme::default(),
            Some("Agent · unsafe\u{1b}]52;c;payload\u{7}"),
        )
        .to_string();

        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\u{7}'));
        assert!(line.contains("\\u{1b}]52;c;payload\\u{7}"));
    }

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
