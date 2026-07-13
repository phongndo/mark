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

impl HighlightedSide {
    pub(crate) fn memory_bytes(&self) -> usize {
        let lines = self
            .lines
            .iter()
            .map(|line| {
                std::mem::size_of::<HighlightedLine>().saturating_add(
                    line.segments.len() * std::mem::size_of::<mark_syntax::SyntaxSegment>(),
                )
            })
            .sum::<usize>();
        // A highlighting result shares one immutable table across its lines.
        // Syntax workers currently produce one result per HighlightedSide.
        lines.saturating_add(
            self.lines
                .first()
                .map_or(0, |line| line.scope_table.memory_bytes()),
        )
    }

    pub(crate) fn scope_table_stats(&self) -> (usize, usize, u64, u64) {
        self.lines.first().map_or((0, 0, 0, 0), |line| {
            let (hits, misses) = line.scope_table.style_cache_stats();
            (
                line.scope_table.stack_count(),
                line.scope_table.memory_bytes(),
                hits,
                misses,
            )
        })
    }
}
