use std::{collections::HashMap, ops::Range, sync::Arc};

use mark_diff::{Changeset, DiffLine, DiffLineKind};

use crate::{controls::DiffLayoutMode, syntax::DiffSide};

const MAX_EAGER_UI_MODEL_ROWS: usize = 200_000;

macro_rules! typed_index {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) struct $name(u32);

        impl $name {
            pub(crate) const fn new(index: usize) -> Self {
                Self(if index > u32::MAX as usize {
                    u32::MAX
                } else {
                    index as u32
                })
            }

            pub(crate) const fn get(self) -> usize {
                self.0 as usize
            }
        }

        impl From<usize> for $name {
            fn from(index: usize) -> Self {
                Self::new(index)
            }
        }

        impl From<$name> for usize {
            fn from(index: $name) -> Self {
                index.get()
            }
        }

        impl<T> std::ops::Index<$name> for [T] {
            type Output = T;

            fn index(&self, index: $name) -> &Self::Output {
                &self[index.get()]
            }
        }

        impl<T> std::ops::IndexMut<$name> for [T] {
            fn index_mut(&mut self, index: $name) -> &mut Self::Output {
                &mut self[index.get()]
            }
        }

        impl<T> std::ops::Index<$name> for Vec<T> {
            type Output = T;

            fn index(&self, index: $name) -> &Self::Output {
                &self[index.get()]
            }
        }

        impl<T> std::ops::IndexMut<$name> for Vec<T> {
            fn index_mut(&mut self, index: $name) -> &mut Self::Output {
                &mut self[index.get()]
            }
        }
    };
}

typed_index!(FileIndex);
typed_index!(HunkIndex);
typed_index!(DiffLineIndex);
typed_index!(ModelRow);
typed_index!(VisualRow);
typed_index!(ScrollOffset);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MaybeDiffLineIndex(u32);

impl MaybeDiffLineIndex {
    const NONE: u32 = u32::MAX;

    pub(crate) const fn none() -> Self {
        Self(Self::NONE)
    }

    pub(crate) const fn some(index: DiffLineIndex) -> Self {
        Self(index.0)
    }

    pub(crate) const fn get(self) -> Option<DiffLineIndex> {
        if self.0 == Self::NONE {
            None
        } else {
            Some(DiffLineIndex(self.0))
        }
    }

    pub(crate) const fn is_some(self) -> bool {
        self.0 != Self::NONE
    }

    pub(crate) fn and_then<T>(self, f: impl FnOnce(DiffLineIndex) -> Option<T>) -> Option<T> {
        self.get().and_then(f)
    }

    pub(crate) fn or(self, other: Self) -> Option<DiffLineIndex> {
        self.get().or_else(|| other.get())
    }
}

