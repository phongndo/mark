use mark_diff::FileStatus;
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TAB_WIDTH: usize = 4;

const STATIC_SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

pub(crate) fn fit_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let (fitted, _, complete) = fit_with_width(text, width);
    if complete {
        return fitted;
    }
    if width <= 3 {
        return fit("...", width);
    }

    format!("{}...", fit(text, width - 3))
}

pub(crate) fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }

    formatted
}

pub(crate) fn status_code(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "M",
        FileStatus::Added => "A",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
        FileStatus::Copied => "C",
        FileStatus::TypeChanged => "T",
        FileStatus::Unknown => "?",
    }
}

pub(crate) fn progress_label(scroll: usize, max_scroll: usize) -> String {
    if max_scroll == 0 {
        return "100%".to_owned();
    }

    format!(
        "{}%",
        scroll.min(max_scroll).saturating_mul(100) / max_scroll
    )
}

pub(crate) fn fit_padded(text: &str, width: usize) -> String {
    fit_padded_from(text, 0, width)
}

pub(crate) fn fit_padded_from(text: &str, horizontal_scroll: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let (mut out, _, len, _) = fit_with_width_from(text, horizontal_scroll, width);
    if len < width {
        out.reserve(width - len);
        out.extend(std::iter::repeat_n(' ', width - len));
    }
    out
}

pub(crate) fn skip_display_prefix(text: &str, columns: usize) -> (&str, usize) {
    if columns == 0 {
        return (text, 0);
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, columns);
    if ascii_prefix == text.len()
        || (ascii_prefix == columns && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        let skipped = ascii_prefix;
        return (&text[skipped..], skipped);
    }

    let mut skipped = 0usize;
    let mut byte_index = 0usize;
    for chunk in DisplayChunks::new(text) {
        if skipped >= columns {
            if let DisplayChunk::Text(run) = chunk {
                let zero_width_prefix = zero_width_prefix_len(run);
                byte_index += zero_width_prefix;
            }
            break;
        }

        match chunk {
            DisplayChunk::Text(run) => {
                let (byte_len, run_skipped) = skip_normal_run_prefix(run, columns - skipped);
                skipped = skipped.saturating_add(run_skipped);
                byte_index += byte_len;
                if byte_len < run.len() {
                    break;
                }
            }
            DisplayChunk::Special(ch) => {
                skipped = skipped.saturating_add(display_char_width(ch));
                byte_index += ch.len_utf8();
            }
        }
    }

    (&text[byte_index..], skipped)
}

pub(crate) fn fit(text: &str, width: usize) -> String {
    fit_with_width(text, width).0
}

pub(crate) fn fit_with_width(text: &str, width: usize) -> (String, usize, bool) {
    if width == 0 {
        return (String::new(), 0, text.is_empty());
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, width);
    if ascii_prefix == text.len()
        || (ascii_prefix == width && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        return (
            text[..ascii_prefix].to_owned(),
            ascii_prefix,
            ascii_prefix == text.len(),
        );
    }

    let (out, _, used, complete) = fit_with_width_from(text, 0, width);
    (out, used, complete)
}

pub(crate) fn fit_byte_prefix_with_width(text: &str, width: usize) -> (usize, usize, bool) {
    if width == 0 {
        return (0, 0, text.is_empty());
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, width);
    if ascii_prefix == text.len()
        || (ascii_prefix == width && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        return (ascii_prefix, ascii_prefix, ascii_prefix == text.len());
    }

    let mut used = 0usize;
    let mut byte_end = 0usize;
    for chunk in DisplayChunks::new(text) {
        match chunk {
            DisplayChunk::Text(run) => {
                let (byte_len, run_used, complete) =
                    fit_normal_run_prefix_with_width(run, width.saturating_sub(used));
                used = used.saturating_add(run_used);
                byte_end += byte_len;
                if !complete {
                    return (byte_end, used, false);
                }
            }
            DisplayChunk::Special(ch) => {
                let ch_width = display_char_width(ch);
                if used.saturating_add(ch_width) > width {
                    return (byte_end, used, false);
                }
                used = used.saturating_add(ch_width);
                byte_end += ch.len_utf8();
            }
        }
    }

    (byte_end, used, true)
}

