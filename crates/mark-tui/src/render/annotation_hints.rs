use ratatui::prelude::{Line, Modifier, Span, Style};

use crate::{
    annotation::AnnotationSide,
    controls::DiffLayoutMode,
    render::text::{display_width, fit_with_width, skip_display_prefix, spaces},
    theme::{DiffTheme, GUTTER_WIDTH, UNIFIED_GUTTER_WIDTH},
};

const LINE_NUMBER_WIDTH: usize = 5;
const UNIFIED_NEW_LINE_OFFSET: usize = LINE_NUMBER_WIDTH + 1;

pub(crate) struct AnnotationTargetHint<'a> {
    pub(crate) side: AnnotationSide,
    pub(crate) hint: &'a str,
    pub(crate) existing_annotation: bool,
    pub(crate) uppercase: bool,
}

pub(crate) fn apply_annotation_target_hint(
    line: Line<'static>,
    layout: DiffLayoutMode,
    width: usize,
    target: AnnotationTargetHint<'_>,
    theme: DiffTheme,
) -> Line<'static> {
    let hint = if target.uppercase {
        target.hint.to_ascii_uppercase()
    } else {
        target.hint.to_owned()
    };
    let Some((start, field_width)) =
        annotation_target_hint_range(layout, width, target.side, display_width(&hint))
    else {
        return line;
    };
    let hint_style = if target.existing_annotation {
        Style::default()
            .fg(theme.hunk)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default()
            .fg(theme.search_match_fg)
            .bg(theme.search_match_bg)
            .add_modifier(Modifier::BOLD)
    };
    overlay_line_cells(line, start, field_width, &hint, hint_style)
}

fn annotation_target_hint_range(
    layout: DiffLayoutMode,
    width: usize,
    side: AnnotationSide,
    hint_width: usize,
) -> Option<(usize, usize)> {
    let range = match layout {
        DiffLayoutMode::Unified => {
            let offset = match side {
                AnnotationSide::Old => 0,
                AnnotationSide::New => UNIFIED_NEW_LINE_OFFSET,
            };
            hint_range_in_cell(0, width, UNIFIED_GUTTER_WIDTH, offset, hint_width)
        }
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let (cell_start, cell_width) = match side {
                AnnotationSide::Old => (0, left_width),
                AnnotationSide::New => (left_width, width.saturating_sub(left_width)),
            };
            hint_range_in_cell(cell_start, cell_width, GUTTER_WIDTH, 0, hint_width)
        }
    };

    range.or_else(|| {
        // Extremely narrow split cells may not fit a hint even though the full
        // row does. Showing it across the cell boundary is preferable to
        // displaying an ambiguous prefix.
        (hint_width > 0 && hint_width <= width).then_some(match side {
            AnnotationSide::Old => (0, hint_width),
            AnnotationSide::New => (width.saturating_sub(hint_width), hint_width),
        })
    })
}

fn hint_range_in_cell(
    cell_start: usize,
    cell_width: usize,
    max_gutter_width: usize,
    field_offset: usize,
    hint_width: usize,
) -> Option<(usize, usize)> {
    if hint_width == 0 || cell_width == 0 {
        return None;
    }

    let indicator_width = 1.min(cell_width);
    let gutter_width = max_gutter_width.min(cell_width.saturating_sub(indicator_width));
    // The final gutter cell is the diff sign and must not be treated as part
    // of either line-number field.
    let gutter_body_width = gutter_width.saturating_sub(1);
    let preferred =
        line_number_field_range(cell_start, indicator_width, gutter_body_width, field_offset);
    if preferred.is_some_and(|(_, field_width)| field_width >= hint_width) {
        return preferred;
    }

    // Hints wider than a line number can consume available gutter separators
    // while still preserving the +/- sign.
    if let Some((start, _)) = preferred {
        let gutter_body_end = cell_start
            .saturating_add(indicator_width)
            .saturating_add(gutter_body_width);
        if gutter_body_end.saturating_sub(start) >= hint_width {
            return Some((start, hint_width));
        }
    }

    // Longer codes belong in the content area rather than being truncated to
    // a duplicate gutter prefix.
    let content_start = cell_start
        .saturating_add(indicator_width)
        .saturating_add(gutter_width);
    let cell_end = cell_start.saturating_add(cell_width);
    if cell_end.saturating_sub(content_start) >= hint_width {
        return Some((content_start, hint_width));
    }

    // In a narrow unified gutter the selected side's number may be clipped.
    // Fall back to the first number field, as the short-hint behavior did.
    let fallback = line_number_field_range(cell_start, indicator_width, gutter_body_width, 0);
    if fallback.is_some_and(|(_, field_width)| field_width >= hint_width) {
        return fallback;
    }
    if let Some((start, _)) = fallback {
        let gutter_body_end = cell_start
            .saturating_add(indicator_width)
            .saturating_add(gutter_body_width);
        if gutter_body_end.saturating_sub(start) >= hint_width {
            return Some((start, hint_width));
        }
    }

    (cell_width >= hint_width).then_some((cell_start, hint_width))
}

