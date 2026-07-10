use std::{
    collections::HashMap,
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    ops::Range,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use crate::{HighlightedLine, HighlightedText, LineTextFingerprint, SyntaxClass, SyntaxSegment};

use super::cache::{CachedLine, LineCache, LineCacheKey};
use super::checkpoint::CheckpointTable;
use super::counters::{EngineCounters, PatternHotspot};
use super::grammar::{
    CaptureSpec, CompiledGrammar, GrammarLoadError, GrammarValidationError, InjectionPriority,
    RuleBody, RuleRef, load_dev_grammar_from_str,
};
use super::line::{LineChunks, next_char_boundary};
use super::regex::captures::{capture_texts, substitute_end_pattern};
use super::regex::{AnchorContext, FallbackError, MatchResult, PatternSetMatcher, RegexMatcher};
use super::scopes::ScopeClassifier;
use super::state::{GrammarId, LineTokens, PatternId, RuleId, ScopeStackId, StateId};

const MAX_INCLUDE_DEPTH: usize = 128;
const MAX_TOKENIZER_STEPS_PER_LINE: usize = 20_000;
const MAX_FALLBACK_STEPS_PER_LINE: u64 = 250_000;
const MIN_FALLBACK_STEPS_PER_CALL: u64 = 10_000_000;
const FALLBACK_STEPS_PER_SOURCE_BYTE: u64 = 512;
const MAX_SUBSTITUTED_END_PATTERN_LEN: usize = 4096;

#[derive(Debug, Default)]
pub struct Tokenizer;

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn tokenize_line(&mut self, line: &str, entry: StateId) -> LineTokens {
        // Compatibility seam retained for early engine tests. The real TextMate
        // tokenizer is `TextMateTokenizer` below.
        let tokens = if line.is_empty() {
            Vec::new()
        } else {
            vec![(0..line.len(), ScopeStackId::default())]
        };
        LineTokens {
            tokens,
            exit: entry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedToken {
    pub range: Range<usize>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizedLine {
    pub tokens: Arc<[ScopedToken]>,
    pub state: TokenizerState,
    pub entry_state_id: StateId,
    pub exit_state_id: StateId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TokenizerState {
    frames: Vec<Frame>,
}

impl TokenizerState {
    pub fn is_initial(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    pub fn state_id(&self) -> StateId {
        let mut hash = 0x811c9dc5u32;
        for frame in &self.frames {
            hash = fnv_mix(hash, u32::from(frame.grammar_id.0));
            hash = fnv_mix(hash, u32::from(frame.base_grammar_id.0));
            hash = fnv_mix(hash, frame.rule_id.0);
            hash = fnv_mix_opt_str(hash, frame.scope_prefix.as_deref());
            hash = fnv_mix_opt_str(hash, frame.name.as_deref());
            hash = fnv_mix_opt_str(hash, frame.content_name.as_deref());
            hash = fnv_mix_opt_str(hash, frame.end_pattern.as_deref());
            hash = fnv_mix_opt_str(hash, frame.while_pattern.as_deref());
        }
        StateId(hash)
    }
}

fn fnv_mix(mut hash: u32, part: u32) -> u32 {
    for byte in part.to_le_bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn fnv_mix_opt_str(hash: u32, value: Option<&str>) -> u32 {
    let mut hash = fnv_mix(hash, value.map_or(0, |value| value.len() as u32));
    if let Some(value) = value {
        for byte in value.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Frame {
    grammar_id: GrammarId,
    base_grammar_id: GrammarId,
    rule_id: RuleId,
    scope_prefix: Option<String>,
    name: Option<String>,
    content_name: Option<String>,
    end_pattern: Option<String>,
    while_pattern: Option<String>,
    end_captures: CaptureSpec,
    while_captures: CaptureSpec,
    patterns: Vec<RuleRef>,
    apply_end_pattern_last: bool,
    begin_captured_eol: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GrammarSet {
    grammars: Vec<CompiledGrammar>,
    scope_to_id: HashMap<String, GrammarId>,
}

impl GrammarSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, grammar: CompiledGrammar) -> GrammarId {
        let id = grammar.id;
        self.scope_to_id.insert(grammar.scope_name.clone(), id);
        let index = id.0 as usize;
        if index == self.grammars.len() {
            self.grammars.push(grammar);
        } else if index < self.grammars.len() {
            self.grammars[index] = grammar;
        } else {
            panic!("grammar ids must be dense and insertion ordered");
        }
        id
    }

    pub fn load_and_add(&mut self, contents: &str) -> Result<GrammarId, GrammarLoadError> {
        let id = GrammarId(self.grammars.len() as u16);
        let grammar = load_dev_grammar_from_str(id, contents)?;
        Ok(self.add(grammar))
    }

    pub fn grammar(&self, id: GrammarId) -> Option<&CompiledGrammar> {
        self.grammars.get(id.0 as usize)
    }

    pub fn grammar_by_scope(&self, scope: &str) -> Option<&CompiledGrammar> {
        let id = *self.scope_to_id.get(scope)?;
        self.grammar(id)
    }

    pub fn grammar_id_by_scope(&self, scope: &str) -> Option<GrammarId> {
        self.scope_to_id.get(scope).copied()
    }

    pub fn grammars(&self) -> &[CompiledGrammar] {
        &self.grammars
    }

    pub fn validate_include_graph(&self) -> Result<(), GrammarValidationError> {
        for grammar in &self.grammars {
            grammar.validate_local_refs()?;
            self.validate_refs_for_grammar(grammar, &grammar.top_level, "patterns")?;
            for (name, rule_ref) in &grammar.repository {
                self.validate_refs_for_grammar(
                    grammar,
                    std::slice::from_ref(rule_ref),
                    format!("repository.{name}").as_str(),
                )?;
            }
            for injection in &grammar.injections {
                self.validate_refs_for_grammar(
                    grammar,
                    &injection.patterns,
                    format!("injections.{}", injection.selector).as_str(),
                )?;
            }
            for rule in &grammar.rules {
                match &rule.body {
                    RuleBody::Match { captures, .. } => {
                        self.validate_capture_refs(
                            grammar,
                            captures,
                            format!("rule.{}.captures", rule.id.0).as_str(),
                        )?;
                    }
                    RuleBody::BeginEnd {
                        begin_captures,
                        end_captures,
                        patterns,
                        ..
                    } => {
                        self.validate_capture_refs(
                            grammar,
                            begin_captures,
                            format!("rule.{}.beginCaptures", rule.id.0).as_str(),
                        )?;
                        self.validate_capture_refs(
                            grammar,
                            end_captures,
                            format!("rule.{}.endCaptures", rule.id.0).as_str(),
                        )?;
                        self.validate_refs_for_grammar(
                            grammar,
                            patterns,
                            format!("rule.{}.patterns", rule.id.0).as_str(),
                        )?;
                    }
                    RuleBody::BeginWhile {
                        begin_captures,
                        while_captures,
                        patterns,
                        ..
                    } => {
                        self.validate_capture_refs(
                            grammar,
                            begin_captures,
                            format!("rule.{}.beginCaptures", rule.id.0).as_str(),
                        )?;
                        self.validate_capture_refs(
                            grammar,
                            while_captures,
                            format!("rule.{}.whileCaptures", rule.id.0).as_str(),
                        )?;
                        self.validate_refs_for_grammar(
                            grammar,
                            patterns,
                            format!("rule.{}.patterns", rule.id.0).as_str(),
                        )?;
                    }
                    RuleBody::IncludeOnly { patterns } => {
                        self.validate_refs_for_grammar(
                            grammar,
                            patterns,
                            format!("rule.{}.patterns", rule.id.0).as_str(),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_capture_refs(
        &self,
        grammar: &CompiledGrammar,
        captures: &CaptureSpec,
        path: &str,
    ) -> Result<(), GrammarValidationError> {
        for (group, entry) in &captures.entries {
            self.validate_refs_for_grammar(
                grammar,
                &entry.patterns,
                format!("{path}.{group}.patterns").as_str(),
            )?;
        }
        Ok(())
    }

    fn validate_refs_for_grammar(
        &self,
        grammar: &CompiledGrammar,
        refs: &[RuleRef],
        path: &str,
    ) -> Result<(), GrammarValidationError> {
        for (index, rule_ref) in refs.iter().enumerate() {
            match rule_ref {
                RuleRef::External { scope, repository } => {
                    let scope_text = grammar.scope(*scope).ok_or_else(|| {
                        GrammarValidationError::new(
                            grammar.scope_name.clone(),
                            format!("{path}[{index}]"),
                            "include",
                            format!("bad external scope id {}", scope.0),
                        )
                    })?;
                    let external = self.grammar_by_scope(scope_text).ok_or_else(|| {
                        GrammarValidationError::new(
                            grammar.scope_name.clone(),
                            format!("{path}[{index}]"),
                            "include",
                            format!("unknown external grammar {scope_text}"),
                        )
                    })?;
                    if let Some(repository) = repository
                        && !external.repository.contains_key(repository)
                    {
                        return Err(GrammarValidationError::new(
                            grammar.scope_name.clone(),
                            format!("{path}[{index}]"),
                            "include",
                            format!("unknown external include {scope_text}#{repository}"),
                        ));
                    }
                }
                other => {
                    grammar.validate_rule_ref(other, format!("{path}[{index}]").as_str(), false)?
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TextMateTokenizer {
    grammars: GrammarSet,
    root: GrammarId,
    root_scope_key: String,
    matcher_cache: HashMap<String, Arc<RegexMatcher>>,
    classifier: ScopeClassifier,
    state_interner: StateInterner,
    line_cache: LineCache<LineCacheKey, CachedLine>,
    candidate_cache: HashMap<StateId, Arc<CandidateSet>>,
    pattern_hotspots: HashMap<PatternHotspotKey, PatternHotspot>,
    max_line_bytes: Option<usize>,
    fallback_call_budget_remaining: Option<u64>,
    counters: EngineCounters,
    counters_enabled: bool,
    hot_counters_enabled: bool,
}

impl TextMateTokenizer {
    pub fn new(grammars: GrammarSet, root: GrammarId) -> Self {
        let root_scope_key = grammars
            .grammar(root)
            .map(|grammar| grammar.scope_name.clone())
            .unwrap_or_else(|| format!("grammar:{}", root.0));
        Self {
            grammars,
            root,
            root_scope_key,
            matcher_cache: HashMap::new(),
            classifier: ScopeClassifier::default(),
            state_interner: StateInterner::new(),
            line_cache: LineCache::new(0),
            candidate_cache: HashMap::new(),
            pattern_hotspots: HashMap::new(),
            max_line_bytes: None,
            fallback_call_budget_remaining: None,
            counters: EngineCounters::default(),
            counters_enabled: false,
            hot_counters_enabled: false,
        }
    }

    pub fn from_grammar(contents: &str) -> Result<Self, GrammarLoadError> {
        let mut grammars = GrammarSet::new();
        let root = grammars.load_and_add(contents)?;
        Ok(Self::new(grammars, root))
    }

    pub fn tokenize_source(&mut self, source: &str) -> HighlightedText {
        let previous_budget = self
            .fallback_call_budget_remaining
            .replace(fallback_call_budget(source.len()));
        let mut state = TokenizerState::default();
        let mut lines = Vec::new();
        for (line_index, chunk) in LineChunks::new(source).enumerate() {
            let tokenized = self.tokenize_line_scopes_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            lines.push(self.build_highlighted_line(chunk.text, &tokenized.tokens));
        }
        self.fallback_call_budget_remaining = previous_budget;
        HighlightedText { lines }
    }

    pub fn tokenize_viewport_scopes(
        &mut self,
        source: &str,
        visible: Range<usize>,
        checkpoints: &mut CheckpointTable,
    ) -> Vec<TokenizedLine> {
        let chunks = LineChunks::new(source).collect::<Vec<_>>();
        if visible.start >= visible.end || visible.start >= chunks.len() {
            return Vec::new();
        }
        let visible_end = visible.end.min(chunks.len());
        let checkpoint = checkpoints.nearest_before(visible.start).unwrap_or(
            super::checkpoint::LineCheckpoint {
                line_index: 0,
                state: StateId(0),
            },
        );
        let (resume_line, mut state) = self
            .state_for_id(checkpoint.state)
            .cloned()
            .map(|state| (checkpoint.line_index, state))
            .unwrap_or((0, TokenizerState::default()));
        self.record_checkpoint_replay_lines(visible.start.saturating_sub(resume_line));

        let mut visible_lines = Vec::new();
        for (line_index, chunk) in chunks
            .iter()
            .enumerate()
            .take(visible_end)
            .skip(resume_line)
        {
            let tokenized = self.tokenize_line_scopes_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            checkpoints.record_if_boundary(line_index + 1, tokenized.exit_state_id);
            if line_index >= visible.start {
                visible_lines.push(tokenized);
            }
        }
        visible_lines
    }

    pub fn highlight_viewport(
        &mut self,
        source: &str,
        visible: Range<usize>,
        checkpoints: &mut CheckpointTable,
    ) -> HighlightedText {
        let chunks = LineChunks::new(source).collect::<Vec<_>>();
        let visible_start = visible.start;
        let previous_budget = self
            .fallback_call_budget_remaining
            .replace(fallback_call_budget(source.len()));
        let tokenized = self.tokenize_viewport_scopes(source, visible, checkpoints);
        self.fallback_call_budget_remaining = previous_budget;
        let lines = tokenized
            .iter()
            .enumerate()
            .filter_map(|(offset, tokenized)| {
                let chunk = chunks.get(visible_start + offset)?;
                Some(self.build_highlighted_line(chunk.text, &tokenized.tokens))
            })
            .collect();
        HighlightedText { lines }
    }

    pub fn tokenize_line_scopes(
        &mut self,
        parse_text: &str,
        state: TokenizerState,
    ) -> TokenizedLine {
        self.tokenize_line_scopes_at_line(parse_text, state, 0)
    }

    pub fn tokenize_line_scopes_at_line(
        &mut self,
        parse_text: &str,
        mut state: TokenizerState,
        line_index: usize,
    ) -> TokenizedLine {
        let is_first_line = line_index == 0;
        self.record_line_tokenized();
        let entry_state_id = self.intern_state(state.clone());
        if self.fallback_call_budget_remaining == Some(0) {
            self.record_line_skipped();
            self.record_degraded_line();
            return TokenizedLine {
                tokens: plain_scoped_tokens(parse_text, self.current_stack(&state, true)).into(),
                state,
                entry_state_id,
                exit_state_id: entry_state_id,
            };
        }
        if self
            .max_line_bytes
            .is_some_and(|max_line_bytes| parse_text.len() > max_line_bytes)
        {
            self.record_line_skipped();
            self.record_degraded_line();
            return TokenizedLine {
                tokens: plain_scoped_tokens(parse_text, self.current_stack(&state, true)).into(),
                state,
                entry_state_id,
                exit_state_id: entry_state_id,
            };
        }
        let cache_key = self.line_cache_key(entry_state_id, parse_text, is_first_line);
        if self.line_cache.is_enabled() {
            if let Some(cached) = self.line_cache.get(&cache_key) {
                if let Some(exit_state) = self.state_for_id(cached.exit).cloned() {
                    self.record_line_cache_hit();
                    return TokenizedLine {
                        tokens: cached.tokens,
                        state: exit_state,
                        entry_state_id,
                        exit_state_id: cached.exit,
                    };
                }
            }
            self.record_line_cache_miss();
        }

        let mut tokens = Vec::with_capacity(parse_text.len().div_ceil(2).min(256));
        let mut cursor = 0usize;
        let suppressed_begin_rules =
            self.apply_while_continuations(parse_text, &mut state, &mut tokens, &mut cursor);

        let mut steps = 0usize;
        let mut fallback_steps = 0u64;
        let mut degraded = false;
        let mut anchor_pos = if cursor > 0 {
            Some(cursor)
        } else {
            state
                .frames
                .last()
                .is_some_and(|frame| frame.begin_captured_eol)
                .then_some(0)
        };
        // vscode-textmate keeps a line-local anchor position stack for `\G`.
        // Ordinary matches do not move it; begin rules set it and end rules
        // restore the value from before the corresponding push.
        let mut frame_anchor_positions = state
            .frames
            .iter()
            .enumerate()
            .map(|(index, _)| {
                index
                    .checked_sub(1)
                    .and_then(|parent| state.frames.get(parent))
                    .is_some_and(|frame| frame.begin_captured_eol)
                    .then_some(0)
            })
            .collect::<Vec<_>>();
        let mut loop_candidates = None;
        // End rules such as `$` are zero-width at the logical line end. Keep
        // evaluating while frames remain so line-scoped rules close even when
        // callers pass a line without its terminating newline.
        while (cursor < parse_text.len() || !state.frames.is_empty())
            && steps < MAX_TOKENIZER_STEPS_PER_LINE
        {
            steps += 1;
            if loop_candidates.is_none() {
                loop_candidates = Some(self.cached_candidates_for_state(&state));
            }
            let candidates = loop_candidates
                .as_ref()
                .expect("candidate set initialized for tokenizer step");
            let search = self.find_best_candidate(
                candidates,
                parse_text,
                cursor,
                is_first_line,
                anchor_pos,
                Some(&suppressed_begin_rules),
            );
            degraded |= search.fallback_budget_killed;
            fallback_steps = fallback_steps.saturating_add(search.fallback_steps);
            if fallback_steps > MAX_FALLBACK_STEPS_PER_LINE
                || !self.consume_fallback_call_budget(search.fallback_steps)
            {
                if let Some(counters) = self.counters_mut() {
                    counters.record_fallback_budget_kill();
                }
                degraded = true;
                let current_stack = self.current_stack(&state, true);
                self.push_token(&mut tokens, cursor..parse_text.len(), current_stack);
                break;
            }
            let Some((candidate_index, result)) = search.best else {
                let current_stack = self.current_stack(&state, true);
                self.push_token(&mut tokens, cursor..parse_text.len(), current_stack);
                break;
            };
            let state_changes = !matches!(
                candidates.candidates[candidate_index].kind,
                CandidateKind::Match { .. }
            );

            if result.start > cursor {
                let current_stack = self.current_stack(&state, true);
                self.push_token(&mut tokens, cursor..result.start, current_stack);
            }

            let depth_before = state.depth();
            let next_cursor = self.apply_candidate(
                parse_text,
                &mut state,
                &mut tokens,
                &candidates.candidates[candidate_index],
                &result,
                &mut anchor_pos,
                &mut frame_anchor_positions,
                None,
            );
            cursor = if next_cursor == result.start && state.depth() != depth_before {
                next_cursor
            } else if next_cursor <= result.start {
                next_char_boundary(parse_text, result.start)
            } else {
                next_cursor
            };
            if state_changes {
                loop_candidates = None;
            }
        }

        if steps >= MAX_TOKENIZER_STEPS_PER_LINE && cursor < parse_text.len() {
            degraded = true;
            let stack = self.current_stack(&state, true);
            self.push_token(&mut tokens, cursor..parse_text.len(), stack);
        }
        if degraded {
            self.record_degraded_line();
        }

        let exit_state_id = self.intern_state(state.clone());
        let tokens: Arc<[ScopedToken]> = tokens.into();
        if self.line_cache.is_enabled() {
            let evicted = self.line_cache.insert(
                cache_key,
                CachedLine {
                    tokens: Arc::clone(&tokens),
                    exit: exit_state_id,
                },
            );
            if evicted {
                self.record_line_cache_eviction();
            }
        }
        TokenizedLine {
            tokens,
            state,
            entry_state_id,
            exit_state_id,
        }
    }

    pub fn grammars(&self) -> &GrammarSet {
        &self.grammars
    }

    pub fn set_root(&mut self, root: GrammarId) {
        if self.root == root {
            return;
        }
        debug_assert!(self.grammars.grammar(root).is_some());
        self.root = root;
        self.root_scope_key = self
            .grammars
            .grammar(root)
            .map(|grammar| grammar.scope_name.clone())
            .unwrap_or_else(|| format!("grammar:{}", root.0));
        self.clear_line_cache();
        self.clear_candidate_cache();
    }

    pub fn intern_state(&mut self, state: TokenizerState) -> StateId {
        let (id, inserted) = self.state_interner.intern(state);
        if let Some(counters) = self.counters_mut() {
            if inserted {
                counters.record_state_cache_miss();
            } else {
                counters.record_state_cache_hit();
            }
        }
        id
    }

    pub fn state_for_id(&self, id: StateId) -> Option<&TokenizerState> {
        self.state_interner.get(id)
    }

    pub fn interned_state_count(&self) -> usize {
        self.state_interner.len()
    }

    pub fn set_line_cache_capacity(&mut self, capacity: usize) {
        self.line_cache.set_capacity(capacity);
    }

    pub fn line_cache_capacity(&self) -> usize {
        self.line_cache.capacity()
    }

    pub fn line_cache_len(&self) -> usize {
        self.line_cache.len()
    }

    pub fn clear_line_cache(&mut self) {
        self.line_cache.clear();
    }

    pub fn candidate_cache_len(&self) -> usize {
        self.candidate_cache.len()
    }

    pub fn clear_candidate_cache(&mut self) {
        self.candidate_cache.clear();
    }

    pub fn set_max_line_bytes(&mut self, max_line_bytes: Option<usize>) {
        self.max_line_bytes = max_line_bytes;
    }

    pub fn max_line_bytes(&self) -> Option<usize> {
        self.max_line_bytes
    }

    pub fn configure_limits(&mut self, limits: crate::SyntaxLimits) {
        self.set_line_cache_capacity(limits.engine_line_cache_entries());
        self.set_max_line_bytes(Some(limits.max_line_bytes));
    }

    pub fn set_counters_enabled(&mut self, enabled: bool) {
        self.counters_enabled = enabled;
    }

    pub fn set_hot_counters_enabled(&mut self, enabled: bool) {
        self.hot_counters_enabled = enabled;
    }

    pub fn counters_enabled(&self) -> bool {
        self.counters_enabled
    }

    pub fn counters(&self) -> EngineCounters {
        let mut counters = self.counters.clone();
        for hotspot in self.sorted_pattern_hotspots() {
            counters.merge_pattern_hotspot(hotspot);
        }
        counters.prune_pattern_hotspots();
        counters
    }

    pub fn reset_counters(&mut self) {
        self.counters = EngineCounters::default();
        self.pattern_hotspots.clear();
    }

    pub fn take_counters(&mut self) -> EngineCounters {
        let mut counters = std::mem::take(&mut self.counters);
        for hotspot in self.sorted_pattern_hotspots() {
            counters.merge_pattern_hotspot(hotspot);
        }
        counters.prune_pattern_hotspots();
        self.pattern_hotspots.clear();
        counters
    }

    fn sorted_pattern_hotspots(&self) -> Vec<PatternHotspot> {
        let mut hotspots = self.pattern_hotspots.values().cloned().collect::<Vec<_>>();
        hotspots.sort_by(|left, right| {
            right
                .total_micros
                .cmp(&left.total_micros)
                .then_with(|| right.fallback_steps_total.cmp(&left.fallback_steps_total))
                .then_with(|| right.attempts.cmp(&left.attempts))
                .then_with(|| left.pattern.cmp(&right.pattern))
        });
        hotspots.truncate(128);
        hotspots
    }

    #[allow(clippy::too_many_arguments)]
    fn record_pattern_hotspot(
        &mut self,
        pattern: &str,
        engine: &'static str,
        elapsed_micros: u64,
        matched: bool,
        fallback_steps: u64,
        fallback_budget_killed: bool,
        prefilter_may_match: Option<bool>,
    ) {
        if !self.counters_enabled || !self.hot_counters_enabled {
            return;
        }
        let key = PatternHotspotKey {
            root_scope: self.root_scope_key.clone(),
            engine: engine.to_owned(),
            pattern: pattern.to_owned(),
        };
        let hotspot = self
            .pattern_hotspots
            .entry(key)
            .or_insert_with(|| PatternHotspot {
                root_scope: self.root_scope_key.clone(),
                engine: engine.to_owned(),
                pattern: pattern.to_owned(),
                ..PatternHotspot::default()
            });
        hotspot.attempts = hotspot.attempts.saturating_add(1);
        if matched {
            hotspot.matches = hotspot.matches.saturating_add(1);
        }
        hotspot.total_micros = hotspot.total_micros.saturating_add(elapsed_micros);
        hotspot.fallback_steps_total = hotspot.fallback_steps_total.saturating_add(fallback_steps);
        hotspot.fallback_steps_max = hotspot.fallback_steps_max.max(fallback_steps);
        if fallback_budget_killed {
            hotspot.fallback_budget_kills = hotspot.fallback_budget_kills.saturating_add(1);
        }
        match prefilter_may_match {
            Some(true) => hotspot.prefilter_hits = hotspot.prefilter_hits.saturating_add(1),
            Some(false) => hotspot.prefilter_skips = hotspot.prefilter_skips.saturating_add(1),
            None => {}
        }
    }

    fn counters_mut(&mut self) -> Option<&mut EngineCounters> {
        if self.counters_enabled {
            Some(&mut self.counters)
        } else {
            None
        }
    }

    fn record_line_tokenized(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_line_tokenized();
        }
    }

    fn record_line_skipped(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_line_skipped();
        }
    }

    fn record_degraded_line(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_degraded_line();
        }
    }

    fn record_line_cache_hit(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_line_cache_hit();
        }
    }

    fn record_line_cache_miss(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_line_cache_miss();
        }
    }

    fn record_line_cache_eviction(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_line_cache_eviction();
        }
    }

    fn record_candidate_cache_hit(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_candidate_list_cache_hit();
        }
    }

    fn record_candidate_cache_miss(&mut self) {
        if let Some(counters) = self.counters_mut() {
            counters.record_candidate_list_cache_miss();
        }
    }

    fn record_prefilter_check(&mut self, may_match: bool) {
        if let Some(counters) = self.counters_mut() {
            counters.record_prefilter_check(may_match);
        }
    }

    fn record_checkpoint_replay_lines(&mut self, lines: usize) {
        if lines > 0
            && let Some(counters) = self.counters_mut()
        {
            counters.record_checkpoint_replay_lines(lines);
        }
    }

    fn consume_fallback_call_budget(&mut self, steps: u64) -> bool {
        let Some(remaining) = self.fallback_call_budget_remaining.as_mut() else {
            return true;
        };
        if steps == 0 {
            return *remaining > 0;
        }
        if steps >= *remaining {
            *remaining = 0;
            false
        } else {
            *remaining -= steps;
            true
        }
    }

    fn line_cache_key(&self, entry: StateId, parse_text: &str, first_line: bool) -> LineCacheKey {
        LineCacheKey {
            language: self.root_language_key(),
            bundle_version: crate::types::TEXTMATE_BUNDLE_VERSION.to_owned(),
            entry,
            first_line,
            fingerprint: LineTextFingerprint::from_text(parse_text),
        }
    }

    fn root_language_key(&self) -> String {
        self.root_scope_key.clone()
    }

    fn build_highlighted_line(
        &mut self,
        text: &str,
        scoped_tokens: &[ScopedToken],
    ) -> HighlightedLine {
        let mut line = HighlightedLine {
            fingerprint: LineTextFingerprint::from_text(text),
            segments: Vec::with_capacity(scoped_tokens.len()),
        };
        for token in scoped_tokens {
            let start = token.range.start.min(text.len());
            let end = token.range.end.min(text.len());
            if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                continue;
            }
            let class = self.classifier.class_for_stack(&token.scopes);
            push_segment(&mut line.segments, start, end, class);
        }
        line
    }

    fn apply_while_continuations(
        &mut self,
        line: &str,
        state: &mut TokenizerState,
        tokens: &mut Vec<ScopedToken>,
        cursor: &mut usize,
    ) -> HashSet<(GrammarId, RuleId)> {
        let mut suppressed = HashSet::new();
        let while_frames = state
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, frame)| frame.while_pattern.as_ref().map(|_| index))
            .collect::<Vec<_>>();
        for index in while_frames {
            let Some(frame) = state.frames.get(index).cloned() else {
                break;
            };
            let Some(pattern) = frame.while_pattern.clone() else {
                continue;
            };
            let ctx = AnchorContext::continuation(*cursor);
            let result = self.find_pattern(&pattern, line, *cursor, ctx);
            match result {
                Some(result) if result.start == *cursor => {
                    let frame_state = TokenizerState {
                        frames: state.frames[..=index].to_vec(),
                    };
                    let stack = self.stack_for_frame_end(&frame_state);
                    self.emit_match(
                        tokens,
                        line,
                        &result,
                        frame.grammar_id,
                        stack,
                        None,
                        &frame.while_captures,
                    );
                    // A zero-width while match only validates continuation; it
                    // must not consume the first byte of the continued line.
                    *cursor = result.end;
                }
                _ => {
                    // A failed ancestor while condition also removes every
                    // child frame opened inside that continuation.
                    if state.frames[index + 1..]
                        .iter()
                        .any(|child| child.end_pattern.is_some())
                    {
                        suppressed.insert((frame.grammar_id, frame.rule_id));
                    }
                    state.frames.truncate(index);
                    break;
                }
            }
        }
        suppressed
    }

    fn candidates_for_state(&self, state: &TokenizerState, stack: &[String]) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        let mut order = 0usize;

        let (grammar_id, base_grammar_id, refs, end_candidate, apply_end_last) =
            if let Some(frame) = state.frames.last() {
                let end = frame.end_pattern.as_ref().map(|pattern| Candidate {
                    order: 0,
                    base_grammar_id: frame.base_grammar_id,
                    pattern: pattern.clone(),
                    scope_prefix: frame.scope_prefix.clone(),
                    kind: CandidateKind::End {
                        grammar_id: frame.grammar_id,
                        captures: frame.end_captures.clone(),
                    },
                });
                (
                    frame.grammar_id,
                    frame.base_grammar_id,
                    frame.patterns.clone(),
                    end,
                    frame.apply_end_pattern_last,
                )
            } else {
                let Some(grammar) = self.grammars.grammar(self.root) else {
                    return candidates;
                };
                (self.root, self.root, grammar.top_level.clone(), None, false)
            };

        let (left_injections, right_injections) = self.injection_candidates(stack);
        for injection in left_injections {
            self.flatten_refs(
                injection.grammar_id,
                base_grammar_id,
                &injection.patterns,
                None,
                &mut candidates,
                &mut order,
                0,
            );
        }

        if let Some(end) = end_candidate.clone().filter(|_| !apply_end_last) {
            candidates.push(Candidate { order, ..end });
            order += 1;
        }

        self.flatten_refs(
            grammar_id,
            base_grammar_id,
            &refs,
            None,
            &mut candidates,
            &mut order,
            0,
        );

        if let Some(end) = end_candidate.filter(|_| apply_end_last) {
            candidates.push(Candidate { order, ..end });
            order += 1;
        }

        for injection in right_injections {
            self.flatten_refs(
                injection.grammar_id,
                base_grammar_id,
                &injection.patterns,
                None,
                &mut candidates,
                &mut order,
                0,
            );
        }

        candidates
    }

    #[allow(clippy::too_many_arguments)]
    fn flatten_refs(
        &self,
        grammar_id: GrammarId,
        base_grammar_id: GrammarId,
        refs: &[RuleRef],
        scope_prefix: Option<String>,
        out: &mut Vec<Candidate>,
        order: &mut usize,
        depth: usize,
    ) {
        if depth >= MAX_INCLUDE_DEPTH {
            return;
        }
        let Some(grammar) = self.grammars.grammar(grammar_id) else {
            return;
        };
        for rule_ref in refs {
            match rule_ref {
                RuleRef::Rule(rule_id) => {
                    let Some(rule) = grammar.rule(*rule_id) else {
                        continue;
                    };
                    match &rule.body {
                        RuleBody::Match {
                            pattern,
                            captures,
                            name,
                        } => {
                            if let Some(pattern) = grammar.pattern(*pattern) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: pattern.to_owned(),
                                    scope_prefix: scope_prefix.clone(),
                                    kind: CandidateKind::Match {
                                        grammar_id,
                                        name: scope_name(grammar, *name),
                                        captures: captures.clone(),
                                    },
                                });
                                *order += 1;
                            }
                        }
                        RuleBody::BeginEnd {
                            begin,
                            end,
                            begin_captures,
                            end_captures,
                            name,
                            content_name,
                            apply_end_pattern_last,
                            patterns,
                        } => {
                            if self.only_unavailable_external_includes(grammar, patterns) {
                                continue;
                            }
                            if let Some(begin) = grammar.pattern(*begin) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: begin.to_owned(),
                                    scope_prefix: scope_prefix.clone(),
                                    kind: CandidateKind::BeginEnd {
                                        grammar_id,
                                        rule_id: rule.id,
                                        end: *end,
                                        begin_captures: begin_captures.clone(),
                                        end_captures: end_captures.clone(),
                                        name: scope_name(grammar, *name),
                                        content_name: scope_name(grammar, *content_name),
                                        patterns: patterns.clone(),
                                        apply_end_pattern_last: *apply_end_pattern_last,
                                    },
                                });
                                *order += 1;
                            }
                        }
                        RuleBody::BeginWhile {
                            begin,
                            while_pattern,
                            begin_captures,
                            while_captures,
                            name,
                            content_name,
                            patterns,
                        } => {
                            if self.only_unavailable_external_includes(grammar, patterns) {
                                continue;
                            }
                            if let Some(begin) = grammar.pattern(*begin) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: begin.to_owned(),
                                    scope_prefix: scope_prefix.clone(),
                                    kind: CandidateKind::BeginWhile {
                                        grammar_id,
                                        rule_id: rule.id,
                                        while_pattern: *while_pattern,
                                        begin_captures: begin_captures.clone(),
                                        while_captures: while_captures.clone(),
                                        name: scope_name(grammar, *name),
                                        content_name: scope_name(grammar, *content_name),
                                        patterns: patterns.clone(),
                                    },
                                });
                                *order += 1;
                            }
                        }
                        RuleBody::IncludeOnly { patterns } => self.flatten_refs(
                            grammar_id,
                            base_grammar_id,
                            patterns,
                            scope_prefix.clone(),
                            out,
                            order,
                            depth + 1,
                        ),
                    }
                }
                RuleRef::Repository(name) => {
                    if let Some(rule_ref) = grammar.repository.get(name) {
                        self.flatten_refs(
                            grammar_id,
                            base_grammar_id,
                            std::slice::from_ref(rule_ref),
                            scope_prefix.clone(),
                            out,
                            order,
                            depth + 1,
                        );
                    }
                }
                RuleRef::SelfRef => {
                    self.flatten_refs(
                        grammar_id,
                        base_grammar_id,
                        &grammar.top_level,
                        scope_prefix.clone(),
                        out,
                        order,
                        depth + 1,
                    );
                }
                RuleRef::BaseRef => {
                    let Some(base) = self.grammars.grammar(base_grammar_id) else {
                        continue;
                    };
                    self.flatten_refs(
                        base_grammar_id,
                        base_grammar_id,
                        &base.top_level,
                        scope_prefix.clone(),
                        out,
                        order,
                        depth + 1,
                    );
                }
                RuleRef::External { scope, repository } => {
                    let Some(scope_text) = grammar.scope(*scope) else {
                        continue;
                    };
                    let Some(external_id) = self.grammars.grammar_id_by_scope(scope_text) else {
                        continue;
                    };
                    let Some(external) = self.grammars.grammar(external_id) else {
                        continue;
                    };
                    if let Some(repository) = repository {
                        if let Some(rule_ref) = external.repository.get(repository) {
                            self.flatten_refs(
                                external_id,
                                base_grammar_id,
                                std::slice::from_ref(rule_ref),
                                None,
                                out,
                                order,
                                depth + 1,
                            );
                        }
                    } else {
                        self.flatten_refs(
                            external_id,
                            base_grammar_id,
                            &external.top_level,
                            None,
                            out,
                            order,
                            depth + 1,
                        );
                    }
                }
            }
        }
    }

    fn only_unavailable_external_includes(
        &self,
        grammar: &CompiledGrammar,
        refs: &[RuleRef],
    ) -> bool {
        !refs.is_empty()
            && refs.iter().all(|rule_ref| {
                let RuleRef::External { scope, .. } = rule_ref else {
                    return false;
                };
                grammar
                    .scope(*scope)
                    .is_none_or(|scope| self.grammars.grammar_id_by_scope(scope).is_none())
            })
    }

    fn injection_candidates(
        &self,
        stack: &[String],
    ) -> (Vec<InjectionCandidate>, Vec<InjectionCandidate>) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut seen = HashSet::new();
        for grammar in self.grammars.grammars() {
            for injection in &grammar.injections {
                if selector_matches(&injection.selector_body, stack) {
                    if !seen.insert((injection.priority, grammar.id, injection.patterns.clone())) {
                        continue;
                    }
                    let candidate = InjectionCandidate {
                        grammar_id: grammar.id,
                        patterns: injection.patterns.clone(),
                    };
                    if injection.priority == InjectionPriority::Left {
                        left.push(candidate);
                    } else {
                        right.push(candidate);
                    }
                }
            }
        }
        (left, right)
    }

    fn cached_candidates_for_state(&mut self, state: &TokenizerState) -> Arc<CandidateSet> {
        let state_id = self.intern_state(state.clone());
        if let Some(candidates) = self.candidate_cache.get(&state_id).cloned() {
            self.record_candidate_cache_hit();
            return candidates;
        }
        self.record_candidate_cache_miss();
        let stack = self.current_stack(state, true);
        let candidates = self.candidates_for_state(state, &stack);
        let candidate_set = Arc::new(self.build_candidate_set(candidates));
        self.candidate_cache.insert(state_id, candidate_set.clone());
        candidate_set
    }

    fn build_candidate_set(&mut self, candidates: Vec<Candidate>) -> CandidateSet {
        let matchers = candidates
            .iter()
            .map(|candidate| self.cached_matcher(&candidate.pattern))
            .collect::<Vec<_>>();
        let patterns = candidates
            .iter()
            .map(|candidate| candidate.pattern.clone())
            .collect::<Vec<_>>();
        let pattern_set_search = (patterns.len() > 1)
            .then(|| PatternSetMatcher::new(&patterns).ok())
            .flatten();
        CandidateSet {
            candidates,
            matchers,
            pattern_set_search,
        }
    }

    fn cached_matcher(&mut self, pattern: &str) -> Arc<RegexMatcher> {
        if let Some(matcher) = self.matcher_cache.get(pattern) {
            return matcher.clone();
        }
        let matcher = Arc::new(RegexMatcher::new(pattern));
        self.matcher_cache
            .insert(pattern.to_owned(), matcher.clone());
        matcher
    }

    fn find_best_candidate(
        &mut self,
        candidate_set: &CandidateSet,
        line: &str,
        from: usize,
        is_first_line: bool,
        anchor_pos: Option<usize>,
        suppressed_begin_rules: Option<&HashSet<(GrammarId, RuleId)>>,
    ) -> CandidateSearchResult {
        let mut best: Option<(usize, MatchResult)> = None;
        let mut fallback_budget_killed = false;
        let mut fallback_steps = 0u64;

        let suppression_active = suppressed_begin_rules.is_some_and(|rules| !rules.is_empty());
        let unified_search_active = !suppression_active && !self.counters_enabled;
        let ctx = scan_anchor_context(from, is_first_line, anchor_pos);
        if unified_search_active && let Some(pattern_set) = &candidate_set.pattern_set_search {
            if let Some((pattern_index, set_result)) =
                pattern_set.find_with_context(line, from, ctx)
                && pattern_index < candidate_set.candidates.len()
                && set_result.start >= from
                && set_result.end <= line.len()
            {
                best = Some((pattern_index, set_result));
            }
        } else {
            for (index, candidate) in candidate_set.candidates.iter().enumerate() {
                if suppressed_begin_rules
                    .is_some_and(|rules| candidate_is_suppressed(candidate, rules))
                {
                    continue;
                }
                if let Some((best_index, best_result)) = &best
                    && best_result.start == from
                    && candidate.order > candidate_set.candidates[*best_index].order
                {
                    break;
                }
                let pattern = self.find_cached_pattern_selection_report(
                    &candidate.pattern,
                    candidate_set.matchers[index].as_ref(),
                    line,
                    from,
                    ctx,
                );
                fallback_budget_killed |= pattern.fallback_budget_killed;
                fallback_steps = fallback_steps.saturating_add(pattern.fallback_steps);
                let Some(result) = pattern.result else {
                    continue;
                };
                if result.start < from || result.end > line.len() {
                    continue;
                }
                let replace = match &best {
                    None => true,
                    Some((best_index, best_result)) => {
                        result.start < best_result.start
                            || (result.start == best_result.start
                                && candidate.order < candidate_set.candidates[*best_index].order)
                    }
                };
                if replace {
                    best = Some((index, result));
                }
            }
        }
        if let Some((index, selection_result)) = &best
            && selection_result.captures.is_empty()
            && candidate_requires_capture_replay(&candidate_set.candidates[*index])
        {
            let ctx = scan_anchor_context(from, is_first_line, anchor_pos);
            match candidate_set.matchers[*index].find_report_at(line, selection_result.start, ctx) {
                Ok((Some(result), steps)) => {
                    let steps = steps.unwrap_or(0) as u64;
                    fallback_steps = fallback_steps.saturating_add(steps);
                    best = Some((*index, result));
                }
                Ok((None, steps)) => {
                    fallback_steps = fallback_steps.saturating_add(steps.unwrap_or(0) as u64);
                    best = None;
                }
                Err(FallbackError::BudgetExceeded { steps }) => {
                    fallback_steps = fallback_steps.saturating_add(steps as u64);
                    fallback_budget_killed = true;
                    best = None;
                }
                Err(FallbackError::InvalidStart { .. }) => best = None,
            }
        }
        CandidateSearchResult {
            best,
            fallback_budget_killed,
            fallback_steps,
        }
    }

    fn find_pattern(
        &mut self,
        pattern: &str,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Option<MatchResult> {
        self.find_pattern_report(pattern, line, from, ctx).result
    }

    fn find_pattern_report(
        &mut self,
        pattern: &str,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> PatternSearchResult {
        let matcher = self.cached_matcher(pattern);
        self.find_cached_pattern_report(pattern, matcher.as_ref(), line, from, ctx)
    }

    fn find_cached_pattern_report(
        &mut self,
        pattern: &str,
        matcher: &RegexMatcher,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> PatternSearchResult {
        self.find_cached_pattern_report_impl(pattern, matcher, line, from, ctx, false)
    }

    fn find_cached_pattern_selection_report(
        &mut self,
        pattern: &str,
        matcher: &RegexMatcher,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> PatternSearchResult {
        self.find_cached_pattern_report_impl(pattern, matcher, line, from, ctx, true)
    }

    fn find_cached_pattern_report_impl(
        &mut self,
        pattern: &str,
        matcher: &RegexMatcher,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        selection_only: bool,
    ) -> PatternSearchResult {
        let counters_enabled = self.counters_enabled;
        let hot_counters_enabled = self.hot_counters_enabled;
        let start = hot_counters_enabled.then(Instant::now);
        let engine = matcher.engine_name();
        let prefilter_may_match = counters_enabled
            .then(|| matcher.prefilter_may_match(line, from))
            .flatten();
        trace_regex_search(pattern, line, from, ctx, engine);
        let report = if selection_only {
            matcher.find_report_for_selection(line, from, ctx)
        } else {
            matcher.find_report(line, from, ctx)
        };
        let elapsed_micros = start
            .map(|start| start.elapsed().as_micros() as u64)
            .unwrap_or(0);
        if let Some(counters) = self.counters_mut() {
            match engine {
                "dfa" => counters.record_dfa_attempt(),
                "fallback" => counters.record_fallback_attempt(),
                _ => {}
            }
        }
        if let Some(may_match) = prefilter_may_match {
            self.record_prefilter_check(may_match);
        }
        match report {
            Ok((result, steps)) => {
                let matched = result.is_some();
                let fallback_steps = steps.unwrap_or(0) as u64;
                if let Some(steps) = steps
                    && let Some(counters) = self.counters_mut()
                {
                    counters.record_fallback_steps(steps);
                }
                self.record_pattern_hotspot(
                    pattern,
                    engine,
                    elapsed_micros,
                    matched,
                    fallback_steps,
                    false,
                    prefilter_may_match,
                );
                PatternSearchResult {
                    result,
                    fallback_budget_killed: false,
                    fallback_steps,
                }
            }
            Err(FallbackError::BudgetExceeded { steps }) => {
                if let Some(counters) = self.counters_mut() {
                    counters.record_fallback_steps(steps);
                    counters.record_fallback_budget_kill();
                }
                self.record_pattern_hotspot(
                    pattern,
                    engine,
                    elapsed_micros,
                    false,
                    steps as u64,
                    true,
                    prefilter_may_match,
                );
                PatternSearchResult {
                    result: None,
                    fallback_budget_killed: true,
                    fallback_steps: steps as u64,
                }
            }
            Err(FallbackError::InvalidStart { .. }) => {
                self.record_pattern_hotspot(
                    pattern,
                    engine,
                    elapsed_micros,
                    false,
                    0,
                    false,
                    prefilter_may_match,
                );
                PatternSearchResult {
                    result: None,
                    fallback_budget_killed: false,
                    fallback_steps: 0,
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_candidate(
        &mut self,
        line: &str,
        state: &mut TokenizerState,
        tokens: &mut Vec<ScopedToken>,
        candidate: &Candidate,
        result: &MatchResult,
        anchor_pos: &mut Option<usize>,
        frame_anchor_positions: &mut Vec<Option<usize>>,
        base_stack_override: Option<&[String]>,
    ) -> usize {
        match &candidate.kind {
            CandidateKind::Match {
                grammar_id,
                name,
                captures,
            } => {
                let consumed_end = specified_outside_capture_end(result, captures);
                let mut stack = self.current_stack_with_base(state, true, base_stack_override);
                if let Some(prefix) = &candidate.scope_prefix {
                    push_scope_once(&mut stack, prefix.clone());
                }
                self.emit_match(
                    tokens,
                    line,
                    result,
                    *grammar_id,
                    stack,
                    name.clone(),
                    captures,
                );
                advance_zero_width(line, &(result.start..consumed_end))
            }
            CandidateKind::BeginEnd {
                grammar_id,
                rule_id,
                end,
                begin_captures,
                end_captures,
                name,
                content_name,
                patterns,
                apply_end_pattern_last,
            } => {
                let consumed_end = specified_outside_capture_end(result, begin_captures);
                let name = name
                    .as_ref()
                    .map(|name| substitute_scope_text(name, line, &result.captures));
                let content_name = content_name
                    .as_ref()
                    .map(|name| substitute_scope_text(name, line, &result.captures));
                let mut stack = self.current_stack_with_base(state, true, base_stack_override);
                if let Some(prefix) = candidate.scope_prefix.clone() {
                    push_scope_once(&mut stack, prefix);
                }
                self.emit_match(
                    tokens,
                    line,
                    result,
                    *grammar_id,
                    stack,
                    name.clone(),
                    begin_captures,
                );
                let end_pattern = self.substituted_pattern(*grammar_id, *end, line, result);
                state.frames.push(Frame {
                    grammar_id: *grammar_id,
                    base_grammar_id: candidate.base_grammar_id,
                    rule_id: *rule_id,
                    scope_prefix: candidate.scope_prefix.clone(),
                    name,
                    content_name,
                    end_pattern,
                    while_pattern: None,
                    end_captures: end_captures.clone(),
                    while_captures: CaptureSpec::default(),
                    patterns: patterns.clone(),
                    apply_end_pattern_last: *apply_end_pattern_last,
                    begin_captured_eol: result.end == line.len() && line.ends_with('\n'),
                });
                frame_anchor_positions.push(*anchor_pos);
                *anchor_pos = Some(result.end);
                consumed_end
            }
            CandidateKind::BeginWhile {
                grammar_id,
                rule_id,
                while_pattern,
                begin_captures,
                while_captures,
                name,
                content_name,
                patterns,
            } => {
                let consumed_end = specified_outside_capture_end(result, begin_captures);
                let name = name
                    .as_ref()
                    .map(|name| substitute_scope_text(name, line, &result.captures));
                let content_name = content_name
                    .as_ref()
                    .map(|name| substitute_scope_text(name, line, &result.captures));
                let mut stack = self.current_stack_with_base(state, true, base_stack_override);
                if let Some(prefix) = candidate.scope_prefix.clone() {
                    push_scope_once(&mut stack, prefix);
                }
                if begin_captures.entries.is_empty()
                    && content_name.is_some()
                    && !patterns.is_empty()
                {
                    let mut content_stack = stack.clone();
                    if let Some(name) = &name {
                        push_scopes(&mut content_stack, name);
                    }
                    if let Some(content_name) = &content_name {
                        push_scopes(&mut content_stack, content_name);
                    }
                    self.tokenize_inline_patterns(
                        tokens,
                        line,
                        result.start..result.end,
                        *grammar_id,
                        content_stack,
                        patterns,
                        false,
                    );
                } else {
                    self.emit_match(
                        tokens,
                        line,
                        result,
                        *grammar_id,
                        stack,
                        name.clone(),
                        begin_captures,
                    );
                }
                let while_pattern =
                    self.substituted_pattern(*grammar_id, *while_pattern, line, result);
                state.frames.push(Frame {
                    grammar_id: *grammar_id,
                    base_grammar_id: candidate.base_grammar_id,
                    rule_id: *rule_id,
                    scope_prefix: candidate.scope_prefix.clone(),
                    name,
                    content_name,
                    end_pattern: None,
                    while_pattern,
                    end_captures: CaptureSpec::default(),
                    while_captures: while_captures.clone(),
                    patterns: patterns.clone(),
                    apply_end_pattern_last: false,
                    begin_captured_eol: result.end == line.len() && line.ends_with('\n'),
                });
                frame_anchor_positions.push(*anchor_pos);
                *anchor_pos = Some(result.end);
                consumed_end
            }
            CandidateKind::End {
                grammar_id,
                captures,
            } => {
                let consumed_end = specified_outside_capture_end(result, captures);
                let stack = self.current_stack_with_base(state, false, base_stack_override);
                self.emit_match(tokens, line, result, *grammar_id, stack, None, captures);
                state.frames.pop();
                *anchor_pos = frame_anchor_positions.pop().flatten();
                consumed_end
            }
        }
    }

    fn substituted_pattern(
        &self,
        grammar_id: GrammarId,
        pattern_id: PatternId,
        line: &str,
        result: &MatchResult,
    ) -> Option<String> {
        let grammar = self.grammars.grammar(grammar_id)?;
        let pattern = grammar.pattern(pattern_id)?;
        let capture_texts = capture_texts(line, &result.captures);
        Some(
            substitute_end_pattern(pattern, &capture_texts, MAX_SUBSTITUTED_END_PATTERN_LEN)
                .unwrap_or_else(|_| pattern.to_owned()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_match(
        &mut self,
        tokens: &mut Vec<ScopedToken>,
        line: &str,
        result: &MatchResult,
        grammar_id: GrammarId,
        mut base_stack: Vec<String>,
        name: Option<String>,
        captures: &CaptureSpec,
    ) {
        if let Some(name) = name {
            push_scopes(
                &mut base_stack,
                &substitute_scope_text(&name, line, &result.captures),
            );
        }
        if captures.entries.is_empty() {
            self.push_token(tokens, result.start..result.end, base_stack);
            return;
        }
        let match_end = result.end;
        let result_captures = &result.captures;
        let outside = captures
            .entries
            .iter()
            .filter_map(|(group, entry)| {
                if entry.name.is_none() && entry.patterns.is_empty() {
                    return None;
                }
                let range = result_captures
                    .get(*group as usize)
                    .and_then(Clone::clone)?;
                (match_end > result.start && range.start >= match_end && range.end > match_end)
                    .then_some((range, entry.clone()))
            })
            .collect::<Vec<_>>();
        if outside.is_empty() {
            self.emit_capture_range(
                tokens,
                line,
                result.start..result.end,
                grammar_id,
                base_stack,
                captures,
                result_captures,
            );
            return;
        }
        self.emit_capture_range(
            tokens,
            line,
            result.start..result.end,
            grammar_id,
            base_stack.clone(),
            captures,
            result_captures,
        );
        for (range, entry) in outside {
            let range = range.start.max(match_end)..range.end;
            let mut stack = base_stack.clone();
            if let Some(name) = entry
                .name
                .and_then(|id| self.grammars.grammar(grammar_id)?.scope(id))
                .map(str::to_owned)
            {
                push_scopes(
                    &mut stack,
                    &substitute_scope_text(&name, line, result_captures),
                );
            }
            if entry.patterns.is_empty() {
                self.push_token(tokens, range, stack);
            } else {
                self.tokenize_inline_patterns(
                    tokens,
                    line,
                    range,
                    grammar_id,
                    stack,
                    &entry.patterns,
                    true,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_capture_range(
        &mut self,
        tokens: &mut Vec<ScopedToken>,
        line: &str,
        range: Range<usize>,
        grammar_id: GrammarId,
        base_stack: Vec<String>,
        capture_spec: &CaptureSpec,
        captures: &[Option<Range<usize>>],
    ) {
        if range.start >= range.end {
            return;
        }
        let Some(grammar) = self.grammars.grammar(grammar_id) else {
            self.push_token(tokens, range, base_stack);
            return;
        };
        let mut base_stack = base_stack;
        let mut items = Vec::new();
        for (group, entry) in &capture_spec.entries {
            let Some(capture_range) = captures.get(*group as usize).and_then(Clone::clone) else {
                continue;
            };
            if capture_range.start >= capture_range.end {
                continue;
            }
            let name = entry
                .name
                .and_then(|id| grammar.scope(id).map(str::to_owned))
                .map(|name| substitute_scope_text(&name, line, captures));
            if capture_range == range {
                if let Some(name) = name {
                    push_scopes(&mut base_stack, &name);
                }
                if !entry.patterns.is_empty() {
                    self.tokenize_inline_patterns(
                        tokens,
                        line,
                        range,
                        grammar_id,
                        base_stack,
                        &entry.patterns,
                        true,
                    );
                    return;
                }
                continue;
            }
            items.push(CaptureItem {
                group: *group,
                range: capture_range,
                name,
                patterns: entry.patterns.clone(),
            });
        }
        items.sort_by(|left, right| {
            left.range
                .start
                .cmp(&right.range.start)
                .then_with(|| right.range.end.cmp(&left.range.end))
                .then_with(|| left.group.cmp(&right.group))
        });
        self.emit_nested_capture_items(tokens, line, range, grammar_id, base_stack, &items);
    }

    fn emit_nested_capture_items(
        &mut self,
        tokens: &mut Vec<ScopedToken>,
        line: &str,
        range: Range<usize>,
        grammar_id: GrammarId,
        mut base_stack: Vec<String>,
        items: &[CaptureItem],
    ) {
        let mut same_range_patterns = None;
        for item in items.iter().filter(|item| item.range == range) {
            if let Some(name) = &item.name {
                push_scopes(&mut base_stack, name);
            }
            if same_range_patterns.is_none() && !item.patterns.is_empty() {
                same_range_patterns = Some(item.patterns.as_slice());
            }
        }
        if let Some(patterns) = same_range_patterns {
            self.tokenize_inline_patterns(
                tokens, line, range, grammar_id, base_stack, patterns, true,
            );
            return;
        }
        let direct = direct_children(range.clone(), items);
        if direct.is_empty() {
            self.push_token(tokens, range, base_stack);
            return;
        }
        let mut cursor = range.start;
        for item_index in direct {
            let item = &items[item_index];
            let item_range = clamp_range(item.range.clone(), range.clone());
            if cursor < item_range.start {
                self.push_token(tokens, cursor..item_range.start, base_stack.clone());
            }
            let mut stack = base_stack.clone();
            if let Some(name) = item.name.clone() {
                push_scopes(&mut stack, &name);
            }
            if item.patterns.is_empty() {
                let nested = items
                    .iter()
                    .filter(|candidate| {
                        candidate.group != item.group
                            && contains_range(&item_range, &candidate.range)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                self.emit_nested_capture_items(
                    tokens,
                    line,
                    item_range.clone(),
                    grammar_id,
                    stack,
                    &nested,
                );
            } else {
                self.tokenize_inline_patterns(
                    tokens,
                    line,
                    item_range.clone(),
                    grammar_id,
                    stack,
                    &item.patterns,
                    true,
                );
            }
            cursor = item_range.end;
        }
        if cursor < range.end {
            self.push_token(tokens, cursor..range.end, base_stack);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tokenize_inline_patterns(
        &mut self,
        tokens: &mut Vec<ScopedToken>,
        line: &str,
        range: Range<usize>,
        grammar_id: GrammarId,
        base_stack: Vec<String>,
        patterns: &[RuleRef],
        compound_patterns: bool,
    ) {
        let mut state = TokenizerState::default();
        let mut candidate_cache = HashMap::<TokenizerState, Arc<CandidateSet>>::new();
        let mut cursor = range.start;
        let mut steps = 0usize;
        let mut fallback_steps = 0u64;
        let mut anchor_pos = Some(range.start);
        let mut frame_anchor_positions = Vec::new();
        // Capture retokenization is bounded by the capture. Let lookbehind see
        // the original prefix, but do not let a greedy child consume text
        // after the capture (for example the closing `]` after a TOML key).
        let scan_line = line.get(..range.end).unwrap_or(line);
        while cursor < range.end && steps < MAX_TOKENIZER_STEPS_PER_LINE {
            steps += 1;
            let candidate_set = if let Some(cached) = candidate_cache.get(&state) {
                cached.clone()
            } else {
                let candidates = if state.is_initial() {
                    let mut candidates = Vec::new();
                    let mut order = 0usize;
                    self.flatten_refs(
                        grammar_id,
                        grammar_id,
                        patterns,
                        None,
                        &mut candidates,
                        &mut order,
                        0,
                    );
                    candidates
                } else {
                    let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                    self.candidates_for_state(&state, &stack)
                };
                let candidate_set = Arc::new(self.build_candidate_set(candidates));
                candidate_cache.insert(state.clone(), candidate_set.clone());
                candidate_set
            };
            if candidate_set.candidates.is_empty() {
                let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                self.push_token(tokens, cursor..range.end, stack);
                return;
            }
            let search = self.find_best_candidate(
                &candidate_set,
                scan_line,
                cursor,
                false,
                anchor_pos,
                None,
            );
            fallback_steps = fallback_steps.saturating_add(search.fallback_steps);
            if fallback_steps > MAX_FALLBACK_STEPS_PER_LINE
                || !self.consume_fallback_call_budget(search.fallback_steps)
            {
                if let Some(counters) = self.counters_mut() {
                    counters.record_fallback_budget_kill();
                }
                let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                self.push_token(tokens, cursor..range.end, stack);
                return;
            }
            let Some((candidate_index, result)) = search.best else {
                let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                self.push_token(tokens, cursor..range.end, stack);
                return;
            };
            if result.start >= range.end || result.end > range.end {
                let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                self.push_token(tokens, cursor..range.end, stack);
                return;
            }
            if cursor < result.start {
                let stack = self.current_stack_with_base(&state, true, Some(&base_stack));
                self.push_token(tokens, cursor..result.start, stack);
            }
            let candidate = &candidate_set.candidates[candidate_index];
            if !compound_patterns
                && state.is_initial()
                && !matches!(candidate.kind, CandidateKind::Match { .. })
            {
                self.push_token(tokens, result.start..result.end, base_stack.clone());
                cursor = advance_zero_width(scan_line, &(result.start..result.end));
                continue;
            }
            let depth_before = state.depth();
            let next_cursor = self.apply_candidate(
                scan_line,
                &mut state,
                tokens,
                candidate,
                &result,
                &mut anchor_pos,
                &mut frame_anchor_positions,
                Some(&base_stack),
            );
            cursor = if next_cursor == result.start && state.depth() != depth_before {
                next_cursor
            } else if next_cursor <= result.start {
                next_char_boundary(scan_line, result.start)
            } else {
                next_cursor
            };
        }
    }

    fn current_stack(&self, state: &TokenizerState, include_top_content: bool) -> Vec<String> {
        self.current_stack_with_base(state, include_top_content, None)
    }

    fn current_stack_with_base(
        &self,
        state: &TokenizerState,
        include_top_content: bool,
        base_stack: Option<&[String]>,
    ) -> Vec<String> {
        let mut stack = base_stack.map_or_else(|| self.root_stack(), <[String]>::to_vec);
        for (index, frame) in state.frames.iter().enumerate() {
            if let Some(prefix) = &frame.scope_prefix {
                push_scope_once(&mut stack, prefix.clone());
            }
            if let Some(name) = &frame.name {
                push_scopes(&mut stack, name);
            }
            if (include_top_content || index + 1 < state.frames.len())
                && let Some(content) = &frame.content_name
            {
                push_scopes(&mut stack, content);
            }
        }
        stack
    }

    fn stack_for_frame_end(&self, state: &TokenizerState) -> Vec<String> {
        self.current_stack(state, false)
    }

    fn root_stack(&self) -> Vec<String> {
        self.grammars
            .grammar(self.root)
            .map(|grammar| vec![grammar.scope_name.clone()])
            .unwrap_or_default()
    }

    fn push_token(&self, tokens: &mut Vec<ScopedToken>, range: Range<usize>, scopes: Vec<String>) {
        if range.start >= range.end {
            return;
        }
        if let Some(last) = tokens.last_mut()
            && last.range.end == range.start
            && last.scopes == scopes
        {
            last.range.end = range.end;
            return;
        }
        tokens.push(ScopedToken { range, scopes });
    }
}

#[derive(Debug, Clone)]
struct CandidateSet {
    candidates: Vec<Candidate>,
    matchers: Vec<Arc<RegexMatcher>>,
    pattern_set_search: Option<PatternSetMatcher>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PatternHotspotKey {
    root_scope: String,
    engine: String,
    pattern: String,
}

#[derive(Debug, Clone)]
struct Candidate {
    order: usize,
    base_grammar_id: GrammarId,
    pattern: String,
    scope_prefix: Option<String>,
    kind: CandidateKind,
}

#[derive(Debug, Clone)]
enum CandidateKind {
    Match {
        grammar_id: GrammarId,
        name: Option<String>,
        captures: CaptureSpec,
    },
    BeginEnd {
        grammar_id: GrammarId,
        rule_id: RuleId,
        end: PatternId,
        begin_captures: CaptureSpec,
        end_captures: CaptureSpec,
        name: Option<String>,
        content_name: Option<String>,
        patterns: Vec<RuleRef>,
        apply_end_pattern_last: bool,
    },
    BeginWhile {
        grammar_id: GrammarId,
        rule_id: RuleId,
        while_pattern: PatternId,
        begin_captures: CaptureSpec,
        while_captures: CaptureSpec,
        name: Option<String>,
        content_name: Option<String>,
        patterns: Vec<RuleRef>,
    },
    End {
        grammar_id: GrammarId,
        captures: CaptureSpec,
    },
}

fn candidate_is_suppressed(
    candidate: &Candidate,
    suppressed: &HashSet<(GrammarId, RuleId)>,
) -> bool {
    match &candidate.kind {
        CandidateKind::BeginEnd {
            grammar_id,
            rule_id,
            ..
        }
        | CandidateKind::BeginWhile {
            grammar_id,
            rule_id,
            ..
        } => suppressed.contains(&(*grammar_id, *rule_id)),
        CandidateKind::Match { .. } | CandidateKind::End { .. } => false,
    }
}

fn candidate_requires_capture_replay(candidate: &Candidate) -> bool {
    match &candidate.kind {
        CandidateKind::Match { name, captures, .. } => {
            !captures.entries.is_empty() || name.as_ref().is_some_and(|name| name.contains('$'))
        }
        CandidateKind::End { captures, .. } => !captures.entries.is_empty(),
        CandidateKind::BeginEnd { .. } | CandidateKind::BeginWhile { .. } => true,
    }
}

#[derive(Debug, Clone)]
struct CaptureItem {
    group: u32,
    range: Range<usize>,
    name: Option<String>,
    patterns: Vec<RuleRef>,
}

#[derive(Debug, Clone)]
struct InjectionCandidate {
    grammar_id: GrammarId,
    patterns: Vec<RuleRef>,
}

#[derive(Debug, Clone)]
struct StateInterner {
    states: Vec<TokenizerState>,
    ids: HashMap<TokenizerState, StateId>,
}

impl StateInterner {
    fn new() -> Self {
        let mut interner = Self {
            states: Vec::new(),
            ids: HashMap::new(),
        };
        interner.intern(TokenizerState::default());
        interner
    }

    fn intern(&mut self, state: TokenizerState) -> (StateId, bool) {
        if let Some(id) = self.ids.get(&state) {
            return (*id, false);
        }
        let id = StateId(self.states.len() as u32);
        self.states.push(state.clone());
        self.ids.insert(state, id);
        (id, true)
    }

    fn get(&self, id: StateId) -> Option<&TokenizerState> {
        self.states.get(id.0 as usize)
    }

    fn len(&self) -> usize {
        self.states.len()
    }
}

#[derive(Debug, Clone)]
struct CandidateSearchResult {
    best: Option<(usize, MatchResult)>,
    fallback_budget_killed: bool,
    fallback_steps: u64,
}

#[derive(Debug, Clone)]
struct PatternSearchResult {
    result: Option<MatchResult>,
    fallback_budget_killed: bool,
    fallback_steps: u64,
}

fn scan_anchor_context(
    cursor: usize,
    is_first_line: bool,
    anchor_pos: Option<usize>,
) -> AnchorContext {
    AnchorContext {
        allow_a: is_first_line && cursor == 0,
        allow_g: anchor_pos == Some(cursor),
        g_pos: cursor,
    }
}

static REGEX_TRACE_FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

fn trace_regex_search(pattern: &str, line: &str, from: usize, ctx: AnchorContext, engine: &str) {
    let Some(file) = REGEX_TRACE_FILE
        .get_or_init(|| {
            let path = std::env::var_os("MARK_REGEX_TRACE")?;
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()?;
            Some(Mutex::new(file))
        })
        .as_ref()
    else {
        return;
    };
    let record = serde_json::json!({
        "pattern": pattern,
        "line": line,
        "from": from,
        "allowA": ctx.allow_a,
        "allowG": ctx.allow_g,
        "gPos": ctx.g_pos,
        "engine": engine,
    });
    if let Ok(mut file) = file.lock() {
        let _ = writeln!(file, "{record}");
    }
}

pub fn advance_zero_width(line: &str, range: &Range<usize>) -> usize {
    if range.start == range.end {
        next_char_boundary(line, range.end)
    } else {
        range.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSpan {
    pub range: Range<usize>,
    pub scope: &'static str,
}

pub fn tokenize_json_string_smoke(line: &str) -> Vec<ScopeSpan> {
    let bytes = line.as_bytes();
    let Some(start) = bytes.iter().position(|byte| *byte == b'"') else {
        return Vec::new();
    };
    let mut spans = vec![ScopeSpan {
        range: start..start + 1,
        scope: "punctuation.definition.string.begin.json",
    }];
    let mut cursor = start + 1;
    let mut content_start = cursor;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                if content_start < cursor {
                    spans.push(ScopeSpan {
                        range: content_start..cursor,
                        scope: "string.quoted.double.json",
                    });
                }
                let end = next_char_boundary(line, next_char_boundary(line, cursor));
                spans.push(ScopeSpan {
                    range: cursor..end,
                    scope: "constant.character.escape.json",
                });
                cursor = end;
                content_start = cursor;
            }
            b'"' => {
                if content_start < cursor {
                    spans.push(ScopeSpan {
                        range: content_start..cursor,
                        scope: "string.quoted.double.json",
                    });
                }
                spans.push(ScopeSpan {
                    range: cursor..cursor + 1,
                    scope: "punctuation.definition.string.end.json",
                });
                return spans;
            }
            _ => cursor = next_char_boundary(line, cursor),
        }
    }
    if content_start < line.len() {
        spans.push(ScopeSpan {
            range: content_start..line.len(),
            scope: "string.quoted.double.json",
        });
    }
    spans
}

fn scope_name(grammar: &CompiledGrammar, id: Option<super::state::ScopeId>) -> Option<String> {
    id.and_then(|id| grammar.scope(id).map(str::to_owned))
}

fn substitute_scope_text(scope: &str, line: &str, captures: &[Option<Range<usize>>]) -> String {
    if !scope.contains('$') {
        return scope.to_owned();
    }
    let mut output = String::with_capacity(scope.len());
    let bytes = scope.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let ch = scope[index..].chars().next().expect("valid scope char");
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'{' {
            if let Some(close_offset) = scope[index + 2..].find('}') {
                let body_start = index + 2;
                let body_end = body_start + close_offset;
                let body = &scope[body_start..body_end];
                if let Some((group, transform)) = parse_scope_placeholder_body(body) {
                    push_scope_capture(&mut output, line, captures, group, transform);
                    index = body_end + 1;
                    continue;
                }
            }
        } else if index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(group) = scope[index + 1..end].parse::<usize>() {
                push_scope_capture(&mut output, line, captures, group, ScopeTransform::None);
                index = end;
                continue;
            }
        }
        output.push('$');
        index += 1;
    }
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeTransform {
    None,
    Downcase,
    Upcase,
}

fn parse_scope_placeholder_body(body: &str) -> Option<(usize, ScopeTransform)> {
    let (group, transform) = body.split_once(':').unwrap_or((body, ""));
    let group = group.parse::<usize>().ok()?;
    let transform = match transform {
        "" => ScopeTransform::None,
        "/downcase" => ScopeTransform::Downcase,
        "/upcase" => ScopeTransform::Upcase,
        _ => return None,
    };
    Some((group, transform))
}

fn push_scope_capture(
    output: &mut String,
    line: &str,
    captures: &[Option<Range<usize>>],
    group: usize,
    transform: ScopeTransform,
) {
    let Some(text) = captures
        .get(group)
        .and_then(|range| range.as_ref())
        .and_then(|range| line.get(range.clone()))
    else {
        return;
    };
    match transform {
        ScopeTransform::None => output.push_str(text),
        ScopeTransform::Downcase => output.push_str(&text.to_lowercase()),
        ScopeTransform::Upcase => output.push_str(&text.to_uppercase()),
    }
}

fn push_scope_once(stack: &mut Vec<String>, scope: String) {
    for scope in scope.split_whitespace() {
        if stack.last().is_none_or(|last| last != scope) {
            stack.push(scope.to_owned());
        }
    }
}

fn fallback_call_budget(source_bytes: usize) -> u64 {
    MIN_FALLBACK_STEPS_PER_CALL.max(
        u64::try_from(source_bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(FALLBACK_STEPS_PER_SOURCE_BYTE),
    )
}

fn push_scopes(stack: &mut Vec<String>, scopes: &str) {
    stack.extend(scopes.split_whitespace().filter_map(|scope| {
        if !scope.starts_with('.') && !scope.ends_with('.') && !scope.contains("..") {
            return Some(scope.to_owned());
        }
        let normalized = scope
            .split('.')
            .filter(|component| !component.is_empty())
            .collect::<Vec<_>>()
            .join(".");
        (!normalized.is_empty()).then_some(normalized)
    }));
}

fn specified_outside_capture_end(result: &MatchResult, captures: &CaptureSpec) -> usize {
    if result.start == result.end {
        return result.end;
    }
    captures
        .entries
        .iter()
        .filter(|(_, entry)| entry.name.is_some() || !entry.patterns.is_empty())
        .filter_map(|(group, _)| {
            result
                .captures
                .get(*group as usize)
                .and_then(Option::as_ref)
                .filter(|range| range.start >= result.end)
                .map(|range| range.end)
        })
        .fold(result.end, usize::max)
}

fn plain_scoped_tokens(parse_text: &str, scopes: Vec<String>) -> Vec<ScopedToken> {
    if parse_text.is_empty() {
        Vec::new()
    } else {
        vec![ScopedToken {
            range: 0..parse_text.len(),
            scopes,
        }]
    }
}

fn push_segment(
    segments: &mut Vec<SyntaxSegment>,
    start: usize,
    end: usize,
    class: Option<SyntaxClass>,
) {
    if start >= end {
        return;
    }
    if let Some(last) = segments.last_mut()
        && last.class == class
        && last.byte_end == start
    {
        last.byte_end = end;
        return;
    }
    segments.push(SyntaxSegment::new(start, end, class));
}

fn clamp_range(range: Range<usize>, parent: Range<usize>) -> Range<usize> {
    range.start.max(parent.start)..range.end.min(parent.end)
}

fn contains_range(parent: &Range<usize>, child: &Range<usize>) -> bool {
    parent.start <= child.start && child.end <= parent.end
}

fn direct_children(parent: Range<usize>, items: &[CaptureItem]) -> Vec<usize> {
    let mut direct = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if !contains_range(&parent, &item.range) || item.range == parent {
            continue;
        }
        let contained_by_other = items.iter().enumerate().any(|(other_index, other)| {
            other_index != index
                && contains_range(&parent, &other.range)
                && contains_range(&other.range, &item.range)
                && (other.range != item.range || other.group < item.group)
        });
        if !contained_by_other {
            direct.push(index);
        }
    }
    direct
}

fn selector_matches(selector: &str, stack: &[String]) -> bool {
    let tokens = tokenize_selector(selector);
    if tokens.is_empty() {
        return false;
    }
    let mut parser = SelectorParser {
        tokens: &tokens,
        index: 0,
        stack,
    };
    parser.parse_expression()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectorToken {
    Word(String),
    LeftParen,
    RightParen,
    Or,
    Not,
}

fn tokenize_selector(selector: &str) -> Vec<SelectorToken> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let flush_word = |word: &mut String, tokens: &mut Vec<SelectorToken>| {
        if !word.is_empty() {
            tokens.push(SelectorToken::Word(std::mem::take(word)));
        }
    };
    for ch in selector.chars() {
        match ch {
            '(' => {
                flush_word(&mut word, &mut tokens);
                tokens.push(SelectorToken::LeftParen);
            }
            ')' => {
                flush_word(&mut word, &mut tokens);
                tokens.push(SelectorToken::RightParen);
            }
            ',' | '|' => {
                flush_word(&mut word, &mut tokens);
                tokens.push(SelectorToken::Or);
            }
            '&' => flush_word(&mut word, &mut tokens),
            '-' if word.is_empty() => {
                flush_word(&mut word, &mut tokens);
                tokens.push(SelectorToken::Not);
            }
            ch if ch.is_whitespace() => flush_word(&mut word, &mut tokens),
            ch => word.push(ch),
        }
    }
    flush_word(&mut word, &mut tokens);
    tokens
}

struct SelectorParser<'a> {
    tokens: &'a [SelectorToken],
    index: usize,
    stack: &'a [String],
}

impl SelectorParser<'_> {
    fn parse_expression(&mut self) -> bool {
        self.parse_or()
    }

    fn parse_or(&mut self) -> bool {
        let mut value = self.parse_and();
        while self.consume_or() {
            value |= self.parse_and();
        }
        value
    }

    fn parse_and(&mut self) -> bool {
        let mut saw_term = false;
        let mut value = true;
        while self.index < self.tokens.len() {
            if matches!(
                self.tokens[self.index],
                SelectorToken::Or | SelectorToken::RightParen
            ) {
                break;
            }
            saw_term = true;
            value &= self.parse_unary();
        }
        saw_term && value
    }

    fn parse_unary(&mut self) -> bool {
        if matches!(self.tokens.get(self.index), Some(SelectorToken::Not)) {
            self.index += 1;
            return !self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> bool {
        match self.tokens.get(self.index) {
            Some(SelectorToken::Word(word)) => {
                self.index += 1;
                scope_path_matches(word, self.stack)
            }
            Some(SelectorToken::LeftParen) => {
                self.index += 1;
                let value = self.parse_expression();
                if matches!(self.tokens.get(self.index), Some(SelectorToken::RightParen)) {
                    self.index += 1;
                }
                value
            }
            Some(SelectorToken::RightParen | SelectorToken::Or) | None => false,
            Some(SelectorToken::Not) => unreachable!("parse_unary handles negation"),
        }
    }

    fn consume_or(&mut self) -> bool {
        if matches!(self.tokens.get(self.index), Some(SelectorToken::Or)) {
            self.index += 1;
            true
        } else {
            false
        }
    }
}

fn scope_path_matches(path: &str, stack: &[String]) -> bool {
    let mut next_index = 0usize;
    for component in path.split_whitespace() {
        let Some(index) = stack[next_index..]
            .iter()
            .position(|scope| scope_component_matches(component, scope))
        else {
            return false;
        };
        next_index += index + 1;
    }
    true
}

fn scope_component_matches(component: &str, scope: &str) -> bool {
    if component.contains('*') {
        return wildcard_scope_component_matches(component, scope);
    }
    scope == component
        || scope
            .strip_prefix(component)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn wildcard_scope_component_matches(component: &str, scope: &str) -> bool {
    let component_parts = component.split('.').collect::<Vec<_>>();
    let scope_parts = scope.split('.').collect::<Vec<_>>();
    if component_parts.len() > scope_parts.len() {
        return false;
    }
    component_parts
        .iter()
        .zip(scope_parts.iter())
        .all(|(component, scope)| *component == "*" || component == scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_placeholder_line_without_copying_text() {
        let mut tokenizer = Tokenizer::new();
        let tokens = tokenizer.tokenize_line("let π = 1;", StateId(7));
        assert_eq!(tokens.exit, StateId(7));
        assert_eq!(tokens.tokens[0].0, 0..11);
    }

    #[test]
    fn zero_width_advance_stays_on_char_boundary() {
        assert_eq!(advance_zero_width("π", &(0..0)), 2);
    }

    #[test]
    fn json_string_smoke_matches_migration_worked_example() {
        let spans = tokenize_json_string_smoke(r#""a\n""#);
        assert_eq!(
            spans,
            vec![
                ScopeSpan {
                    range: 0..1,
                    scope: "punctuation.definition.string.begin.json",
                },
                ScopeSpan {
                    range: 1..2,
                    scope: "string.quoted.double.json",
                },
                ScopeSpan {
                    range: 2..4,
                    scope: "constant.character.escape.json",
                },
                ScopeSpan {
                    range: 4..5,
                    scope: "punctuation.definition.string.end.json",
                },
            ]
        );
    }

    #[test]
    fn text_start_anchor_only_matches_document_first_line() {
        let grammar = r##"{
            "scopeName": "source.anchor-a",
            "patterns": [
                {"match":"\\Afoo", "name":"keyword.anchor-a"},
                {"match":"foo", "name":"identifier.anchor-a"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let first = tokenizer.tokenize_line_scopes_at_line("foo", TokenizerState::default(), 0);
        let second = tokenizer.tokenize_line_scopes_at_line("foo", TokenizerState::default(), 1);

        assert!(line_has_scope(&first, "keyword.anchor-a"), "{first:#?}");
        assert!(!line_has_scope(&second, "keyword.anchor-a"), "{second:#?}");
        assert!(
            line_has_scope(&second, "identifier.anchor-a"),
            "{second:#?}"
        );
    }

    #[test]
    fn continuation_anchor_is_invalid_at_fresh_line_start() {
        let grammar = r##"{
            "scopeName": "source.anchor-g",
            "patterns": [
                {"match":"\\Gfoo", "name":"keyword.anchor-g"},
                {"match":"foo", "name":"identifier.anchor-g"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes_at_line("foo", TokenizerState::default(), 0);

        assert!(!line_has_scope(&line, "keyword.anchor-g"), "{line:#?}");
        assert!(line_has_scope(&line, "identifier.anchor-g"), "{line:#?}");
    }

    #[test]
    fn tokenizes_json_with_real_grammar() {
        let mut tokenizer = TextMateTokenizer::from_grammar(include_str!(
            "../../../../assets/tm-grammars/languages/json.tmLanguage.json"
        ))
        .unwrap();
        let line = tokenizer.tokenize_line_scopes("{\"ok\": true}", TokenizerState::default());
        assert!(line.tokens.iter().any(|token| token.scopes.len() > 1));
        assert!(line.tokens.iter().any(|token| {
            token
                .scopes
                .iter()
                .any(|scope| scope.contains("constant.language.json"))
        }));
    }

    #[test]
    fn opt_in_counters_record_line_and_regex_attempts() {
        let grammar = r##"{
            "scopeName": "source.counters",
            "patterns": [{"match":"x", "name":"keyword.counter"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        assert_eq!(tokenizer.counters(), EngineCounters::default());

        tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        assert_eq!(tokenizer.counters(), EngineCounters::default());

        tokenizer.set_counters_enabled(true);
        tokenizer.set_hot_counters_enabled(true);
        tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        let counters = tokenizer.counters();
        assert_eq!(counters.lines_tokenized, 1);
        assert!(counters.regex_dfa_attempts > 0, "{counters:#?}");
        assert_eq!(counters.pattern_hotspots.len(), 1, "{counters:#?}");
        assert_eq!(counters.pattern_hotspots[0].root_scope, "source.counters");
        assert_eq!(counters.pattern_hotspots[0].pattern, "x");
        assert_eq!(counters.pattern_hotspots[0].engine, "dfa");
        assert_eq!(counters.pattern_hotspots[0].attempts, 1);
        assert_eq!(counters.pattern_hotspots[0].matches, 1);

        let taken = tokenizer.take_counters();
        assert_eq!(taken.lines_tokenized, 1);
        assert_eq!(taken.pattern_hotspots.len(), 1, "{taken:#?}");
        assert_eq!(tokenizer.counters(), EngineCounters::default());
    }

    #[test]
    fn counters_record_fallback_budget_kills_as_degraded_lines() {
        let grammar = r##"{
            "scopeName": "source.counter-budget",
            "patterns": [
                {"match":"(?=(a+)+b)(a+)+b", "name":"invalid.counter-budget"},
                {"match":"ok", "name":"keyword.counter-budget"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);
        let line = format!("{} ok", "a".repeat(256));
        tokenizer.tokenize_line_scopes(&line, TokenizerState::default());

        let counters = tokenizer.counters();
        assert!(counters.regex_fallback_attempts > 0, "{counters:#?}");
        assert!(counters.fallback_steps_total > 0, "{counters:#?}");
        assert!(counters.fallback_budget_kills > 0, "{counters:#?}");
        assert_eq!(counters.degraded_lines, 1, "{counters:#?}");
    }

    #[test]
    fn state_interner_assigns_stable_ids_across_replay() {
        let grammar = r##"{
            "scopeName": "source.state-counter",
            "patterns": [{"begin":"/\\*", "end":"\\*/", "name":"comment.block.state-counter"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        assert_eq!(tokenizer.interned_state_count(), 1);
        assert_eq!(
            tokenizer.intern_state(TokenizerState::default()),
            StateId(0)
        );

        let first = tokenizer.tokenize_line_scopes("/* open", TokenizerState::default());
        assert_eq!(first.entry_state_id, StateId(0));
        assert_eq!(
            tokenizer.intern_state(first.state.clone()),
            first.exit_state_id
        );
        assert_eq!(
            tokenizer.state_for_id(first.exit_state_id),
            Some(&first.state)
        );

        let second = tokenizer.tokenize_line_scopes("inside", first.state.clone());
        assert_eq!(second.entry_state_id, first.exit_state_id);

        let replay = tokenizer.tokenize_line_scopes("inside", first.state);
        assert_eq!(replay.entry_state_id, first.exit_state_id);
        assert_eq!(replay.exit_state_id, second.exit_state_id);
    }

    #[test]
    fn counters_record_state_interner_hits_and_misses() {
        let grammar = r##"{
            "scopeName": "source.state-counters",
            "patterns": [{"begin":"/\\*", "end":"\\*/", "name":"comment.block.state-counters"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        let first = tokenizer.tokenize_line_scopes("/* open", TokenizerState::default());
        let after_first = tokenizer.counters();
        assert!(after_first.state_cache_hits >= 1, "{after_first:#?}");
        assert!(after_first.state_cache_misses >= 1, "{after_first:#?}");

        tokenizer.tokenize_line_scopes("inside", first.state);
        let after_second = tokenizer.counters();
        assert!(
            after_second.state_cache_hits > after_first.state_cache_hits,
            "before={after_first:#?} after={after_second:#?}"
        );
    }

    #[test]
    fn line_cache_reuses_same_entry_state_and_line() {
        let grammar = r##"{
            "scopeName": "source.line-cache",
            "patterns": [{"match":"x", "name":"keyword.line-cache"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_line_cache_capacity(8);
        tokenizer.set_counters_enabled(true);

        let first = tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        let second = tokenizer.tokenize_line_scopes("x", TokenizerState::default());

        assert_eq!(first.tokens, second.tokens);
        assert_eq!(second.entry_state_id, StateId(0));
        assert_eq!(tokenizer.line_cache_len(), 1);
        let counters = tokenizer.counters();
        assert_eq!(counters.line_cache_misses, 1, "{counters:#?}");
        assert_eq!(counters.line_cache_hits, 1, "{counters:#?}");
    }

    #[test]
    fn line_cache_key_includes_entry_state() {
        let grammar = r##"{
            "scopeName": "source.line-cache-state",
            "patterns": [{"begin":"/\\*", "end":"\\*/", "name":"comment.block.line-cache-state"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_line_cache_capacity(8);
        tokenizer.set_counters_enabled(true);

        let first = tokenizer.tokenize_line_scopes("/* open", TokenizerState::default());
        tokenizer.tokenize_line_scopes("inside", first.state.clone());
        tokenizer.tokenize_line_scopes("inside", first.state);

        let counters = tokenizer.counters();
        assert_eq!(counters.line_cache_misses, 2, "{counters:#?}");
        assert_eq!(counters.line_cache_hits, 1, "{counters:#?}");
    }

    #[test]
    fn line_cache_evicts_oldest_entry() {
        let grammar = r##"{
            "scopeName": "source.line-cache-evict",
            "patterns": [{"match":"x|y", "name":"keyword.line-cache-evict"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_line_cache_capacity(1);
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        tokenizer.tokenize_line_scopes("y", TokenizerState::default());
        tokenizer.tokenize_line_scopes("x", TokenizerState::default());

        assert_eq!(tokenizer.line_cache_len(), 1);
        let counters = tokenizer.counters();
        assert_eq!(counters.line_cache_hits, 0, "{counters:#?}");
        assert_eq!(counters.line_cache_misses, 3, "{counters:#?}");
        assert_eq!(counters.line_cache_evictions, 2, "{counters:#?}");
    }

    #[test]
    fn checkpoint_viewport_replay_matches_replay_from_zero() {
        let grammar = r##"{
            "scopeName": "source.checkpoint-engine",
            "patterns": [
                {"begin":"/\\*", "end":"\\*/", "name":"comment.block.checkpoint-engine"},
                {"match":"\\b(let|return)\\b", "name":"keyword.control.checkpoint-engine"}
            ]
        }"##;
        let source = [
            "let before = 1;",
            "/* comment starts",
            "still in comment",
            "ends */ let after = 2;",
            "return after;",
        ]
        .join("\n");

        let mut full = TextMateTokenizer::from_grammar(grammar).unwrap();
        let mut state = TokenizerState::default();
        let mut full_lines = Vec::new();
        for (line_index, chunk) in LineChunks::new(&source).enumerate() {
            let tokenized = full.tokenize_line_scopes_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            full_lines.push(tokenized);
        }

        let mut viewport = TextMateTokenizer::from_grammar(grammar).unwrap();
        viewport.set_counters_enabled(true);
        let mut checkpoints = crate::engine::checkpoint::CheckpointTable::new(2);

        let first = viewport.tokenize_viewport_scopes(&source, 0..2, &mut checkpoints);
        assert_eq!(first.len(), 2);
        assert!(
            checkpoints
                .nearest_before(3)
                .is_some_and(|checkpoint| checkpoint.line_index == 2)
        );

        let replayed = viewport.tokenize_viewport_scopes(&source, 3..5, &mut checkpoints);
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].tokens, full_lines[3].tokens);
        assert_eq!(replayed[1].tokens, full_lines[4].tokens);

        let counters = viewport.counters();
        assert_eq!(counters.checkpoint_replay_lines, 1, "{counters:#?}");
    }

    #[test]
    fn checkpoint_with_unknown_state_replays_from_zero() {
        let grammar = r##"{
            "scopeName": "source.checkpoint-missing",
            "patterns": [
                {"begin":"/\\*", "end":"\\*/", "name":"comment.block.checkpoint-missing"},
                {"match":"\\breturn\\b", "name":"keyword.control.checkpoint-missing"}
            ]
        }"##;
        let source = ["/* open", "still", "ends */", "return ok;"].join("\n");

        let mut full = TextMateTokenizer::from_grammar(grammar).unwrap();
        let mut state = TokenizerState::default();
        let mut full_lines = Vec::new();
        for (line_index, chunk) in LineChunks::new(&source).enumerate() {
            let tokenized = full.tokenize_line_scopes_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            full_lines.push(tokenized);
        }

        let mut viewport = TextMateTokenizer::from_grammar(grammar).unwrap();
        viewport.set_counters_enabled(true);
        let mut checkpoints = crate::engine::checkpoint::CheckpointTable::new(2);
        checkpoints.record(2, StateId(999));

        let replayed = viewport.tokenize_viewport_scopes(&source, 3..4, &mut checkpoints);
        assert_eq!(replayed[0].tokens, full_lines[3].tokens);
        let counters = viewport.counters();
        assert_eq!(counters.checkpoint_replay_lines, 3, "{counters:#?}");
    }

    #[test]
    fn candidate_cache_reuses_state_across_lines_without_reprobing_within_a_line() {
        let grammar = r##"{
            "scopeName": "source.candidate-cache",
            "patterns": [
                {"match":"x", "name":"keyword.x.candidate-cache"},
                {"match":"y", "name":"keyword.y.candidate-cache"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("x y", TokenizerState::default());
        tokenizer.tokenize_line_scopes("x y", TokenizerState::default());

        assert_eq!(tokenizer.candidate_cache_len(), 1);
        let counters = tokenizer.counters();
        assert_eq!(counters.candidate_list_cache_misses, 1, "{counters:#?}");
        assert_eq!(counters.candidate_list_cache_hits, 1, "{counters:#?}");
    }

    #[test]
    fn candidate_cache_key_includes_dynamic_end_state() {
        let grammar = r##"{
            "scopeName": "source.candidate-cache-end",
            "patterns": [
                {"begin":"/\\*", "end":"\\*/", "name":"comment.block.candidate-cache-end"},
                {"match":"x", "name":"keyword.x.candidate-cache-end"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        let first = tokenizer.tokenize_line_scopes("/* open", TokenizerState::default());
        tokenizer.tokenize_line_scopes("inside */ x", first.state);

        assert!(tokenizer.candidate_cache_len() >= 2);
        let counters = tokenizer.counters();
        assert!(counters.candidate_list_cache_misses >= 2, "{counters:#?}");
    }

    #[test]
    fn candidate_cache_distinguishes_same_length_dynamic_end_patterns() {
        let grammar = r##"{
            "scopeName": "source.candidate-cache-dynamic-end",
            "patterns": [
                {"begin":"^<<([A-Z]{3})$", "end":"^\\1$", "name":"string.heredoc.candidate-cache-dynamic-end"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        let foo = tokenizer.tokenize_line_scopes("<<FOO", TokenizerState::default());
        let bar = tokenizer.tokenize_line_scopes("<<BAR", TokenizerState::default());
        assert_ne!(foo.exit_state_id, bar.exit_state_id);

        tokenizer.tokenize_line_scopes("body", foo.state);
        tokenizer.tokenize_line_scopes("body", bar.state);

        assert!(tokenizer.candidate_cache_len() >= 3);
        let counters = tokenizer.counters();
        assert!(counters.candidate_list_cache_misses >= 3, "{counters:#?}");
    }

    #[test]
    fn candidate_cache_builds_multi_pattern_set_search() {
        let grammar = r##"{
            "scopeName": "source.candidate-dfa",
            "patterns": [
                {"match":"alpha", "name":"keyword.alpha.candidate-dfa"},
                {"match":"beta", "name":"keyword.beta.candidate-dfa"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.tokenize_line_scopes("beta", TokenizerState::default());

        let set = tokenizer
            .candidate_cache
            .get(&StateId(0))
            .expect("initial state candidates should be cached");
        assert!(set.pattern_set_search.is_some());
    }

    #[test]
    fn capture_replay_is_skipped_only_for_static_capture_free_matches() {
        let candidate = |name: &str| Candidate {
            order: 0,
            base_grammar_id: GrammarId(0),
            pattern: "pattern".to_owned(),
            scope_prefix: None,
            kind: CandidateKind::Match {
                grammar_id: GrammarId(0),
                name: Some(name.to_owned()),
                captures: CaptureSpec::default(),
            },
        };

        assert!(!candidate_requires_capture_replay(&candidate(
            "keyword.static"
        )));
        assert!(candidate_requires_capture_replay(&candidate(
            "keyword.dynamic.$1"
        )));
    }

    #[test]
    fn multi_pattern_dfa_preserves_candidate_order_tie_break() {
        let grammar = r##"{
            "scopeName": "source.candidate-dfa-order",
            "patterns": [
                {"match":"ab", "name":"keyword.long.candidate-dfa-order"},
                {"match":"a", "name":"keyword.short.candidate-dfa-order"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes("ab", TokenizerState::default());

        assert!(
            line.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "keyword.long.candidate-dfa-order"),
            "{:#?}",
            line.tokens
        );
    }

    #[test]
    fn fallback_candidates_can_beat_later_dfa_candidates() {
        let grammar = r##"{
            "scopeName": "source.candidate-fallback-order",
            "patterns": [
                {"match":"(?=a)a", "name":"keyword.fallback.candidate-fallback-order"},
                {"match":"a", "name":"keyword.dfa.candidate-fallback-order"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes("a", TokenizerState::default());

        assert!(
            line.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "keyword.fallback.candidate-fallback-order"),
            "{:#?}",
            line.tokens
        );
    }

    #[test]
    fn counters_record_prefilter_hits_and_skips() {
        let grammar = r##"{
            "scopeName": "source.prefilter-counters",
            "patterns": [{"match":"z+", "name":"keyword.prefilter-counters"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("abc", TokenizerState::default());
        tokenizer.tokenize_line_scopes("zz", TokenizerState::default());

        let counters = tokenizer.counters();
        assert!(counters.prefilter_checks >= 2, "{counters:#?}");
        assert!(counters.prefilter_skips >= 1, "{counters:#?}");
        assert!(counters.prefilter_hits >= 1, "{counters:#?}");
    }

    #[test]
    fn line_byte_limit_degrades_only_that_line() {
        let grammar = r##"{
            "scopeName": "source.line-limit",
            "patterns": [{"match":"ok", "name":"keyword.line-limit"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);
        tokenizer.set_max_line_bytes(Some(4));

        let long = tokenizer.tokenize_line_scopes("too long", TokenizerState::default());
        let short = tokenizer.tokenize_line_scopes("ok", TokenizerState::default());

        assert_eq!(long.tokens.len(), 1);
        assert_eq!(long.tokens[0].range, 0..8);
        assert!(
            short.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "keyword.line-limit"),
            "{:#?}",
            short.tokens
        );
        let counters = tokenizer.counters();
        assert_eq!(counters.lines_skipped, 1, "{counters:#?}");
        assert_eq!(counters.degraded_lines, 1, "{counters:#?}");
    }

    #[test]
    fn applies_capture_zero_scope() {
        let grammar = r##"{
            "scopeName": "source.fixture",
            "patterns": [{"match":"x", "captures":{"0":{"name":"punctuation.fixture"}}}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        assert!(
            line.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "punctuation.fixture"),
            "{:#?}",
            line.tokens
        );
    }

    #[test]
    fn substitutes_capture_text_in_scope_names() {
        let grammar = r##"{
            "scopeName": "source.dynamic-scope",
            "patterns": [
                {
                    "match":"^(#)([A-Z]+)",
                    "name":"meta.directive.${2:/downcase}.dynamic-scope",
                    "captures": {
                        "2": {"name":"keyword.control.directive.$2.dynamic-scope"}
                    }
                }
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes("#INCLUDE", TokenizerState::default());
        let scopes = line
            .tokens
            .iter()
            .flat_map(|token| token.scopes.iter())
            .collect::<Vec<_>>();
        assert!(
            scopes
                .iter()
                .any(|scope| *scope == "meta.directive.include.dynamic-scope"),
            "{scopes:#?}"
        );
        assert!(
            scopes
                .iter()
                .any(|scope| *scope == "keyword.control.directive.INCLUDE.dynamic-scope"),
            "{scopes:#?}"
        );
    }

    #[test]
    fn begin_end_state_crosses_lines() {
        let grammar = r##"{
            "scopeName": "source.fixture",
            "patterns": [{"begin":"/\\*", "end":"\\*/", "name":"comment.block.fixture"}]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let first = tokenizer.tokenize_line_scopes("/* hello", TokenizerState::default());
        assert_eq!(first.state.depth(), 1);
        let second = tokenizer.tokenize_line_scopes("done */", first.state);
        assert!(second.state.is_initial());
        assert!(
            second.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "comment.block.fixture")
        );
    }

    #[test]
    fn tokenize_source_produces_shape_compatible_highlighted_text() {
        let mut tokenizer = TextMateTokenizer::from_grammar(include_str!(
            "../../../../assets/tm-grammars/languages/json.tmLanguage.json"
        ))
        .unwrap();
        let highlighted = tokenizer.tokenize_source("{\"ok\": true}\n");
        assert_eq!(highlighted.lines.len(), 2);
        assert!(highlighted.lines[0].matches_text("{\"ok\": true}"));
        assert!(highlighted.lines[1].matches_text(""));
        assert!(
            highlighted.lines[0]
                .segments
                .iter()
                .all(|segment| segment.byte_start < segment.byte_end)
        );
    }

    fn core_tokenizer(language: &str) -> TextMateTokenizer {
        let mut set = GrammarSet::new();
        let mut root = None;
        for asset in crate::grammars::registry::CORE_ASSETS {
            let id = set.load_and_add(asset.source).unwrap();
            if asset.language == language {
                root = Some(id);
            }
        }
        TextMateTokenizer::new(set, root.expect("root language"))
    }

    #[test]
    fn html_script_uses_external_javascript_scope() {
        let mut tokenizer = core_tokenizer("html");
        let line = tokenizer
            .tokenize_line_scopes("<script>let x = 1;</script>", TokenizerState::default());
        assert!(
            line.tokens
                .iter()
                .any(|token| token.scopes.iter().any(|scope| scope == "source.js")),
            "{:#?}",
            line.tokens
        );
    }

    #[test]
    fn core_fixture_languages_tokenize_without_panics() {
        let mut set = GrammarSet::new();
        for asset in crate::grammars::registry::CORE_ASSETS {
            set.load_and_add(asset.source).unwrap();
        }
        let cases = [
            ("rust", "fn main() { println!(\"hi\"); }"),
            ("typescript", "const value: number = 1;"),
            ("json", "{\"ok\": true}"),
            ("yaml", "ok: true"),
            ("toml", "name = \"mark\""),
            ("markdown", "# title"),
            ("html", "<div class=\"x\">hi</div>"),
            ("css", ".x { color: red; }"),
            ("python", "def f(x): return x + 1"),
            ("go", "func main() { println(1) }"),
            ("c", "int main(void) { return 0; }"),
            ("cpp", "auto value = std::string{};"),
            ("bash", "echo $(pwd)"),
        ];
        for (language, source) in cases {
            let asset = crate::grammars::registry::GrammarRegistry::asset(language).unwrap();
            let root = set.grammar_id_by_scope(asset.scope_name).unwrap();
            let mut tokenizer = TextMateTokenizer::new(set.clone(), root);
            let line = tokenizer.tokenize_line_scopes(source, TokenizerState::default());
            assert!(!line.tokens.is_empty(), "{language} should emit tokens");
            assert!(line.tokens.iter().all(|token| {
                source.is_char_boundary(token.range.start.min(source.len()))
                    && source.is_char_boundary(token.range.end.min(source.len()))
            }));
        }
    }

    #[test]
    fn markdown_fence_uses_external_rust_scope() {
        let mut tokenizer = core_tokenizer("markdown");
        let first = tokenizer.tokenize_line_scopes("```rust", TokenizerState::default());
        let second = tokenizer.tokenize_line_scopes("fn main() {}", first.state);
        assert!(
            second.tokens.iter().any(|token| {
                token
                    .scopes
                    .iter()
                    .any(|scope| scope.contains("embedded.block.rust"))
            }),
            "{:#?}",
            second.tokens
        );
    }

    #[test]
    fn selector_prefix_matches_dot_boundary() {
        let stack = vec!["text.html.markdown".to_owned(), "markup.raw".to_owned()];
        assert!(selector_matches("text.html markup.raw", &stack));
        assert!(!selector_matches("text.htmlx", &stack));
    }

    #[test]
    fn selector_matches_grouped_or_and_subtractions() {
        let stack = vec![
            "text.html.markdown".to_owned(),
            "meta.script.svelte".to_owned(),
            "meta.lang.ts".to_owned(),
        ];
        assert!(selector_matches(
            "(meta.script.svelte | meta.style.svelte) (meta.lang.js | meta.lang.ts)",
            &stack
        ));
        assert!(selector_matches("source.js, meta.lang.ts", &stack));
        assert!(!selector_matches(
            "meta.script.svelte - (meta.lang.ts | comment.block)",
            &stack
        ));
        assert!(selector_matches(
            "meta.script.svelte - (meta.lang.js | comment.block)",
            &stack
        ));

        let html_stack = vec![
            "text.html.basic".to_owned(),
            "meta.tag.script.begin.html".to_owned(),
        ];
        assert!(selector_matches("meta.tag.*.*.html", &html_stack));
        assert!(!selector_matches(
            "text.html - (meta.tag.*.*.html)",
            &html_stack
        ));
    }

    #[test]
    fn grammar_set_validates_external_include_graph() {
        let host = r##"{
            "scopeName": "source.host",
            "patterns": [{"include":"source.external#value"}]
        }"##;
        let external = r##"{
            "scopeName": "source.external",
            "repository": {"value": {"match":"ok", "name":"keyword.external"}}
        }"##;
        let mut set = GrammarSet::new();
        set.load_and_add(host).unwrap();
        set.load_and_add(external).unwrap();
        set.validate_include_graph().unwrap();

        let mut missing = GrammarSet::new();
        missing.load_and_add(host).unwrap();
        let error = missing.validate_include_graph().unwrap_err().to_string();
        assert!(error.contains("source.external"), "{error}");
    }

    #[test]
    fn base_include_resolves_to_including_grammar() {
        let host = r##"{
            "scopeName": "source.host",
            "patterns": [
                {"match":"hostword", "name":"keyword.host"},
                {"include":"source.external#entry"}
            ]
        }"##;
        let external = r##"{
            "scopeName": "source.external",
            "repository": {"entry": {"patterns": [{"include":"$base"}]}}
        }"##;
        let mut set = GrammarSet::new();
        let root = set.load_and_add(host).unwrap();
        set.load_and_add(external).unwrap();
        let mut tokenizer = TextMateTokenizer::new(set, root);
        let line = tokenizer.tokenize_line_scopes("hostword", TokenizerState::default());
        assert!(
            line.tokens
                .iter()
                .any(|token| { token.scopes.iter().any(|scope| scope == "keyword.host") })
        );
    }

    fn line_has_scope(line: &TokenizedLine, expected: &str) -> bool {
        line.tokens
            .iter()
            .any(|token| token.scopes.iter().any(|scope| scope == expected))
    }
}
