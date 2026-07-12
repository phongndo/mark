use std::{collections::HashSet, fmt, sync::Arc};

use super::ast::{AnchorKind, Ast, ClassAtom, PerlClassKind, RegexFlags};
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
    WordSet(WordSetMatcher),
    NixUri(NixUriMatcher),
    /// General native VM for DFA-routable (and best-effort) patterns.
    Vm(FallbackMatcher),
}

/// Native matcher for the Nix unquoted URI production
/// `([A-Za-z][-+.0-9A-Za-z]*:[!$-'*-:=?-Z_a-z~]+)`. The generic VM is
/// correct but hot on Nix files because every identifier-like token probes for
/// a following `:`. This keeps the exact ASCII byte language and capture shape
/// while reducing each negative probe to a tight byte scan.
#[derive(Debug, Clone, Copy)]
struct NixUriMatcher;

impl NixUriMatcher {
    const PATTERN: &'static str = "([A-Za-z][-+.0-9A-Za-z]*:[!$-'*-:=?-Z_a-z~]+)";

    fn try_from_translation(translation: &Translation) -> Option<Self> {
        (translation.pattern == Self::PATTERN).then_some(Self)
    }

    fn find(&self, line: &str, from: usize) -> Option<MatchResult> {
        let bytes = line.as_bytes();
        let mut pos = from;
        while pos < bytes.len() {
            let found = memchr::memchr2(b':', b'<', &bytes[pos..])
                .map(|offset| pos + offset)
                .unwrap_or(bytes.len());
            // A colon is required. If the next potentially interesting byte is
            // not a colon, continue after it (notably over Nix `<spath>`s).
            if found >= bytes.len() {
                return None;
            }
            if bytes[found] != b':' {
                pos = found + 1;
                continue;
            }
            let mut run_start = found;
            while run_start > from && is_nix_uri_scheme_byte(bytes[run_start - 1]) {
                run_start -= 1;
            }
            for (start, byte) in bytes.iter().enumerate().take(found).skip(run_start) {
                if is_ascii_alpha(*byte)
                    && line.is_char_boundary(start)
                    && let Some(result) = self.match_at(line, start)
                {
                    return Some(result);
                }
            }
            pos = found + 1;
        }
        None
    }

    fn match_at(&self, line: &str, start: usize) -> Option<MatchResult> {
        let bytes = line.as_bytes();
        let first = *bytes.get(start)?;
        if !is_ascii_alpha(first) || !line.is_char_boundary(start) {
            return None;
        }
        let mut pos = start + 1;
        while pos < bytes.len() && is_nix_uri_scheme_byte(bytes[pos]) {
            pos += 1;
        }
        if bytes.get(pos).copied() != Some(b':') {
            return None;
        }
        pos += 1;
        let body_start = pos;
        while pos < bytes.len() && is_nix_uri_body_byte(bytes[pos]) {
            pos += 1;
        }
        if pos == body_start || !line.is_char_boundary(pos) {
            return None;
        }
        Some(MatchResult {
            start,
            end: pos,
            captures: vec![Some(start..pos), Some(start..pos)],
        })
    }
}

fn is_ascii_alpha(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

fn is_nix_uri_scheme_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.')
}

fn is_nix_uri_body_byte(byte: u8) -> bool {
    matches!(byte, b'!' | b'$'..=b'\'' | b'*'..=b':' | b'=' | b'?'..=b'Z' | b'_' | b'a'..=b'z' | b'~')
}

/// Specialized matcher for the common keyword-list spelling
/// `\b(?i)(foo|bar|...)\b` / `(?i:\b(?:foo|bar|...)\b)`, plus the SQL
/// function-list suffix `\s*\(`.
#[derive(Debug, Clone)]
struct WordSetMatcher {
    buckets: Vec<WordBucket>,
    case_insensitive: bool,
    suffix: WordSetSuffix,
    start_bytes: Vec<u8>,
    capture_group: Option<u32>,
    capture_count: usize,
}

