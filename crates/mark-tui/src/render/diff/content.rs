use mark_diff::DiffLineKind;
use mark_syntax::{DiffBackground, HighlightedLine, SyntaxClass, SyntaxSegment};
use ratatui::prelude::{Color, Modifier, Span, Style};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    render::{
        style::{diff_indicator_span, diff_sign_style, focused_diff_indicator_span},
        text::{fit, fit_padded, fit_padded_from, fit_with_width_from, spaces},
    },
    syntax::InlineRange,
    theme::{
        DiffTheme, EMPTY_DIFF_FILL, EMPTY_DIFF_FILL_SPACING, GUTTER_WIDTH, TextMateEngineMode,
        UNIFIED_GUTTER_WIDTH, line_gutter_bg, line_gutter_fg, textmate_engine_mode,
        with_scope_override_theme,
    },
};

use super::line_style;

pub(crate) fn diff_indicator_span_for_focus(
    kind: DiffLineKind,
    theme: DiffTheme,
    focused: bool,
) -> Span<'static> {
    if focused {
        focused_diff_indicator_span(kind, theme)
    } else {
        diff_indicator_span(kind, theme)
    }
}

pub(super) fn unified_gutter_text(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let mut gutter = String::with_capacity(UNIFIED_GUTTER_WIDTH);
    push_right_aligned_number(&mut gutter, old_line, 5);
    gutter.push(' ');
    push_right_aligned_number(&mut gutter, new_line, 5);
    gutter.push(' ');
    gutter
}

pub(super) fn split_gutter_text(line: Option<usize>) -> String {
    let mut gutter = String::with_capacity(GUTTER_WIDTH.saturating_sub(1));
    push_right_aligned_number(&mut gutter, line, 5);
    gutter.push(' ');
    gutter
}