pub(crate) fn fit_with_width_from(
    text: &str,
    horizontal_scroll: usize,
    width: usize,
) -> (String, usize, usize, bool) {
    if width == 0 {
        return (String::new(), 0, 0, text.is_empty());
    }

    if let Some(fitted) = fit_bounded_single_width_ascii_from(text, horizontal_scroll, width) {
        return fitted;
    }
    if let Some(fitted) = fit_bounded_normal_run_from(text, horizontal_scroll, width) {
        return fitted;
    }

    let mut out = String::with_capacity(text.len().min(width.saturating_mul(4)));
    let mut skipped = 0usize;
    let mut skip_remaining = horizontal_scroll;
    let mut drop_zero_width_after_skip = false;
    let mut used = 0usize;
    for chunk in DisplayChunks::new(text) {
        match chunk {
            DisplayChunk::Text(run) => {
                if !push_fitted_normal_run(
                    &mut out,
                    run,
                    &mut skipped,
                    &mut skip_remaining,
                    &mut drop_zero_width_after_skip,
                    &mut used,
                    width,
                ) {
                    return (out, skipped, used, false);
                }
            }
            DisplayChunk::Special(ch) => {
                if !push_fitted_special_char(
                    &mut out,
                    ch,
                    &mut skipped,
                    &mut skip_remaining,
                    &mut drop_zero_width_after_skip,
                    &mut used,
                    width,
                ) {
                    return (out, skipped, used, false);
                }
            }
        }
    }
    (out, skipped, used, true)
}

pub(crate) fn spaces(width: usize) -> Cow<'static, str> {
    if width <= STATIC_SPACES.len() {
        Cow::Borrowed(&STATIC_SPACES[..width])
    } else {
        Cow::Owned(" ".repeat(width))
    }
}

pub(crate) fn display_width(text: &str) -> usize {
    if text
        .as_bytes()
        .iter()
        .all(|byte| is_single_width_printable_ascii(*byte))
    {
        return text.len();
    }

    DisplayChunks::new(text)
        .map(|chunk| match chunk {
            DisplayChunk::Text(run) => run.width(),
            DisplayChunk::Special(ch) => display_char_width(ch),
        })
        .sum()
}

pub(crate) fn display_char_width(ch: char) -> usize {
    match ch {
        '\t' => TAB_WIDTH,
        _ if ch.is_control() => control_escape_width(ch),
        _ => UnicodeWidthChar::width(ch).unwrap_or(0),
    }
}

pub(crate) fn display_char_supports_partial_render(ch: char) -> bool {
    ch == '\t' || ch.is_control()
}

pub(crate) fn for_display_width_units(text: &str, mut visit: impl FnMut(usize, bool)) {
    for chunk in DisplayChunks::new(text) {
        match chunk {
            DisplayChunk::Text(run) => {
                for_normal_run_width_units(run, |width| visit(width, false));
            }
            DisplayChunk::Special(ch) => visit(display_char_width(ch), true),
        }
    }
}

pub(crate) fn terminal_text(text: &str) -> String {
    if !text.chars().any(display_char_supports_partial_render) {
        return text.to_owned();
    }

    let mut out = String::with_capacity(display_width(text));
    for chunk in DisplayChunks::new(text) {
        match chunk {
            DisplayChunk::Text(run) => out.push_str(run),
            DisplayChunk::Special(ch) => {
                push_display_char_range(&mut out, ch, 0, display_char_width(ch));
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum DisplayChunk<'a> {
    Text(&'a str),
    Special(char),
}

struct DisplayChunks<'a> {
    text: &'a str,
    cursor: usize,
}

impl<'a> DisplayChunks<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, cursor: 0 }
    }
}

impl<'a> Iterator for DisplayChunks<'a> {
    type Item = DisplayChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.text.len() {
            return None;
        }

        let remaining = &self.text[self.cursor..];
        let Some((offset, ch)) = remaining
            .char_indices()
            .find(|(_, ch)| display_char_supports_partial_render(*ch))
        else {
            self.cursor = self.text.len();
            return Some(DisplayChunk::Text(remaining));
        };
        if offset > 0 {
            let start = self.cursor;
            self.cursor += offset;
            return Some(DisplayChunk::Text(&self.text[start..self.cursor]));
        }

        self.cursor += ch.len_utf8();
        Some(DisplayChunk::Special(ch))
    }
}

fn push_fitted_normal_run(
    out: &mut String,
    mut run: &str,
    skipped: &mut usize,
    skip_remaining: &mut usize,
    drop_zero_width_after_skip: &mut bool,
    used: &mut usize,
    width: usize,
) -> bool {
    if *skip_remaining > 0 {
        let (byte_len, run_skipped) = skip_normal_run_prefix(run, *skip_remaining);
        *skipped = (*skipped).saturating_add(run_skipped);
        *skip_remaining = (*skip_remaining).saturating_sub(run_skipped);
        run = &run[byte_len..];
        if run.is_empty() {
            return true;
        }
    }

    if *drop_zero_width_after_skip {
        let byte_len = zero_width_prefix_len(run);
        run = &run[byte_len..];
        *drop_zero_width_after_skip = false;
        if run.is_empty() {
            return true;
        }
    }

    let available = width.saturating_sub(*used);
    let (byte_len, run_used, complete) = fit_normal_run_prefix_with_width(run, available);
    if byte_len > 0 {
        out.push_str(&run[..byte_len]);
        *used = (*used).saturating_add(run_used);
    }
    complete
}

