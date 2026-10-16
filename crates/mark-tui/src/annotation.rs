use std::collections::HashMap;

use mark_diff::{Changeset, DiffFile, DiffLine, DiffLineKind};

use crate::{
    model::UiRow,
    syntax::{DiffSide, changed_block_around, path_for_side_or_other},
};

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

    pub(crate) fn sign(self) -> char {
        match self {
            Self::Old => '-',
            Self::New => '+',
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

    pub(crate) fn for_file_line(
        file: &DiffFile,
        side: AnnotationSide,
        line: usize,
    ) -> Option<Self> {
        Self::path_for_side(file, side)
            .map(|path| Self::new(path, side, line, AnnotationScope::Line))
    }

    pub(crate) fn path_for_side(file: &DiffFile, side: AnnotationSide) -> Option<&str> {
        let side = match side {
            AnnotationSide::Old => DiffSide::Old,
            AnnotationSide::New => DiffSide::New,
        };
        path_for_side_or_other(file, side)
    }

    pub(crate) fn for_file(file: &DiffFile) -> Option<Self> {
        let (path, side) = preferred_file_path_and_side(file)?;
        Some(Self::new(path, side, 0, AnnotationScope::File))
    }

    pub(crate) fn for_hunk(file: &DiffFile, hunk: &mark_diff::DiffHunk) -> Option<Self> {
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

    /// Index of the hunk in `file` that a hunk-scoped key was created from.
    pub(crate) fn hunk_index_in(&self, file: &DiffFile) -> Option<usize> {
        let AnnotationScope::Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
        } = self.scope
        else {
            return None;
        };
        file.hunks().iter().position(|hunk| {
            hunk.old_start() == old_start
                && hunk.old_count() == old_count
                && hunk.new_start() == new_start
                && hunk.new_count() == new_count
        })
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

    pub(crate) fn target_label(&self) -> String {
        match self.scope {
            AnnotationScope::File => "file".to_owned(),
            AnnotationScope::Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
            } => format!(
                "hunk {} → {}",
                format_source_range(AnnotationSide::Old, old_start, old_count),
                format_source_range(AnnotationSide::New, new_start, new_count)
            ),
            AnnotationScope::Line => format!("{}{}", self.side.sign(), self.line),
        }
    }

    /// Path-qualified label used to list a mark in the annotation menu.
    pub(crate) fn menu_label(&self) -> String {
        let target = match self.scope {
            AnnotationScope::File => "file".to_owned(),
            AnnotationScope::Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
            } => format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
            AnnotationScope::Line => format!("{}{}", self.side.label(), self.line),
        };
        format!("{} {target}", self.path)
    }
}

fn format_source_range(side: AnnotationSide, start: usize, count: usize) -> String {
    let sign = side.sign();
    if count <= 1 {
        format!("{sign}{start}")
    } else {
        let end = start.saturating_add(count.saturating_sub(1));
        format!("{sign}{start}–{end}")
    }
}

fn preferred_file_path_and_side(file: &DiffFile) -> Option<(&str, AnnotationSide)> {
    file.new_path()
        .map(|path| (path, AnnotationSide::New))
        .or_else(|| file.old_path().map(|path| (path, AnnotationSide::Old)))
}

/// Whether `path` names `file`: either side of a rename, or its display path.
pub(crate) fn diff_file_has_path(file: &DiffFile, path: &str) -> bool {
    file.old_path() == Some(path) || file.new_path() == Some(path) || file.display_path() == path
}

pub(crate) fn paired_old_line_for_addition(
    lines: &[DiffLine],
    addition_index: usize,
) -> Option<usize> {
    let block = changed_block_around(lines, addition_index, usize::MAX)?;
    let (deletion_index, _) = block.paired_lines(addition_index, DiffLineKind::Addition)?;
    lines.get(deletion_index)?.old_line()
}

#[derive(Debug, Clone)]
pub(crate) struct AnnotationDraft {
    pub(crate) key: AnnotationKey,
    pub(crate) model_row_index: usize,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Default)]
pub(crate) struct AnnotationKeyIndex {
    pub(crate) anchors_by_model_row: HashMap<usize, Vec<AnnotationKey>>,
}

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
    /// Cursor is on the saved mark card under the current row, not the code.
    pub(crate) on_mark: bool,
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
