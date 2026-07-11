use std::{fmt, sync::Arc};

use super::ast::{AnchorKind, Ast};
use super::backtrack::FallbackMatcher;
use super::prefilter::{LiteralSet, Prefilter};
use super::scanner::Scanner;
use super::translate::{AnchorStrategy, Translation, translate};
use super::{AnchorContext, MatchResult, Matcher};

/// Build error for the native fast-path matcher. Kept as a distinct type so
/// call sites that previously handled regex-automata compile failures continue
/// to compile against a Display/Error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomataBuildError {
    message: String,
}

impl AutomataBuildError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AutomataBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AutomataBuildError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralMatcher {
    literal: String,
}

impl LiteralMatcher {
    pub fn new(literal: impl Into<String>) -> Self {
        Self {
            literal: literal.into(),
        }
    }
}

impl Matcher for LiteralMatcher {
    fn find(&self, line: &str, from: usize, _ctx: AnchorContext) -> Option<MatchResult> {
        if !line.is_char_boundary(from) {
            return None;
        }
        let haystack = line.get(from..)?;
        let offset = haystack.find(&self.literal)?;
        let start = from + offset;
        let end = start + self.literal.len();
        Some(MatchResult {
            start,
            end,
            captures: vec![Some(start..end)],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleMatcher {
    Literal(String),
    LineStartLiteral(String),
    FileStartLiteral(String),
    ContinuationLiteral(String),
}

impl SimpleMatcher {
    pub fn from_pattern(pattern: &str) -> Self {
        if let Some(rest) = pattern.strip_prefix(r"\A") {
            Self::FileStartLiteral(unescape_literal(rest))
        } else if let Some(rest) = pattern.strip_prefix(r"\G") {
            Self::ContinuationLiteral(unescape_literal(rest))
        } else if let Some(rest) = pattern.strip_prefix('^') {
            Self::LineStartLiteral(unescape_literal(rest))
        } else {
            Self::Literal(unescape_literal(pattern))
        }
    }

    /// Build a simple matcher from a parsed AST when the pattern is a pure
    /// literal (optionally with a single leading anchor). Returns `None` when
    /// the pattern needs the general VM (classes, quantifiers, groups, …).
    pub fn try_from_translation(translation: &Translation) -> Option<Self> {
        if translation.parsed.capture_count > 0 {
            return None;
        }
        let flags = translation.parsed.flags;
        if flags.case_insensitive || flags.multi_line || flags.dot_matches_new_line {
            return None;
        }
        if matches!(translation.anchor_strategy, AnchorStrategy::Fallback) {
            return None;
        }
        let literal = pure_literal_body(&translation.parsed.ast)?;
        Some(match translation.anchor_strategy {
            AnchorStrategy::None => Self::Literal(literal),
            AnchorStrategy::LineStartGuard => Self::LineStartLiteral(literal),
            AnchorStrategy::TextStartGuard => Self::FileStartLiteral(literal),
            AnchorStrategy::ContinuationGuard => Self::ContinuationLiteral(literal),
            AnchorStrategy::Fallback => return None,
        })
    }

    fn match_at(&self, line: &str, start: usize) -> Option<MatchResult> {
        let literal = match self {
            Self::Literal(literal)
            | Self::LineStartLiteral(literal)
            | Self::FileStartLiteral(literal)
            | Self::ContinuationLiteral(literal) => literal,
        };
        let end = start.checked_add(literal.len())?;
        (line.get(start..end)? == literal).then(|| MatchResult {
            start,
            end,
            captures: vec![Some(start..end)],
        })
    }

    fn unanchored_literal(&self) -> Option<&str> {
        match self {
            Self::Literal(literal) => Some(literal),
            Self::LineStartLiteral(_)
            | Self::FileStartLiteral(_)
            | Self::ContinuationLiteral(_) => None,
        }
    }
}

impl Matcher for SimpleMatcher {
    fn find(&self, line: &str, from: usize, ctx: AnchorContext) -> Option<MatchResult> {
        if !line.is_char_boundary(from) {
            return None;
        }
        match self {
            Self::Literal(literal) => {
                let haystack = line.get(from..)?;
                let offset = haystack.find(literal)?;
                self.match_at(line, from + offset)
            }
            Self::LineStartLiteral(_) => {
                if from == 0 {
                    self.match_at(line, 0)
                } else {
                    None
                }
            }
            Self::FileStartLiteral(_) => {
                if ctx.allow_a && from == 0 {
                    self.match_at(line, 0)
                } else {
                    None
                }
            }
            Self::ContinuationLiteral(_) => {
                if ctx.allow_g && ctx.g_pos >= from && line.is_char_boundary(ctx.g_pos) {
                    self.match_at(line, ctx.g_pos)
                } else {
                    None
                }
            }
        }
    }
}

/// Pure-literal body of an AST that is at most `anchor? · literal*`.
fn pure_literal_body(ast: &Ast) -> Option<String> {
    match ast {
        Ast::Empty => Some(String::new()),
        Ast::Literal(literal) => Some(literal.clone()),
        Ast::Concat(nodes) => {
            let mut out = String::new();
            let mut saw_non_anchor = false;
            for node in nodes {
                match node {
                    Ast::Empty => {}
                    Ast::Literal(literal) => {
                        saw_non_anchor = true;
                        out.push_str(literal);
                    }
                    Ast::Anchor(kind)
                        if !saw_non_anchor
                            && matches!(
                                kind,
                                AnchorKind::LineStart
                                    | AnchorKind::TextStart
                                    | AnchorKind::Continuation
                            ) => {}
                    _ => return None,
                }
            }
            Some(out)
        }
        Ast::Anchor(AnchorKind::LineStart | AnchorKind::TextStart | AnchorKind::Continuation) => {
            Some(String::new())
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum NativeEngine {
    Simple(SimpleMatcher),
    /// General native VM for DFA-routable (and best-effort) patterns.
    Vm(FallbackMatcher),
}

/// Native "fast path" matcher. Previously backed by regex-automata; now uses
/// literal specializations plus the AST fallback VM
/// with a required-literal prefilter.
#[derive(Debug, Clone)]
pub struct AutomataMatcher {
    engine: NativeEngine,
    translation: Translation,
    prefilter: Prefilter,
}

impl AutomataMatcher {
    pub fn new(pattern: &str) -> Result<Self, AutomataBuildError> {
        let translation = translate(pattern);
        Self::from_translation(translation)
    }

    pub fn from_translation(translation: Translation) -> Result<Self, AutomataBuildError> {
        let prefilter = Prefilter::from_regex(&translation.parsed);
        let engine = if let Some(simple) = SimpleMatcher::try_from_translation(&translation) {
            NativeEngine::Simple(simple)
        } else {
            // Match against the original Oniguruma pattern so anchors/classes
            // keep AST semantics (the translated spelling is diagnostic-only).
            NativeEngine::Vm(FallbackMatcher::from_parsed(
                Arc::clone(&translation.parsed),
                super::backtrack::DEFAULT_STEP_BUDGET,
            ))
        };
        Ok(Self {
            engine,
            translation,
            prefilter,
        })
    }

    pub fn translation(&self) -> &Translation {
        &self.translation
    }

    pub fn is_simple(&self) -> bool {
        matches!(self.engine, NativeEngine::Simple(_))
    }

    pub(crate) fn unanchored_literal(&self) -> Option<&str> {
        match &self.engine {
            NativeEngine::Simple(matcher) => matcher.unanchored_literal(),
            NativeEngine::Vm(_) => None,
        }
    }

    pub(crate) fn restricted_start_bytes(&self) -> Option<Vec<u8>> {
        match &self.engine {
            NativeEngine::Simple(matcher) => matcher
                .unanchored_literal()
                .and_then(|literal| literal.as_bytes().first().copied())
                .map(|byte| vec![byte]),
            NativeEngine::Vm(matcher) => matcher.restricted_start_bytes(),
        }
    }

    pub fn prefilter_may_match(&self, line: &str, from: usize) -> Option<bool> {
        self.prefilter
            .is_enabled()
            .then(|| self.prefilter.may_match(line, from))
    }

    pub(crate) fn find_report_for_selection(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Result<(Option<MatchResult>, Option<usize>), super::FallbackError> {
        match &self.engine {
            NativeEngine::Simple(matcher) => Ok((matcher.find(line, from, ctx), None)),
            NativeEngine::Vm(matcher) => {
                if !anchor_permits_search(self.translation.anchor_strategy, from, ctx, line) {
                    return Ok((None, None));
                }
                matcher
                    .try_find_for_selection(line, from, ctx)
                    .map(|report| (report.result, Some(report.steps)))
            }
        }
    }

    pub(crate) fn find_report_at(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
    ) -> Result<(Option<MatchResult>, Option<usize>), super::FallbackError> {
        if !anchor_permits_at(self.translation.anchor_strategy, start, ctx, line) {
            return Ok((None, None));
        }
        match &self.engine {
            NativeEngine::Simple(matcher) => Ok((matcher.match_at(line, start), None)),
            NativeEngine::Vm(matcher) => matcher
                .try_find_at(line, start, ctx)
                .map(|report| (report.result, Some(report.steps))),
        }
    }

    pub(crate) fn find_at_without_captures_with_scratch(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
    ) -> Result<Option<MatchResult>, super::FallbackError> {
        if !anchor_permits_at(self.translation.anchor_strategy, start, ctx, line) {
            return Ok(None);
        }
        match &self.engine {
            NativeEngine::Simple(matcher) => Ok(matcher.match_at(line, start)),
            NativeEngine::Vm(matcher) => matcher
                .try_find_at_without_captures_with_scratch(line, start, ctx, scratch)
                .map(|report| report.result),
        }
    }
}

impl Matcher for AutomataMatcher {
    fn find(&self, line: &str, from: usize, ctx: AnchorContext) -> Option<MatchResult> {
        match &self.engine {
            NativeEngine::Simple(matcher) => matcher.find(line, from, ctx),
            NativeEngine::Vm(matcher) => {
                // Anchor strategies previously enforced by regex-automata Input
                // spans. The VM also re-checks anchors, but bail early when the
                // strategy makes a match impossible.
                if !anchor_permits_search(self.translation.anchor_strategy, from, ctx, line) {
                    return None;
                }
                // FallbackMatcher applies its own prefilter; skip a redundant
                // outer check for the Vm path to avoid double work.
                matcher.find(line, from, ctx)
            }
        }
    }
}

fn anchor_permits_search(
    strategy: AnchorStrategy,
    from: usize,
    ctx: AnchorContext,
    line: &str,
) -> bool {
    match strategy {
        AnchorStrategy::None => true,
        AnchorStrategy::TextStartGuard => ctx.allow_a && from == 0,
        AnchorStrategy::LineStartGuard => from == 0,
        AnchorStrategy::ContinuationGuard => {
            ctx.allow_g && ctx.g_pos >= from && line.is_char_boundary(ctx.g_pos)
        }
        // Non-leading anchors are handled inside the VM.
        AnchorStrategy::Fallback => true,
    }
}

fn anchor_permits_at(
    strategy: AnchorStrategy,
    start: usize,
    ctx: AnchorContext,
    line: &str,
) -> bool {
    match strategy {
        AnchorStrategy::ContinuationGuard => {
            ctx.allow_g && ctx.g_pos == start && line.is_char_boundary(start)
        }
        _ => anchor_permits_search(strategy, start, ctx, line),
    }
}

#[derive(Debug, Clone)]
enum PatternEntry {
    Literal(String),
    Matcher(Arc<super::CompiledPattern>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrontierBuildPolicy {
    Auto,
    #[cfg(test)]
    Force,
}

#[derive(Debug, Clone)]
struct CandidateFrontier {
    scanner: Scanner,
    opaque_unrestricted_entries: Vec<usize>,
    opaque_start_byte_entries: Vec<Vec<usize>>,
}

/// Native multi-pattern matcher. Leftmost match wins; on equal start offsets
/// the lowest pattern index wins (regex-automata compatible).
#[derive(Debug, Clone)]
pub struct PatternSetMatcher {
    entries: Vec<PatternEntry>,
    unrestricted_entries: Vec<usize>,
    start_byte_entries: Vec<Vec<usize>>,
    /// Translated pattern spellings (diagnostic / inventory surface).
    patterns: Vec<String>,
    /// Present when every pattern is a pure unanchored literal.
    literal_set: Option<LiteralSet>,
    /// Shared ordered NFA for candidate sets wholly inside the regular subset.
    scanner: Option<Scanner>,
    /// Exact mixed frontier: regular candidates are scanned together, and the
    /// opaque candidates are searched only as far as the regular bound they can
    /// still beat. Candidate indexes are always the original grammar order.
    frontier: Option<CandidateFrontier>,
    #[cfg(test)]
    force_regular_replay_failure: Option<usize>,
}

impl PatternSetMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, AutomataBuildError> {
        let patterns = patterns
            .iter()
            .map(|pattern| Arc::new(super::CompiledPattern::new(pattern)))
            .collect::<Vec<_>>();
        Ok(Self::from_compiled(&patterns))
    }

    /// Builds the ordered set from already-compiled patterns. The tokenizer
    /// uses this path so constructing a candidate set never reparses regexes.
    pub fn from_compiled(patterns: &[Arc<super::CompiledPattern>]) -> Self {
        Self::from_compiled_with_frontier_policy(patterns, FrontierBuildPolicy::Auto)
    }

    fn from_compiled_with_frontier_policy(
        patterns: &[Arc<super::CompiledPattern>],
        frontier_policy: FrontierBuildPolicy,
    ) -> Self {
        let mut entries = Vec::with_capacity(patterns.len());
        let mut translated = Vec::with_capacity(patterns.len());
        let mut all_literals = Vec::with_capacity(patterns.len());
        let mut literals_only = true;
        let mut unrestricted_entries = Vec::new();
        let mut start_byte_entries = (0..=u8::MAX)
            .map(|_| Vec::<usize>::new())
            .collect::<Vec<_>>();
        for (index, pattern) in patterns.iter().enumerate() {
            translated.push(pattern.translated_pattern().to_owned());
            if let Some(literal) = pattern.unanchored_literal() {
                let literal = literal.to_owned();
                if literals_only {
                    all_literals.push(literal.clone());
                }
                if let Some(byte) = literal.as_bytes().first().copied() {
                    start_byte_entries[byte as usize].push(index);
                } else {
                    unrestricted_entries.push(index);
                }
                entries.push(PatternEntry::Literal(literal));
            } else {
                literals_only = false;
                all_literals.clear();
                if let Some(bytes) = pattern.restricted_start_bytes() {
                    for byte in bytes {
                        start_byte_entries[*byte as usize].push(index);
                    }
                } else {
                    unrestricted_entries.push(index);
                }
                entries.push(PatternEntry::Matcher(Arc::clone(pattern)));
            }
        }
        let literal_set = if literals_only && !entries.is_empty() {
            Some(LiteralSet::new(all_literals))
        } else {
            None
        };
        let (scanner, frontier) = if !literals_only && entries.len() > 1 {
            if frontier_explicitly_disabled() {
                (
                    Scanner::compile_with_hints(patterns.iter().enumerate().map(
                        |(index, pattern)| {
                            (index, pattern.parsed(), pattern.restricted_start_bytes())
                        },
                    ))
                    .ok(),
                    None,
                )
            } else {
                let regular = patterns
                    .iter()
                    .filter(|pattern| Scanner::supports(pattern.parsed()))
                    .count();
                let opaque = entries.len() - regular;
                if opaque == 0 {
                    (
                        Scanner::compile_with_hints(patterns.iter().enumerate().map(
                            |(index, pattern)| {
                                (index, pattern.parsed(), pattern.restricted_start_bytes())
                            },
                        ))
                        .ok(),
                        None,
                    )
                } else if regular != 0
                    && frontier_enabled(frontier_policy, entries.len(), regular, opaque)
                {
                    let (partial, failures) = Scanner::compile_partial_with_hints(
                        patterns.iter().enumerate().map(|(index, pattern)| {
                            (index, pattern.parsed(), pattern.restricted_start_bytes())
                        }),
                    );
                    let frontier = if partial.entry_count() != 0 {
                        let opaque_entries = failures
                            .into_iter()
                            .map(|failure| failure.pattern)
                            .collect::<Vec<_>>();
                        Some(CandidateFrontier::new(partial, &opaque_entries, patterns))
                    } else {
                        None
                    };
                    (None, frontier)
                } else {
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        Self {
            entries,
            unrestricted_entries,
            start_byte_entries,
            patterns: translated,
            literal_set,
            scanner,
            frontier,
            #[cfg(test)]
            force_regular_replay_failure: None,
        }
    }

    #[cfg(test)]
    fn from_compiled_with_forced_frontier(patterns: &[Arc<super::CompiledPattern>]) -> Self {
        Self::from_compiled_with_frontier_policy(patterns, FrontierBuildPolicy::Force)
    }

    #[cfg(test)]
    fn force_regular_replay_failure_for(&mut self, pattern: usize) {
        self.force_regular_replay_failure = Some(pattern);
    }

    pub fn find(&self, line: &str, from: usize) -> Option<(usize, MatchResult)> {
        self.find_with_context(line, from, AnchorContext::default())
    }

    pub fn find_with_context(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Option<(usize, MatchResult)> {
        self.find_with_context_and_scratch(
            line,
            from,
            ctx,
            &mut super::bytecode::BytecodeScratch::default(),
        )
    }

    pub(crate) fn find_with_context_and_scratch(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
    ) -> Option<(usize, MatchResult)> {
        if !line.is_char_boundary(from) {
            return None;
        }
        if let Some(set) = &self.literal_set {
            let (idx, start, end) = set.find(line, from)?;
            return Some((
                idx,
                MatchResult {
                    start,
                    end,
                    captures: vec![Some(start..end)],
                },
            ));
        }
        if let Some(frontier) = &self.frontier {
            return self.find_with_frontier(line, from, ctx, scratch, frontier);
        }
        if scanner_candidate_enabled()
            && let Some(result) = self.find_with_scanner(line, from, ctx, scratch)
        {
            return result;
        }

        self.find_reference_with_buckets(
            line,
            from,
            ctx,
            scratch,
            &self.unrestricted_entries,
            &self.start_byte_entries,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn find_reference_with_buckets(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
        unrestricted_entries: &[usize],
        start_byte_entries: &[Vec<usize>],
        bound: Option<(usize, usize)>,
    ) -> Option<(usize, MatchResult)> {
        debug_assert_eq!(start_byte_entries.len(), usize::from(u8::MAX) + 1);

        let ascii_line = scratch.line_is_ascii(line);
        let mut start = from;
        loop {
            if bound.is_some_and(|(bound_start, _)| start > bound_start) {
                break;
            }
            let restricted = line.as_bytes().get(start).map_or(&[][..], |byte| {
                start_byte_entries[*byte as usize].as_slice()
            });
            let mut unrestricted_index = 0usize;
            let mut restricted_index = 0usize;
            while unrestricted_index < unrestricted_entries.len()
                || restricted_index < restricted.len()
            {
                let unrestricted = unrestricted_entries.get(unrestricted_index).copied();
                let restricted = restricted.get(restricted_index).copied();
                let idx = match (unrestricted, restricted) {
                    (Some(left), Some(right)) if left < right => {
                        unrestricted_index += 1;
                        left
                    }
                    (Some(_), Some(right)) => {
                        restricted_index += 1;
                        right
                    }
                    (Some(left), None) => {
                        unrestricted_index += 1;
                        left
                    }
                    (None, Some(right)) => {
                        restricted_index += 1;
                        right
                    }
                    (None, None) => break,
                };
                if bound.is_some_and(|(bound_start, bound_index)| {
                    start == bound_start && idx >= bound_index
                }) {
                    continue;
                }
                let result = self.match_entry_at(idx, line, start, ctx, scratch);
                if let Some(result) = result {
                    return Some((idx, result));
                }
            }
            if start == line.len() {
                break;
            }
            start += if ascii_line {
                1
            } else {
                line[start..]
                    .chars()
                    .next()
                    .expect("start is before line end")
                    .len_utf8()
            };
        }
        None
    }

    fn find_with_frontier(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
        frontier: &CandidateFrontier,
    ) -> Option<(usize, MatchResult)> {
        let regular = frontier.scanner.find(line, from, ctx, scratch.scanner());
        let opaque = if let Some(selected) = regular {
            self.find_reference_with_buckets(
                line,
                from,
                ctx,
                scratch,
                &frontier.opaque_unrestricted_entries,
                &frontier.opaque_start_byte_entries,
                Some((selected.start, selected.pattern)),
            )
        } else {
            self.find_reference_with_buckets(
                line,
                from,
                ctx,
                scratch,
                &frontier.opaque_unrestricted_entries,
                &frontier.opaque_start_byte_entries,
                None,
            )
        };
        if let Some(opaque) = opaque {
            return Some(opaque);
        }

        let selected = regular?;
        let replay = if selected.pattern < self.entries.len() {
            #[cfg(test)]
            {
                if self.force_regular_replay_failure == Some(selected.pattern) {
                    None
                } else {
                    self.match_entry_at(selected.pattern, line, selected.start, ctx, scratch)
                }
            }
            #[cfg(not(test))]
            {
                self.match_entry_at(selected.pattern, line, selected.start, ctx, scratch)
            }
        } else {
            None
        };
        match replay {
            Some(result)
                if result.start == selected.start
                    && result.end <= line.len()
                    && line.is_char_boundary(result.end) =>
            {
                Some((selected.pattern, result))
            }
            _ => {
                // The frontier is a selector only. If exact replay of the
                // selected regular candidate fails (or a future scanner bug
                // picks a non-authoritative endpoint), fall back to the known
                // exact reference traversal instead of reporting no match.
                self.find_reference_with_buckets(
                    line,
                    from,
                    ctx,
                    scratch,
                    &self.unrestricted_entries,
                    &self.start_byte_entries,
                    None,
                )
            }
        }
    }

    fn find_with_scanner(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
    ) -> Option<Option<(usize, MatchResult)>> {
        let scanner = self.scanner.as_ref()?;
        let Some(selected) = scanner.find(line, from, ctx, scratch.scanner()) else {
            return Some(None);
        };
        let result = self.match_entry_at(selected.pattern, line, selected.start, ctx, scratch)?;
        Some(Some((selected.pattern, result)))
    }

    fn match_entry_at(
        &self,
        index: usize,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        scratch: &mut super::bytecode::BytecodeScratch,
    ) -> Option<MatchResult> {
        let entry = self.entries.get(index)?;
        match entry {
            PatternEntry::Literal(literal) => match_literal_at(line, start, literal),
            PatternEntry::Matcher(pattern) => pattern
                .matcher()
                .find_at_without_captures_with_scratch(line, start, ctx, scratch)
                .ok()
                .flatten(),
        }
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl CandidateFrontier {
    fn new(
        scanner: Scanner,
        opaque_entries: &[usize],
        patterns: &[Arc<super::CompiledPattern>],
    ) -> Self {
        let mut opaque_unrestricted_entries = Vec::new();
        let mut opaque_start_byte_entries = (0..=u8::MAX)
            .map(|_| Vec::<usize>::new())
            .collect::<Vec<_>>();
        for &index in opaque_entries {
            if let Some(bytes) = patterns
                .get(index)
                .and_then(|pattern| pattern.restricted_start_bytes())
                .filter(|bytes| !bytes.is_empty())
            {
                for byte in bytes {
                    opaque_start_byte_entries[*byte as usize].push(index);
                }
            } else {
                opaque_unrestricted_entries.push(index);
            }
        }
        for bucket in &mut opaque_start_byte_entries {
            bucket.sort_unstable();
            bucket.dedup();
        }
        opaque_unrestricted_entries.sort_unstable();
        opaque_unrestricted_entries.dedup();
        Self {
            scanner,
            opaque_unrestricted_entries,
            opaque_start_byte_entries,
        }
    }
}

fn frontier_enabled(
    policy: FrontierBuildPolicy,
    total: usize,
    regular: usize,
    opaque: usize,
) -> bool {
    match policy {
        #[cfg(test)]
        FrontierBuildPolicy::Force => return true,
        FrontierBuildPolicy::Auto => {}
    }
    match std::env::var("MARK_TEXTMATE_FRONTIER").as_deref() {
        Ok("off" | "0" | "false") => return false,
        Ok("on" | "1" | "true" | "candidate") => return true,
        _ => {}
    }
    total >= frontier_min_total()
        && regular >= frontier_min_regular()
        && opaque <= frontier_max_opaque()
        && regular >= opaque.saturating_mul(frontier_min_regular_per_opaque())
}

fn frontier_explicitly_disabled() -> bool {
    matches!(
        std::env::var("MARK_TEXTMATE_FRONTIER").as_deref(),
        Ok("off" | "0" | "false")
    )
}

fn frontier_min_total() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| frontier_env_usize("MARK_TEXTMATE_FRONTIER_MIN_TOTAL", 10))
}

fn frontier_min_regular() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| frontier_env_usize("MARK_TEXTMATE_FRONTIER_MIN_REGULAR", 8))
}

fn frontier_max_opaque() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| frontier_env_usize("MARK_TEXTMATE_FRONTIER_MAX_OPAQUE", 4))
}

fn frontier_min_regular_per_opaque() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| frontier_env_usize("MARK_TEXTMATE_FRONTIER_REGULAR_PER_OPAQUE", 4))
}

fn frontier_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn scanner_candidate_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MARK_TEXTMATE_SCANNER").as_deref(),
            Ok("candidate")
        )
    })
}

fn match_literal_at(line: &str, start: usize, literal: &str) -> Option<MatchResult> {
    let end = start.checked_add(literal.len())?;
    (line.as_bytes().get(start..end)? == literal.as_bytes()).then(|| MatchResult {
        start,
        end,
        captures: vec![Some(start..end)],
    })
}

fn unescape_literal(pattern: &str) -> String {
    let mut output = String::new();
    let mut escaped = false;
    for ch in pattern.chars() {
        if escaped {
            output.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }
    if escaped {
        output.push('\\');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::regex::CompiledPattern;

    fn ctx() -> AnchorContext {
        AnchorContext {
            allow_a: false,
            allow_g: false,
            g_pos: 0,
        }
    }

    #[test]
    fn simple_literal_finds_unanchored_match() {
        let matcher = SimpleMatcher::from_pattern("foo");
        assert_eq!(matcher.find("xxfoo", 0, ctx()).unwrap().start, 2);
    }

    #[test]
    fn line_anchor_does_not_match_after_resume() {
        let matcher = SimpleMatcher::from_pattern("^foo");
        assert!(matcher.find("foo", 1, ctx()).is_none());
        assert_eq!(matcher.find("foo", 0, ctx()).unwrap().start, 0);
    }

    #[test]
    fn file_and_g_anchors_use_context() {
        let file = SimpleMatcher::from_pattern(r"\Afoo");
        assert!(file.find("foo", 0, ctx()).is_none());
        assert!(
            file.find(
                "foo",
                0,
                AnchorContext {
                    allow_a: true,
                    allow_g: false,
                    g_pos: 0,
                },
            )
            .is_some()
        );

        let g = SimpleMatcher::from_pattern(r"\Gfoo");
        assert!(
            g.find(
                "xxfoo",
                0,
                AnchorContext {
                    allow_a: false,
                    allow_g: true,
                    g_pos: 2,
                },
            )
            .is_some()
        );
    }

    #[test]
    fn automata_returns_capture_spans() {
        let matcher = AutomataMatcher::new(r"foo(\d+)").unwrap();
        let result = matcher.find("xxfoo123", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 2..8);
        assert_eq!(result.captures[1], Some(5..8));
    }

    #[test]
    fn automata_anchor_context_for_resume() {
        let matcher = AutomataMatcher::new("^foo").unwrap();
        assert!(matcher.find("foo", 1, ctx()).is_none());
        assert!(matcher.find("foo", 0, ctx()).is_some());

        let matcher = AutomataMatcher::new(r"\Gfoo").unwrap();
        assert!(
            matcher
                .find(
                    "xxfoo",
                    0,
                    AnchorContext {
                        allow_a: false,
                        allow_g: true,
                        g_pos: 2,
                    }
                )
                .is_some()
        );
    }

    #[test]
    fn pure_literal_uses_simple_engine() {
        let matcher = AutomataMatcher::new("foo").unwrap();
        assert!(matcher.is_simple());
        assert_eq!(matcher.find("xxfoo", 0, ctx()).unwrap().start, 2);
    }

    #[test]
    fn pattern_set_leftmost_lowest_index() {
        let patterns = vec!["bb".into(), "b".into(), "a".into()];
        let set = PatternSetMatcher::new(&patterns).unwrap();
        let (idx, result) = set.find("abb", 0).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(result.start..result.end, 0..1);

        let (idx, result) = set.find("abb", 1).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 1..3);
    }

    #[test]
    fn pattern_set_handles_regex_entries() {
        let patterns = vec![r"\d+".into(), "foo".into()];
        let set = PatternSetMatcher::new(&patterns).unwrap();
        let (idx, result) = set.find("xfoo9", 0).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(result.start..result.end, 1..4);
        let (idx, result) = set.find("x9foo", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 1..2);
    }

    #[test]
    fn pattern_set_replays_continuation_anchor_only_at_g_position() {
        let patterns = vec![r"\Gfoo".into(), "nomatch".into()];
        let set = PatternSetMatcher::new(&patterns).unwrap();
        let (idx, result) = set
            .find_with_context("foo xxfoo", 0, AnchorContext::continuation(6))
            .unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 6..9);
    }

    fn forced_frontier(patterns: &[&str]) -> PatternSetMatcher {
        let compiled = patterns
            .iter()
            .map(|pattern| Arc::new(CompiledPattern::new(pattern)))
            .collect::<Vec<_>>();
        PatternSetMatcher::from_compiled_with_forced_frontier(&compiled)
    }

    #[test]
    fn partial_frontier_opaque_can_beat_regular_bound() {
        let set = forced_frontier(&["a", "(?=x)x"]);
        let (idx, result) = set.find("x a", 0).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(result.start..result.end, 0..1);
    }

    #[test]
    fn partial_frontier_preserves_equal_start_grammar_order() {
        let set = forced_frontier(&["(?=a)a", "a"]);
        let (idx, result) = set.find("a", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 0..1);

        let set = forced_frontier(&["a", "(?=a)a"]);
        let (idx, result) = set.find("a", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 0..1);
    }

    #[test]
    fn partial_frontier_ignores_opaque_after_regular_bound() {
        let set = forced_frontier(&["a", "(?=z)z"]);
        let (idx, result) = set.find("a z", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 0..1);
    }

    #[test]
    fn partial_frontier_falls_back_when_regular_replay_fails() {
        let mut set = forced_frontier(&["a", "(?=z)z"]);
        set.force_regular_replay_failure_for(0);
        let (idx, result) = set.find("a z", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 0..1);
    }

    #[test]
    fn partial_frontier_honors_continuation_anchor() {
        let set = forced_frontier(&[r"\Gfoo", "(?=q)q"]);
        let (idx, result) = set
            .find_with_context("xxfoo z", 0, AnchorContext::continuation(2))
            .unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 2..5);
        assert!(
            set.find_with_context("xxfoo z", 0, AnchorContext::continuation(3))
                .is_none()
        );
    }

    #[test]
    fn partial_frontier_selection_still_defers_captures_to_replay() {
        let set = forced_frontier(&["(a)", "(?=z)z"]);
        let (idx, result) = set.find("a", 0).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(result.start..result.end, 0..1);
        assert!(
            result.captures.is_empty(),
            "candidate selection should not eagerly materialize captures"
        );
    }

    #[test]
    fn partial_frontier_matches_reference_search_on_mixed_sets() {
        let cases = [
            (
                vec!["a+", "(?=a)a", "b"],
                vec![
                    ("", 0, AnchorContext::default()),
                    ("baaa", 0, AnchorContext::default()),
                    ("xaa", 0, AnchorContext::default()),
                    ("xaa", 1, AnchorContext::default()),
                ],
            ),
            (
                vec![r"\Gfoo", "(?<=x)y", "foo"],
                vec![
                    ("xxfoo y", 0, AnchorContext::continuation(2)),
                    ("xy foo", 0, AnchorContext::continuation(0)),
                    ("xy foo", 2, AnchorContext::continuation(2)),
                ],
            ),
            (
                vec!["[A-Z]+", "(?=z)z", "end$"],
                vec![
                    ("aaZ z", 0, AnchorContext::default()),
                    ("end", 0, AnchorContext::default()),
                    ("lower", 0, AnchorContext::default()),
                ],
            ),
        ];

        for (patterns, probes) in cases {
            let set = forced_frontier(&patterns);
            for (line, from, ctx) in probes {
                let actual = set.find_with_context(line, from, ctx).map(summarize_match);
                let mut scratch = super::super::bytecode::BytecodeScratch::default();
                let expected = set
                    .find_reference_with_buckets(
                        line,
                        from,
                        ctx,
                        &mut scratch,
                        &set.unrestricted_entries,
                        &set.start_byte_entries,
                        None,
                    )
                    .map(summarize_match);
                assert_eq!(actual, expected, "patterns={patterns:?} line={line:?}");
            }
        }
    }

    fn summarize_match((idx, result): (usize, MatchResult)) -> (usize, usize, usize) {
        (idx, result.start, result.end)
    }
}
