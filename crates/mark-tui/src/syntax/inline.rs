use std::{collections::HashMap, ops::Deref, sync::Arc};

use mark_diff::{DiffLine, DiffLineKind};

use crate::theme::{MAX_INLINE_DIFF_LINE_BYTES, MAX_INLINE_DIFF_TOKENS};

const MAX_INLINE_EAGER_HUNK_LINES: usize = 2_048;
const MAX_INLINE_CHANGED_BLOCK_LINES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineHunkKey {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) hunk: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineRange {
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InlineRanges(Option<Arc<[InlineRange]>>);

impl From<Vec<InlineRange>> for InlineRanges {
    fn from(ranges: Vec<InlineRange>) -> Self {
        if ranges.is_empty() {
            Self::default()
        } else {
            Self(Some(Arc::from(ranges)))
        }
    }
}

impl Deref for InlineRanges {
    type Target = [InlineRange];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InlineLineEmphasis {
    pub(crate) ranges: InlineRanges,
}

#[derive(Debug)]
pub(crate) struct InlineHunkEmphasisCache {
    pub(crate) lines: HashMap<usize, InlineLineEmphasis>,
    pub(crate) blocks: Vec<InlineChangedBlock>,
    line_count: usize,
}

#[derive(Debug)]
pub(crate) struct InlineChangedBlock {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) deletions: Vec<usize>,
    pub(crate) additions: Vec<usize>,
}

impl InlineHunkEmphasisCache {
    pub(crate) fn new(lines: &[DiffLine]) -> Self {
        if lines.len() > MAX_INLINE_EAGER_HUNK_LINES {
            return Self {
                lines: HashMap::new(),
                blocks: Vec::new(),
                line_count: lines.len(),
            };
        }

        let mut blocks = Vec::new();
        let mut index = 0usize;

        while index < lines.len() {
            if !matches!(
                lines[index].kind(),
                DiffLineKind::Deletion | DiffLineKind::Addition
            ) {
                index += 1;
                continue;
            }

            let start = index;
            let mut deletions = Vec::new();
            let mut additions = Vec::new();
            while index < lines.len()
                && matches!(
                    lines[index].kind(),
                    DiffLineKind::Deletion | DiffLineKind::Addition
                )
            {
                match lines[index].kind() {
                    DiffLineKind::Deletion => deletions.push(index),
                    DiffLineKind::Addition => additions.push(index),
                    DiffLineKind::Context | DiffLineKind::Meta => {}
                }
                index += 1;
            }
            blocks.push(InlineChangedBlock {
                start,
                end: index,
                deletions,
                additions,
            });
        }

        Self {
            lines: HashMap::new(),
            blocks,
            line_count: lines.len(),
        }
    }

    pub(crate) fn ranges_for_line(&mut self, lines: &[DiffLine], line: usize) -> InlineRanges {
        if let Some(emphasis) = self.lines.get(&line) {
            return emphasis.ranges.clone();
        }

        self.compute_line(lines, line);
        self.lines
            .get(&line)
            .map(|emphasis| emphasis.ranges.clone())
            .unwrap_or_default()
    }

    pub(crate) fn compute_line(&mut self, lines: &[DiffLine], line: usize) {
        let Some(diff_line) = lines.get(line) else {
            return;
        };
        if !matches!(
            diff_line.kind(),
            DiffLineKind::Deletion | DiffLineKind::Addition
        ) {
            self.set_emphasis(line, Vec::new());
            return;
        }

        let pair = if let Some(block) = self
            .blocks
            .iter()
            .find(|block| line >= block.start && line < block.end)
        {
            paired_changed_lines(block, line, diff_line.kind())
        } else {
            inline_changed_block_around(lines, line)
                .as_ref()
                .and_then(|block| paired_changed_lines(block, line, diff_line.kind()))
        };
        let Some((old_index, new_index)) = pair else {
            self.set_emphasis(line, Vec::new());
            return;
        };

        let old_text = lines[old_index].text_lossy();
        let new_text = lines[new_index].text_lossy();
        let (old_ranges, new_ranges) = changed_token_ranges(&old_text, &new_text);
        self.set_emphasis(old_index, old_ranges);
        self.set_emphasis(new_index, new_ranges);
    }

    pub(crate) fn set_emphasis(&mut self, line: usize, ranges: Vec<InlineRange>) {
        if line < self.line_count {
            self.lines.insert(
                line,
                InlineLineEmphasis {
                    ranges: ranges.into(),
                },
            );
        }
    }
}

fn paired_changed_lines(
    block: &InlineChangedBlock,
    line: usize,
    kind: DiffLineKind,
) -> Option<(usize, usize)> {
    match kind {
        DiffLineKind::Deletion => {
            let pair_index = block.deletions.binary_search(&line).ok()?;
            Some((line, *block.additions.get(pair_index)?))
        }
        DiffLineKind::Addition => {
            let pair_index = block.additions.binary_search(&line).ok()?;
            Some((*block.deletions.get(pair_index)?, line))
        }
        DiffLineKind::Context | DiffLineKind::Meta => None,
    }
}

fn inline_changed_block_around(lines: &[DiffLine], line: usize) -> Option<InlineChangedBlock> {
    let diff_line = lines.get(line)?;
    if !matches!(
        diff_line.kind(),
        DiffLineKind::Deletion | DiffLineKind::Addition
    ) {
        return None;
    }

    let mut start = line;
    let mut scanned = 0usize;
    while start > 0
        && matches!(
            lines[start - 1].kind(),
            DiffLineKind::Deletion | DiffLineKind::Addition
        )
    {
        start -= 1;
        scanned += 1;
        if scanned > MAX_INLINE_CHANGED_BLOCK_LINES {
            return None;
        }
    }

    let mut end = line.saturating_add(1);
    while end < lines.len()
        && matches!(
            lines[end].kind(),
            DiffLineKind::Deletion | DiffLineKind::Addition
        )
    {
        end += 1;
        scanned += 1;
        if scanned > MAX_INLINE_CHANGED_BLOCK_LINES {
            return None;
        }
    }

    let mut deletions = Vec::new();
    let mut additions = Vec::new();
    for (offset, diff_line) in lines[start..end].iter().enumerate() {
        match diff_line.kind() {
            DiffLineKind::Deletion => deletions.push(start + offset),
            DiffLineKind::Addition => additions.push(start + offset),
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    Some(InlineChangedBlock {
        start,
        end,
        deletions,
        additions,
    })
}

#[cfg(test)]
pub(crate) fn compute_hunk_inline_emphasis(lines: &[DiffLine]) -> Vec<InlineLineEmphasis> {
    let mut emphasis = vec![InlineLineEmphasis::default(); lines.len()];
    let mut index = 0usize;

    while index < lines.len() {
        match lines[index].kind() {
            DiffLineKind::Deletion | DiffLineKind::Addition => {
                let mut deletions = Vec::new();
                let mut additions = Vec::new();
                while index < lines.len()
                    && matches!(
                        lines[index].kind(),
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    match lines[index].kind() {
                        DiffLineKind::Deletion => deletions.push(index),
                        DiffLineKind::Addition => additions.push(index),
                        DiffLineKind::Context | DiffLineKind::Meta => {}
                    }
                    index += 1;
                }
                compute_changed_block_inline_emphasis(lines, &deletions, &additions, &mut emphasis);
            }
            DiffLineKind::Context | DiffLineKind::Meta => index += 1,
        }
    }

    emphasis
}

#[cfg(test)]
pub(crate) fn compute_changed_block_inline_emphasis(
    lines: &[DiffLine],
    deletions: &[usize],
    additions: &[usize],
    emphasis: &mut [InlineLineEmphasis],
) {
    let paired_rows = deletions.len().max(additions.len());
    for pair_index in 0..paired_rows {
        match (deletions.get(pair_index), additions.get(pair_index)) {
            (Some(deletion), Some(addition)) => {
                let old_text = lines[*deletion].text_lossy();
                let new_text = lines[*addition].text_lossy();
                let (old_ranges, new_ranges) = changed_token_ranges(&old_text, &new_text);
                emphasis[*deletion].ranges = old_ranges.into();
                emphasis[*addition].ranges = new_ranges.into();
            }
            (Some(deletion), None) => {
                emphasis[*deletion].ranges = InlineRanges::default();
            }
            (None, Some(addition)) => {
                emphasis[*addition].ranges = InlineRanges::default();
            }
            (None, None) => {}
        }
    }
}

pub(crate) fn changed_token_ranges(old: &str, new: &str) -> (Vec<InlineRange>, Vec<InlineRange>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    if old.len() > MAX_INLINE_DIFF_LINE_BYTES || new.len() > MAX_INLINE_DIFF_LINE_BYTES {
        return (Vec::new(), Vec::new());
    }

    let old_tokens = inline_tokens(old);
    let new_tokens = inline_tokens(new);
    if old_tokens.len() > MAX_INLINE_DIFF_TOKENS || new_tokens.len() > MAX_INLINE_DIFF_TOKENS {
        return (Vec::new(), Vec::new());
    }

    let mut old_changed = vec![true; old_tokens.len()];
    let mut new_changed = vec![true; new_tokens.len()];
    mark_unchanged_lcs_tokens(
        old,
        &old_tokens,
        new,
        &new_tokens,
        &mut old_changed,
        &mut new_changed,
    );

    (
        inline_ranges_from_tokens(&old_tokens, &old_changed),
        inline_ranges_from_tokens(&new_tokens, &new_changed),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineToken {
    pub(crate) byte_start: u32,
    pub(crate) byte_end: u32,
    fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineCharClass {
    Word,
    Whitespace,
    Other,
}

pub(crate) fn inline_tokens(text: &str) -> Vec<InlineToken> {
    // Inline comparison rejects sources above 4 KiB before tokenization, so
    // compact offsets preserve the old token size while leaving ample headroom.
    assert!(
        text.len() <= u32::MAX as usize,
        "inline token source exceeds 4 GiB"
    );
    if text.is_ascii() {
        return inline_tokens_ascii(text);
    }

    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let class = inline_char_class(ch);
        let mut end = start + ch.len_utf8();

        if class != InlineCharClass::Other {
            while let Some((_, next)) = chars.peek().copied() {
                if inline_char_class(next) != class {
                    break;
                }
                let Some((next_start, next)) = chars.next() else {
                    break;
                };
                end = next_start + next.len_utf8();
            }
        }

        tokens.push(InlineToken {
            byte_start: start as u32,
            byte_end: end as u32,
            fingerprint: inline_token_fingerprint(&text[start..end]),
        });
    }

    tokens
}

fn inline_tokens_ascii(text: &str) -> Vec<InlineToken> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut start = 0usize;

    while start < bytes.len() {
        let class = inline_ascii_class(bytes[start]);
        let mut end = start + 1;

        if class != InlineCharClass::Other {
            while end < bytes.len() && inline_ascii_class(bytes[end]) == class {
                end += 1;
            }
        }

        tokens.push(InlineToken {
            byte_start: start as u32,
            byte_end: end as u32,
            fingerprint: inline_token_fingerprint(&text[start..end]),
        });
        start = end;
    }

    tokens
}

fn inline_token_fingerprint(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(crate) fn inline_ascii_class(byte: u8) -> InlineCharClass {
    if byte.is_ascii_whitespace() || byte == 0x0B {
        InlineCharClass::Whitespace
    } else if byte == b'_' || byte.is_ascii_alphanumeric() {
        InlineCharClass::Word
    } else {
        InlineCharClass::Other
    }
}

pub(crate) fn inline_char_class(ch: char) -> InlineCharClass {
    if ch.is_whitespace() {
        InlineCharClass::Whitespace
    } else if ch == '_' || ch.is_alphanumeric() {
        InlineCharClass::Word
    } else {
        InlineCharClass::Other
    }
}

pub(crate) fn mark_unchanged_lcs_tokens(
    old: &str,
    old_tokens: &[InlineToken],
    new: &str,
    new_tokens: &[InlineToken],
    old_changed: &mut [bool],
    new_changed: &mut [bool],
) {
    // Backtracking starts at the bottom-right cell and unconditionally selects
    // equal tail tokens. Remove that exact suffix before sizing the matrix.
    let mut old_end = old_tokens.len();
    let mut new_end = new_tokens.len();
    while old_end > 0
        && new_end > 0
        && inline_tokens_equal(old, old_tokens[old_end - 1], new, new_tokens[new_end - 1])
    {
        old_end -= 1;
        new_end -= 1;
        old_changed[old_end] = false;
        new_changed[new_end] = false;
    }

    let cols = new_end + 1;
    let mut lengths = vec![0u16; (old_end + 1) * cols];

    for old_index in 0..old_end {
        for new_index in 0..new_end {
            let cell = (old_index + 1) * cols + new_index + 1;
            lengths[cell] =
                if inline_tokens_equal(old, old_tokens[old_index], new, new_tokens[new_index]) {
                    lengths[old_index * cols + new_index].saturating_add(1)
                } else {
                    lengths[old_index * cols + new_index + 1]
                        .max(lengths[(old_index + 1) * cols + new_index])
                };
        }
    }

    let mut old_index = old_end;
    let mut new_index = new_end;
    while old_index > 0 && new_index > 0 {
        if inline_tokens_equal(
            old,
            old_tokens[old_index - 1],
            new,
            new_tokens[new_index - 1],
        ) {
            old_changed[old_index - 1] = false;
            new_changed[new_index - 1] = false;
            old_index -= 1;
            new_index -= 1;
        } else if lengths[(old_index - 1) * cols + new_index]
            >= lengths[old_index * cols + new_index - 1]
        {
            old_index -= 1;
        } else {
            new_index -= 1;
        }
    }
}

// Fingerprints reject unequal tokens cheaply; exact bytes remain authoritative
// so collisions cannot change emphasis ranges.
fn inline_tokens_equal(
    old: &str,
    old_token: InlineToken,
    new: &str,
    new_token: InlineToken,
) -> bool {
    old_token.fingerprint == new_token.fingerprint
        && inline_token_text(old, old_token) == inline_token_text(new, new_token)
}

pub(crate) fn inline_token_text(text: &str, token: InlineToken) -> &str {
    &text[token.byte_start as usize..token.byte_end as usize]
}

pub(crate) fn inline_ranges_from_tokens(
    tokens: &[InlineToken],
    changed: &[bool],
) -> Vec<InlineRange> {
    let mut ranges: Vec<InlineRange> = Vec::new();
    for (token, is_changed) in tokens.iter().zip(changed) {
        if !*is_changed {
            continue;
        }
        let byte_start = token.byte_start as usize;
        let byte_end = token.byte_end as usize;
        if let Some(last) = ranges.last_mut()
            && last.byte_end == byte_start
        {
            last.byte_end = byte_end;
            continue;
        }
        ranges.push(InlineRange {
            byte_start,
            byte_end,
        });
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_trimmed_lcs_matches_full_matrix_reference() {
        const ATOMS: &[&str] = &["a", "b", " ", "_", "+", ";", "界", "é", "\t"];
        let mut state = 0x39d4_72a1_8bc0_5ef6u64;

        for case in 0..4_096 {
            let old = generated_inline_text(&mut state, ATOMS);
            let new = generated_inline_text(&mut state, ATOMS);
            let old_tokens = inline_tokens(&old);
            let new_tokens = inline_tokens(&new);
            let mut expected_old = vec![true; old_tokens.len()];
            let mut expected_new = vec![true; new_tokens.len()];
            let mut actual_old = expected_old.clone();
            let mut actual_new = expected_new.clone();

            reference_mark_unchanged_lcs_tokens(
                &old,
                &old_tokens,
                &new,
                &new_tokens,
                &mut expected_old,
                &mut expected_new,
            );
            mark_unchanged_lcs_tokens(
                &old,
                &old_tokens,
                &new,
                &new_tokens,
                &mut actual_old,
                &mut actual_new,
            );

            assert_eq!(actual_old, expected_old, "old-side mismatch in case {case}");
            assert_eq!(actual_new, expected_new, "new-side mismatch in case {case}");
        }
    }

    fn generated_inline_text(state: &mut u64, atoms: &[&str]) -> String {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let count = (*state as usize) % 32;
        let mut text = String::new();
        for _ in 0..count {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            text.push_str(atoms[*state as usize % atoms.len()]);
        }
        text
    }

    fn reference_mark_unchanged_lcs_tokens(
        old: &str,
        old_tokens: &[InlineToken],
        new: &str,
        new_tokens: &[InlineToken],
        old_changed: &mut [bool],
        new_changed: &mut [bool],
    ) {
        let cols = new_tokens.len() + 1;
        let mut lengths = vec![0u16; (old_tokens.len() + 1) * cols];
        for old_index in 0..old_tokens.len() {
            for new_index in 0..new_tokens.len() {
                let cell = (old_index + 1) * cols + new_index + 1;
                lengths[cell] = if inline_token_text(old, old_tokens[old_index])
                    == inline_token_text(new, new_tokens[new_index])
                {
                    lengths[old_index * cols + new_index].saturating_add(1)
                } else {
                    lengths[old_index * cols + new_index + 1]
                        .max(lengths[(old_index + 1) * cols + new_index])
                };
            }
        }

        let mut old_index = old_tokens.len();
        let mut new_index = new_tokens.len();
        while old_index > 0 && new_index > 0 {
            if inline_token_text(old, old_tokens[old_index - 1])
                == inline_token_text(new, new_tokens[new_index - 1])
            {
                old_changed[old_index - 1] = false;
                new_changed[new_index - 1] = false;
                old_index -= 1;
                new_index -= 1;
            } else if lengths[(old_index - 1) * cols + new_index]
                >= lengths[old_index * cols + new_index - 1]
            {
                old_index -= 1;
            } else {
                new_index -= 1;
            }
        }
    }

    #[test]
    fn inline_cache_for_large_hunk_is_sparse_but_keeps_visible_pair() {
        let mut lines = Vec::new();
        for index in 0..MAX_INLINE_EAGER_HUNK_LINES + 16 {
            lines.push(DiffLine::context(index + 1, index + 1, "same"));
        }
        let deletion = MAX_INLINE_EAGER_HUNK_LINES + 4;
        lines[deletion] = DiffLine::deletion(deletion + 1, "let value = old_name;");
        lines[deletion + 1] = DiffLine::addition(deletion + 1, "let value = new_name;");

        let mut cache = InlineHunkEmphasisCache::new(&lines);
        assert!(
            cache.blocks.is_empty(),
            "large hunks should not pre-index blocks"
        );

        let ranges = cache.ranges_for_line(&lines, deletion);
        assert!(
            !ranges.is_empty(),
            "visible local pair should still be emphasized"
        );
        assert!(
            cache.lines.len() <= 2,
            "cache should remain viewport-line sparse"
        );
    }

    #[test]
    fn inline_cache_reuses_nonempty_range_storage() {
        let lines = vec![
            DiffLine::deletion(1, "let value = old_name;"),
            DiffLine::addition(1, "let value = new_name;"),
        ];
        let mut cache = InlineHunkEmphasisCache::new(&lines);

        let first = cache.ranges_for_line(&lines, 0);
        let second = cache.ranges_for_line(&lines, 0);
        let (Some(first), Some(second)) = (&first.0, &second.0) else {
            panic!("changed pair should have shared ranges");
        };
        assert!(Arc::ptr_eq(first, second));
    }

    #[test]
    fn inline_cache_skips_oversized_changed_block() {
        let mut lines = Vec::new();
        for index in 0..MAX_INLINE_CHANGED_BLOCK_LINES + 2 {
            lines.push(DiffLine::deletion(index + 1, "old"));
        }
        for index in 0..MAX_INLINE_CHANGED_BLOCK_LINES + 2 {
            lines.push(DiffLine::addition(index + 1, "new"));
        }

        let mut cache = InlineHunkEmphasisCache::new(&lines);
        let ranges = cache.ranges_for_line(&lines, 0);
        assert!(
            ranges.is_empty(),
            "oversized changed blocks degrade to line style"
        );
        assert!(cache.lines.len() <= 1);
    }
}