#[derive(Debug, Clone)]
struct WordBucket {
    len: usize,
    words: HashSet<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct WordSetSpec {
    words: Vec<String>,
    case_insensitive: bool,
    suffix: WordSetSuffix,
    capture_group: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordSetSuffix {
    None,
    OpenParenAfterOptionalSpace,
}

impl WordSetMatcher {
    fn try_from_translation(translation: &Translation) -> Option<Self> {
        if !matches!(translation.anchor_strategy, AnchorStrategy::None) {
            return None;
        }
        let spec = word_set_spec(&translation.parsed.ast, translation.parsed.flags)?;
        if !spec.case_insensitive || spec.words.len() < word_set_min_words() {
            return None;
        }
        Some(Self::new(
            spec.words,
            spec.case_insensitive,
            spec.suffix,
            spec.capture_group,
            translation.parsed.capture_count as usize + 1,
        ))
    }

    fn new(
        words: Vec<String>,
        case_insensitive: bool,
        suffix: WordSetSuffix,
        capture_group: Option<u32>,
        capture_count: usize,
    ) -> Self {
        let mut buckets = Vec::<WordBucket>::new();
        let mut start_bitmap = [0u64; 4];
        for word in words {
            debug_assert!(!word.is_empty());
            debug_assert!(word.bytes().all(is_ascii_word_byte));
            let mut bytes = word.into_bytes();
            if case_insensitive {
                bytes.make_ascii_lowercase();
            }
            if let Some(&first) = bytes.first() {
                if case_insensitive && first.is_ascii_alphabetic() {
                    let lower = first.to_ascii_lowercase();
                    let upper = first.to_ascii_uppercase();
                    start_bitmap[lower as usize >> 6] |= 1u64 << (lower & 63);
                    start_bitmap[upper as usize >> 6] |= 1u64 << (upper & 63);
                } else {
                    start_bitmap[first as usize >> 6] |= 1u64 << (first & 63);
                }
            }
            let len = bytes.len();
            match buckets.binary_search_by_key(&len, |bucket| bucket.len) {
                Ok(index) => {
                    buckets[index].words.insert(bytes);
                }
                Err(index) => {
                    let mut set = HashSet::new();
                    set.insert(bytes);
                    buckets.insert(index, WordBucket { len, words: set });
                }
            }
        }
        let start_bytes = (0u8..=u8::MAX)
            .filter(|byte| start_bitmap[*byte as usize >> 6] & (1u64 << (*byte & 63)) != 0)
            .collect();
        Self {
            buckets,
            case_insensitive,
            suffix,
            start_bytes,
            capture_group,
            capture_count,
        }
    }

    fn find(&self, line: &str, from: usize) -> Option<MatchResult> {
        if !line.is_char_boundary(from) {
            return None;
        }
        let bytes = line.as_bytes();
        let mut position = from;
        while position < bytes.len() {
            let offset = bytes[position..]
                .iter()
                .position(|byte| is_ascii_word_byte(*byte))?;
            position += offset;
            if previous_char(line, position).is_some_and(is_word_char) {
                position = ascii_word_end(bytes, position);
                continue;
            }
            let end = ascii_word_end(bytes, position);
            if char_at(line, end).is_some_and(|(ch, _)| is_word_char(ch)) {
                position = end;
                continue;
            }
            if self.contains(&bytes[position..end])
                && let Some(match_end) = self.suffix_end(line, end)
            {
                return Some(self.match_result(position, end, match_end));
            }
            position = end;
        }
        None
    }

    fn match_at(&self, line: &str, start: usize) -> Option<MatchResult> {
        if start > line.len() || !line.is_char_boundary(start) {
            return None;
        }
        let bytes = line.as_bytes();
        let first = *bytes.get(start)?;
        if !is_ascii_word_byte(first) || previous_char(line, start).is_some_and(is_word_char) {
            return None;
        }
        let end = ascii_word_end(bytes, start);
        if char_at(line, end).is_some_and(|(ch, _)| is_word_char(ch)) {
            return None;
        }
        if !self.contains(&bytes[start..end]) {
            return None;
        }
        let match_end = self.suffix_end(line, end)?;
        Some(self.match_result(start, end, match_end))
    }

    fn contains(&self, token: &[u8]) -> bool {
        let Ok(index) = self
            .buckets
            .binary_search_by_key(&token.len(), |bucket| bucket.len)
        else {
            return false;
        };
        if self.case_insensitive {
            let mut folded = token.to_vec();
            folded.make_ascii_lowercase();
            self.buckets[index].words.contains(&folded)
        } else {
            self.buckets[index].words.contains(token)
        }
    }

    fn suffix_end(&self, line: &str, word_end: usize) -> Option<usize> {
        match self.suffix {
            WordSetSuffix::None => Some(word_end),
            WordSetSuffix::OpenParenAfterOptionalSpace => {
                let mut position = word_end;
                while let Some((ch, next)) = char_at(line, position) {
                    if !ch.is_whitespace() {
                        break;
                    }
                    position = next;
                }
                (line.as_bytes().get(position) == Some(&b'(')).then_some(position + 1)
            }
        }
    }

    fn match_result(&self, start: usize, word_end: usize, match_end: usize) -> MatchResult {
        let mut captures = vec![None; self.capture_count];
        captures[0] = Some(start..match_end);
        if let Some(group) = self.capture_group
            && let Some(slot) = captures.get_mut(group as usize)
        {
            *slot = Some(start..word_end);
        }
        MatchResult {
            start,
            end: match_end,
            captures,
        }
    }
}

fn word_set_min_words() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MARK_TEXTMATE_WORD_SET_MIN_WORDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4)
    })
}