fn push_right_aligned_number(out: &mut String, line: Option<usize>, width: usize) {
    let Some(mut value) = line else {
        out.extend(std::iter::repeat_n(' ', width));
        return;
    };

    let mut digits = [0u8; 39];
    let mut len = 0usize;
    loop {
        digits[digits.len() - 1 - len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    if len < width {
        out.extend(std::iter::repeat_n(' ', width - len));
    }
    for digit in &digits[digits.len() - len..] {
        out.push(*digit as char);
    }
}

pub(crate) fn gutter_spans(
    body: &str,
    sign: &str,
    width: usize,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let body_style = Style::default()
        .fg(line_gutter_fg(kind, theme))
        .bg(line_gutter_bg(kind, theme));
    if sign.trim().is_empty() || width == 1 {
        return vec![Span::styled(
            fit_padded(&format!("{body}{sign}"), width),
            body_style,
        )];
    }

    let sign_width = 1;
    let body_width = width.saturating_sub(sign_width);
    vec![
        Span::styled(fit_padded(body, body_width), body_style),
        Span::styled(fit(sign, sign_width), diff_sign_style(kind, theme)),
    ]
}

pub(crate) fn empty_diff_fill_from(
    width: usize,
    row_index: usize,
    column_offset: usize,
    enabled: bool,
) -> String {
    if !enabled {
        return spaces(width).into_owned();
    }

    let mut fill = String::with_capacity(width.saturating_mul(EMPTY_DIFF_FILL.len_utf8()));
    for column in 0..width {
        fill.push(
            if (column + column_offset + row_index).is_multiple_of(EMPTY_DIFF_FILL_SPACING) {
                EMPTY_DIFF_FILL
            } else {
                ' '
            },
        );
    }
    fill
}

pub(crate) fn content_spans_at_scroll(
    text: &str,
    syntax: Option<&HighlightedLine>,
    inline: &[InlineRange],
    kind: DiffLineKind,
    width: usize,
    theme: DiffTheme,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let valid_inline;
    let inline = if inline.is_empty() {
        &[][..]
    } else {
        valid_inline = valid_inline_ranges(text, inline);
        valid_inline.as_slice()
    };
    let syntax = syntax.filter(|syntax| syntax_line_matches_text(syntax, text));
    if syntax.is_none() && inline.is_empty() {
        return vec![Span::styled(
            fit_padded_from(text, horizontal_scroll, width),
            line_style(kind, theme),
        )];
    }

    let span_capacity = syntax.map_or(1, |syntax| syntax.segments.len()) + inline.len() * 2 + 1;
    let mut writer =
        ContentSpanWriter::new(inline, kind, width, theme, horizontal_scroll, span_capacity);

    if let Some(syntax) = syntax {
        for segment in &syntax.segments {
            let byte_start = segment.byte_start;
            let byte_end = segment.byte_end;
            debug_assert!(byte_start <= byte_end);
            debug_assert!(byte_end <= text.len());
            if !writer.push_segment(
                &text[byte_start..byte_end],
                byte_start,
                segment_syntax_style(segment, syntax, kind, theme),
            ) {
                break;
            }
        }
    } else {
        writer.push_segment(text, 0, line_style(kind, theme));
    }

    writer.finish()
}

pub(crate) fn valid_inline_ranges(text: &str, ranges: &[InlineRange]) -> Vec<InlineRange> {
    if ranges.is_empty() {
        return Vec::new();
    }

    let grapheme_boundaries = (!text.is_ascii()).then(|| grapheme_boundary_indices(text));
    let mut valid = Vec::with_capacity(ranges.len());
    for range in ranges {
        let mut byte_start = range.byte_start.min(text.len());
        let mut byte_end = range.byte_end.min(text.len());
        if byte_start < byte_end
            && text.is_char_boundary(byte_start)
            && text.is_char_boundary(byte_end)
        {
            if let Some(boundaries) = grapheme_boundaries.as_deref() {
                byte_start = previous_grapheme_boundary(boundaries, byte_start);
                byte_end = next_grapheme_boundary(boundaries, byte_end);
            }
            valid.push(InlineRange {
                byte_start,
                byte_end,
            });
        }
    }
    if valid.len() > 1 {
        valid.sort_by_key(|range| (range.byte_start, range.byte_end));
    }

    merge_inline_ranges(valid)
}

pub(crate) struct ContentSpanWriter<'a> {
    spans: Vec<Span<'static>>,
    inline: &'a [InlineRange],
    kind: DiffLineKind,
    width: usize,
    skip: usize,
    used: usize,
    theme: DiffTheme,
}

impl<'a> ContentSpanWriter<'a> {
    pub(crate) fn new(
        inline: &'a [InlineRange],
        kind: DiffLineKind,
        width: usize,
        theme: DiffTheme,
        horizontal_scroll: usize,
        span_capacity: usize,
    ) -> Self {
        Self {
            spans: Vec::with_capacity(span_capacity),
            inline,
            kind,
            width,
            skip: horizontal_scroll,
            used: 0,
            theme,
        }
    }

    pub(crate) fn push_segment(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        style: Style,
    ) -> bool {
        let segment_byte_end = segment_byte_start + segment_text.len();
        let mut cursor = segment_byte_start;

        for range in self.inline {
            if self.used >= self.width {
                return false;
            }
            if range.byte_end <= cursor {
                continue;
            }
            if range.byte_start >= segment_byte_end {
                break;
            }

            let normal_end = range.byte_start.min(segment_byte_end);
            if !self.push_piece(segment_text, segment_byte_start, cursor, normal_end, style) {
                return false;
            }

            let inline_start = range.byte_start.max(cursor).min(segment_byte_end);
            let inline_end = range.byte_end.min(segment_byte_end);
            if !self.push_piece(
                segment_text,
                segment_byte_start,
                inline_start,
                inline_end,
                inline_style(style, self.kind, self.theme),
            ) {
                return false;
            }
            cursor = inline_end;
        }

        self.push_piece(
            segment_text,
            segment_byte_start,
            cursor,
            segment_byte_end,
            style,
        )
    }

    pub(crate) fn push_piece(
        &mut self,
        segment_text: &str,
        segment_byte_start: usize,
        byte_start: usize,
        byte_end: usize,
        style: Style,
    ) -> bool {
        if byte_start >= byte_end {
            return true;
        }
        let remaining = self.width.saturating_sub(self.used);
        if remaining == 0 {
            return false;
        }

        let relative_start = byte_start.saturating_sub(segment_byte_start);
        let relative_end = byte_end.saturating_sub(segment_byte_start);
        let piece = &segment_text[relative_start..relative_end];
        let (fitted, skipped, fitted_width, complete) =
            fit_with_width_from(piece, self.skip, remaining);
        self.skip = self.skip.saturating_sub(skipped);
        if fitted.is_empty() {
            return complete;
        }

        self.used += fitted_width;
        self.spans.push(Span::styled(fitted, style));
        complete
    }

    pub(crate) fn finish(mut self) -> Vec<Span<'static>> {
        if self.used < self.width {
            self.spans.push(Span::styled(
                spaces(self.width - self.used),
                line_style(self.kind, self.theme),
            ));
        }
        self.spans
    }
}

