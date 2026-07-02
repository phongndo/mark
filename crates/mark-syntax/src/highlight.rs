use crate::detect_language_name;
#[cfg(test)]
use crate::{HighlightedLine, SyntaxClass, SyntaxSegment};

pub fn detect_language_from_path(path: &str) -> Option<String> {
    detect_language_name(path)
}

#[cfg(test)]
pub(crate) fn push_source_segment(
    lines: &mut [HighlightedLine],
    line_index: &mut usize,
    byte_start: usize,
    mut bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    let mut offset = 0usize;
    while let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        push_line_segment(
            lines,
            *line_index,
            byte_start.saturating_add(offset),
            byte_start.saturating_add(offset).saturating_add(newline),
            &bytes[..newline],
            class,
        );
        *line_index = line_index
            .saturating_add(1)
            .min(lines.len().saturating_sub(1));
        offset = offset.saturating_add(newline + 1);
        bytes = &bytes[newline + 1..];
    }
    push_line_segment(
        lines,
        *line_index,
        byte_start.saturating_add(offset),
        byte_start
            .saturating_add(offset)
            .saturating_add(bytes.len()),
        bytes,
        class,
    );
}

#[cfg(test)]
pub(crate) fn push_line_segment(
    lines: &mut [HighlightedLine],
    line_index: usize,
    byte_start: usize,
    byte_end: usize,
    bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    if bytes.is_empty() || line_index >= lines.len() {
        return;
    }

    let Some(last) = lines[line_index].segments.last_mut() else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            class,
        });
        return;
    };

    if last.class == class && last.byte_end == byte_start {
        last.byte_end = byte_end;
    } else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            class,
        });
    }
}

#[cfg(test)]
pub(crate) fn syntax_class(name: &str) -> Option<SyntaxClass> {
    mark_textmate::classify_scope_name(name)
}