fn word_set_spec(ast: &Ast, flags: RegexFlags) -> Option<WordSetSpec> {
    match ast {
        Ast::Flags {
            flags: scoped_flags,
            child,
        } => word_set_spec(child, *scoped_flags),
        Ast::Concat(nodes) => word_set_spec_from_concat(nodes, flags),
        _ => None,
    }
}

fn word_set_spec_from_concat(nodes: &[Ast], flags: RegexFlags) -> Option<WordSetSpec> {
    let significant = nodes
        .iter()
        .filter(|node| !matches!(node, Ast::Empty))
        .collect::<Vec<_>>();
    if significant.len() < 3 {
        return None;
    }
    if !matches!(significant[0], Ast::Anchor(AnchorKind::WordBoundary)) {
        return None;
    }
    if !matches!(significant[2], Ast::Anchor(AnchorKind::WordBoundary)) {
        return None;
    }
    let mut effective_flags = flags;
    let mut body = significant[1];
    if let Ast::Flags {
        flags: scoped_flags,
        child,
    } = body
    {
        effective_flags = *scoped_flags;
        body = child.as_ref();
    }
    let suffix = word_set_suffix(&significant[3..])?;
    let (alternation, capture_group) = match body {
        Ast::Group {
            index,
            child,
            name: None,
        } => (child.as_ref(), *index),
        Ast::Alternation(_) => (body, None),
        _ => return None,
    };
    let Ast::Alternation(branches) = alternation else {
        return None;
    };
    let mut words = Vec::with_capacity(branches.len());
    let mut unsupported_branches = 0usize;
    for branch in branches {
        let Some(variants) = word_variants(branch) else {
            unsupported_branches += 1;
            continue;
        };
        let mut accepted = false;
        for word in variants {
            if word.is_empty() || !word.bytes().all(is_ascii_word_byte) {
                continue;
            }
            accepted = true;
            words.push(word);
        }
        if !accepted {
            unsupported_branches += 1;
        }
    }
    if words.len() != branches.len() && (words.len() < 32 || unsupported_branches > 16) {
        return None;
    }
    Some(WordSetSpec {
        words,
        case_insensitive: effective_flags.case_insensitive,
        suffix,
        capture_group,
    })
}

