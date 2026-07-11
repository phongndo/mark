pub mod ast;
pub mod backtrack;
pub(crate) mod bytecode;
pub mod captures;
pub mod dfa;
pub mod prefilter;
pub(crate) mod scanner;
pub mod translate;

use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_COMPILED_PATTERN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompiledPatternId(u64);

pub use ast::{ParsedRegex, RegexFeatures, parse};
pub use backtrack::{FallbackError, FallbackMatcher, FallbackReport};
pub use dfa::{
    AutomataBuildError, AutomataMatcher, LiteralMatcher, PatternSetMatcher, SimpleMatcher,
};
pub use translate::{AnchorStrategy, Route, Translation, translate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorContext {
    pub allow_a: bool,
    pub allow_g: bool,
    pub g_pos: usize,
}

impl AnchorContext {
    pub fn start_of_file() -> Self {
        Self {
            allow_a: true,
            allow_g: false,
            g_pos: 0,
        }
    }

    pub fn line_start() -> Self {
        Self {
            allow_a: false,
            allow_g: false,
            g_pos: 0,
        }
    }

    pub fn continuation(g_pos: usize) -> Self {
        Self {
            allow_a: false,
            allow_g: true,
            g_pos,
        }
    }
}

impl Default for AnchorContext {
    fn default() -> Self {
        Self::line_start()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub start: usize,
    pub end: usize,
    pub captures: Vec<Option<Range<usize>>>,
}

pub trait Matcher {
    fn find(&self, line: &str, from: usize, ctx: AnchorContext) -> Option<MatchResult>;
}

#[derive(Debug, Clone)]
pub enum RegexMatcher {
    Automata(Box<AutomataMatcher>),
    Fallback(Box<FallbackMatcher>),
}

impl RegexMatcher {
    pub fn new(pattern: &str) -> Self {
        let translation = translate(pattern);
        Self::from_translation(pattern, translation)
    }

    fn from_translation(pattern: &str, translation: Translation) -> Self {
        match translation.route {
            Route::Dfa => match AutomataMatcher::from_translation(translation) {
                Ok(matcher) => Self::Automata(Box::new(matcher)),
                Err(_) => Self::Fallback(Box::new(FallbackMatcher::new(pattern))),
            },
            Route::Fallback { .. } => Self::Fallback(Box::new(FallbackMatcher::from_parsed(
                Arc::clone(&translation.parsed),
                backtrack::DEFAULT_STEP_BUDGET,
            ))),
        }
    }

    fn unanchored_literal(&self) -> Option<&str> {
        match self {
            Self::Automata(matcher) => matcher.unanchored_literal(),
            Self::Fallback(_) => None,
        }
    }

    fn restricted_start_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Automata(matcher) => matcher.restricted_start_bytes(),
            Self::Fallback(matcher) => matcher.restricted_start_bytes(),
        }
    }

    pub fn engine_name(&self) -> &'static str {
        match self {
            Self::Automata(_) => "dfa",
            Self::Fallback(_) => "fallback",
        }
    }

    pub fn prefilter_may_match(&self, line: &str, from: usize) -> Option<bool> {
        match self {
            Self::Automata(matcher) => matcher.prefilter_may_match(line, from),
            Self::Fallback(matcher) => matcher.prefilter_may_match(line, from),
        }
    }

    pub fn find_report(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Result<(Option<MatchResult>, Option<usize>), FallbackError> {
        match self {
            Self::Automata(matcher) => Ok((matcher.find(line, from, ctx), None)),
            Self::Fallback(matcher) => matcher
                .try_find(line, from, ctx)
                .map(|report| (report.result, Some(report.steps))),
        }
    }

    pub(crate) fn find_report_for_selection(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Result<(Option<MatchResult>, Option<usize>), FallbackError> {
        match self {
            Self::Automata(matcher) => matcher.find_report_for_selection(line, from, ctx),
            Self::Fallback(matcher) => matcher
                .try_find_for_selection(line, from, ctx)
                .map(|report| (report.result, Some(report.steps))),
        }
    }

    pub(crate) fn find_report_at(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
    ) -> Result<(Option<MatchResult>, Option<usize>), FallbackError> {
        match self {
            Self::Automata(matcher) => matcher.find_report_at(line, start, ctx),
            Self::Fallback(matcher) => matcher
                .try_find_at(line, start, ctx)
                .map(|report| (report.result, Some(report.steps))),
        }
    }

    pub(crate) fn find_at_without_captures_with_scratch(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        scratch: &mut bytecode::BytecodeScratch,
    ) -> Result<Option<MatchResult>, FallbackError> {
        match self {
            Self::Automata(matcher) => {
                matcher.find_at_without_captures_with_scratch(line, start, ctx, scratch)
            }
            Self::Fallback(matcher) => matcher
                .try_find_at_without_captures_with_scratch(line, start, ctx, scratch)
                .map(|report| report.result),
        }
    }
}

/// Immutable regex data shared by individual candidate matching and the
/// ordered multi-pattern selector. Construction parses the source exactly
/// once; candidate-set construction only clones an `Arc` to this object.
#[derive(Debug)]
pub struct CompiledPattern {
    id: CompiledPatternId,
    source: Arc<str>,
    translated_pattern: String,
    matcher: RegexMatcher,
    unanchored_literal: Option<String>,
    restricted_start_bytes: Option<Vec<u8>>,
    parsed: Arc<ParsedRegex>,
    live_captures: Arc<[u32]>,
    capture_program: std::sync::OnceLock<Option<Arc<bytecode::Program>>>,
}

impl CompiledPattern {
    pub fn new(pattern: &str) -> Self {
        let translation = translate(pattern);
        let live_captures = (0..=translation.parsed.capture_count).collect::<Vec<_>>();
        Self::from_translation_with_live_captures(pattern, translation, live_captures)
    }

    pub(crate) fn new_with_live_captures(pattern: &str, live_captures: Vec<u32>) -> Self {
        let translation = translate(pattern);
        Self::from_translation_with_live_captures(pattern, translation, live_captures)
    }

    fn from_translation_with_live_captures(
        pattern: &str,
        translation: Translation,
        live_captures: Vec<u32>,
    ) -> Self {
        let translated_pattern = translation.pattern.clone();
        let parsed = Arc::clone(&translation.parsed);
        let matcher = RegexMatcher::from_translation(pattern, translation);
        let unanchored_literal = matcher.unanchored_literal().map(str::to_owned);
        let restricted_start_bytes = matcher.restricted_start_bytes();
        Self {
            id: CompiledPatternId(NEXT_COMPILED_PATTERN_ID.fetch_add(1, Ordering::Relaxed)),
            source: Arc::from(pattern),
            translated_pattern,
            matcher,
            unanchored_literal,
            restricted_start_bytes,
            parsed,
            live_captures: live_captures.into(),
            capture_program: std::sync::OnceLock::new(),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn id(&self) -> CompiledPatternId {
        self.id
    }

    pub fn matcher(&self) -> &RegexMatcher {
        &self.matcher
    }

    pub(crate) fn translated_pattern(&self) -> &str {
        &self.translated_pattern
    }

    pub(crate) fn unanchored_literal(&self) -> Option<&str> {
        self.unanchored_literal.as_deref()
    }

    pub(crate) fn restricted_start_bytes(&self) -> Option<&[u8]> {
        self.restricted_start_bytes.as_deref()
    }

    pub(crate) fn parsed(&self) -> &ParsedRegex {
        &self.parsed
    }

    pub(crate) fn needs_capture_replay(&self) -> bool {
        !self.live_captures.is_empty()
    }

    pub(crate) fn find_live_captures_at(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        scratch: &mut bytecode::BytecodeScratch,
    ) -> Option<Result<(Option<MatchResult>, usize), FallbackError>> {
        use backtrack::{PositionEngineMode, StepBudget};

        if backtrack::capture_engine_mode() == PositionEngineMode::Recursive {
            return None;
        }
        let program = self
            .capture_program
            .get_or_init(|| {
                bytecode::Program::compile_captures(&self.parsed, &self.live_captures)
                    .ok()
                    .map(Arc::new)
            })
            .as_deref()?;
        let mut budget = StepBudget::new(backtrack::DEFAULT_STEP_BUDGET);
        let capture_match = match program.execute_captures(line, start, ctx, &mut budget, scratch) {
            Ok(result) => result,
            Err(_) => {
                return Some(Err(FallbackError::BudgetExceeded {
                    steps: budget.used(),
                }));
            }
        };
        let result = capture_match.map(|capture_match| {
            let mut captures = vec![None; self.parsed.capture_count as usize + 1];
            for (group, capture) in program.capture_layout().iter().zip(capture_match.captures) {
                captures[*group as usize] = capture;
            }
            MatchResult {
                start,
                end: capture_match.end,
                captures,
            }
        });
        Some(Ok((result, budget.used())))
    }
}

impl Matcher for RegexMatcher {
    fn find(&self, line: &str, from: usize, ctx: AnchorContext) -> Option<MatchResult> {
        match self {
            Self::Automata(matcher) => matcher.find(line, from, ctx),
            Self::Fallback(matcher) => matcher.find(line, from, ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_matcher() {
        assert_eq!(RegexMatcher::new("foo").engine_name(), "dfa");
        assert_eq!(RegexMatcher::new(r"foo(?=bar)").engine_name(), "fallback");
    }

    #[test]
    fn dfa_route_literal_is_simple() {
        match RegexMatcher::new("keyword") {
            RegexMatcher::Automata(matcher) => assert!(matcher.is_simple()),
            RegexMatcher::Fallback(_) => panic!("expected dfa route"),
        }
    }

    #[test]
    fn all_core_fixture_regexes_are_routed() {
        use crate::engine::grammar::load_dev_grammar_from_str;
        use crate::engine::state::GrammarId;
        use crate::grammars::registry::CORE_ASSETS;

        let mut total = 0usize;
        let mut fallback = 0usize;
        for (index, asset) in CORE_ASSETS.iter().enumerate() {
            let grammar = load_dev_grammar_from_str(GrammarId(index as u16), asset.source)
                .unwrap_or_else(|error| panic!("{} grammar should parse: {error}", asset.language));
            for pattern in &grammar.patterns {
                total += 1;
                let translation = translate(pattern);
                if let Route::Fallback { reasons } = translation.route {
                    fallback += 1;
                    assert!(
                        !reasons.is_empty(),
                        "{} pattern {pattern:?} routed to fallback without reason",
                        asset.language
                    );
                }
            }
        }
        assert!(total > 0);
        assert!(fallback > 0);
    }
}