fn push_fitted_special_char(
    out: &mut String,
    ch: char,
    skipped: &mut usize,
    skip_remaining: &mut usize,
    drop_zero_width_after_skip: &mut bool,
    used: &mut usize,
    width: usize,
) -> bool {
    let ch_width = display_char_width(ch);
    if *skip_remaining > 0 {
        if *skip_remaining >= ch_width {
            *skip_remaining -= ch_width;
            *skipped = (*skipped).saturating_add(ch_width);
            *drop_zero_width_after_skip = *skip_remaining == 0;
            return true;
        }

        let partial_skip = *skip_remaining;
        *skip_remaining = 0;
        *skipped = (*skipped).saturating_add(partial_skip);
        *drop_zero_width_after_skip = false;
        let remaining_char_width = ch_width.saturating_sub(partial_skip);
        let available = width.saturating_sub(*used);
        let visible = remaining_char_width.min(available);
        push_display_char_range(out, ch, partial_skip, partial_skip + visible);
        *used = (*used).saturating_add(visible);
        return visible == remaining_char_width;
    }

    *drop_zero_width_after_skip = false;
    let available = width.saturating_sub(*used);
    let visible = ch_width.min(available);
    if visible == 0 {
        return false;
    }
    push_display_char_range(out, ch, 0, visible);
    *used = (*used).saturating_add(visible);
    visible == ch_width
}

fn fit_bounded_single_width_ascii_from(
    text: &str,
    horizontal_scroll: usize,
    width: usize,
) -> Option<(String, usize, usize, bool)> {
    let display_end = horizontal_scroll.saturating_add(width);
    let ascii_prefix = single_width_ascii_prefix_len(text, display_end);
    if ascii_prefix == text.len() {
        let byte_start = horizontal_scroll.min(text.len());
        let byte_end = display_end.min(text.len());
        return Some((
            text[byte_start..byte_end].to_owned(),
            byte_start,
            byte_end.saturating_sub(byte_start),
            true,
        ));
    }

    if ascii_prefix == display_end && next_byte_is_single_width_ascii(text, ascii_prefix) {
        return Some((
            text[horizontal_scroll..display_end].to_owned(),
            horizontal_scroll,
            width,
            false,
        ));
    }

    None
}

fn fit_bounded_normal_run_from(
    text: &str,
    horizontal_scroll: usize,
    width: usize,
) -> Option<(String, usize, usize, bool)> {
    let (byte_start, skipped) = skip_bounded_normal_run_prefix(text, horizontal_scroll)?;
    if byte_start == text.len() {
        return Some((String::new(), skipped, 0, true));
    }

    let (byte_end, used, complete) = fit_bounded_normal_run_prefix(&text[byte_start..], width)?;
    Some((
        text[byte_start..byte_start + byte_end].to_owned(),
        skipped,
        used,
        complete,
    ))
}

fn skip_bounded_normal_run_prefix(text: &str, columns: usize) -> Option<(usize, usize)> {
    if columns == 0 {
        return Some((0, 0));
    }

    let mut byte_end = 0usize;
    let mut skipped = 0usize;
    let mut reached_width = None;
    for (index, grapheme) in text.grapheme_indices(true) {
        if grapheme_has_special_display_char(grapheme) {
            return None;
        }

        let end = index + grapheme.len();
        let prefix_width = skipped.saturating_add(grapheme.width());
        if let Some(width) = reached_width {
            if prefix_width == width {
                byte_end = end;
                continue;
            }
            break;
        }

        byte_end = end;
        skipped = prefix_width;
        if prefix_width >= columns {
            reached_width = Some(prefix_width);
        }
    }

    Some((byte_end, skipped))
}

fn fit_bounded_normal_run_prefix(text: &str, width: usize) -> Option<(usize, usize, bool)> {
    let mut byte_end = 0usize;
    let mut used = 0usize;
    for (index, grapheme) in text.grapheme_indices(true) {
        if grapheme_has_special_display_char(grapheme) {
            return (used == width).then_some((byte_end, used, false));
        }

        let end = index + grapheme.len();
        let prefix_width = used.saturating_add(grapheme.width());
        if prefix_width > width {
            return Some((byte_end, used, false));
        }
        byte_end = end;
        used = prefix_width;
    }

    Some((byte_end, used, true))
}