fn word_set_suffix(nodes: &[&Ast]) -> Option<WordSetSuffix> {
    match nodes {
        [] => Some(WordSetSuffix::None),
        [
            Ast::Repeat {
                node,
                min: 0,
                max: None,
                possessive: false,
                ..
            },
            Ast::Literal(literal),
        ] if literal == "(" && is_perl_space_class(node) => {
            Some(WordSetSuffix::OpenParenAfterOptionalSpace)
        }
        _ => None,
    }
}

fn is_perl_space_class(ast: &Ast) -> bool {
    let Ast::Class(class) = ast else {
        return false;
    };
    !class.negated
        && matches!(
            class.atoms.as_slice(),
            [ClassAtom::Perl(PerlClassKind::Space)]
        )
}

fn word_variants(ast: &Ast) -> Option<Vec<String>> {
    match ast {
        Ast::Empty => Some(vec![String::new()]),
        Ast::Literal(literal) => Some(vec![literal.clone()]),
        Ast::Group {
            child, name: None, ..
        } => word_variants(child),
        Ast::Concat(nodes) => {
            let mut variants = vec![String::new()];
            for node in nodes {
                let part = word_variants(node)?;
                if variants.len().saturating_mul(part.len()) > 8 {
                    return None;
                }
                let mut next = Vec::with_capacity(variants.len() * part.len());
                for prefix in &variants {
                    for suffix in &part {
                        let mut combined = prefix.clone();
                        combined.push_str(suffix);
                        next.push(combined);
                    }
                }
                variants = next;
            }
            Some(variants)
        }
        Ast::Repeat {
            node,
            min: 0,
            max: Some(1),
            possessive: false,
            atomic: false,
            ..
        } => {
            let mut variants = vec![String::new()];
            variants.extend(word_variants(node)?);
            Some(variants)
        }
        _ => None,
    }
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn ascii_word_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while bytes.get(end).is_some_and(|byte| is_ascii_word_byte(*byte)) {
        end += 1;
    }
    end
}

fn char_at(line: &str, pos: usize) -> Option<(char, usize)> {
    let ch = line.get(pos..)?.chars().next()?;
    Some((ch, pos + ch.len_utf8()))
}

fn previous_char(line: &str, pos: usize) -> Option<char> {
    line.get(..pos)?.chars().next_back()
}