fn line_number_field_range(
    cell_start: usize,
    indicator_width: usize,
    gutter_body_width: usize,
    field_offset: usize,
) -> Option<(usize, usize)> {
    let field_width = gutter_body_width
        .saturating_sub(field_offset)
        .min(LINE_NUMBER_WIDTH);
    (field_width > 0).then_some((
        cell_start
            .saturating_add(indicator_width)
            .saturating_add(field_offset),
        field_width,
    ))
}

fn overlay_line_cells(
    line: Line<'static>,
    start: usize,
    width: usize,
    replacement: &str,
    replacement_style: Style,
) -> Line<'static> {
    if width == 0 {
        return line;
    }

    let replacement_width = display_width(replacement);
    if replacement_width > width {
        return line;
    }

    let end = start.saturating_add(width);
    let replacement = replacement.to_owned();
    let padding_width = width.saturating_sub(replacement_width);
    let line_style = line.style;
    let alignment = line.alignment;
    let mut column = 0usize;
    let mut inserted = false;
    let mut spans = Vec::with_capacity(line.spans.len().saturating_add(3));

    for span in line.spans {
        let span_width = display_width(span.content.as_ref());
        let span_end = column.saturating_add(span_width);
        if span_end <= start || column >= end {
            spans.push(span);
            column = span_end;
            continue;
        }

        if column < start {
            let prefix_width = start.saturating_sub(column);
            let (prefix, used, _) = fit_with_width(span.content.as_ref(), prefix_width);
            if !prefix.is_empty() {
                spans.push(Span::styled(prefix, span.style));
            }
            if used < prefix_width {
                // A cell boundary can bisect a wide grapheme. Replace its
                // visible remainder with spaces so later cells stay aligned.
                spans.push(Span::styled(spaces(prefix_width - used), span.style));
            }
        }
        if !inserted {
            if padding_width > 0 {
                spans.push(Span::styled(spaces(padding_width), span.style));
            }
            spans.push(Span::styled(
                replacement.clone(),
                span.style.patch(replacement_style),
            ));
            inserted = true;
        }
        if span_end > end {
            let skip = end.saturating_sub(column);
            let (suffix, skipped) = skip_display_prefix(span.content.as_ref(), skip);
            if skipped > skip {
                spans.push(Span::styled(spaces(skipped - skip), span.style));
            }
            if !suffix.is_empty() {
                spans.push(Span::styled(suffix.to_owned(), span.style));
            }
        }

        column = span_end;
    }

    Line {
        style: line_style,
        alignment,
        spans,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::prelude::{Color, Line, Span, Style};
    use unicode_width::UnicodeWidthStr;

    use super::{
        AnnotationTargetHint, annotation_target_hint_range, apply_annotation_target_hint,
        overlay_line_cells,
    };
    use crate::{
        annotation::{AnnotationSide, annotation_hint_codes},
        controls::DiffLayoutMode,
        theme::DiffTheme,
    };

    #[test]
    fn split_hint_replaces_the_selected_side_line_number() {
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Split, 60, AnnotationSide::Old, 2),
            Some((1, 5))
        );
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Split, 60, AnnotationSide::New, 2),
            Some((31, 5))
        );
    }

    #[test]
    fn split_hint_uses_the_separator_or_content_when_wider_than_a_line_number() {
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Split, 60, AnnotationSide::Old, 6),
            Some((1, 6))
        );
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Split, 60, AnnotationSide::New, 7),
            Some((38, 7))
        );
    }

    #[test]
    fn unified_hint_replaces_the_selected_side_line_number() {
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Unified, 80, AnnotationSide::Old, 2),
            Some((1, 5))
        );
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Unified, 80, AnnotationSide::New, 2),
            Some((7, 5))
        );
    }

    #[test]
    fn unified_hint_uses_the_separator_or_content_when_wider_than_a_line_number() {
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Unified, 80, AnnotationSide::New, 6),
            Some((7, 6))
        );
        assert_eq!(
            annotation_target_hint_range(DiffLayoutMode::Unified, 80, AnnotationSide::New, 7),
            Some((14, 7))
        );
    }

    #[test]
    fn generated_hints_wider_than_line_numbers_are_not_truncated() {
        let hints = annotation_hint_codes(33, "as");
        let long_hints = hints
            .iter()
            .filter(|hint| hint.width() > 5)
            .collect::<Vec<_>>();
        assert_eq!(long_hints.len(), 2);

        for hint in long_hints {
            let mut source = " ".repeat(13);
            source.push('+');
            source.push_str(&"x".repeat(26));
            let rendered = apply_annotation_target_hint(
                Line::from(source),
                DiffLayoutMode::Unified,
                40,
                AnnotationTargetHint {
                    side: AnnotationSide::New,
                    hint,
                    existing_annotation: false,
                    uppercase: false,
                },
                DiffTheme::default(),
            );
            let text = rendered
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();

            assert_eq!(text.width(), 40);
            assert_eq!(text.chars().skip(7).take(6).collect::<String>(), **hint);
            assert_eq!(text.chars().nth(13), Some('+'));
            assert!(
                rendered
                    .spans
                    .iter()
                    .any(|span| span.content.as_ref() == hint.as_str())
            );
        }
    }

    #[test]
    fn hint_overlay_preserves_width_and_surrounding_styles() {
        let left = Style::default().fg(Color::Red);
        let right = Style::default().fg(Color::Blue);
        let hint = Style::default().fg(Color::Black).bg(Color::Yellow);
        let line = Line::from(vec![
            Span::styled("abcdef", left),
            Span::styled("ghijkl", right),
        ]);

        let overlaid = overlay_line_cells(line, 5, 2, "as", hint);
        let text = overlaid
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, "abcdeashijkl");
        assert_eq!(text.width(), 12);
        assert!(overlaid.spans.iter().any(|span| span.style == hint));
    }

    #[test]
    fn hint_overlay_preserves_width_when_its_end_bisects_a_wide_grapheme() {
        let base = Style::default().fg(Color::Red);
        let hint = Style::default().fg(Color::Black).bg(Color::Yellow);
        let line = Line::from(Span::styled("💣💣💣💣xxxx", base));

        let overlaid = overlay_line_cells(line, 0, 7, "ssssssa", hint);
        let text = overlaid
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, "ssssssa xxxx");
        assert_eq!(text.width(), 12);
    }

    #[test]
    fn hint_overlay_clears_and_right_aligns_the_line_number_field() {
        let gutter = Style::default().fg(Color::DarkGray).bg(Color::Black);
        let hint = Style::default().fg(Color::Black).bg(Color::Yellow);
        let line = Line::from(Span::styled("  123 +code", gutter));

        let overlaid = overlay_line_cells(line, 0, 5, "as", hint);
        let text = overlaid
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, "   as +code");
        assert_eq!(text.width(), 11);
        assert!(
            overlaid.spans.iter().any(|span| {
                span.content.as_ref() == "as" && span.style.bg == Some(Color::Yellow)
            })
        );
    }
}
