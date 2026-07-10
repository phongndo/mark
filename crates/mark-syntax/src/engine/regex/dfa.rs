use std::fmt;

use super::ast::{AnchorKind, Ast};
use super::backtrack::FallbackMatcher;
use super::prefilter::{LiteralSet, Prefilter};
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
        Self::from_translation(pattern, translation)
    }

    pub fn from_translation(
        pattern: &str,
        translation: Translation,
    ) -> Result<Self, AutomataBuildError> {
        let prefilter = Prefilter::from_regex(&translation.parsed);
        let engine = if let Some(simple) = SimpleMatcher::try_from_translation(&translation) {
            NativeEngine::Simple(simple)
        } else {
            // Match against the original Oniguruma pattern so anchors/classes
            // keep AST semantics (the translated spelling is diagnostic-only).
            NativeEngine::Vm(FallbackMatcher::new(pattern))
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
                if !anchor_permits(self.translation.anchor_strategy, from, ctx, line) {
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
        if !anchor_permits(self.translation.anchor_strategy, start, ctx, line) {
            return Ok((None, None));
        }
        match &self.engine {
            NativeEngine::Simple(matcher) => Ok((matcher.match_at(line, start), None)),
            NativeEngine::Vm(matcher) => matcher
                .try_find_at(line, start, ctx)
                .map(|report| (report.result, Some(report.steps))),
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
                if !anchor_permits(self.translation.anchor_strategy, from, ctx, line) {
                    return None;
                }
                // FallbackMatcher applies its own prefilter; skip a redundant
                // outer check for the Vm path to avoid double work.
                matcher.find(line, from, ctx)
            }
        }
    }
}

fn anchor_permits(strategy: AnchorStrategy, from: usize, ctx: AnchorContext, line: &str) -> bool {
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

#[derive(Debug, Clone)]
enum PatternEntry {
    Literal(String),
    Matcher(FallbackMatcher),
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
}

impl PatternSetMatcher {
    pub fn new(patterns: &[String]) -> Result<Self, AutomataBuildError> {
        let mut entries = Vec::with_capacity(patterns.len());
        let mut translated = Vec::with_capacity(patterns.len());
        let mut all_literals = Vec::with_capacity(patterns.len());
        let mut literals_only = true;
        let mut unrestricted_entries = Vec::new();
        let mut start_byte_entries = (0..=u8::MAX)
            .map(|_| Vec::<usize>::new())
            .collect::<Vec<_>>();

        for (index, pattern) in patterns.iter().enumerate() {
            let translation = translate(pattern);
            translated.push(translation.pattern.clone());
            if let Some(SimpleMatcher::Literal(literal)) =
                SimpleMatcher::try_from_translation(&translation)
            {
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
                let matcher = FallbackMatcher::new(pattern);
                if let Some(bytes) = matcher.restricted_start_bytes() {
                    for byte in bytes {
                        start_byte_entries[byte as usize].push(index);
                    }
                } else {
                    unrestricted_entries.push(index);
                }
                entries.push(PatternEntry::Matcher(matcher));
            }
        }

        let literal_set = if literals_only && !entries.is_empty() {
            Some(LiteralSet::new(all_literals))
        } else {
            None
        };

        Ok(Self {
            entries,
            unrestricted_entries,
            start_byte_entries,
            patterns: translated,
            literal_set,
        })
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

        for start in line[from..]
            .char_indices()
            .map(|(offset, _)| from + offset)
            .chain(std::iter::once(line.len()))
        {
            let restricted = line.as_bytes().get(start).map_or(&[][..], |byte| {
                self.start_byte_entries[*byte as usize].as_slice()
            });
            let mut unrestricted_index = 0usize;
            let mut restricted_index = 0usize;
            while unrestricted_index < self.unrestricted_entries.len()
                || restricted_index < restricted.len()
            {
                let unrestricted = self.unrestricted_entries.get(unrestricted_index).copied();
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
                let entry = &self.entries[idx];
                let result = match entry {
                    PatternEntry::Literal(literal) => match_literal_at(line, start, literal),
                    PatternEntry::Matcher(matcher) => matcher
                        .try_find_at_without_captures(line, start, ctx)
                        .ok()
                        .flatten(),
                };
                if let Some(result) = result {
                    return Some((idx, result));
                }
            }
        }
        None
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

fn match_literal_at(line: &str, start: usize, literal: &str) -> Option<MatchResult> {
    let end = start.checked_add(literal.len())?;
    (line.get(start..end)? == literal).then(|| MatchResult {
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
}