fn is_word_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
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
        } else if let Some(word_set) = WordSetMatcher::try_from_translation(&translation) {
            NativeEngine::WordSet(word_set)
        } else if let Some(uri) = NixUriMatcher::try_from_translation(&translation) {
            NativeEngine::NixUri(uri)
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

    pub fn is_word_set(&self) -> bool {
        matches!(self.engine, NativeEngine::WordSet(_))
    }

    pub(crate) fn unanchored_literal(&self) -> Option<&str> {
        match &self.engine {
            NativeEngine::Simple(matcher) => matcher.unanchored_literal(),
            NativeEngine::WordSet(_) => None,
            NativeEngine::NixUri(_) => None,
            NativeEngine::Vm(_) => None,
        }
    }

    pub(crate) fn restricted_start_bytes(&self) -> Option<Vec<u8>> {
        match &self.engine {
            NativeEngine::Simple(matcher) => matcher
                .unanchored_literal()
                .and_then(|literal| literal.as_bytes().first().copied())
                .map(|byte| vec![byte]),
            NativeEngine::WordSet(matcher) => Some(matcher.start_bytes.clone()),
            NativeEngine::NixUri(_) => Some((b'A'..=b'Z').chain(b'a'..=b'z').collect()),
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
            NativeEngine::WordSet(matcher) => Ok((matcher.find(line, from), None)),
            NativeEngine::NixUri(matcher) => Ok((matcher.find(line, from), None)),
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
            NativeEngine::WordSet(matcher) => Ok((matcher.match_at(line, start), None)),
            NativeEngine::NixUri(matcher) => Ok((matcher.match_at(line, start), None)),
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
            NativeEngine::WordSet(matcher) => Ok(matcher.match_at(line, start)),
            NativeEngine::NixUri(matcher) => Ok(matcher.match_at(line, start)),
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
            NativeEngine::WordSet(matcher) => matcher.find(line, from),
            NativeEngine::NixUri(matcher) => matcher.find(line, from),
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
    opaque_start_prefilter: Option<StartBytePrefilter>,
}

#[derive(Debug, Clone)]
struct StartBytePrefilter {
    bytes: Vec<u8>,
    bitmap: [u64; 4],
}

/// Native multi-pattern matcher. Leftmost match wins; on equal start offsets
/// the lowest pattern index wins (regex-automata compatible).
#[derive(Debug, Clone)]
pub struct PatternSetMatcher {
    entries: Vec<PatternEntry>,
    unrestricted_entries: Vec<usize>,
    start_byte_entries: Vec<Vec<usize>>,
    start_prefilter: Option<StartBytePrefilter>,
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
        let start_prefilter =
            StartBytePrefilter::from_buckets(&unrestricted_entries, &start_byte_entries);
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
            start_prefilter,
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
            self.start_prefilter.as_ref(),
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
        start_prefilter: Option<&StartBytePrefilter>,
        bound: Option<(usize, usize)>,
    ) -> Option<(usize, MatchResult)> {
        debug_assert_eq!(start_byte_entries.len(), usize::from(u8::MAX) + 1);

        let ascii_line = scratch.line_is_ascii(line);
        let mut start = match start_prefilter {
            Some(prefilter) => prefilter.next_start(line, from)?,
            None => from,
        };
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
            let next = start
                + if ascii_line {
                    1
                } else {
                    line[start..]
                        .chars()
                        .next()
                        .expect("start is before line end")
                        .len_utf8()
                };
            start = match start_prefilter {
                Some(prefilter) => prefilter.next_start(line, next)?,
                None => next,
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
                frontier.opaque_start_prefilter.as_ref(),
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
                frontier.opaque_start_prefilter.as_ref(),
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
                    self.start_prefilter.as_ref(),
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
        let opaque_start_prefilter = StartBytePrefilter::from_buckets(
            &opaque_unrestricted_entries,
            &opaque_start_byte_entries,
        );
        Self {
            scanner,
            opaque_unrestricted_entries,
            opaque_start_byte_entries,
            opaque_start_prefilter,
        }
    }
}

impl StartBytePrefilter {
    fn from_buckets(
        unrestricted_entries: &[usize],
        start_byte_entries: &[Vec<usize>],
    ) -> Option<Self> {
        if !unrestricted_entries.is_empty() {
            return None;
        }
        let mut bytes = Vec::new();
        let mut bitmap = [0u64; 4];
        for (byte, bucket) in start_byte_entries.iter().enumerate() {
            if !bucket.is_empty() {
                let byte = byte as u8;
                bytes.push(byte);
                bitmap[byte as usize >> 6] |= 1u64 << (byte & 63);
            }
        }
        (!bytes.is_empty()).then_some(Self { bytes, bitmap })
    }

    fn next_start(&self, line: &str, from: usize) -> Option<usize> {
        if from > line.len() {
            return None;
        }
        let mut base = from;
        loop {
            let slice = line.as_bytes().get(base..)?;
            let offset = find_start_byte(slice, &self.bytes, &self.bitmap)?;
            let position = base + offset;
            if line.is_char_boundary(position) {
                return Some(position);
            }
            base = position.saturating_add(1);
        }
    }
}

fn find_start_byte(haystack: &[u8], bytes: &[u8], bitmap: &[u64; 4]) -> Option<usize> {
    match bytes {
        [] => None,
        [byte] => memchr::memchr(*byte, haystack),
        [a, b] => memchr::memchr2(*a, *b, haystack),
        [a, b, c] => memchr::memchr3(*a, *b, *c, haystack),
        _ => haystack
            .iter()
            .position(|byte| bitmap[*byte as usize >> 6] & (1u64 << (*byte & 63)) != 0),
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
        !matches!(
            std::env::var("MARK_TEXTMATE_SCANNER").as_deref(),
            Ok("off" | "0" | "false")
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
    fn case_insensitive_word_set_matches_keyword_tokens() {
        let matcher =
            AutomataMatcher::new(r"\b(?i)(select|from|where|abort_after_wait)\b").unwrap();
        assert!(matcher.is_word_set());
        let result = matcher
            .find(
                "xx SELECT abort_after_waiting abort_after_wait",
                0,
                AnchorContext::default(),
            )
            .unwrap();
        assert_eq!(result.start..result.end, 3..9);
        assert_eq!(result.captures[1], Some(3..9));
        let result = matcher
            .find(
                "xx abort_after_waiting abort_after_wait",
                0,
                AnchorContext::default(),
            )
            .unwrap();
        assert_eq!(result.start..result.end, 23..39);
        assert!(
            matcher
                .find("éSELECT", 0, AnchorContext::default())
                .is_none()
        );
        assert!(
            matcher
                .find("SELECTé", 0, AnchorContext::default())
                .is_none()
        );
    }

    #[test]
    fn scoped_word_set_supports_noncapturing_inventory() {
        let matcher = AutomataMatcher::new(r"(?i:\b(?:select|from|where|join)\b)").unwrap();
        assert!(matcher.is_word_set());
        let result = matcher
            .find("xx JOIN", 0, AnchorContext::default())
            .unwrap();
        assert_eq!(result.start..result.end, 3..7);
        assert_eq!(result.captures, vec![Some(3..7)]);
    }

    #[test]
    fn word_set_allows_sql_function_call_suffix() {
        let matcher = AutomataMatcher::new(r"(?i)\b(abs|acos|asin|atan)\b\s*\(").unwrap();
        assert!(matcher.is_word_set());
        let result = matcher
            .find("xx ACOS (value)", 0, AnchorContext::default())
            .unwrap();
        assert_eq!(result.start..result.end, 3..9);
        assert_eq!(result.captures[1], Some(3..7));
        assert!(
            matcher
                .find("xx ACOS value", 0, AnchorContext::default())
                .is_none()
        );
    }

    #[test]
    fn start_byte_prefilter_jumps_to_candidate_bytes() {
        let mut buckets = (0..=u8::MAX)
            .map(|_| Vec::<usize>::new())
            .collect::<Vec<_>>();
        buckets[b'x' as usize].push(0);
        buckets["é".as_bytes()[0] as usize].push(1);
        let prefilter = StartBytePrefilter::from_buckets(&[], &buckets).unwrap();
        assert_eq!(prefilter.next_start("abcx", 0), Some(3));
        assert_eq!(prefilter.next_start("abc", 0), None);
        assert_eq!(prefilter.next_start("aéx", 0), Some(1));
        assert!(StartBytePrefilter::from_buckets(&[0], &buckets).is_none());
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
                        set.start_prefilter.as_ref(),
                        None,
                    )
                    .map(summarize_match);
                assert_eq!(actual, expected, "patterns={patterns:?} line={line:?}");
            }
        }
    }

    #[test]
    fn nix_uri_specialization_matches_ascii_uri_shape() {
        let matcher = AutomataMatcher::new(NixUriMatcher::PATTERN).unwrap();
        let result = matcher
            .find(
                "xx 1abc:def <nixpkgs> mailto:user",
                0,
                AnchorContext::default(),
            )
            .unwrap();
        assert_eq!(result.start..result.end, 4..11);
        assert_eq!(result.captures, vec![Some(4..11), Some(4..11)]);
        assert!(
            matcher
                .find("<nixpkgs>", 0, AnchorContext::default())
                .is_none()
        );
        assert_eq!(
            matcher
                .find("prefix mailto:user", 7, AnchorContext::default())
                .unwrap()
                .start,
            7
        );
    }

    fn summarize_match((idx, result): (usize, MatchResult)) -> (usize, usize, usize) {
        (idx, result.start, result.end)
    }
}
