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
        annotation_ranges::AnnotationBlockGeometry,
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
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
    label: Option<&str>,
) -> Line<'static> {
    let card_width = geometry.card_width();
    if card_width < 2 {
        return Line::from(Span::styled(
            spaces(width),
            Style::default().bg(base_bg(theme)),
        ));
    }
    let (top_left, _, _, _, top_right) = annotation_frame_characters(theme);
    let top_left = if geometry.connected {
        if theme.decorations.is_fancy() {
            '├'
        } else {
            '+'
        }
    } else {
        top_left
    };
    let interior = card_width.saturating_sub(2);
    let close = if interior >= ANNOTATION_CLOSE_BUTTON_WIDTH {
        ANNOTATION_CLOSE_BUTTON
    } else {
        ""
    };
    let close_width = close.width();
    let label_width = interior.saturating_sub(close_width);
    let title_rule = if theme.decorations.is_fancy() {
        '─'
    } else {
        '-'
    };
    let has_title_rule = label.is_some() && label_width > 0;
    let title_width = label_width.saturating_sub(usize::from(has_title_rule));
    let title = label
        .map(|label| fit(&format!(" {label} "), title_width))
        .unwrap_or_default();
    let used = title.width().saturating_add(usize::from(has_title_rule));
    let fill = annotation_rule(label_width.saturating_sub(used), theme);
    let mut spans = annotation_line_prefix(width, geometry, theme);
    spans.push(Span::styled(
        top_left.to_string(),
        annotation_border_style(theme),
    ));
    if has_title_rule {
        spans.push(Span::styled(
            title_rule.to_string(),
            annotation_border_style(theme),
        ));
    }
    if !title.is_empty() {
        spans.push(Span::styled(
            title,
            Style::default().fg(theme.foreground).bg(base_bg(theme)),
        ));
    }
    if !fill.is_empty() {
        spans.push(Span::styled(fill, annotation_border_style(theme)));
    }
    if !close.is_empty() {
        spans.push(Span::styled(
            close.to_owned(),
            Style::default()
                .fg(theme.deletion_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        top_right.to_string(),
        annotation_border_style(theme),
    ));
    push_annotation_line_suffix(&mut spans, width, geometry, theme);
    Line::from(spans)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationFooterButton {
    Edit,
    Submit,
}

fn annotation_bottom_border_line(
    width: usize,
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
    button: AnnotationFooterButton,
) -> Line<'static> {
    let card_width = geometry.card_width();
    if card_width < 2 {
        return Line::from(Span::styled(
            spaces(width),
            Style::default().bg(base_bg(theme)),
        ));
    }
    let (_, _, bottom_left, bottom_right, _) = annotation_frame_characters(theme);
    let interior = card_width.saturating_sub(2);
    let label = annotation_footer_button_label(interior, button, theme);
    let label_width = label.width();
    let button_fg = match button {
        AnnotationFooterButton::Edit => theme.search_match_bg,
        AnnotationFooterButton::Submit => theme.addition_fg,
    };
    let mut spans = annotation_line_prefix(width, geometry, theme);
    spans.push(Span::styled(
        bottom_left.to_string(),
        annotation_border_style(theme),
    ));
    spans.push(Span::styled(
        annotation_rule(interior.saturating_sub(label_width), theme),
        annotation_border_style(theme),
    ));
    if !label.is_empty() {
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(button_fg)
                .bg(base_bg(theme))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        bottom_right.to_string(),
        annotation_border_style(theme),
    ));
    push_annotation_line_suffix(&mut spans, width, geometry, theme);
    Line::from(spans)
}

fn annotation_frame_characters(theme: DiffTheme) -> (char, char, char, char, char) {
    if theme.decorations.is_fancy() {
        ('┌', '│', '└', '┘', '┐')
    } else {
        ('+', '|', '+', '+', '+')
    }
}

fn annotation_rule(width: usize, theme: DiffTheme) -> String {
    let character = if theme.decorations.is_fancy() {
        '─'
    } else {
        '-'
    };
    std::iter::repeat_n(character, width).collect()
}

