use std::{ops::Deref, sync::Arc};

use mark_syntax::HighlightedLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxPosition {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) hunk: usize,
    pub(crate) side: DiffSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SyntaxSourceKind {
    HunkSide { hunk: usize },
    FullFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxSourceId {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) side: DiffSide,
    pub(crate) kind: SyntaxSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxKey {
    pub(crate) source: SyntaxSourceId,
    pub(crate) language_hash: u64,
    pub(crate) theme_id: u64,
}

impl SyntaxKey {
    pub(crate) fn generation(self) -> u64 {
        self.source.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxSkipReason {
    InvalidPosition,
    NoPath,
    NoLanguage,
    NoSource,
    TooLarge,
    QueueClosed,
    HighlightError,
}

#[derive(Debug, Clone)]
pub(crate) struct HighlightedSide {
    pub(crate) lines: Vec<HighlightedLine>,
}

/// Cheap owned access to one cached highlighted line.
///
/// Rendering cannot borrow a line directly from the mutable LRU while it also
/// borrows the rest of `DiffApp`. Keeping the result side behind one `Arc`
/// avoids deep-cloning the line's segment vector on every rendered frame.
#[derive(Debug, Clone)]
pub(crate) struct HighlightedLineRef {
    side: Arc<HighlightedSide>,
    line: usize,
}

impl HighlightedLineRef {
    pub(crate) fn new(side: Arc<HighlightedSide>, line: usize) -> Option<Self> {
        (line < side.lines.len()).then_some(Self { side, line })
    }
}

impl Deref for HighlightedLineRef {
    type Target = HighlightedLine;

    fn deref(&self) -> &Self::Target {
        &self.side.lines[self.line]
    }
}

impl HighlightedSide {
    pub(crate) fn line_memory_bytes(&self) -> usize {
        self.lines
            .iter()
            .map(|line| {
                std::mem::size_of::<HighlightedLine>().saturating_add(
                    line.segments.len() * std::mem::size_of::<mark_syntax::SyntaxSegment>(),
                )
            })
            .sum()
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        // A highlighting result shares one immutable table across its lines.
        // The weighted LRU stays conservative and charges each result; runtime
        // diagnostics deduplicate tables shared between several results.
        self.line_memory_bytes().saturating_add(
            self.lines
                .first()
                .map_or(0, |line| line.scope_table.memory_bytes()),
        )
    }
}
