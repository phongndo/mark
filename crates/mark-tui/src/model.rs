use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use mark_diff::{Changeset, DiffLine, DiffLineKind};

use crate::{controls::DiffLayoutMode, syntax::DiffSide};

const MAX_EAGER_UI_MODEL_ROWS: usize = 200_000;
const SPARSE_ANNOTATION_CANDIDATE_CHUNK_ROWS: usize = 256;
const MAX_SYNCHRONOUS_SPARSE_CANDIDATE_SEGMENT_ROWS: usize = 4_096;
const SPARSE_ANNOTATION_CANDIDATE_WORDS: usize =
    SPARSE_ANNOTATION_CANDIDATE_CHUNK_ROWS / u64::BITS as usize;
static NEXT_UI_MODEL_IDENTITY: AtomicU64 = AtomicU64::new(1);

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
            Self::FileHeader(_)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextLines {
    text: Arc<String>,
    ranges: Arc<[Range<u32>]>,
}

impl ContextLines {
    pub(crate) fn new(text: String, max_lines: usize, max_line_bytes: usize) -> Option<Self> {
        let base = text.as_ptr() as usize;
        let mut ranges = Vec::new();
        for line in text.lines() {
            if ranges.len() >= max_lines || line.len() > max_line_bytes {
                return None;
            }
            let start = (line.as_ptr() as usize).checked_sub(base)?;
            let end = start.checked_add(line.len())?;
            ranges.push(u32::try_from(start).ok()?..u32::try_from(end).ok()?);
        }
        Some(Self {
            text: Arc::new(text),
            ranges: Arc::from(ranges.into_boxed_slice()),
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&str> {
        let range = self.ranges.get(index)?;
        self.text
            .get(usize::try_from(range.start).ok()?..usize::try_from(range.end).ok()?)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        (0..self.ranges.len()).filter_map(|index| self.get(index))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ContextSourceEntry {
    Lines(Arc<ContextLines>),
    Loading,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiModel {
    identity: UiModelIdentity,
    pub(crate) rows: Vec<UiRow>,
    row_count: usize,
    row_segments: Vec<RowSegment>,
    pub(crate) file_start_rows: Vec<Option<ModelRow>>,
    pub(crate) file_row_starts: Vec<(FileIndex, ModelRow)>,
    pub(crate) visible_files: Vec<FileIndex>,
    pub(crate) hunk_start_rows: Vec<ModelRow>,
    pub(crate) hunk_row_starts: Vec<((FileIndex, HunkIndex), ModelRow)>,
    hunk_row_ends: Vec<ModelRow>,
    /// Compact content blocks let annotation navigation skip model chrome.
    annotation_candidate_blocks: AnnotationCandidateIndex,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct UiModelBuildOptions {
    show_context_controls: bool,
    show_context_expansion_controls: bool,
    build_annotation_candidates: bool,
}

impl UiModelBuildOptions {
    pub(crate) const fn new(
        show_context_controls: bool,
        show_context_expansion_controls: bool,
        build_annotation_candidates: bool,
    ) -> Self {
        Self {
            show_context_controls,
            show_context_expansion_controls,
            build_annotation_candidates,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct UiModelIdentity(u64);

impl UiModelIdentity {
    fn new() -> Self {
        Self(
            NEXT_UI_MODEL_IDENTITY
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |identity| {
                    identity.checked_add(1)
                })
                .expect("UI model identity space exhausted"),
        )
    }
}

impl PartialEq for UiModelIdentity {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for UiModelIdentity {}

#[derive(Debug, Clone)]
enum AnnotationCandidateIndexState {
    Disabled,
    Eager(Vec<AnnotationCandidateBlock>),
    Sparse(HashMap<usize, Vec<AnnotationCandidateBlock>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnnotationCandidateSearchResult {
    Candidate(usize),
    // Traversal reached an intentionally unindexed segment; viewport discovery
    // must take over rather than treating this as the end of the document.
    Unindexed,
    Exhausted,
}

#[derive(Debug)]
struct AnnotationCandidateIndex(RefCell<AnnotationCandidateIndexState>);

impl AnnotationCandidateIndex {
    fn disabled() -> Self {
        Self(RefCell::new(AnnotationCandidateIndexState::Disabled))
    }

    fn eager(blocks: Vec<AnnotationCandidateBlock>) -> Self {
        Self(RefCell::new(AnnotationCandidateIndexState::Eager(blocks)))
    }

    fn sparse() -> Self {
        Self(RefCell::new(AnnotationCandidateIndexState::Sparse(
            HashMap::new(),
        )))
    }

    fn len(&self) -> usize {
        match &*self.0.borrow() {
            AnnotationCandidateIndexState::Disabled => 0,
            AnnotationCandidateIndexState::Eager(blocks) => blocks.len(),
            AnnotationCandidateIndexState::Sparse(segments) => {
                segments.values().map(Vec::len).sum()
            }
        }
    }
}

impl Clone for AnnotationCandidateIndex {
    fn clone(&self) -> Self {
        Self(RefCell::new(self.0.borrow().clone()))
    }
}

impl PartialEq for AnnotationCandidateIndex {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for AnnotationCandidateIndex {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AnnotationCandidateBlock {
    Range(Range<usize>),
    UnifiedChange {
        range: Range<usize>,
        file: FileIndex,
        hunk: HunkIndex,
        line_start: u32,
        first_addition: Option<u32>,
        last_addition: Option<u32>,
        first_unpaired_deletion: Option<u32>,
        last_unpaired_deletion: Option<u32>,
    },
    SparseCandidates {
        range: Range<usize>,
        bits: [u64; SPARSE_ANNOTATION_CANDIDATE_WORDS],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnifiedCandidateScan {
    file: FileIndex,
    hunk: HunkIndex,
    line_start: usize,
    block_start: usize,
    block_end: usize,
    first_unpaired_deletion: Option<usize>,
}

impl AnnotationCandidateBlock {
    fn start(&self) -> usize {
        match self {
            Self::Range(range)
            | Self::UnifiedChange { range, .. }
            | Self::SparseCandidates { range, .. } => range.start,
        }
    }

    fn end(&self) -> usize {
        match self {
            Self::Range(range)
            | Self::UnifiedChange { range, .. }
            | Self::SparseCandidates { range, .. } => range.end,
        }
    }

    fn candidate_at_or_after(&self, changeset: &Changeset, row: usize) -> Option<usize> {
        match self {
            Self::Range(range) => {
                let candidate = row.max(range.start);
                (candidate < range.end).then_some(candidate)
            }
            Self::UnifiedChange {
                range,
                file,
                hunk,
                line_start,
                first_addition,
                last_addition,
                first_unpaired_deletion,
                last_unpaired_deletion,
            } => {
                let offset = row.max(range.start).checked_sub(range.start)?;
                if offset >= range.len() {
                    return None;
                }
                let lines = changeset
                    .files
                    .get(file.get())?
                    .hunks()
                    .get(hunk.get())?
                    .lines
                    .as_slice();
                let addition = candidate_line_offset_at_or_after(
                    lines,
                    *line_start as usize,
                    offset,
                    *first_addition,
                    *last_addition,
                    DiffLineKind::Addition,
                );
                let deletion = candidate_line_offset_at_or_after(
                    lines,
                    *line_start as usize,
                    offset,
                    *first_unpaired_deletion,
                    *last_unpaired_deletion,
                    DiffLineKind::Deletion,
                );
                addition
                    .into_iter()
                    .chain(deletion)
                    .min()
                    .map(|offset| range.start.saturating_add(offset))
            }
            Self::SparseCandidates { range, bits } => {
                sparse_candidate_at_or_after(range, bits, row)
            }
        }
    }

    fn candidate_at_or_before(&self, changeset: &Changeset, row: usize) -> Option<usize> {
        match self {
            Self::Range(range) => Some(row.min(range.end.saturating_sub(1)))
                .filter(|candidate| *candidate >= range.start),
            Self::UnifiedChange {
                range,
                file,
                hunk,
                line_start,
                first_addition,
                last_addition,
                first_unpaired_deletion,
                last_unpaired_deletion,
            } => {
                if row < range.start {
                    return None;
                }
                let offset = row
                    .min(range.end.saturating_sub(1))
                    .saturating_sub(range.start);
                let lines = changeset
                    .files
                    .get(file.get())?
                    .hunks()
                    .get(hunk.get())?
                    .lines
                    .as_slice();
                let addition = candidate_line_offset_at_or_before(
                    lines,
                    *line_start as usize,
                    offset,
                    *first_addition,
                    *last_addition,
                    DiffLineKind::Addition,
                );
                let deletion = candidate_line_offset_at_or_before(
                    lines,
                    *line_start as usize,
                    offset,
                    *first_unpaired_deletion,
                    *last_unpaired_deletion,
                    DiffLineKind::Deletion,
                );
                addition
                    .into_iter()
                    .chain(deletion)
                    .max()
                    .map(|offset| range.start.saturating_add(offset))
            }
            Self::SparseCandidates { range, bits } => {
                sparse_candidate_at_or_before(range, bits, row)
            }
        }
    }
}

fn sparse_candidate_at_or_after(
    range: &Range<usize>,
    bits: &[u64; SPARSE_ANNOTATION_CANDIDATE_WORDS],
    row: usize,
) -> Option<usize> {
    let offset = row.max(range.start).checked_sub(range.start)?;
    if offset >= range.len() {
        return None;
    }
    let first_word = offset / u64::BITS as usize;
    for (word_index, word) in bits.iter().copied().enumerate().skip(first_word) {
        let candidates = if word_index == first_word {
            word & (u64::MAX << (offset % u64::BITS as usize))
        } else {
            word
        };
        if candidates != 0 {
            let candidate = word_index
                .saturating_mul(u64::BITS as usize)
                .saturating_add(candidates.trailing_zeros() as usize);
            return (candidate < range.len()).then_some(range.start.saturating_add(candidate));
        }
    }
    None
}

fn sparse_candidate_at_or_before(
    range: &Range<usize>,
    bits: &[u64; SPARSE_ANNOTATION_CANDIDATE_WORDS],
    row: usize,
) -> Option<usize> {
    if row < range.start || range.is_empty() {
        return None;
    }
    let offset = row
        .min(range.end.saturating_sub(1))
        .saturating_sub(range.start);
    let last_word = offset / u64::BITS as usize;
    for word_index in (0..=last_word).rev() {
        let word = bits[word_index];
        let candidates = if word_index == last_word {
            word & (u64::MAX >> (u64::BITS as usize - 1 - offset % u64::BITS as usize))
        } else {
            word
        };
        if candidates != 0 {
            let candidate = word_index
                .saturating_mul(u64::BITS as usize)
                .saturating_add(u64::BITS as usize - 1 - candidates.leading_zeros() as usize);
            return Some(range.start.saturating_add(candidate));
        }
    }
    None
}

fn candidate_line_offset_at_or_after(
    lines: &[DiffLine],
    line_start: usize,
    offset: usize,
    first: Option<u32>,
    last: Option<u32>,
    kind: DiffLineKind,
) -> Option<usize> {
    let first = first? as usize;
    let last = last? as usize;
    if offset <= first {
        return Some(first);
    }
    (offset..=last).find(|offset| {
        lines
            .get(line_start.saturating_add(*offset))
            .is_some_and(|line| candidate_line_has_kind(line, kind))
    })
}

fn candidate_line_offset_at_or_before(
    lines: &[DiffLine],
    line_start: usize,
    offset: usize,
    first: Option<u32>,
    last: Option<u32>,
    kind: DiffLineKind,
) -> Option<usize> {
    let first = first? as usize;
    let last = last? as usize;
    if offset >= last {
        return Some(last);
    }
    if offset < first {
        return None;
    }
    (first..=offset).rev().find(|offset| {
        lines
            .get(line_start.saturating_add(*offset))
            .is_some_and(|line| candidate_line_has_kind(line, kind))
    })
}

fn candidate_line_has_kind(line: &DiffLine, kind: DiffLineKind) -> bool {
    line.kind() == kind
        && match kind {
            DiffLineKind::Addition => line.new_line().is_some(),
            DiffLineKind::Deletion => line.old_line().is_some(),
            DiffLineKind::Context | DiffLineKind::Meta => false,
        }
}

fn unified_candidate_scan(
    lines: &[DiffLine],
    file: FileIndex,
    hunk: HunkIndex,
    line_start: usize,
    line_end: usize,
    index: usize,
    cached_scan: &mut Option<UnifiedCandidateScan>,
) -> UnifiedCandidateScan {
    if let Some(scan) = cached_scan.as_ref().filter(|scan| {
        scan.file == file
            && scan.hunk == hunk
            && scan.line_start == line_start
            && index >= scan.block_start
            && index < scan.block_end
    }) {
        return *scan;
    }

    let (block_start, block_end) = unified_change_block_bounds(lines, line_start, line_end, index);
    let additions = lines[block_start..block_end]
        .iter()
        .filter(|line| line.kind() == DiffLineKind::Addition)
        .count();
    let mut deletion = 0usize;
    let mut first_unpaired_deletion = None;
    for (offset, line) in lines[block_start..block_end].iter().enumerate() {
        if line.kind() != DiffLineKind::Deletion {
            continue;
        }
        if deletion >= additions && first_unpaired_deletion.is_none() {
            first_unpaired_deletion = Some(block_start.saturating_add(offset));
        }
        deletion = deletion.saturating_add(1);
    }
    let scan = UnifiedCandidateScan {
        file,
        hunk,
        line_start,
        block_start,
        block_end,
        first_unpaired_deletion,
    };
    *cached_scan = Some(scan);
    scan
}

fn unified_change_block_bounds(
    lines: &[DiffLine],
    line_start: usize,
    line_end: usize,
    index: usize,
) -> (usize, usize) {
    let mut block_start = index;
    while block_start > line_start
        && lines
            .get(block_start.saturating_sub(1))
            .is_some_and(|line| line.kind() != DiffLineKind::Context)
    {
        block_start = block_start.saturating_sub(1);
    }
    let mut block_end = index.saturating_add(1);
    while block_end < line_end
        && lines
            .get(block_end)
            .is_some_and(|line| line.kind() != DiffLineKind::Context)
    {
        block_end = block_end.saturating_add(1);
    }
    (block_start, block_end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowSegment {
    start: ModelRow,
    len: u32,
    kind: RowSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowSegmentKind {
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
        left_candidate_start: u32,
        right_start: u32,
        right_len: u32,
    },
    SplitExplicit {
        file: FileIndex,
        hunk: HunkIndex,
        left: MaybeDiffLineIndex,
        left_candidate: bool,
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
                left_candidate_start: _,
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
                left_candidate: _,
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
    #[cfg(test)]
    pub(crate) fn new(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
    ) -> Self {
        Self::new_with_trailing_context(changeset, layout, context_expansions, &HashMap::new())
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn new_with_trailing_context_and_controls(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        show_context_controls: bool,
    ) -> Self {
        Self::new_with_trailing_context_controls_and_annotation_candidates(
            changeset,
            layout,
            context_expansions,
            trailing_context_lines,
            UiModelBuildOptions::new(show_context_controls, true, true),
        )
    }

    pub(crate) fn new_with_trailing_context_controls_and_annotation_candidates(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        options: UiModelBuildOptions,
    ) -> Self {
        let visible_files: Vec<_> = (0..changeset.files.len()).map(FileIndex::new).collect();
        Self::new_filtered_with_trailing_context_controls_and_annotation_candidates(
            changeset,
            layout,
            context_expansions,
            trailing_context_lines,
            &visible_files,
            options,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_filtered_with_trailing_context_and_controls(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        visible_files: &[FileIndex],
        show_context_controls: bool,
    ) -> Self {
        Self::new_filtered_with_trailing_context_controls_and_annotation_candidates(
            changeset,
            layout,
            context_expansions,
            trailing_context_lines,
            visible_files,
            UiModelBuildOptions::new(show_context_controls, true, true),
        )
    }

    pub(crate) fn new_filtered_with_trailing_context_controls_and_annotation_candidates(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        visible_files: &[FileIndex],
        options: UiModelBuildOptions,
    ) -> Self {
        let UiModelBuildOptions {
            show_context_controls,
            show_context_expansion_controls,
            build_annotation_candidates,
        } = options;
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
            let mut model = Self::new_filtered_sparse(
                changeset,
                layout,
                context_expansions,
                trailing_context_lines,
                visible_files,
                total_hunks,
                options,
            );
            if !build_annotation_candidates {
                model.annotation_candidate_blocks = AnnotationCandidateIndex::disabled();
            }
            return model;
        }

        let mut rows = Vec::with_capacity(estimated_rows);
        let mut file_start_rows = vec![None; changeset.files.len()];
        let mut file_row_starts = Vec::with_capacity(visible_files.len());
        let mut hunk_start_rows = Vec::with_capacity(total_hunks);
        let mut hunk_row_starts = Vec::with_capacity(total_hunks);
        let mut hunk_row_ends = Vec::with_capacity(total_hunks);

        for file_index in visible_files.iter().copied() {
            let Some(file) = changeset.files.get(file_index.get()) else {
                continue;
            };
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
                        if remaining > 0 && show_context_expansion_controls {
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

                        if remaining > 0 && show_context_expansion_controls {
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
            if remaining > 0 && show_context_expansion_controls {
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

        let annotation_candidate_blocks = if build_annotation_candidates {
            annotation_candidate_blocks_from_rows(changeset, &rows)
        } else {
            Vec::new()
        };
        Self {
            identity: UiModelIdentity::new(),
            row_count: rows.len(),
            rows,
            row_segments: Vec::new(),
            file_start_rows,
            file_row_starts,
            visible_files: visible_files.to_vec(),
            hunk_start_rows,
            hunk_row_starts,
            hunk_row_ends,
            annotation_candidate_blocks: AnnotationCandidateIndex::eager(
                annotation_candidate_blocks,
            ),
        }
    }

    fn new_filtered_sparse(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        trailing_context_lines: &HashMap<ContextKey, usize>,
        visible_files: &[FileIndex],
        total_hunks: usize,
        options: UiModelBuildOptions,
    ) -> Self {
        let UiModelBuildOptions {
            show_context_controls,
            show_context_expansion_controls,
            ..
        } = options;
        let mut row_count = 0usize;
        let base_segment_capacity = changeset
            .files
            .len()
            .saturating_add(total_hunks.saturating_mul(4));
        let split_segment_capacity = if layout == DiffLayoutMode::Split {
            visible_files
                .iter()
                .filter_map(|file| changeset.files.get(file.get()))
                .flat_map(|file| file.hunks())
                .map(|hunk| split_hunk_segment_count(&hunk.lines))
                .fold(0usize, usize::saturating_add)
        } else {
            0
        };
        let mut row_segments =
            Vec::with_capacity(base_segment_capacity.saturating_add(split_segment_capacity));
        let mut file_start_rows = vec![None; changeset.files.len()];
        let mut file_row_starts = Vec::with_capacity(visible_files.len());
        let mut hunk_start_rows = Vec::with_capacity(total_hunks);
        let mut hunk_row_starts = Vec::with_capacity(total_hunks);
        let mut hunk_row_ends = Vec::with_capacity(total_hunks);

        for file_index in visible_files.iter().copied() {
            let Some(file) = changeset.files.get(file_index.get()) else {
                continue;
            };
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
                        if remaining > 0 && show_context_expansion_controls {
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

                        if remaining > 0 && show_context_expansion_controls {
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
            if remaining > 0 && show_context_expansion_controls {
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
            identity: UiModelIdentity::new(),
            rows: Vec::new(),
            row_count,
            row_segments,
            file_start_rows,
            file_row_starts,
            visible_files: visible_files.to_vec(),
            hunk_start_rows,
            hunk_row_starts,
            hunk_row_ends,
            annotation_candidate_blocks: AnnotationCandidateIndex::sparse(),
        }
    }

    pub(crate) fn identity(&self) -> u64 {
        self.identity.0
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
            .saturating_add(
                self.annotation_candidate_blocks
                    .len()
                    .saturating_mul(std::mem::size_of::<AnnotationCandidateBlock>()),
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

    pub(crate) fn annotation_candidate_at_or_after(
        &self,
        changeset: &Changeset,
        row: usize,
    ) -> Option<usize> {
        match self.annotation_candidate_search_at_or_after(changeset, row) {
            AnnotationCandidateSearchResult::Candidate(candidate) => Some(candidate),
            AnnotationCandidateSearchResult::Unindexed
            | AnnotationCandidateSearchResult::Exhausted => None,
        }
    }

    pub(crate) fn annotation_candidate_search_at_or_after(
        &self,
        changeset: &Changeset,
        row: usize,
    ) -> AnnotationCandidateSearchResult {
        {
            let index = self.annotation_candidate_blocks.0.borrow();
            match &*index {
                AnnotationCandidateIndexState::Disabled => {
                    return AnnotationCandidateSearchResult::Exhausted;
                }
                AnnotationCandidateIndexState::Eager(blocks) => {
                    let block_index = blocks.partition_point(|block| block.end() <= row);
                    return blocks
                        .iter()
                        .skip(block_index)
                        .find_map(|block| block.candidate_at_or_after(changeset, row))
                        .map_or(
                            AnnotationCandidateSearchResult::Exhausted,
                            AnnotationCandidateSearchResult::Candidate,
                        );
                }
                AnnotationCandidateIndexState::Sparse(_) => {}
            }
        }
        let first_segment = self
            .row_segments
            .partition_point(|segment| segment.start.get() <= row)
            .saturating_sub(1);
        for segment_index in first_segment..self.row_segments.len() {
            let candidate = self.candidate_in_sparse_segment(changeset, segment_index, row, false);
            if let Some(candidate) = candidate {
                return AnnotationCandidateSearchResult::Candidate(candidate);
            }
            if !self.annotation_candidate_segment_is_bounded(segment_index) {
                return AnnotationCandidateSearchResult::Unindexed;
            }
        }
        AnnotationCandidateSearchResult::Exhausted
    }

    pub(crate) fn annotation_candidate_at_or_before(
        &self,
        changeset: &Changeset,
        row: usize,
    ) -> Option<usize> {
        match self.annotation_candidate_search_at_or_before(changeset, row) {
            AnnotationCandidateSearchResult::Candidate(candidate) => Some(candidate),
            AnnotationCandidateSearchResult::Unindexed
            | AnnotationCandidateSearchResult::Exhausted => None,
        }
    }

    pub(crate) fn annotation_candidate_search_at_or_before(
        &self,
        changeset: &Changeset,
        row: usize,
    ) -> AnnotationCandidateSearchResult {
        {
            let index = self.annotation_candidate_blocks.0.borrow();
            match &*index {
                AnnotationCandidateIndexState::Disabled => {
                    return AnnotationCandidateSearchResult::Exhausted;
                }
                AnnotationCandidateIndexState::Eager(blocks) => {
                    let block_index = blocks.partition_point(|block| block.start() <= row);
                    return blocks[..block_index]
                        .iter()
                        .rev()
                        .find_map(|block| block.candidate_at_or_before(changeset, row))
                        .map_or(
                            AnnotationCandidateSearchResult::Exhausted,
                            AnnotationCandidateSearchResult::Candidate,
                        );
                }
                AnnotationCandidateIndexState::Sparse(_) => {}
            }
        }
        let Some(last_segment) = self
            .row_segments
            .partition_point(|segment| segment.start.get() <= row)
            .checked_sub(1)
        else {
            return AnnotationCandidateSearchResult::Exhausted;
        };
        for segment_index in (0..=last_segment).rev() {
            let candidate = self.candidate_in_sparse_segment(changeset, segment_index, row, true);
            if let Some(candidate) = candidate {
                return AnnotationCandidateSearchResult::Candidate(candidate);
            }
            if !self.annotation_candidate_segment_is_bounded(segment_index) {
                return AnnotationCandidateSearchResult::Unindexed;
            }
        }
        AnnotationCandidateSearchResult::Exhausted
    }

    fn candidate_in_sparse_segment(
        &self,
        changeset: &Changeset,
        segment_index: usize,
        row: usize,
        before: bool,
    ) -> Option<usize> {
        let needs_index = match &*self.annotation_candidate_blocks.0.borrow() {
            AnnotationCandidateIndexState::Sparse(segments) => {
                !segments.contains_key(&segment_index)
            }
            AnnotationCandidateIndexState::Disabled | AnnotationCandidateIndexState::Eager(_) => {
                return None;
            }
        };
        if needs_index {
            let segment = self.row_segments.get(segment_index)?;
            if !self.annotation_candidate_segment_is_bounded(segment_index) {
                return None;
            }
            let blocks =
                annotation_candidate_blocks_from_segments(changeset, std::slice::from_ref(segment));
            if let AnnotationCandidateIndexState::Sparse(segments) =
                &mut *self.annotation_candidate_blocks.0.borrow_mut()
            {
                segments.insert(segment_index, blocks);
            }
        }
        let index = self.annotation_candidate_blocks.0.borrow();
        let AnnotationCandidateIndexState::Sparse(segments) = &*index else {
            return None;
        };
        let blocks = segments.get(&segment_index)?;
        if before {
            blocks
                .iter()
                .rev()
                .find_map(|block| block.candidate_at_or_before(changeset, row))
        } else {
            blocks
                .iter()
                .find_map(|block| block.candidate_at_or_after(changeset, row))
        }
    }

    // Oversized unified segments are intentionally left unindexed so viewport
    // synchronization can discover their visible targets without an O(hunk) scan.
    // Candidate traversal must treat them as boundaries, not targetless segments.
    fn annotation_candidate_segment_is_bounded(&self, segment_index: usize) -> bool {
        self.row_segments.get(segment_index).is_none_or(|segment| {
            !matches!(segment.kind, RowSegmentKind::UnifiedLines { .. })
                || segment.len as usize <= MAX_SYNCHRONOUS_SPARSE_CANDIDATE_SEGMENT_ROWS
        })
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

    pub(crate) fn file_row_range(&self, file: FileIndex) -> Option<Range<usize>> {
        let position = self
            .file_row_starts
            .iter()
            .position(|(candidate, _)| *candidate == file)?;
        let start = self.file_row_starts.get(position)?.1.get();
        let end = self
            .file_row_starts
            .get(position.saturating_add(1))
            .map(|(_, row)| row.get())
            .unwrap_or(self.row_count);
        Some(start..end)
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
        self.context_line_row_for_side(file, DiffSide::New, new_line)
    }

    pub(crate) fn context_line_row_for_side(
        &self,
        file: FileIndex,
        side: DiffSide,
        line: usize,
    ) -> Option<ModelRow> {
        if !self.rows.is_empty() {
            return self
                .rows
                .iter()
                .position(|row| {
                    let UiRow::ContextLine {
                        file: row_file,
                        old_line,
                        new_line,
                    } = row
                    else {
                        return false;
                    };
                    let row_line = match side {
                        DiffSide::Old => old_line,
                        DiffSide::New => new_line,
                    };
                    *row_file == file && *row_line == line
                })
                .map(ModelRow::new);
        }

        self.row_segments.iter().find_map(|segment| {
            let RowSegmentKind::ContextLines {
                file: row_file,
                old_start,
                new_start,
            } = segment.kind
            else {
                return None;
            };
            if row_file != file {
                return None;
            }
            let start = match side {
                DiffSide::Old => old_start,
                DiffSide::New => new_start,
            };
            let offset = line.checked_sub(start as usize)?;
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
                    left_candidate_start: _,
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
                    left_candidate: _,
                    right,
                } if row_file == file && row_hunk == hunk => (left.get()
                    == Some(DiffLineIndex(line))
                    || right.get() == Some(DiffLineIndex(line)))
                .then_some(segment.start.get()),
                RowSegmentKind::FileHeader(_)
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

    pub(crate) fn visual_line_block_at(&self, model_row: usize) -> Option<Range<usize>> {
        let row = self.row(model_row)?;
        if let Some((file, hunk)) = row.typed_hunk_key() {
            return self.hunk_row_range(file.get(), hunk.get());
        }
        let UiRow::ContextLine { file, .. } = row else {
            return None;
        };
        if !self.rows.is_empty() {
            let same_context_file = |row| {
                matches!(
                    self.row(row),
                    Some(UiRow::ContextLine {
                        file: candidate,
                        ..
                    }) if candidate == file
                )
            };
            let mut start = model_row;
            while start > 0 && same_context_file(start - 1) {
                start -= 1;
            }
            let mut end = model_row.saturating_add(1);
            while end < self.len() && same_context_file(end) {
                end += 1;
            }
            return Some(start..end);
        }
        let index = self
            .row_segments
            .partition_point(|segment| segment.start.get() <= model_row)
            .checked_sub(1)?;
        let segment = *self.row_segments.get(index)?;
        matches!(segment.kind, RowSegmentKind::ContextLines { file: candidate, .. } if candidate == file)
            .then_some(segment.start.get()..segment.end())
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
        UiRow::FileHeader(_)
        | UiRow::FileBodyNotice(_)
        | UiRow::Collapsed { .. }
        | UiRow::ContextLine { .. }
        | UiRow::ContextHide { .. }
        | UiRow::HunkHeader { .. } => false,
    }
}

fn annotation_candidate_blocks_from_rows(
    changeset: &Changeset,
    rows: &[UiRow],
) -> Vec<AnnotationCandidateBlock> {
    let mut blocks = Vec::new();
    let mut split_scan = None;
    let mut model_row = 0;
    while let Some(row) = rows.get(model_row).copied() {
        if let UiRow::UnifiedLine { file, hunk, line } = row {
            let mut end = model_row.saturating_add(1);
            while matches!(
                rows.get(end),
                Some(
                    UiRow::UnifiedLine {
                        file: row_file,
                        hunk: row_hunk,
                        line: row_line,
                    }
                    | UiRow::MetaLine {
                        file: row_file,
                        hunk: row_hunk,
                        line: row_line,
                    }
                ) if *row_file == file
                    && *row_hunk == hunk
                    && row_line.get() == line.get().saturating_add(end - model_row)
            ) {
                end = end.saturating_add(1);
            }
            push_unified_annotation_candidate_blocks(
                &mut blocks,
                changeset,
                file,
                hunk,
                model_row,
                line.get(),
                end.saturating_sub(model_row),
            );
            model_row = end;
            continue;
        }
        if direct_row_is_annotation_candidate(changeset, row, &mut split_scan) {
            push_annotation_candidate_range(&mut blocks, model_row..model_row.saturating_add(1));
        }
        model_row = model_row.saturating_add(1);
    }
    blocks
}

fn annotation_candidate_blocks_from_segments(
    changeset: &Changeset,
    segments: &[RowSegment],
) -> Vec<AnnotationCandidateBlock> {
    let mut blocks = Vec::new();
    for segment in segments {
        match segment.kind {
            RowSegmentKind::UnifiedLines {
                file,
                hunk,
                line_start,
            } if diff_file_has_annotation_path(changeset, file) => {
                push_sparse_unified_annotation_candidate_chunks(
                    &mut blocks,
                    changeset,
                    file,
                    hunk,
                    segment.start.get(),
                    line_start as usize,
                    segment.len as usize,
                );
            }
            RowSegmentKind::ContextLines { file, .. }
            | RowSegmentKind::SplitContextLines { file, .. }
                if diff_file_has_annotation_path(changeset, file) =>
            {
                push_annotation_candidate_range(&mut blocks, segment.start.get()..segment.end());
            }
            RowSegmentKind::SplitChangeRun {
                file,
                left_len,
                left_candidate_start,
                right_len,
                ..
            } if diff_file_has_annotation_path(changeset, file) => {
                let start = segment.start.get();
                push_annotation_candidate_range(
                    &mut blocks,
                    start..start.saturating_add(right_len as usize),
                );
                let left_start = left_candidate_start.max(right_len) as usize;
                push_annotation_candidate_range(
                    &mut blocks,
                    start.saturating_add(left_start)..start.saturating_add(left_len as usize),
                );
            }
            RowSegmentKind::SplitExplicit {
                file,
                left_candidate,
                right,
                ..
            } if diff_file_has_annotation_path(changeset, file)
                && (left_candidate || right.is_some()) =>
            {
                push_annotation_candidate_range(&mut blocks, segment.start.get()..segment.end());
            }
            RowSegmentKind::FileHeader(_)
            | RowSegmentKind::FileBodyNotice(_)
            | RowSegmentKind::Collapsed { .. }
            | RowSegmentKind::ContextLines { .. }
            | RowSegmentKind::ContextHide { .. }
            | RowSegmentKind::HunkHeader { .. }
            | RowSegmentKind::UnifiedLines { .. }
            | RowSegmentKind::SplitContextLines { .. }
            | RowSegmentKind::SplitMetaLines { .. }
            | RowSegmentKind::SplitChangeRun { .. }
            | RowSegmentKind::SplitExplicit { .. } => {}
        }
    }
    blocks
}

fn push_sparse_unified_annotation_candidate_chunks(
    blocks: &mut Vec<AnnotationCandidateBlock>,
    changeset: &Changeset,
    file: FileIndex,
    hunk: HunkIndex,
    model_start: usize,
    line_start: usize,
    len: usize,
) {
    let Some(lines) = changeset
        .files
        .get(file.get())
        .filter(|file| file.old_path().is_some() || file.new_path().is_some())
        .and_then(|file| file.hunks().get(hunk.get()))
        .map(|hunk| hunk.lines.as_slice())
    else {
        return;
    };
    let line_end = line_start.saturating_add(len).min(lines.len());
    let model_end = model_start.saturating_add(line_end.saturating_sub(line_start));
    let mut chunk_start = model_start;
    let mut bits = [0u64; SPARSE_ANNOTATION_CANDIDATE_WORDS];
    let mut index = line_start;
    while index < line_end {
        if lines[index].kind() == DiffLineKind::Context {
            if lines[index].new_line().is_some() || lines[index].old_line().is_some() {
                set_sparse_annotation_candidate(
                    blocks,
                    model_start,
                    &mut chunk_start,
                    &mut bits,
                    model_start.saturating_add(index.saturating_sub(line_start)),
                    model_end,
                );
            }
            index = index.saturating_add(1);
            continue;
        }

        let block_start = index;
        while index < line_end && lines[index].kind() != DiffLineKind::Context {
            index = index.saturating_add(1);
        }
        let additions = lines[block_start..index]
            .iter()
            .filter(|line| line.kind() == DiffLineKind::Addition)
            .count();
        let mut deletion = 0usize;
        for (block_offset, line) in lines[block_start..index].iter().enumerate() {
            let candidate = match line.kind() {
                DiffLineKind::Addition => line.new_line().is_some(),
                DiffLineKind::Deletion => {
                    let unpaired = deletion >= additions;
                    deletion = deletion.saturating_add(1);
                    unpaired && line.old_line().is_some()
                }
                DiffLineKind::Context | DiffLineKind::Meta => false,
            };
            if candidate {
                set_sparse_annotation_candidate(
                    blocks,
                    model_start,
                    &mut chunk_start,
                    &mut bits,
                    model_start
                        .saturating_add(block_start.saturating_sub(line_start))
                        .saturating_add(block_offset),
                    model_end,
                );
            }
        }
    }
    push_sparse_annotation_candidate_chunk(blocks, chunk_start, model_end, bits);
}

fn set_sparse_annotation_candidate(
    blocks: &mut Vec<AnnotationCandidateBlock>,
    model_start: usize,
    chunk_start: &mut usize,
    bits: &mut [u64; SPARSE_ANNOTATION_CANDIDATE_WORDS],
    model_row: usize,
    model_end: usize,
) {
    let chunk_index =
        model_row.saturating_sub(model_start) / SPARSE_ANNOTATION_CANDIDATE_CHUNK_ROWS;
    let candidate_chunk_start = model_start
        .saturating_add(chunk_index.saturating_mul(SPARSE_ANNOTATION_CANDIDATE_CHUNK_ROWS));
    if candidate_chunk_start != *chunk_start {
        push_sparse_annotation_candidate_chunk(blocks, *chunk_start, model_end, *bits);
        bits.fill(0);
        *chunk_start = candidate_chunk_start;
    }
    let offset = model_row.saturating_sub(candidate_chunk_start);
    bits[offset / u64::BITS as usize] |= 1u64 << (offset % u64::BITS as usize);
}

fn push_sparse_annotation_candidate_chunk(
    blocks: &mut Vec<AnnotationCandidateBlock>,
    chunk_start: usize,
    model_end: usize,
    bits: [u64; SPARSE_ANNOTATION_CANDIDATE_WORDS],
) {
    if bits.iter().all(|word| *word == 0) {
        return;
    }
    blocks.push(AnnotationCandidateBlock::SparseCandidates {
        range: chunk_start
            ..chunk_start
                .saturating_add(SPARSE_ANNOTATION_CANDIDATE_CHUNK_ROWS)
                .min(model_end),
        bits,
    });
}

fn push_unified_annotation_candidate_blocks(
    blocks: &mut Vec<AnnotationCandidateBlock>,
    changeset: &Changeset,
    file: FileIndex,
    hunk: HunkIndex,
    model_start: usize,
    line_start: usize,
    len: usize,
) {
    let Some(file_diff) = changeset.files.get(file.get()) else {
        return;
    };
    if file_diff.old_path().is_none() && file_diff.new_path().is_none() {
        return;
    }
    let Some(lines) = file_diff.hunks().get(hunk.get()).map(|hunk| &hunk.lines) else {
        return;
    };
    let line_end = line_start.saturating_add(len).min(lines.len());
    let mut index = line_start;
    while index < line_end {
        if lines[index].kind() == DiffLineKind::Context {
            let context_start = index;
            while index < line_end && lines[index].kind() == DiffLineKind::Context {
                index = index.saturating_add(1);
            }
            let row_start = model_start.saturating_add(context_start.saturating_sub(line_start));
            push_annotation_candidate_range(
                blocks,
                row_start..row_start.saturating_add(index.saturating_sub(context_start)),
            );
            continue;
        }

        let block_start = index;
        while index < line_end && lines[index].kind() != DiffLineKind::Context {
            index = index.saturating_add(1);
        }
        let block_end = index;
        let additions = lines[block_start..block_end]
            .iter()
            .filter(|line| line.kind() == DiffLineKind::Addition)
            .count();
        let mut deletion = 0;
        let mut first_addition = None;
        let mut last_addition = None;
        let mut first_unpaired_deletion = None;
        let mut last_unpaired_deletion = None;
        for (offset, line) in lines[block_start..block_end].iter().enumerate() {
            match line.kind() {
                DiffLineKind::Addition if line.new_line().is_some() => {
                    let offset = Some(row_count_u32(offset));
                    first_addition = first_addition.or(offset);
                    last_addition = offset;
                }
                DiffLineKind::Deletion => {
                    let unpaired = deletion >= additions;
                    deletion = deletion.saturating_add(1);
                    if unpaired && line.old_line().is_some() {
                        let offset = Some(row_count_u32(offset));
                        first_unpaired_deletion = first_unpaired_deletion.or(offset);
                        last_unpaired_deletion = offset;
                    }
                }
                DiffLineKind::Context | DiffLineKind::Addition | DiffLineKind::Meta => {}
            }
        }
        if first_addition.is_some() || first_unpaired_deletion.is_some() {
            let row_start = model_start.saturating_add(block_start.saturating_sub(line_start));
            blocks.push(AnnotationCandidateBlock::UnifiedChange {
                range: row_start..row_start.saturating_add(block_end - block_start),
                file,
                hunk,
                line_start: row_count_u32(block_start),
                first_addition,
                last_addition,
                first_unpaired_deletion,
                last_unpaired_deletion,
            });
        }
    }
}

fn direct_row_is_annotation_candidate(
    changeset: &Changeset,
    row: UiRow,
    split_scan: &mut Option<UnifiedCandidateScan>,
) -> bool {
    match row {
        UiRow::ContextLine { file, .. } => diff_file_has_annotation_path(changeset, file),
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => {
            let Some(file_diff) = changeset.files.get(file.get()) else {
                return false;
            };
            if file_diff.old_path().is_none() && file_diff.new_path().is_none() {
                return false;
            }
            let Some(lines) = file_diff.hunks().get(hunk.get()).map(|hunk| &hunk.lines) else {
                return false;
            };
            right
                .get()
                .and_then(|line| lines.get(line.get()))
                .is_some_and(|line| line.new_line().is_some())
                || (right.get().is_none()
                    && left.get().is_some_and(|line| {
                        let line_index = line.get();
                        if !lines
                            .get(line_index)
                            .is_some_and(|line| line.old_line().is_some())
                        {
                            return false;
                        }
                        unified_candidate_scan(
                            lines,
                            file,
                            hunk,
                            0,
                            lines.len(),
                            line_index,
                            split_scan,
                        )
                        .first_unpaired_deletion
                        .is_some_and(|first| line_index >= first)
                    }))
        }
        UiRow::FileHeader(_)
        | UiRow::FileBodyNotice(_)
        | UiRow::Collapsed { .. }
        | UiRow::ContextHide { .. }
        | UiRow::HunkHeader { .. }
        | UiRow::UnifiedLine { .. }
        | UiRow::MetaLine { .. } => false,
    }
}

fn diff_file_has_annotation_path(changeset: &Changeset, file: FileIndex) -> bool {
    changeset
        .files
        .get(file.get())
        .is_some_and(|file| file.old_path().is_some() || file.new_path().is_some())
}

fn push_annotation_candidate_range(
    blocks: &mut Vec<AnnotationCandidateBlock>,
    range: Range<usize>,
) {
    if range.is_empty() {
        return;
    }
    if let Some(AnnotationCandidateBlock::Range(previous)) = blocks.last_mut()
        && previous.end == range.start
    {
        previous.end = range.end;
    } else {
        blocks.push(AnnotationCandidateBlock::Range(range));
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
                let change_start = index;
                while index < lines.len()
                    && matches!(
                        lines[index].kind(),
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    index += 1;
                }

                let shape = change_run_shape(lines, change_start..index);
                let paired_rows = shape.deletion_count.max(shape.addition_count);
                if shape.deletions_contiguous && shape.additions_contiguous {
                    for pair_index in 0..paired_rows {
                        rows.push(UiRow::SplitLine {
                            file: file_index,
                            hunk: hunk_index,
                            left: (pair_index < shape.deletion_count)
                                .then(|| DiffLineIndex::new(shape.deletion_start + pair_index))
                                .into(),
                            right: (pair_index < shape.addition_count)
                                .then(|| DiffLineIndex::new(shape.addition_start + pair_index))
                                .into(),
                        });
                    }
                } else {
                    let mut deletions = (change_start..index)
                        .filter(|line| lines[*line].kind() == DiffLineKind::Deletion);
                    let mut additions = (change_start..index)
                        .filter(|line| lines[*line].kind() == DiffLineKind::Addition);
                    for _ in 0..paired_rows {
                        rows.push(UiRow::SplitLine {
                            file: file_index,
                            hunk: hunk_index,
                            left: deletions.next().map(DiffLineIndex::new).into(),
                            right: additions.next().map(DiffLineIndex::new).into(),
                        });
                    }
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
    let mut paired_deletions_remaining = 0usize;
    while index < lines.len() {
        if lines[index].kind() != DiffLineKind::Context
            && (index == 0 || lines[index - 1].kind() == DiffLineKind::Context)
        {
            paired_deletions_remaining = lines[index..]
                .iter()
                .take_while(|line| line.kind() != DiffLineKind::Context)
                .filter(|line| line.kind() == DiffLineKind::Addition)
                .count();
        }
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
                let change_start = index;
                while index < lines.len()
                    && matches!(
                        lines[index].kind(),
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    index += 1;
                }

                let shape = change_run_shape(lines, change_start..index);
                let left_candidate_start = paired_deletions_remaining.min(shape.deletion_count);
                paired_deletions_remaining =
                    paired_deletions_remaining.saturating_sub(shape.deletion_count);
                let paired_rows = shape.deletion_count.max(shape.addition_count);

                if shape.deletions_contiguous && shape.additions_contiguous {
                    push_row_segment(
                        row_segments,
                        row_count,
                        paired_rows,
                        RowSegmentKind::SplitChangeRun {
                            file: file_index,
                            hunk: hunk_index,
                            left_start: row_count_u32(shape.deletion_start),
                            left_len: row_count_u32(shape.deletion_count),
                            left_candidate_start: row_count_u32(left_candidate_start),
                            right_start: row_count_u32(shape.addition_start),
                            right_len: row_count_u32(shape.addition_count),
                        },
                    );
                } else {
                    let mut deletions = (change_start..index)
                        .filter(|line| lines[*line].kind() == DiffLineKind::Deletion);
                    let mut additions = (change_start..index)
                        .filter(|line| lines[*line].kind() == DiffLineKind::Addition);
                    for pair_index in 0..paired_rows {
                        push_row_segment(
                            row_segments,
                            row_count,
                            1,
                            RowSegmentKind::SplitExplicit {
                                file: file_index,
                                hunk: hunk_index,
                                left: deletions.next().map(DiffLineIndex::new).into(),
                                left_candidate: pair_index >= left_candidate_start
                                    && pair_index < shape.deletion_count,
                                right: additions.next().map(DiffLineIndex::new).into(),
                            },
                        );
                    }
                }
            }
        }
    }
}

fn split_hunk_segment_count(lines: &[DiffLine]) -> usize {
    let mut segments = 0usize;
    let mut index = 0usize;
    while index < lines.len() {
        match lines[index].kind() {
            DiffLineKind::Context => {
                while index < lines.len() && lines[index].kind() == DiffLineKind::Context {
                    index += 1;
                }
                segments += 1;
            }
            DiffLineKind::Meta => {
                while index < lines.len() && lines[index].kind() == DiffLineKind::Meta {
                    index += 1;
                }
                segments += 1;
            }
            DiffLineKind::Deletion | DiffLineKind::Addition => {
                let change_start = index;
                while index < lines.len()
                    && matches!(
                        lines[index].kind(),
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    index += 1;
                }
                let shape = change_run_shape(lines, change_start..index);
                segments = segments.saturating_add(
                    if shape.deletions_contiguous && shape.additions_contiguous {
                        1
                    } else {
                        shape.deletion_count.max(shape.addition_count)
                    },
                );
            }
        }
    }
    segments
}

#[derive(Debug, Clone, Copy)]
struct ChangeRunShape {
    deletion_start: usize,
    deletion_count: usize,
    deletions_contiguous: bool,
    addition_start: usize,
    addition_count: usize,
    additions_contiguous: bool,
}

fn change_run_shape(lines: &[DiffLine], range: Range<usize>) -> ChangeRunShape {
    let mut deletion_start = None;
    let mut previous_deletion = None;
    let mut deletion_count = 0usize;
    let mut deletions_contiguous = true;
    let mut addition_start = None;
    let mut previous_addition = None;
    let mut addition_count = 0usize;
    let mut additions_contiguous = true;

    for index in range {
        match lines[index].kind() {
            DiffLineKind::Deletion => {
                deletion_start.get_or_insert(index);
                if previous_deletion.is_some_and(|previous| index != previous + 1) {
                    deletions_contiguous = false;
                }
                previous_deletion = Some(index);
                deletion_count += 1;
            }
            DiffLineKind::Addition => {
                addition_start.get_or_insert(index);
                if previous_addition.is_some_and(|previous| index != previous + 1) {
                    additions_contiguous = false;
                }
                previous_addition = Some(index);
                addition_count += 1;
            }
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    ChangeRunShape {
        deletion_start: deletion_start.unwrap_or(0),
        deletion_count,
        deletions_contiguous,
        addition_start: addition_start.unwrap_or(0),
        addition_count,
        additions_contiguous,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use mark_diff::{
        Changeset, DiffFile, DiffFileBody, DiffHunk, DiffLine, FileChange, HunkLineRanges, RepoRoot,
    };

    use super::*;

    #[test]
    fn context_lines_share_text_and_enforce_index_limits() {
        let lines = ContextLines::new("one\ntwo\nthree\n".to_owned(), 3, 5)
            .expect("three lines should fit");
        assert_eq!(lines.iter().collect::<Vec<_>>(), ["one", "two", "three"]);
        assert!(ContextLines::new("one\ntwo\nthree\n".to_owned(), 2, 5).is_none());
        assert!(ContextLines::new("oversized".to_owned(), 1, 5).is_none());
    }

    #[test]
    fn split_change_runs_pair_interleaved_deletions_and_additions_without_staging() {
        let lines = vec![
            DiffLine::deletion(1, "old one"),
            DiffLine::addition(1, "new one"),
            DiffLine::deletion(2, "old two"),
            DiffLine::addition(2, "new two"),
        ];
        assert_eq!(split_hunk_segment_count(&lines), 2);

        let file = FileIndex::new(0);
        let hunk = HunkIndex::new(0);
        let mut eager = Vec::new();
        push_split_hunk_rows(&mut eager, file, hunk, &lines);

        assert_eq!(
            eager,
            vec![
                UiRow::SplitLine {
                    file,
                    hunk,
                    left: MaybeDiffLineIndex::some(DiffLineIndex::new(0)),
                    right: MaybeDiffLineIndex::some(DiffLineIndex::new(1)),
                },
                UiRow::SplitLine {
                    file,
                    hunk,
                    left: MaybeDiffLineIndex::some(DiffLineIndex::new(2)),
                    right: MaybeDiffLineIndex::some(DiffLineIndex::new(3)),
                },
            ]
        );

        let mut segments = Vec::new();
        let mut row_count = 0;
        push_split_hunk_segments(&mut segments, &mut row_count, file, hunk, &lines);
        let sparse = segments
            .iter()
            .flat_map(|segment| {
                let start = segment.start.get();
                (start..start + segment.len as usize).filter_map(|row| segment.row_at(row))
            })
            .collect::<Vec<_>>();
        assert_eq!(sparse, eager);
    }

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
                    UiModelBuildOptions::new(show_context_controls, true, true),
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