fn annotation_footer_button_label(
    width: usize,
    button: AnnotationFooterButton,
    theme: DiffTheme,
) -> String {
    match button {
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

fn annotation_line_prefix(
    width: usize,
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(8);
    let prefix = geometry.start.min(width);
    if prefix > 0 {
        spans.push(Span::styled(
            spaces(prefix),
            Style::default().bg(base_bg(theme)),
        ));
    }
    spans
}

fn push_annotation_line_suffix(
    spans: &mut Vec<Span<'static>>,
    width: usize,
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
) {
    let suffix = width.saturating_sub(geometry.end.min(width));
    if suffix > 0 {
        spans.push(Span::styled(
            spaces(suffix),
            Style::default().bg(base_bg(theme)),
        ));
    }
}

fn annotation_body_line(
    text: &str,
    width: usize,
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
    fg: Color,
) -> Line<'static> {
    let bg = base_bg(theme);
    if geometry.card_width() < 4 {
        return Line::from(Span::styled(spaces(width), Style::default().bg(bg)));
    }
    let (_, side, _, _, _) = annotation_frame_characters(theme);
    let body_width = geometry.body_width();
    let display = fit_padded(text, body_width);
    let mut spans = annotation_line_prefix(width, geometry, theme);
    spans.push(Span::styled(
        side.to_string(),
        annotation_border_style(theme),
    ));
    spans.push(Span::styled(" ", Style::default().bg(bg)));
    if display.contains(INPUT_CURSOR) {
        spans.extend(spans_with_input_cursor(
            &display,
            Style::default().fg(fg).bg(bg),
            input_cursor_style(theme, bg),
            theme.decorations.input_cursor(),
        ));
    } else {
        spans.push(Span::styled(display, Style::default().fg(fg).bg(bg)));
    }
    spans.push(Span::styled(" ", Style::default().bg(bg)));
    spans.push(Span::styled(
        side.to_string(),
        annotation_border_style(theme),
    ));
    push_annotation_line_suffix(&mut spans, width, geometry, theme);
    Line::from(spans)
}

fn annotation_display_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    visit_annotation_display_lines(text, width, |line| {
        lines.push(line.to_owned());
    });
    lines
}

fn annotation_display_line_count(text: &str, width: usize) -> usize {
    visit_annotation_display_lines(text, width, |_| {})
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
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
    label: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = vec![annotation_top_border_line(width, geometry, theme, label)];
    for line in annotation_display_lines(text, geometry.body_width()) {
        lines.push(annotation_body_line(
            &line,
            width,
            geometry,
            theme,
            theme.muted,
        ));
    }
    lines.push(annotation_bottom_border_line(
        width,
        geometry,
        theme,
        AnnotationFooterButton::Edit,
    ));
    lines
}

pub(crate) fn annotation_saved_block_height(text: &str, body_width: usize) -> usize {
    annotation_display_line_count(text, body_width).saturating_add(2)
}

pub(crate) fn render_annotation_compose_block(
    draft: &AnnotationDraft,
    width: usize,
    geometry: AnnotationBlockGeometry,
    theme: DiffTheme,
    label: Option<&str>,
) -> Vec<Line<'static>> {
    let display = text_with_cursor(&draft.input, draft.cursor);
    let mut lines = vec![annotation_top_border_line(width, geometry, theme, label)];
    for line in annotation_display_lines(&display, geometry.body_width()) {
        lines.push(annotation_body_line(
            &line,
            width,
            geometry,
            theme,
            theme.foreground,
        ));
    }
    lines.push(annotation_bottom_border_line(
        width,
        geometry,
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
    if card_width.saturating_sub(2) < button_width {
        return false;
    }
    let end = end.min(usize::from(u16::MAX)) as u16;
    let button_end = end.saturating_sub(1); // square right corner
    let button_start = button_end.saturating_sub(button_width as u16);
    column >= button_start && column < button_end
}

#[cfg(test)]
mod tests {
    use crate::{render::annotation_ranges::AnnotationBlockGeometry, theme::DiffTheme};

    use super::{
        annotation_button_hit_at_column, annotation_display_line_count, annotation_display_lines,
        annotation_top_border_line,
    };

    #[test]
    fn annotation_button_hits_exclude_corners_and_narrow_cards() {
        assert!(!annotation_button_hit_at_column(0, (0, 4), 3));
        assert!(!annotation_button_hit_at_column(5, (0, 10), 3));
        assert!(annotation_button_hit_at_column(6, (0, 10), 3));
        assert!(annotation_button_hit_at_column(8, (0, 10), 3));
        assert!(!annotation_button_hit_at_column(9, (0, 10), 3));
    }

    #[test]
    fn annotation_title_rule_uses_the_border_color() {
        let theme = DiffTheme::default();
        let line = annotation_top_border_line(
            40,
            AnnotationBlockGeometry {
                start: 12,
                end: 40,
                connected: true,
            },
            theme,
            Some("Note · +1"),
        );
        let rule = line
            .spans
            .iter()
            .find(|span| span.content == "─")
            .expect("title rule");
        let title = line
            .spans
            .iter()
            .find(|span| span.content.contains("Note"))
            .expect("title");

        assert_eq!(rule.style.fg, Some(theme.hunk));
        assert_eq!(title.style.fg, Some(theme.foreground));
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
