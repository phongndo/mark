use std::collections::HashMap;

use mark_diff::{Changeset, DiffFile, DiffLine, DiffLineKind};

use crate::model::UiRow;

pub(crate) const ANNOTATION_CLOSE_BUTTON: &str = "[x]";
pub(crate) const ANNOTATION_CLOSE_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_SUBMIT_BUTTON: &str = "[✓]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_ASCII: &str = "[s]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_EDIT_BUTTON: &str = "[↻]";
pub(crate) const ANNOTATION_EDIT_BUTTON_ASCII: &str = "[e]";
pub(crate) const ANNOTATION_EDIT_BUTTON_WIDTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct AnnotationKey {
    pub(crate) path: String,
    pub(crate) side: AnnotationSide,
    pub(crate) line: usize,
    pub(crate) scope: AnnotationScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum AnnotationScope {
    File,
    Hunk {
        old_start: usize,
        old_count: usize,
        new_start: usize,
        new_count: usize,
    },
    Line,
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
    const CURSOR_ONLY_PATH: &'static str = "\0mark-cursor";

    pub(crate) fn from_ui_row(changeset: &Changeset, row: UiRow) -> Option<Self> {
        match row {
            UiRow::FileHeader(file) => Self::for_file(changeset.files.get(file.get())?),
            UiRow::HunkHeader { file, hunk } => {
                let file = changeset.files.get(file.get())?;
                Self::for_hunk(file, file.hunks().get(hunk.get())?)
            }
            UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
                let file = changeset.files.get(file.get())?;
                let hunk = file.hunks().get(hunk.get())?;
                Self::from_hunk_line(file, &hunk.lines, line.get())
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let file = changeset.files.get(file.get())?;
                let hunk = file.hunks().get(hunk.get())?;
                let lines = &hunk.lines;
                if let Some(index) = right.get() {
                    // Prefer the right/current side when a split row has both sides;
                    // left-only rows remain old-side deletion marks.
                    let line = lines.get(index.get())?.new_line()?;
                    return Self::for_file_line(file, AnnotationSide::New, line);
                }
                Self::deletion_candidate(file, lines, left.get()?.get())
            }
            UiRow::ContextLine {
                file,
                old_line,
                new_line,
            } => {
                let file = changeset.files.get(file.get())?;
                if file.new_path().is_none() {
                    Self::for_file_line(file, AnnotationSide::Old, old_line)
                } else {
                    Self::for_file_line(file, AnnotationSide::New, new_line)
                }
            }
            _ => None,
        }
    }

    pub(crate) fn candidates_from_ui_row(changeset: &Changeset, row: UiRow) -> Vec<Self> {
        let UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } = row
        else {
            return Self::from_ui_row(changeset, row).into_iter().collect();
        };
        let Some(file) = changeset.files.get(file.get()) else {
            return Vec::new();
        };
        let Some(hunk) = file.hunks().get(hunk.get()) else {
            return Vec::new();
        };
        let lines = &hunk.lines;
        let old = left
            .get()
            .and_then(|index| Self::deletion_candidate(file, lines, index.get()));
        let new = right.get().and_then(|index| {
            let line = lines.get(index.get())?.new_line()?;
            Self::for_file_line(file, AnnotationSide::New, line)
        });
        old.into_iter().chain(new).collect()
    }

    fn from_hunk_line(file: &DiffFile, lines: &[DiffLine], line_index: usize) -> Option<Self> {
        let line = lines.get(line_index)?;
        match line.kind() {
            DiffLineKind::Context => line
                .new_line()
                .and_then(|line| Self::for_file_line(file, AnnotationSide::New, line))
                .or_else(|| {
                    line.old_line()
                        .and_then(|line| Self::for_file_line(file, AnnotationSide::Old, line))
                }),
            DiffLineKind::Addition => line
                .new_line()
                .and_then(|line| Self::for_file_line(file, AnnotationSide::New, line)),
            DiffLineKind::Deletion => Self::deletion_candidate(file, lines, line_index),
            DiffLineKind::Meta => None,
        }
    }

    fn deletion_candidate(file: &DiffFile, lines: &[DiffLine], line_index: usize) -> Option<Self> {
        let line = lines.get(line_index)?;
        if !matches!(line.kind(), DiffLineKind::Deletion) {
            return None;
        }
        Self::for_file_line(file, AnnotationSide::Old, line.old_line()?)
    }

    fn for_file_line(file: &DiffFile, side: AnnotationSide, line: usize) -> Option<Self> {
        Self::path_for_side(file, side)
            .map(|path| Self::new(path, side, line, AnnotationScope::Line))
    }

    pub(crate) fn path_for_side(file: &DiffFile, side: AnnotationSide) -> Option<&str> {
        match side {
            AnnotationSide::Old => file.old_path().or(file.new_path()),
            AnnotationSide::New => file.new_path().or(file.old_path()),
        }
    }

    fn for_file(file: &DiffFile) -> Option<Self> {
        let (path, side) = preferred_file_path_and_side(file)?;
        Some(Self::new(path, side, 0, AnnotationScope::File))
    }

    fn for_hunk(file: &DiffFile, hunk: &mark_diff::DiffHunk) -> Option<Self> {
        let (path, side) = preferred_file_path_and_side(file)?;
        let line = match side {
            AnnotationSide::Old => hunk.old_start(),
            AnnotationSide::New => hunk.new_start(),
        };
        Some(Self::new(
            path,
            side,
            line,
            AnnotationScope::Hunk {
                old_start: hunk.old_start(),
                old_count: hunk.old_count(),
                new_start: hunk.new_start(),
                new_count: hunk.new_count(),
            },
        ))
    }

    fn new(path: &str, side: AnnotationSide, line: usize, scope: AnnotationScope) -> Self {
        Self {
            path: path.to_owned(),
            side,
            line,
            scope,
        }
    }

    pub(crate) fn cursor_only(model_row: usize) -> Self {
        Self::new(
            Self::CURSOR_ONLY_PATH,
            AnnotationSide::New,
            model_row,
            AnnotationScope::Line,
        )
    }

    pub(crate) fn is_cursor_only(&self) -> bool {
        self.path == Self::CURSOR_ONLY_PATH
    }

    pub(crate) fn is_line(&self) -> bool {
        matches!(self.scope, AnnotationScope::Line) && !self.is_cursor_only()
    }
}