impl From<Option<DiffLineIndex>> for MaybeDiffLineIndex {
    fn from(index: Option<DiffLineIndex>) -> Self {
        index.map(Self::some).unwrap_or_else(Self::none)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiRow {
    FileSeparator,
    FileHeader(FileIndex),
    FileBodyNotice(FileIndex),
    Collapsed {
        file: FileIndex,
        hunk: HunkIndex,
        old_start: u32,
        new_start: u32,
        lines: u32,
        expanded: u32,
    },
    ContextLine {
        file: FileIndex,
        old_line: usize,
        new_line: usize,
    },
    ContextHide {
        file: FileIndex,
        hunk: HunkIndex,
        lines: usize,
    },
    HunkHeader {
        file: FileIndex,
        hunk: HunkIndex,
    },
    UnifiedLine {
        file: FileIndex,
        hunk: HunkIndex,
        line: DiffLineIndex,
    },
    SplitLine {
        file: FileIndex,
        hunk: HunkIndex,
        left: MaybeDiffLineIndex,
        right: MaybeDiffLineIndex,
    },
    MetaLine {
        file: FileIndex,
        hunk: HunkIndex,
        line: DiffLineIndex,
    },
}

impl UiRow {
    pub(crate) fn typed_hunk_key(self) -> Option<(FileIndex, HunkIndex)> {
        match self {
            Self::HunkHeader { file, hunk }
            | Self::UnifiedLine { file, hunk, .. }
            | Self::SplitLine { file, hunk, .. }
            | Self::MetaLine { file, hunk, .. } => Some((file, hunk)),
            Self::FileSeparator
            | Self::FileHeader(_)
            | Self::FileBodyNotice(_)
            | Self::Collapsed { .. }
            | Self::ContextLine { .. }
            | Self::ContextHide { .. } => None,
        }
    }

    pub(crate) fn hunk_key(self) -> Option<(usize, usize)> {
        self.typed_hunk_key()
            .map(|(file, hunk)| (file.get(), hunk.get()))
    }

    pub(crate) fn is_hunk_row(self, file: usize, hunk: usize) -> bool {
        self.hunk_key() == Some((file, hunk))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ContextKey {
    pub(crate) file: FileIndex,
    /// The hunk whose surrounding context is expanded. A value one past the
    /// final hunk is used for trailing context after that final hunk.
    pub(crate) hunk: HunkIndex,
}

pub(crate) fn context_expands_up(hunk: HunkIndex) -> bool {
    hunk.get() == 0
}

/// Git encodes a zero-count hunk range at the line before the change. Convert
/// that position to the first line at or after the change so context remains
/// ordered around pure insertions and deletions.
pub(crate) fn normalized_hunk_start(start: usize, count: usize) -> usize {
    start.saturating_add(usize::from(count == 0))
}

pub(crate) fn line_after_hunk(start: usize, count: usize) -> usize {
    normalized_hunk_start(start, count).saturating_add(count)
}

pub(crate) fn row_count_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ContextSourceKey {
    pub(crate) file: FileIndex,
    pub(crate) side: DiffSide,
}

#[derive(Debug, Clone)]
pub(crate) enum ContextSourceEntry {
    Lines(Arc<Vec<String>>),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiModel {
    pub(crate) rows: Vec<UiRow>,
    row_count: usize,
    row_segments: Vec<RowSegment>,
    pub(crate) file_start_rows: Vec<Option<ModelRow>>,
    pub(crate) file_row_starts: Vec<(FileIndex, ModelRow)>,
    pub(crate) visible_files: Vec<FileIndex>,
    pub(crate) hunk_start_rows: Vec<ModelRow>,
    pub(crate) hunk_row_starts: Vec<((FileIndex, HunkIndex), ModelRow)>,
    hunk_row_ends: Vec<ModelRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowSegment {
    start: ModelRow,
    len: u32,
    kind: RowSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowSegmentKind {
    FileSeparator,
    FileHeader(FileIndex),
    FileBodyNotice(FileIndex),
    Collapsed {
        file: FileIndex,
        hunk: HunkIndex,
        old_start: u32,
        new_start: u32,
        lines: u32,
        expanded: u32,
    },
    ContextLines {
        file: FileIndex,
        old_start: u32,
        new_start: u32,
    },
    ContextHide {
        file: FileIndex,
        hunk: HunkIndex,
        lines: u32,
    },
    HunkHeader {
        file: FileIndex,
        hunk: HunkIndex,
    },
    UnifiedLines {
        file: FileIndex,
        hunk: HunkIndex,
        line_start: u32,
    },
    SplitContextLines {
        file: FileIndex,
        hunk: HunkIndex,
        line_start: u32,
    },
    SplitMetaLines {
        file: FileIndex,
        hunk: HunkIndex,
        line_start: u32,
    },
    SplitChangeRun {
        file: FileIndex,
        hunk: HunkIndex,
        left_start: u32,
        left_len: u32,
        right_start: u32,
        right_len: u32,
    },
    SplitExplicit {
        file: FileIndex,
        hunk: HunkIndex,
        left: MaybeDiffLineIndex,
        right: MaybeDiffLineIndex,
    },
}

impl RowSegment {
    fn end(self) -> usize {
        self.start.get().saturating_add(self.len as usize)
    }

    fn row_at(self, row: usize) -> Option<UiRow> {
        if row < self.start.get() || row >= self.end() {
            return None;
        }
        self.kind.row_at(row.saturating_sub(self.start.get()))
    }
}

impl RowSegmentKind {
    fn row_at(self, offset: usize) -> Option<UiRow> {
        let offset_u32 = u32::try_from(offset).ok()?;
        Some(match self {
            Self::FileSeparator => UiRow::FileSeparator,
            Self::FileHeader(file) => UiRow::FileHeader(file),
            Self::FileBodyNotice(file) => UiRow::FileBodyNotice(file),
            Self::Collapsed {
                file,
                hunk,
                old_start,
                new_start,
                lines,
                expanded,
            } => UiRow::Collapsed {
                file,
                hunk,
                old_start,
                new_start,
                lines,
                expanded,
            },
            Self::ContextLines {
                file,
                old_start,
                new_start,
            } => UiRow::ContextLine {
                file,
                old_line: old_start.saturating_add(offset_u32) as usize,
                new_line: new_start.saturating_add(offset_u32) as usize,
            },
            Self::ContextHide { file, hunk, lines } => UiRow::ContextHide {
                file,
                hunk,
                lines: lines as usize,
            },
            Self::HunkHeader { file, hunk } => UiRow::HunkHeader { file, hunk },
            Self::UnifiedLines {
                file,
                hunk,
                line_start,
            } => UiRow::UnifiedLine {
                file,
                hunk,
                line: DiffLineIndex(line_start.saturating_add(offset_u32)),
            },
            Self::SplitContextLines {
                file,
                hunk,
                line_start,
            } => {
                let line = DiffLineIndex(line_start.saturating_add(offset_u32));
                UiRow::SplitLine {
                    file,
                    hunk,
                    left: MaybeDiffLineIndex::some(line),
                    right: MaybeDiffLineIndex::some(line),
                }
            }
            Self::SplitMetaLines {
                file,
                hunk,
                line_start,
            } => UiRow::MetaLine {
                file,
                hunk,
                line: DiffLineIndex(line_start.saturating_add(offset_u32)),
            },
            Self::SplitChangeRun {
                file,
                hunk,
                left_start,
                left_len,
                right_start,
                right_len,
            } => UiRow::SplitLine {
                file,
                hunk,
                left: if offset_u32 < left_len {
                    MaybeDiffLineIndex::some(DiffLineIndex(left_start.saturating_add(offset_u32)))
                } else {
                    MaybeDiffLineIndex::none()
                },
                right: if offset_u32 < right_len {
                    MaybeDiffLineIndex::some(DiffLineIndex(right_start.saturating_add(offset_u32)))
                } else {
                    MaybeDiffLineIndex::none()
                },
            },
            Self::SplitExplicit {
                file,
                hunk,
                left,
                right,
            } => UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            },
        })
    }
}

impl UiModel {
    pub(crate) fn new(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
    ) -> Self {
        Self::new_with_trailing_context(changeset, layout, context_expansions, &HashMap::new())
    }

    pub(crate) fn new_with_trailing_context(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
    ) -> Self {
        Self::new_with_trailing_context_and_controls(
            changeset,
            layout,
            context_expansions,
            trailing_context_lines,
            true,
        )
    }

    pub(crate) fn new_with_trailing_context_and_controls(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        show_context_controls: bool,
    ) -> Self {
        let visible_files: Vec<_> = (0..changeset.files.len()).map(FileIndex::new).collect();
        Self::new_filtered_with_trailing_context_and_controls(
            changeset,
            layout,
            context_expansions,
            trailing_context_lines,
            &visible_files,
            show_context_controls,
        )
    }

    pub(crate) fn new_filtered_with_trailing_context_and_controls(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        visible_files: &[FileIndex],
        show_context_controls: bool,
    ) -> Self {
        let total_hunks = changeset
            .files
            .iter()
            .map(|file| file.hunks().len())
            .sum::<usize>();
        let total_hunk_lines = changeset
            .files
            .iter()
            .flat_map(|file| file.hunks().iter())
            .map(|hunk| hunk.lines.len())
            .sum::<usize>();
        let binary_or_empty_rows = changeset
            .files
            .iter()
            .filter(|file| file.is_binary() || file.has_no_textual_changes())
            .count();
        let file_separator_rows = changeset.files.len().saturating_sub(1);
        let expanded_context_rows = context_expansions
            .values()
            .copied()
            .fold(0usize, usize::saturating_add);
        let expanded_context_controls = if show_context_controls {
            context_expansions
                .values()
                .filter(|expanded| **expanded > 0)
                .count()
        } else {
            0
        };
        let estimated_rows = changeset
            .files
            .len()
            .saturating_add(file_separator_rows)
            .saturating_add(binary_or_empty_rows)
            .saturating_add(total_hunks.saturating_mul(2))
            .saturating_add(total_hunk_lines)
            .saturating_add(expanded_context_rows)
            .saturating_add(expanded_context_controls)
            .saturating_add(
                trailing_context_lines
                    .values()
                    .filter(|lines| **lines > 0)
                    .count(),
            );
        if estimated_rows > MAX_EAGER_UI_MODEL_ROWS {
            return Self::new_filtered_sparse(
                changeset,
                layout,
                context_expansions,
                trailing_context_lines,
                visible_files,
                total_hunks,
                show_context_controls,
            );
        }

        let mut rows = Vec::with_capacity(estimated_rows);
        let mut file_start_rows = vec![None; changeset.files.len()];
        let mut file_row_starts = Vec::with_capacity(visible_files.len());
        let mut hunk_start_rows = Vec::with_capacity(total_hunks);
        let mut hunk_row_starts = Vec::with_capacity(total_hunks);
        let mut hunk_row_ends = Vec::with_capacity(total_hunks);

        for (visible_index, file_index) in visible_files.iter().copied().enumerate() {
            let Some(file) = changeset.files.get(file_index.get()) else {
                continue;
            };
            if visible_index > 0 {
                rows.push(UiRow::FileSeparator);
            }
            file_start_rows[file_index] = Some(ModelRow::new(rows.len()));
            file_row_starts.push((file_index, ModelRow::new(rows.len())));
            rows.push(UiRow::FileHeader(file_index));

            if file.is_binary() || file.has_no_textual_changes() {
                rows.push(UiRow::FileBodyNotice(file_index));
                continue;
            }

            let mut next_old_line = 1;
            let mut next_new_line = 1;
            for (hunk_index, hunk) in file.hunks().iter().enumerate() {
                let hunk_index = HunkIndex::new(hunk_index);
                let hunk_old_start = normalized_hunk_start(hunk.old_start(), hunk.old_count());
                let hunk_new_start = normalized_hunk_start(hunk.new_start(), hunk.new_count());
                let collapsed_lines = hunk_old_start
                    .saturating_sub(next_old_line)
                    .min(hunk_new_start.saturating_sub(next_new_line));
                if collapsed_lines > 0 {
                    let key = ContextKey {
                        file: file_index,
                        hunk: hunk_index,
                    };
                    let expanded = context_expansions
                        .get(&key)
                        .copied()
                        .unwrap_or_default()
                        .min(collapsed_lines);
                    let remaining = collapsed_lines.saturating_sub(expanded);

                    if context_expands_up(hunk_index) {
                        if remaining > 0 {
                            rows.push(UiRow::Collapsed {
                                file: file_index,
                                hunk: hunk_index,
                                old_start: row_count_u32(next_old_line),
                                new_start: row_count_u32(next_new_line),
                                lines: row_count_u32(remaining),
                                expanded: row_count_u32(expanded),
                            });
                        }

                        if expanded > 0 {
                            let old_start = next_old_line.saturating_add(remaining);
                            let new_start = next_new_line.saturating_add(remaining);
                            for offset in 0..expanded {
                                rows.push(UiRow::ContextLine {
                                    file: file_index,
                                    old_line: old_start + offset,
                                    new_line: new_start + offset,
                                });
                            }
                            if show_context_controls {
                                rows.push(UiRow::ContextHide {
                                    file: file_index,
                                    hunk: hunk_index,
                                    lines: expanded,
                                });
                            }
                        }
                    } else {
                        if expanded > 0 {
                            if show_context_controls {
                                rows.push(UiRow::ContextHide {
                                    file: file_index,
                                    hunk: hunk_index,
                                    lines: expanded,
                                });
                            }
                            for offset in 0..expanded {
                                rows.push(UiRow::ContextLine {
                                    file: file_index,
                                    old_line: next_old_line + offset,
                                    new_line: next_new_line + offset,
                                });
                            }
                        }

                        if remaining > 0 {
                            rows.push(UiRow::Collapsed {
                                file: file_index,
                                hunk: hunk_index,
                                old_start: row_count_u32(next_old_line.saturating_add(expanded)),
                                new_start: row_count_u32(next_new_line.saturating_add(expanded)),
                                lines: row_count_u32(remaining),
                                expanded: row_count_u32(expanded),
                            });
                        }
                    }
                }

                let hunk_start_row = rows.len();
                let hunk_start_row = ModelRow::new(hunk_start_row);
                hunk_start_rows.push(hunk_start_row);
                hunk_row_starts.push(((file_index, hunk_index), hunk_start_row));
                // Full-file mode omits patch-only chrome, including @@ headers.
                if show_context_controls {
                    rows.push(UiRow::HunkHeader {
                        file: file_index,
                        hunk: hunk_index,
                    });
                }

                match layout {
                    DiffLayoutMode::Unified => {
                        for line_index in 0..hunk.lines.len() {
                            rows.push(UiRow::UnifiedLine {
                                file: file_index,
                                hunk: hunk_index,
                                line: DiffLineIndex::new(line_index),
                            });
                        }
                    }
                    DiffLayoutMode::Split => push_split_hunk_rows(
                        &mut rows,
                        file_index,
                        hunk_index,
                        hunk.lines.as_slice(),
                    ),
                }
                hunk_row_ends.push(ModelRow::new(rows.len()));

                next_old_line = line_after_hunk(hunk.old_start(), hunk.old_count());
                next_new_line = line_after_hunk(hunk.new_start(), hunk.new_count());
            }

            let trailing_context_key = ContextKey {
                file: file_index,
                hunk: HunkIndex::new(file.hunks().len()),
            };
            let available = trailing_context_lines
                .get(&trailing_context_key)
                .copied()
                .unwrap_or_default();
            let expanded = context_expansions
                .get(&trailing_context_key)
                .copied()
                .unwrap_or_default()
                .min(available);
            if expanded > 0 {
                if show_context_controls {
                    rows.push(UiRow::ContextHide {
                        file: file_index,
                        hunk: trailing_context_key.hunk,
                        lines: expanded,
                    });
                }
                for offset in 0..expanded {
                    rows.push(UiRow::ContextLine {
                        file: file_index,
                        old_line: next_old_line.saturating_add(offset),
                        new_line: next_new_line.saturating_add(offset),
                    });
                }
            }
            let remaining = available.saturating_sub(expanded);
            if remaining > 0 {
                rows.push(UiRow::Collapsed {
                    file: file_index,
                    hunk: trailing_context_key.hunk,
                    old_start: row_count_u32(next_old_line.saturating_add(expanded)),
                    new_start: row_count_u32(next_new_line.saturating_add(expanded)),
                    lines: row_count_u32(remaining),
                    expanded: row_count_u32(expanded),
                });
            }
        }

        Self {
            row_count: rows.len(),
            rows,
            row_segments: Vec::new(),
            file_start_rows,
            file_row_starts,
            visible_files: visible_files.to_vec(),
            hunk_start_rows,
            hunk_row_starts,
            hunk_row_ends,
        }
    }

    fn new_filtered_sparse(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        visible_files: &[FileIndex],
        total_hunks: usize,
        show_context_controls: bool,
    ) -> Self {
        let mut row_count = 0usize;
        let mut row_segments = Vec::with_capacity(
            changeset
                .files
                .len()
                .saturating_add(total_hunks.saturating_mul(4)),
        );
        let mut file_start_rows = vec![None; changeset.files.len()];
        let mut file_row_starts = Vec::with_capacity(visible_files.len());
        let mut hunk_start_rows = Vec::with_capacity(total_hunks);
        let mut hunk_row_starts = Vec::with_capacity(total_hunks);
        let mut hunk_row_ends = Vec::with_capacity(total_hunks);

        for (visible_index, file_index) in visible_files.iter().copied().enumerate() {
            let Some(file) = changeset.files.get(file_index.get()) else {
                continue;
            };
            if visible_index > 0 {
                push_row_segment(
                    &mut row_segments,
                    &mut row_count,
                    1,
                    RowSegmentKind::FileSeparator,
                );
            }
            file_start_rows[file_index] = Some(ModelRow::new(row_count));
            file_row_starts.push((file_index, ModelRow::new(row_count)));
            push_row_segment(
                &mut row_segments,
                &mut row_count,
                1,
                RowSegmentKind::FileHeader(file_index),
            );

            if file.is_binary() || file.has_no_textual_changes() {
                push_row_segment(
                    &mut row_segments,
                    &mut row_count,
                    1,
                    RowSegmentKind::FileBodyNotice(file_index),
                );
                continue;
            }

            let mut next_old_line = 1;
            let mut next_new_line = 1;
            for (hunk_index, hunk) in file.hunks().iter().enumerate() {
                let hunk_index = HunkIndex::new(hunk_index);
                let hunk_old_start = normalized_hunk_start(hunk.old_start(), hunk.old_count());
                let hunk_new_start = normalized_hunk_start(hunk.new_start(), hunk.new_count());
                let collapsed_lines = hunk_old_start
                    .saturating_sub(next_old_line)
                    .min(hunk_new_start.saturating_sub(next_new_line));
                if collapsed_lines > 0 {
                    let key = ContextKey {
                        file: file_index,
                        hunk: hunk_index,
                    };
                    let expanded = context_expansions
                        .get(&key)
                        .copied()
                        .unwrap_or_default()
                        .min(collapsed_lines);
                    let remaining = collapsed_lines.saturating_sub(expanded);

                    if context_expands_up(hunk_index) {
                        if remaining > 0 {
                            push_row_segment(
                                &mut row_segments,
                                &mut row_count,
                                1,
                                RowSegmentKind::Collapsed {
                                    file: file_index,
                                    hunk: hunk_index,
                                    old_start: row_count_u32(next_old_line),
                                    new_start: row_count_u32(next_new_line),
                                    lines: row_count_u32(remaining),
                                    expanded: row_count_u32(expanded),
                                },
                            );
                        }

                        if expanded > 0 {
                            let old_start = next_old_line.saturating_add(remaining);
                            let new_start = next_new_line.saturating_add(remaining);
                            push_row_segment(
                                &mut row_segments,
                                &mut row_count,
                                expanded,
                                RowSegmentKind::ContextLines {
                                    file: file_index,
                                    old_start: row_count_u32(old_start),
                                    new_start: row_count_u32(new_start),
                                },
                            );
                            if show_context_controls {
                                push_row_segment(
                                    &mut row_segments,
                                    &mut row_count,
                                    1,
                                    RowSegmentKind::ContextHide {
                                        file: file_index,
                                        hunk: hunk_index,
                                        lines: row_count_u32(expanded),
                                    },
                                );
                            }
                        }
                    } else {
                        if expanded > 0 {
                            if show_context_controls {
                                push_row_segment(
                                    &mut row_segments,
                                    &mut row_count,
                                    1,
                                    RowSegmentKind::ContextHide {
                                        file: file_index,
                                        hunk: hunk_index,
                                        lines: row_count_u32(expanded),
                                    },
                                );
                            }
                            push_row_segment(
                                &mut row_segments,
                                &mut row_count,
                                expanded,
                                RowSegmentKind::ContextLines {
                                    file: file_index,
                                    old_start: row_count_u32(next_old_line),
                                    new_start: row_count_u32(next_new_line),
                                },
                            );
                        }

                        if remaining > 0 {
                            push_row_segment(
                                &mut row_segments,
                                &mut row_count,
                                1,
                                RowSegmentKind::Collapsed {
                                    file: file_index,
                                    hunk: hunk_index,
                                    old_start: row_count_u32(
                                        next_old_line.saturating_add(expanded),
                                    ),
                                    new_start: row_count_u32(
                                        next_new_line.saturating_add(expanded),
                                    ),
                                    lines: row_count_u32(remaining),
                                    expanded: row_count_u32(expanded),
                                },
                            );
                        }
                    }
                }

                let hunk_start_row = ModelRow::new(row_count);
                hunk_start_rows.push(hunk_start_row);
                hunk_row_starts.push(((file_index, hunk_index), hunk_start_row));
                if show_context_controls {
                    push_row_segment(
                        &mut row_segments,
                        &mut row_count,
                        1,
                        RowSegmentKind::HunkHeader {
                            file: file_index,
                            hunk: hunk_index,
                        },
                    );
                }

                match layout {
                    DiffLayoutMode::Unified => push_row_segment(
                        &mut row_segments,
                        &mut row_count,
                        hunk.lines.len(),
                        RowSegmentKind::UnifiedLines {
                            file: file_index,
                            hunk: hunk_index,
                            line_start: 0,
                        },
                    ),
                    DiffLayoutMode::Split => push_split_hunk_segments(
                        &mut row_segments,
                        &mut row_count,
                        file_index,
                        hunk_index,
                        hunk.lines.as_slice(),
                    ),
                }
                hunk_row_ends.push(ModelRow::new(row_count));

                next_old_line = line_after_hunk(hunk.old_start(), hunk.old_count());
                next_new_line = line_after_hunk(hunk.new_start(), hunk.new_count());
            }

            let trailing_context_key = ContextKey {
                file: file_index,
                hunk: HunkIndex::new(file.hunks().len()),
            };
            let available = trailing_context_lines
                .get(&trailing_context_key)
                .copied()
                .unwrap_or_default();
            let expanded = context_expansions
                .get(&trailing_context_key)
                .copied()
                .unwrap_or_default()
                .min(available);
            if expanded > 0 {
                if show_context_controls {
                    push_row_segment(
                        &mut row_segments,
                        &mut row_count,
                        1,
                        RowSegmentKind::ContextHide {
                            file: file_index,
                            hunk: trailing_context_key.hunk,
                            lines: row_count_u32(expanded),
                        },
                    );
                }
                push_row_segment(
                    &mut row_segments,
                    &mut row_count,
                    expanded,
                    RowSegmentKind::ContextLines {
                        file: file_index,
                        old_start: row_count_u32(next_old_line),
                        new_start: row_count_u32(next_new_line),
                    },
                );
            }
            let remaining = available.saturating_sub(expanded);
            if remaining > 0 {
                push_row_segment(
                    &mut row_segments,
                    &mut row_count,
                    1,
                    RowSegmentKind::Collapsed {
                        file: file_index,
                        hunk: trailing_context_key.hunk,
                        old_start: row_count_u32(next_old_line.saturating_add(expanded)),
                        new_start: row_count_u32(next_new_line.saturating_add(expanded)),
                        lines: row_count_u32(remaining),
                        expanded: row_count_u32(expanded),
                    },
                );
            }
        }

        Self {
            rows: Vec::new(),
            row_count,
            row_segments,
            file_start_rows,
            file_row_starts,
            visible_files: visible_files.to_vec(),
            hunk_start_rows,
            hunk_row_starts,
            hunk_row_ends,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.row_count
    }

    pub(crate) fn estimated_memory_bytes(&self) -> usize {
        self.rows
            .len()
            .saturating_mul(std::mem::size_of::<UiRow>())
            .saturating_add(
                self.row_segments
                    .len()
                    .saturating_mul(std::mem::size_of::<RowSegment>()),
            )
            .saturating_add(
                self.file_start_rows
                    .len()
                    .saturating_mul(std::mem::size_of::<Option<ModelRow>>()),
            )
            .saturating_add(
                self.file_row_starts
                    .len()
                    .saturating_mul(std::mem::size_of::<(FileIndex, ModelRow)>()),
            )
            .saturating_add(
                self.visible_files
                    .len()
                    .saturating_mul(std::mem::size_of::<FileIndex>()),
            )
            .saturating_add(
                self.hunk_start_rows
                    .len()
                    .saturating_mul(std::mem::size_of::<ModelRow>()),
            )
            .saturating_add(
                self.hunk_row_starts
                    .len()
                    .saturating_mul(std::mem::size_of::<((FileIndex, HunkIndex), ModelRow)>()),
            )
            .saturating_add(
                self.hunk_row_ends
                    .len()
                    .saturating_mul(std::mem::size_of::<ModelRow>()),
            )
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    pub(crate) fn row(&self, index: usize) -> Option<UiRow> {
        if !self.rows.is_empty() {
            return self.rows.get(index).copied();
        }
        if index >= self.row_count {
            return None;
        }
        let segment_index = self
            .row_segments
            .partition_point(|segment| segment.start.get() <= index)
            .checked_sub(1)?;
        self.row_segments.get(segment_index)?.row_at(index)
    }

    pub(crate) fn iter_rows(&self) -> impl Iterator<Item = UiRow> + '_ {
        (0..self.row_count).filter_map(|index| self.row(index))
    }

    pub(crate) fn cache_key(&self) -> usize {
        if self.rows.is_empty() {
            self.row_segments.as_ptr() as usize
        } else {
            self.rows.as_ptr() as usize
        }
    }

    pub(crate) fn file_start_row(&self, file: usize) -> Option<usize> {
        self.file_start_rows
            .get(file)
            .copied()
            .flatten()
            .map(ModelRow::get)
    }

    pub(crate) fn file_at_row(&self, row: usize) -> Option<usize> {
        self.typed_file_at_row(ModelRow::new(row))
            .map(FileIndex::get)
    }

    pub(crate) fn typed_file_at_row(&self, row: ModelRow) -> Option<FileIndex> {
        if self.file_row_starts.is_empty() {
            return None;
        }
        match self
            .file_row_starts
            .binary_search_by_key(&row, |(_, start)| *start)
        {
            Ok(index) => self.file_row_starts.get(index).map(|(file, _)| *file),
            Err(0) => self.file_row_starts.first().map(|(file, _)| *file),
            Err(index) => self.file_row_starts.get(index - 1).map(|(file, _)| *file),
        }
    }

    pub(crate) fn visible_files(&self) -> &[FileIndex] {
        &self.visible_files
    }

    pub(crate) fn visible_file_position(&self, file: usize) -> Option<usize> {
        self.visible_files.binary_search(&FileIndex::new(file)).ok()
    }

    pub(crate) fn next_hunk_row(&self, row: usize) -> Option<usize> {
        self.typed_next_hunk_row(ModelRow::new(row))
            .map(ModelRow::get)
    }

    pub(crate) fn typed_next_hunk_row(&self, row: ModelRow) -> Option<ModelRow> {
        let index = self.hunk_start_rows.partition_point(|start| *start <= row);
        self.hunk_start_rows.get(index).copied()
    }

    pub(crate) fn previous_hunk_row(&self, row: usize) -> Option<usize> {
        self.typed_previous_hunk_row(ModelRow::new(row))
            .map(ModelRow::get)
    }

    pub(crate) fn typed_previous_hunk_row(&self, row: ModelRow) -> Option<ModelRow> {
        let index = self.hunk_start_rows.partition_point(|start| *start < row);
        index
            .checked_sub(1)
            .and_then(|index| self.hunk_start_rows.get(index))
            .copied()
    }

    pub(crate) fn hunk_start_row(&self, file: usize, hunk: usize) -> Option<usize> {
        self.typed_hunk_start_row(FileIndex::new(file), HunkIndex::new(hunk))
            .map(ModelRow::get)
    }

    pub(crate) fn typed_hunk_start_row(
        &self,
        file: FileIndex,
        hunk: HunkIndex,
    ) -> Option<ModelRow> {
        self.hunk_row_starts
            .binary_search_by_key(&(file, hunk), |(key, _)| *key)
            .ok()
            .and_then(|index| self.hunk_row_starts.get(index))
            .map(|(_, row)| *row)
    }

    pub(crate) fn hunk_header_row(&self, file: FileIndex, hunk: HunkIndex) -> Option<ModelRow> {
        let row = self.typed_hunk_start_row(file, hunk)?;
        matches!(
            self.row(row.get()),
            Some(UiRow::HunkHeader {
                file: row_file,
                hunk: row_hunk,
            }) if row_file == file && row_hunk == hunk
        )
        .then_some(row)
    }

    pub(crate) fn file_body_notice_row(&self, file: FileIndex) -> Option<ModelRow> {
        let row = self.file_start_row(file.get())?.saturating_add(1);
        matches!(self.row(row), Some(UiRow::FileBodyNotice(row_file)) if row_file == file)
            .then_some(ModelRow::new(row))
    }

    pub(crate) fn context_line_row(&self, file: FileIndex, new_line: usize) -> Option<ModelRow> {
        if !self.rows.is_empty() {
            return self
                .rows
                .iter()
                .position(|row| {
                    matches!(
                        row,
                        UiRow::ContextLine {
                            file: row_file,
                            new_line: row_new_line,
                            ..
                        } if *row_file == file && *row_new_line == new_line
                    )
                })
                .map(ModelRow::new);
        }

        self.row_segments.iter().find_map(|segment| {
            let RowSegmentKind::ContextLines {
                file: row_file,
                new_start,
                ..
            } = segment.kind
            else {
                return None;
            };
            if row_file != file {
                return None;
            }
            let offset = new_line.checked_sub(new_start as usize)?;
            (offset < segment.len as usize)
                .then_some(ModelRow::new(segment.start.get().saturating_add(offset)))
        })
    }

    pub(crate) fn diff_line_row(
        &self,
        file: FileIndex,
        hunk: HunkIndex,
        line: DiffLineIndex,
    ) -> Option<ModelRow> {
        if self.rows.is_empty() {
            return self.sparse_diff_line_row(file, hunk, line);
        }
        let range = self.hunk_row_range(file.get(), hunk.get())?;
        range.into_iter().find_map(|row_index| {
            let row = self.row(row_index)?;
            row_contains_diff_line(row, file, hunk, line).then_some(ModelRow::new(row_index))
        })
    }

    fn sparse_diff_line_row(
        &self,
        file: FileIndex,
        hunk: HunkIndex,
        line: DiffLineIndex,
    ) -> Option<ModelRow> {
        let range = self.hunk_row_range(file.get(), hunk.get())?;
        let line = line.0;
        let start_segment = self
            .row_segments
            .partition_point(|segment| segment.end() <= range.start);
        for segment in self.row_segments.iter().skip(start_segment) {
            if segment.start.get() >= range.end {
                break;
            }
            let row = match segment.kind {
                RowSegmentKind::UnifiedLines {
                    file: row_file,
                    hunk: row_hunk,
                    line_start,
                }
                | RowSegmentKind::SplitContextLines {
                    file: row_file,
                    hunk: row_hunk,
                    line_start,
                }
                | RowSegmentKind::SplitMetaLines {
                    file: row_file,
                    hunk: row_hunk,
                    line_start,
                } if row_file == file && row_hunk == hunk => line
                    .checked_sub(line_start)
                    .filter(|offset| *offset < segment.len)
                    .map(|offset| segment.start.get() + offset as usize),
                RowSegmentKind::SplitChangeRun {
                    file: row_file,
                    hunk: row_hunk,
                    left_start,
                    left_len,
                    right_start,
                    right_len,
                } if row_file == file && row_hunk == hunk => {
                    let left_offset = line
                        .checked_sub(left_start)
                        .filter(|offset| *offset < left_len);
                    let right_offset = line
                        .checked_sub(right_start)
                        .filter(|offset| *offset < right_len);
                    left_offset
                        .or(right_offset)
                        .map(|offset| segment.start.get() + offset as usize)
                }
                RowSegmentKind::SplitExplicit {
                    file: row_file,
                    hunk: row_hunk,
                    left,
                    right,
                } if row_file == file && row_hunk == hunk => (left.get()
                    == Some(DiffLineIndex(line))
                    || right.get() == Some(DiffLineIndex(line)))
                .then_some(segment.start.get()),
                RowSegmentKind::FileSeparator
                | RowSegmentKind::FileHeader(_)
                | RowSegmentKind::FileBodyNotice(_)
                | RowSegmentKind::Collapsed { .. }
                | RowSegmentKind::ContextLines { .. }
                | RowSegmentKind::ContextHide { .. }
                | RowSegmentKind::HunkHeader { .. }
                | RowSegmentKind::UnifiedLines { .. }
                | RowSegmentKind::SplitContextLines { .. }
                | RowSegmentKind::SplitMetaLines { .. }
                | RowSegmentKind::SplitChangeRun { .. }
                | RowSegmentKind::SplitExplicit { .. } => None,
            };
            if let Some(row) = row {
                return Some(ModelRow::new(row));
            }
        }
        None
    }

    pub(crate) fn hunk_row_range(&self, file: usize, hunk: usize) -> Option<Range<usize>> {
        let file = FileIndex::new(file);
        let hunk = HunkIndex::new(hunk);
        let index = self
            .hunk_row_starts
            .binary_search_by_key(&(file, hunk), |(key, _)| *key)
            .ok()?;
        let start = self.hunk_row_starts.get(index)?.1.get();
        let end = self.hunk_row_ends.get(index)?.get();
        Some(start..end)
    }
}

fn row_contains_diff_line(
    row: UiRow,
    file: FileIndex,
    hunk: HunkIndex,
    line: DiffLineIndex,
) -> bool {
    match row {
        UiRow::UnifiedLine {
            file: row_file,
            hunk: row_hunk,
            line: row_line,
        }
        | UiRow::MetaLine {
            file: row_file,
            hunk: row_hunk,
            line: row_line,
        } => row_file == file && row_hunk == hunk && row_line == line,
        UiRow::SplitLine {
            file: row_file,
            hunk: row_hunk,
            left,
            right,
        } => {
            row_file == file
                && row_hunk == hunk
                && (left.get() == Some(line) || right.get() == Some(line))
        }
        UiRow::FileSeparator
        | UiRow::FileHeader(_)
        | UiRow::FileBodyNotice(_)
        | UiRow::Collapsed { .. }
        | UiRow::ContextLine { .. }
        | UiRow::ContextHide { .. }
        | UiRow::HunkHeader { .. } => false,
    }
}

fn push_row_segment(
    row_segments: &mut Vec<RowSegment>,
    row_count: &mut usize,
    len: usize,
    kind: RowSegmentKind,
) {
    if len == 0 {
        return;
    }
    row_segments.push(RowSegment {
        start: ModelRow::new(*row_count),
        len: row_count_u32(len),
        kind,
    });
    *row_count = row_count.saturating_add(len);
}

pub(crate) fn push_split_hunk_rows(
    rows: &mut Vec<UiRow>,
    file_index: FileIndex,
    hunk_index: HunkIndex,
    lines: &[DiffLine],
) {
    let mut index = 0;
    while index < lines.len() {
        match lines[index].kind() {
            DiffLineKind::Context => {
                rows.push(UiRow::SplitLine {
                    file: file_index,
                    hunk: hunk_index,
                    left: MaybeDiffLineIndex::some(DiffLineIndex::new(index)),
                    right: MaybeDiffLineIndex::some(DiffLineIndex::new(index)),
                });
                index += 1;
            }
            DiffLineKind::Meta => {
                rows.push(UiRow::MetaLine {
                    file: file_index,
                    hunk: hunk_index,
                    line: DiffLineIndex::new(index),
                });
                index += 1;
            }
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
                        DiffLineKind::Deletion => deletions.push(DiffLineIndex::new(index)),
                        DiffLineKind::Addition => additions.push(DiffLineIndex::new(index)),
                        DiffLineKind::Context | DiffLineKind::Meta => {}
                    }
                    index += 1;
                }

                let paired_rows = deletions.len().max(additions.len());
                for pair_index in 0..paired_rows {
                    rows.push(UiRow::SplitLine {
                        file: file_index,
                        hunk: hunk_index,
                        left: deletions.get(pair_index).copied().into(),
                        right: additions.get(pair_index).copied().into(),
                    });
                }
            }
        }
    }
}

fn push_split_hunk_segments(
    row_segments: &mut Vec<RowSegment>,
    row_count: &mut usize,
    file_index: FileIndex,
    hunk_index: HunkIndex,
    lines: &[DiffLine],
) {
    let mut index = 0;
    while index < lines.len() {
        match lines[index].kind() {
            DiffLineKind::Context => {
                let start = index;
                while index < lines.len() && lines[index].kind() == DiffLineKind::Context {
                    index += 1;
                }
                push_row_segment(
                    row_segments,
                    row_count,
                    index - start,
                    RowSegmentKind::SplitContextLines {
                        file: file_index,
                        hunk: hunk_index,
                        line_start: row_count_u32(start),
                    },
                );
            }
            DiffLineKind::Meta => {
                let start = index;
                while index < lines.len() && lines[index].kind() == DiffLineKind::Meta {
                    index += 1;
                }
                push_row_segment(
                    row_segments,
                    row_count,
                    index - start,
                    RowSegmentKind::SplitMetaLines {
                        file: file_index,
                        hunk: hunk_index,
                        line_start: row_count_u32(start),
                    },
                );
            }
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

                if is_contiguous(&deletions) && is_contiguous(&additions) {
                    push_row_segment(
                        row_segments,
                        row_count,
                        deletions.len().max(additions.len()),
                        RowSegmentKind::SplitChangeRun {
                            file: file_index,
                            hunk: hunk_index,
                            left_start: row_count_u32(deletions.first().copied().unwrap_or(0)),
                            left_len: row_count_u32(deletions.len()),
                            right_start: row_count_u32(additions.first().copied().unwrap_or(0)),
                            right_len: row_count_u32(additions.len()),
                        },
                    );
                } else {
                    for pair_index in 0..deletions.len().max(additions.len()) {
                        push_row_segment(
                            row_segments,
                            row_count,
                            1,
                            RowSegmentKind::SplitExplicit {
                                file: file_index,
                                hunk: hunk_index,
                                left: deletions
                                    .get(pair_index)
                                    .copied()
                                    .map(DiffLineIndex::new)
                                    .into(),
                                right: additions
                                    .get(pair_index)
                                    .copied()
                                    .map(DiffLineIndex::new)
                                    .into(),
                            },
                        );
                    }
                }
            }
        }
    }
}

fn is_contiguous(indexes: &[usize]) -> bool {
    indexes
        .windows(2)
        .all(|window| window[1] == window[0].saturating_add(1))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use mark_diff::{
        Changeset, DiffFile, DiffFileBody, DiffHunk, DiffLine, FileChange, HunkLineRanges, RepoRoot,
    };

    use super::*;

    #[test]
    fn sparse_model_matches_eager_rows_for_unified_and_split() {
        let changeset = sample_changeset();
        let mut expansions = HashMap::new();
        expansions.insert(
            ContextKey {
                file: FileIndex::new(0),
                hunk: HunkIndex::new(0),
            },
            1,
        );
        let visible = [FileIndex::new(0)];
        let trailing = HashMap::from([(
            ContextKey {
                file: FileIndex::new(0),
                hunk: HunkIndex::new(2),
            },
            2,
        )]);
        for show_context_controls in [true, false] {
            for layout in [DiffLayoutMode::Unified, DiffLayoutMode::Split] {
                let eager = UiModel::new_filtered_with_trailing_context_and_controls(
                    &changeset,
                    layout,
                    &expansions,
                    &trailing,
                    &visible,
                    show_context_controls,
                );
                let sparse = UiModel::new_filtered_sparse(
                    &changeset,
                    layout,
                    &expansions,
                    &trailing,
                    &visible,
                    2,
                    show_context_controls,
                );
                assert_eq!(sparse.len(), eager.len());
                for row in 0..eager.len() {
                    assert_eq!(sparse.row(row), eager.row(row), "row {row} in {layout:?}");
                }
                assert_eq!(sparse.hunk_row_range(0, 0), eager.hunk_row_range(0, 0));
                assert_eq!(sparse.hunk_row_range(0, 1), eager.hunk_row_range(0, 1));
                for row in 0..eager.len() {
                    if let Some(
                        UiRow::UnifiedLine { file, hunk, line }
                        | UiRow::MetaLine { file, hunk, line },
                    ) = eager.row(row)
                    {
                        assert_eq!(
                            sparse.diff_line_row(file, hunk, line),
                            Some(ModelRow::new(row))
                        );
                    }
                    if let Some(UiRow::SplitLine {
                        file,
                        hunk,
                        left,
                        right,
                    }) = eager.row(row)
                    {
                        for line in [left.get(), right.get()].into_iter().flatten() {
                            assert_eq!(
                                sparse.diff_line_row(file, hunk, line),
                                Some(ModelRow::new(row))
                            );
                        }
                    }
                }
            }
        }
    }

    fn sample_changeset() -> Changeset {
        Changeset {
            repo: RepoRoot::new(PathBuf::from("/repo")),
            title: String::new(),
            files: vec![DiffFile {
                change: FileChange::modified("src/lib.rs"),
                additions: 2,
                deletions: 2,
                body: DiffFileBody::Text {
                    hunks: vec![
                        DiffHunk {
                            header: "@@ -3,3 +3,3 @@".to_owned(),
                            ranges: HunkLineRanges::new(3, 3, 3, 3),
                            lines: vec![
                                DiffLine::context(3, 3, "same"),
                                DiffLine::deletion(4, "old"),
                                DiffLine::addition(4, "new"),
                                DiffLine::meta("\\ No newline at end of file"),
                            ],
                        },
                        DiffHunk {
                            header: "@@ -10,2 +10,2 @@".to_owned(),
                            ranges: HunkLineRanges::new(10, 2, 10, 2),
                            lines: vec![
                                DiffLine::deletion(10, "old a"),
                                DiffLine::deletion(11, "old b"),
                                DiffLine::addition(10, "new a"),
                                DiffLine::addition(11, "new b"),
                            ],
                        },
                    ],
                },
            }],
            raw_patch: Changeset::empty_raw_patch(),
        }
    }
}