pub(crate) fn syntax_line_matches_text(syntax: &HighlightedLine, text: &str) -> bool {
    if !syntax.matches_text(text) {
        return false;
    }
    let grapheme_boundaries = (!text.is_ascii()).then(|| grapheme_boundary_indices(text));
    let mut cursor = 0usize;
    for segment in &syntax.segments {
        if segment.byte_start != cursor
            || segment.byte_end < segment.byte_start
            || segment.byte_end > text.len()
            || !text.is_char_boundary(segment.byte_start)
            || !text.is_char_boundary(segment.byte_end)
            || !is_grapheme_boundary(grapheme_boundaries.as_deref(), segment.byte_start)
            || !is_grapheme_boundary(grapheme_boundaries.as_deref(), segment.byte_end)
        {
            return false;
        }
        cursor = segment.byte_end;
    }
    cursor == text.len()
}

fn grapheme_boundary_indices(text: &str) -> Vec<usize> {
    let mut boundaries: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .collect();
    boundaries.push(text.len());
    boundaries
}

fn is_grapheme_boundary(boundaries: Option<&[usize]>, index: usize) -> bool {
    boundaries.is_none_or(|boundaries| boundaries.binary_search(&index).is_ok())
}

fn previous_grapheme_boundary(boundaries: &[usize], index: usize) -> usize {
    match boundaries.binary_search(&index) {
        Ok(position) => boundaries[position],
        Err(position) => boundaries[position.saturating_sub(1)],
    }
}

fn next_grapheme_boundary(boundaries: &[usize], index: usize) -> usize {
    match boundaries.binary_search(&index) {
        Ok(position) => boundaries[position],
        Err(position) => boundaries
            .get(position)
            .copied()
            .unwrap_or_else(|| boundaries.last().copied().unwrap_or(0)),
    }
}

fn merge_inline_ranges(ranges: Vec<InlineRange>) -> Vec<InlineRange> {
    let mut merged: Vec<InlineRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.byte_start <= last.byte_end
        {
            last.byte_end = last.byte_end.max(range.byte_end);
            continue;
        }
        merged.push(range);
    }
    merged
}