fn preferred_file_path_and_side(file: &DiffFile) -> Option<(&str, AnnotationSide)> {
    file.new_path()
        .map(|path| (path, AnnotationSide::New))
        .or_else(|| file.old_path().map(|path| (path, AnnotationSide::Old)))
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
    pub(crate) visual_height: usize,
    pub(crate) viewport_row: usize,
    pub(crate) hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnnotationCursor {
    /// Identity of the model whose row coordinates these targets use.
    pub(crate) model_identity: u64,
    /// Eager cursors include every diff row, including structural rows that
    /// cannot hold annotations. Large models keep only the selected row.
    pub(crate) targets: Vec<AnnotationTarget>,
    pub(crate) selected: usize,
    /// Explicit key choices retained while revisiting rows in this model.
    pub(crate) preferred_keys: HashMap<usize, AnnotationKey>,
    /// Large models discover adjacent rows on demand instead of cloning the
    /// complete row list up front.
    pub(crate) lazy: bool,
    /// A failed lazy move is cached until the selected target changes.
    pub(crate) previous_exhausted: bool,
    pub(crate) next_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnnotationTargetMode {
    pub(crate) targets: Vec<AnnotationTarget>,
    pub(crate) prefix: String,
    pub(crate) sticky: bool,
}

impl AnnotationTargetMode {
    pub(crate) fn targets_at_visual_scroll(
        &self,
        visual_scroll: usize,
    ) -> impl Iterator<Item = &AnnotationTarget> {
        self.targets.iter().filter(move |target| {
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
