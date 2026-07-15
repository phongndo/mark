use std::collections::HashMap;

use mark_diff::{Changeset, DiffFile, DiffLine, DiffLineKind};

use crate::model::UiRow;

pub(crate) const ANNOTATION_ADD_BUTTON: &str = " [+]";
pub(crate) const ANNOTATION_ADD_BUTTON_WIDTH: usize = 4;
pub(crate) const ANNOTATION_CLOSE_BUTTON: &str = "[x]";
pub(crate) const ANNOTATION_CLOSE_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_SUBMIT_BUTTON: &str = "[✓]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_ASCII: &str = "[s]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_EDIT_BUTTON: &str = "[↻]";
pub(crate) const ANNOTATION_EDIT_BUTTON_ASCII: &str = "[e]";
pub(crate) const ANNOTATION_EDIT_BUTTON_WIDTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AnnotationKey {
    pub(crate) path: String,
    pub(crate) side: AnnotationSide,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum AnnotationSide {
    Old,
    New,
}

impl AnnotationSide {
    pub(crate) fn label(self) -> char {
        match self {
            Self::Old => 'L',
            Self::New => 'R',
        }
    }
}

impl AnnotationKey {
    pub(crate) fn from_ui_row(changeset: &Changeset, row: UiRow) -> Option<Self> {
        Self::candidates_from_ui_row(changeset, row)
            .into_iter()
            .next()
    }

    pub(crate) fn candidates_from_ui_row(changeset: &Changeset, row: UiRow) -> Vec<Self> {
        match row {
            UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
                let Some(file) = changeset.files.get(file.get()) else {
                    return Vec::new();
                };
                let Some(hunk) = file.hunks().get(hunk.get()) else {
                    return Vec::new();
                };
                Self::candidates_from_hunk_line(file, &hunk.lines, line.get())
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let Some(file) = changeset.files.get(file.get()) else {
                    return Vec::new();
                };
                let Some(hunk) = file.hunks().get(hunk.get()) else {
                    return Vec::new();
                };
                let lines = &hunk.lines;
                let mut candidates = Vec::with_capacity(1);
                if let Some(index) = right.get() {
                    // Prefer the right/current side when a split row has both sides;
                    // unpaired left-only rows remain old-side deletion marks.
                    if let Some(line) = lines.get(index.get()).and_then(|line| line.new_line()) {
                        Self::push_candidate(&mut candidates, file, AnnotationSide::New, line);
                    }
                    return candidates;
                }
                if let Some(index) = left.get() {
                    Self::push_unpaired_deletion_candidate(
                        &mut candidates,
                        file,
                        lines,
                        index.get(),
                    );
                }
                candidates
            }
            UiRow::ContextLine { file, new_line, .. } => {
                let Some(file) = changeset.files.get(file.get()) else {
                    return Vec::new();
                };
                Self::path_for_side(file, AnnotationSide::New)
                    .map(|path| vec![Self::new(path, AnnotationSide::New, new_line)])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn candidates_from_hunk_line(
        file: &DiffFile,
        lines: &[DiffLine],
        line_index: usize,
    ) -> Vec<Self> {
        let Some(line) = lines.get(line_index) else {
            return Vec::new();
        };

        let mut candidates = Vec::with_capacity(1);
        match line.kind() {
            DiffLineKind::Context => {
                if let Some(line) = line.new_line() {
                    Self::push_candidate(&mut candidates, file, AnnotationSide::New, line);
                } else if let Some(line) = line.old_line() {
                    Self::push_candidate(&mut candidates, file, AnnotationSide::Old, line);
                }
            }
            DiffLineKind::Addition => {
                if let Some(line) = line.new_line() {
                    Self::push_candidate(&mut candidates, file, AnnotationSide::New, line);
                }
            }
            DiffLineKind::Deletion => {
                Self::push_unpaired_deletion_candidate(&mut candidates, file, lines, line_index);
            }
            DiffLineKind::Meta => {}
        }
        candidates
    }

    fn push_unpaired_deletion_candidate(
        candidates: &mut Vec<Self>,
        file: &DiffFile,
        lines: &[DiffLine],
        line_index: usize,
    ) {
        let Some(line) = lines.get(line_index) else {
            return;
        };
        if !matches!(line.kind(), DiffLineKind::Deletion)
            || deletion_has_paired_addition(lines, line_index)
        {
            return;
        }
        if let Some(line) = line.old_line() {
            Self::push_candidate(candidates, file, AnnotationSide::Old, line);
        }
    }

    fn push_candidate(
        candidates: &mut Vec<Self>,
        file: &DiffFile,
        side: AnnotationSide,
        line: usize,
    ) {
        if let Some(path) = Self::path_for_side(file, side) {
            candidates.push(Self::new(path, side, line));
        }
    }

    pub(crate) fn path_for_side(file: &DiffFile, side: AnnotationSide) -> Option<&str> {
        match side {
            AnnotationSide::Old => file.old_path().or(file.new_path()),
            AnnotationSide::New => file.new_path().or(file.old_path()),
        }
    }

    fn new(path: &str, side: AnnotationSide, line: usize) -> Self {
        Self {
            path: path.to_owned(),
            side,
            line,
        }
    }
}

pub(crate) fn paired_old_line_for_addition(
    lines: &[DiffLine],
    addition_index: usize,
) -> Option<usize> {
    let (deletions, additions) = change_block_line_indices(lines, addition_index)?;
    let pair_index = additions
        .iter()
        .position(|index| *index == addition_index)?;
    let deletion_index = *deletions.get(pair_index)?;
    lines.get(deletion_index)?.old_line()
}

fn deletion_has_paired_addition(lines: &[DiffLine], deletion_index: usize) -> bool {
    let Some((deletions, additions)) = change_block_line_indices(lines, deletion_index) else {
        return false;
    };
    let Some(pair_index) = deletions.iter().position(|index| *index == deletion_index) else {
        return false;
    };
    pair_index < additions.len()
}

fn change_block_line_indices(lines: &[DiffLine], index: usize) -> Option<(Vec<usize>, Vec<usize>)> {
    if !lines.get(index).is_some_and(is_change_line) {
        return None;
    }

    let mut start = index;
    while start > 0 && lines.get(start - 1).is_some_and(is_change_block_line) {
        start -= 1;
    }

    let mut end = index + 1;
    while end < lines.len() && lines.get(end).is_some_and(is_change_block_line) {
        end += 1;
    }

    let mut deletions = Vec::new();
    let mut additions = Vec::new();
    for (offset, line) in lines[start..end].iter().enumerate() {
        let line_index = start + offset;
        match line.kind() {
            DiffLineKind::Deletion => deletions.push(line_index),
            DiffLineKind::Addition => additions.push(line_index),
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    Some((deletions, additions))
}

fn is_change_line(line: &DiffLine) -> bool {
    matches!(line.kind(), DiffLineKind::Deletion | DiffLineKind::Addition)
}

fn is_change_block_line(line: &DiffLine) -> bool {
    matches!(
        line.kind(),
        DiffLineKind::Deletion | DiffLineKind::Addition | DiffLineKind::Meta
    )
}

#[derive(Debug, Clone)]
pub(crate) struct AnnotationDraft {
    pub(crate) key: AnnotationKey,
    pub(crate) model_row_index: usize,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

pub(crate) type AnnotationStore = HashMap<AnnotationKey, String>;

pub(crate) const ANNOTATION_HINT_ALPHABET: &str = mark_syntax::DEFAULT_ANNOTATION_HINT_KEYS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnnotationTarget {
    pub(crate) key: AnnotationKey,
    pub(crate) model_row_index: usize,
    pub(crate) visual_scroll: usize,
    pub(crate) viewport_row: usize,
    pub(crate) hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnnotationTargetMode {
    pub(crate) targets: Vec<AnnotationTarget>,
    pub(crate) prefix: String,
    pub(crate) sticky: bool,
}

impl AnnotationTargetMode {
    pub(crate) fn target_at_visual_scroll(
        &self,
        visual_scroll: usize,
    ) -> Option<&AnnotationTarget> {
        self.targets.iter().find(|target| {
            target.visual_scroll == visual_scroll && target.hint.starts_with(&self.prefix)
        })
    }

    pub(crate) fn matching_target_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|target| target.hint.starts_with(&self.prefix))
            .count()
    }
}

pub(crate) fn annotation_hint_codes(count: usize, hint_keys: &str) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }

    let mut alphabet = hint_keys.chars().collect::<Vec<_>>();
    if alphabet.len() < 2 {
        alphabet = ANNOTATION_HINT_ALPHABET.chars().collect();
    }
    let radix = alphabet.len();
    let mut depth = 1usize;
    let mut capacity = radix;
    while capacity < count {
        depth = depth.saturating_add(1);
        capacity = capacity.saturating_mul(radix);
    }

    if depth == 1 {
        return (0..count)
            .map(|index| fixed_width_hint(index, 1, &alphabet))
            .collect();
    }

    let shorter_capacity = capacity / radix;
    // Use as many short leaves as possible while leaving enough unused
    // prefixes to fan out into the remaining targets. This keeps every hint
    // prefix-free, so a complete short hint can be selected immediately.
    let short_count = capacity
        .saturating_sub(count)
        .saturating_div(radix.saturating_sub(1))
        .min(shorter_capacity);
    let mut hints = Vec::with_capacity(count);
    for index in 0..short_count {
        hints.push(fixed_width_hint(index, depth - 1, &alphabet));
    }

    let mut remaining = count.saturating_sub(short_count);
    for prefix_index in short_count..shorter_capacity {
        if remaining == 0 {
            break;
        }
        let prefix = fixed_width_hint(prefix_index, depth - 1, &alphabet);
        for character in &alphabet {
            if remaining == 0 {
                break;
            }
            let mut hint = String::with_capacity(depth);
            hint.push_str(&prefix);
            hint.push(*character);
            hints.push(hint);
            remaining -= 1;
        }
    }

    hints
}

fn fixed_width_hint(mut index: usize, width: usize, alphabet: &[char]) -> String {
    let radix = alphabet.len();
    let mut hint = vec![alphabet[0]; width];
    for position in (0..width).rev() {
        hint[position] = alphabet[index % radix];
        index /= radix;
    }
    hint.into_iter().collect()
}

#[cfg(test)]
mod annotation_hint_tests {
    use std::collections::HashSet;

    use super::{ANNOTATION_HINT_ALPHABET, annotation_hint_codes};

    fn assert_prefix_free(hints: &[String]) {
        let unique = hints.iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), hints.len());
        for (index, hint) in hints.iter().enumerate() {
            assert!(
                hints
                    .iter()
                    .enumerate()
                    .all(|(other_index, other)| index == other_index || !other.starts_with(hint)),
                "{hint:?} is a prefix of another annotation hint"
            );
        }
    }

    #[test]
    fn annotation_hints_use_easy_single_keys_when_they_fit() {
        let hints = annotation_hint_codes(26, ANNOTATION_HINT_ALPHABET);

        assert_eq!(hints.first().map(String::as_str), Some("a"));
        assert_eq!(hints.get(1).map(String::as_str), Some("s"));
        assert!(hints.iter().all(|hint| hint.len() == 1));
        assert_prefix_free(&hints);
    }

    #[test]
    fn annotation_hints_mix_short_and_long_prefix_free_codes() {
        let hints = annotation_hint_codes(27, ANNOTATION_HINT_ALPHABET);

        assert_eq!(hints.iter().filter(|hint| hint.len() == 1).count(), 25);
        assert_eq!(hints.iter().filter(|hint| hint.len() == 2).count(), 2);
        assert_prefix_free(&hints);
    }

    #[test]
    fn annotation_hints_scale_beyond_two_keys_without_collisions() {
        let hints = annotation_hint_codes(677, ANNOTATION_HINT_ALPHABET);

        assert_eq!(hints.len(), 677);
        assert_eq!(hints.iter().filter(|hint| hint.len() == 2).count(), 675);
        assert_eq!(hints.iter().filter(|hint| hint.len() == 3).count(), 2);
        assert_prefix_free(&hints);
    }

    #[test]
    fn annotation_hints_use_the_configured_unicode_alphabet() {
        let hints = annotation_hint_codes(4, "αβγδ");

        assert_eq!(hints, ["α", "β", "γ", "δ"]);
        assert_prefix_free(&hints);
    }
}