pub(crate) fn syntax_style(
    class: Option<SyntaxClass>,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Style {
    let mut style = line_style(kind, theme);
    if let Some(color) = class.and_then(|class| syntax_fg(class, theme)) {
        style = style.fg(color);
    }
    style
}

fn segment_syntax_style(
    segment: &SyntaxSegment,
    line: &HighlightedLine,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Style {
    let engine_mode = textmate_engine_mode();
    if engine_mode == TextMateEngineMode::Coarse {
        let coarse = syntax_style(segment.class, kind, theme);
        return apply_scope_override(coarse, segment, line, kind, theme);
    }
    let coarse = (engine_mode == TextMateEngineMode::Compare)
        .then(|| syntax_style(segment.class, kind, theme));
    let Some(exact_theme) = theme.exact_syntax else {
        let coarse = coarse.unwrap_or_else(|| syntax_style(segment.class, kind, theme));
        return apply_scope_override(coarse, segment, line, kind, theme);
    };
    let matched = exact_theme.resolve_style(&line.scope_table, segment.scope_stack);
    let resolved = matched.style;
    let mut style = line_style(kind, theme);
    // Unmatched foregrounds inherit the TextMate theme default. A configured
    // global foreground remains the base for unmatched scopes, while token
    // rules and the more specific overrides below still take precedence.
    if (matched.foreground_matched || !theme.foreground_overridden)
        && let Some(color) = resolved.foreground
    {
        style = style.fg(Color::Rgb(color.red, color.green, color.blue));
    }
    // Diff row backgrounds deliberately win over token backgrounds. Context
    // rows retain the TextMate background unless transparency was requested.
    if kind == DiffLineKind::Context
        && !theme.transparent_background
        && matched.background_matched
        && let Some(color) = resolved.background
    {
        style = style.bg(Color::Rgb(color.red, color.green, color.blue));
    }
    let modifiers = resolved.modifiers;
    if !modifiers.is_empty() {
        if modifiers.contains(mark_syntax::theme::SyntaxModifiers::BOLD) {
            style = style.add_modifier(Modifier::BOLD);
        }
        if modifiers.contains(mark_syntax::theme::SyntaxModifiers::ITALIC) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if modifiers.contains(mark_syntax::theme::SyntaxModifiers::UNDERLINED) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if modifiers.contains(mark_syntax::theme::SyntaxModifiers::CROSSED_OUT) {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
    }
    // Existing class-based user overrides are coarse by design and apply last.
    if let Some(color) = segment
        .class
        .and_then(|class| theme.syntax_overrides.color(class))
    {
        style = style.fg(color);
    }
    style = apply_scope_override(style, segment, line, kind, theme);
    if engine_mode == TextMateEngineMode::Compare {
        let coarse = apply_scope_override(
            coarse.expect("compare mode prepares coarse style"),
            segment,
            line,
            kind,
            theme,
        );
        log_theme_comparison(segment, line, coarse, style);
    }
    style
}

fn apply_scope_override(
    mut style: Style,
    segment: &SyntaxSegment,
    line: &HighlightedLine,
    kind: DiffLineKind,
    theme: DiffTheme,
) -> Style {
    let Some(id) = theme.scope_overrides else {
        return style;
    };
    let Some((foreground_matched, background_matched, modifiers_matched, resolved)) =
        with_scope_override_theme(id, |overrides| {
            let matched = overrides.resolve_style(&line.scope_table, segment.scope_stack);
            (
                matched.foreground_matched,
                matched.background_matched,
                matched.modifiers_matched,
                matched.style,
            )
        })
    else {
        return style;
    };
    if foreground_matched && let Some(color) = resolved.foreground {
        style = style.fg(Color::Rgb(color.red, color.green, color.blue));
    }
    if background_matched
        && kind == DiffLineKind::Context
        && !theme.transparent_background
        && let Some(color) = resolved.background
    {
        style = style.bg(Color::Rgb(color.red, color.green, color.blue));
    }
    if modifiers_matched {
        let all = Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::CROSSED_OUT;
        style = style.remove_modifier(all);
        for (syntax_modifier, modifier) in [
            (mark_syntax::theme::SyntaxModifiers::BOLD, Modifier::BOLD),
            (
                mark_syntax::theme::SyntaxModifiers::ITALIC,
                Modifier::ITALIC,
            ),
            (
                mark_syntax::theme::SyntaxModifiers::UNDERLINED,
                Modifier::UNDERLINED,
            ),
            (
                mark_syntax::theme::SyntaxModifiers::CROSSED_OUT,
                Modifier::CROSSED_OUT,
            ),
        ] {
            if resolved.modifiers.contains(syntax_modifier) {
                style = style.add_modifier(modifier);
            }
        }
    }
    style
}

fn log_theme_comparison(
    segment: &SyntaxSegment,
    line: &HighlightedLine,
    coarse: Style,
    exact: Style,
) {
    if coarse == exact {
        return;
    }
    let scopes = line
        .scope_table
        .stack_names(segment.scope_stack)
        .collect::<Vec<_>>()
        .join(" ");
    let message = format!(
        "TextMate theme difference: scopes={scopes:?} class={:?} coarse={coarse:?} exact={exact:?}",
        segment.class
    );
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if seen.len() < 1_000 && seen.insert(message.clone()) {
        eprintln!("{message}");
    }
}

pub(crate) fn inline_style(style: Style, kind: DiffLineKind, theme: DiffTheme) -> Style {
    if theme.diff.inline_background == DiffBackground::None {
        return match kind {
            DiffLineKind::Addition | DiffLineKind::Deletion => style.add_modifier(Modifier::BOLD),
            DiffLineKind::Context | DiffLineKind::Meta => style,
        };
    }

    match kind {
        DiffLineKind::Addition => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Deletion => style
            .bg(inline_bg(kind, theme))
            .add_modifier(Modifier::BOLD),
        DiffLineKind::Context | DiffLineKind::Meta => style,
    }
}

pub(crate) fn inline_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match (theme.diff.inline_background, kind) {
        (DiffBackground::Subtle, DiffLineKind::Addition) => theme.addition_bg,
        (DiffBackground::Subtle, DiffLineKind::Deletion) => theme.deletion_bg,
        (_, DiffLineKind::Addition) => theme.addition_inline_bg,
        (_, DiffLineKind::Deletion) => theme.deletion_inline_bg,
        _ => Color::Reset,
    }
}

pub(crate) fn syntax_fg(class: SyntaxClass, theme: DiffTheme) -> Option<Color> {
    theme.syntax.color(class)
}