fn grapheme_has_special_display_char(grapheme: &str) -> bool {
    grapheme.chars().any(display_char_supports_partial_render)
}

fn skip_normal_run_prefix(text: &str, columns: usize) -> (usize, usize) {
    if columns == 0 {
        return (zero_width_prefix_len(text), 0);
    }

    let ascii_prefix = single_width_ascii_prefix_len(text, columns);
    if ascii_prefix == text.len()
        || (ascii_prefix == columns && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        return (ascii_prefix, ascii_prefix);
    }

    let mut byte_end = 0usize;
    let mut skipped = 0usize;
    let mut prefix_width = 0usize;
    let mut reached_width = None;
    for (index, grapheme) in text.grapheme_indices(true) {
        let end = index + grapheme.len();
        prefix_width = prefix_width.saturating_add(grapheme.width());
        if let Some(width) = reached_width {
            if prefix_width == width {
                byte_end = end;
                continue;
            }
            break;
        }

        byte_end = end;
        skipped = prefix_width;
        if prefix_width >= columns {
            reached_width = Some(prefix_width);
        }
    }

    (byte_end, skipped)
}

fn fit_normal_run_prefix_with_width(text: &str, width: usize) -> (usize, usize, bool) {
    let ascii_prefix = single_width_ascii_prefix_len(text, width);
    if ascii_prefix == text.len()
        || (ascii_prefix == width && next_byte_is_single_width_ascii(text, ascii_prefix))
    {
        return (ascii_prefix, ascii_prefix, ascii_prefix == text.len());
    }

    let mut byte_end = 0usize;
    let mut used = 0usize;
    for (index, grapheme) in text.grapheme_indices(true) {
        let end = index + grapheme.len();
        let prefix_width = used.saturating_add(grapheme.width());
        if prefix_width > width {
            return (byte_end, used, false);
        }
        byte_end = end;
        used = prefix_width;
    }

    (byte_end, used, true)
}

fn zero_width_prefix_len(text: &str) -> usize {
    let mut byte_end = 0usize;
    for (index, grapheme) in text.grapheme_indices(true) {
        let end = index + grapheme.len();
        if grapheme.width() > 0 {
            break;
        }
        byte_end = end;
    }
    byte_end
}

fn for_normal_run_width_units(text: &str, mut visit: impl FnMut(usize)) {
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if width > 0 {
            visit(width);
        }
    }
}

fn push_display_char_range(out: &mut String, ch: char, start: usize, end: usize) {
    if start >= end {
        return;
    }
    if ch == '\t' {
        push_spaces(out, end - start);
        return;
    }

    if ch.is_control() {
        let escape = control_escape(ch);
        out.push_str(&escape[start..end]);
        return;
    }

    debug_assert_eq!(start, 0);
    debug_assert_eq!(end, display_char_width(ch));
    out.push(ch);
}

fn control_escape_width(ch: char) -> usize {
    match ch {
        '\r' | '\n' | '\t' => 2,
        _ => 4 + (ch as u32).max(1).ilog(16) as usize + 1,
    }
}

fn push_control_escape(out: &mut String, ch: char) {
    match ch {
        '\r' => out.push_str("\\r"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        _ => {
            out.push_str("\\u{");
            push_hex(out, ch as u32);
            out.push('}');
        }
    }
}

fn control_escape(ch: char) -> String {
    let mut out = String::new();
    push_control_escape(&mut out, ch);
    out
}

fn push_hex(out: &mut String, mut value: u32) {
    let mut digits = [0u8; 8];
    let mut len = 0usize;
    loop {
        let digit = (value & 0xf) as u8;
        digits[digits.len() - 1 - len] = match digit {
            0..=9 => b'0' + digit,
            _ => b'a' + (digit - 10),
        };
        len += 1;
        value >>= 4;
        if value == 0 {
            break;
        }
    }
    for digit in &digits[digits.len() - len..] {
        out.push(*digit as char);
    }
}

fn push_spaces(out: &mut String, width: usize) {
    if width <= STATIC_SPACES.len() {
        out.push_str(&STATIC_SPACES[..width]);
    } else {
        out.extend(std::iter::repeat_n(' ', width));
    }
}

fn single_width_ascii_prefix_len(text: &str, max_bytes: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(max_bytes)
        .take_while(|&&byte| is_single_width_printable_ascii(byte))
        .count()
}

fn next_byte_is_single_width_ascii(text: &str, index: usize) -> bool {
    text.as_bytes()
        .get(index)
        .is_some_and(|byte| is_single_width_printable_ascii(*byte))
}

fn is_single_width_printable_ascii(byte: u8) -> bool {
    (b' '..=b'~').contains(&byte)
}
