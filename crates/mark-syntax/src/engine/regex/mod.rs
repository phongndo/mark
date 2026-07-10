pub mod ast;
pub mod backtrack;
pub mod captures;
pub mod dfa;
pub mod prefilter;
pub mod translate;

use std::ops::Range;

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
        match translation.route {
            Route::Dfa => match AutomataMatcher::from_translation(pattern, translation) {
                Ok(matcher) => Self::Automata(Box::new(matcher)),
                Err(_) => Self::Fallback(Box::new(FallbackMatcher::new(pattern))),
            },
            Route::Fallback { .. } => Self::Fallback(Box::new(FallbackMatcher::new(pattern))),
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
