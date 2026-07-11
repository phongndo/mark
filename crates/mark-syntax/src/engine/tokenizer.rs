use std::{
    collections::HashMap,
    collections::HashSet,
    fs::OpenOptions,
    hash::{BuildHasherDefault, Hash, Hasher},
    io::Write,
    ops::{Deref, Range},
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
use super::regex::{
    AnchorContext, CompiledPattern, FallbackError, MatchResult, PatternSetMatcher, RegexMatcher,
};
use super::scopes::{ScopeInterner, ScopeStackInterner, ScopeTemplateId, ScopeTemplateInterner};
use super::state::{GrammarId, LineTokens, PatternId, RuleId, ScopeId, ScopeStackId, StateId};

const MAX_INCLUDE_DEPTH: usize = 128;
const MAX_TOKENIZER_STEPS_PER_LINE: usize = 20_000;
const MAX_FALLBACK_STEPS_PER_LINE: u64 = 2_000_000;
const MIN_FALLBACK_STEPS_PER_CALL: u64 = 10_000_000;
const FALLBACK_STEPS_PER_SOURCE_BYTE: u64 = 512;
const MAX_SUBSTITUTED_END_PATTERN_LEN: usize = 4096;
const MAX_DYNAMIC_MATCHERS: usize = 512;
const MAX_INLINE_CANDIDATE_SETS: usize = 1024;
const MAX_CANDIDATE_SETS: usize = 4096;
const MAX_CANDIDATE_BLUEPRINTS: usize = 1024;
const MAX_INJECTION_OUTCOMES: usize = 1024;
const MAX_SCOPE_STACK_CACHE_ENTRIES: usize = 8192;

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
pub(crate) struct CompactScopedToken {
    pub(crate) range: Range<usize>,
    pub(crate) stack: ScopeStackId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizedLine {
    pub tokens: Arc<[ScopedToken]>,
    pub state: TokenizerState,
    pub entry_state_id: StateId,
    pub exit_state_id: StateId,
}

#[derive(Debug, Clone)]
struct CompactTokenizedLine {
    tokens: CompactLineTokens,
    state: TokenizerState,
    entry_state_id: StateId,
    exit_state_id: StateId,
    parse_fingerprint: LineTextFingerprint,
}

#[derive(Debug, Clone)]
enum CompactLineTokens {
    Owned(Vec<CompactScopedToken>),
    Shared(Arc<[CompactScopedToken]>),
}

impl Deref for CompactLineTokens {
    type Target = [CompactScopedToken];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(tokens) => tokens,
            Self::Shared(tokens) => tokens,
        }
    }
}

impl From<Vec<CompactScopedToken>> for CompactLineTokens {
    fn from(tokens: Vec<CompactScopedToken>) -> Self {
        Self::Owned(tokens)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TokenizerState {
    // Parent-linked immutable chunks keep continuation updates bounded. Pushes
    // copy at most one 32-frame tail chunk instead of cloning every frame
    // pointer in a deep stack; a hash-consed stack id keeps equality exact and
    // O(1) even when equal states were built independently.
    frames: FrameStack,
    interner_hash: u64,
}

impl TokenizerState {
    pub fn is_initial(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    pub fn state_id(&self) -> StateId {
        StateId(
            self.frames
                .last()
                .map_or(0x811c9dc5, |frame| frame.state_hash),
        )
    }

    fn refresh_interner_hash(&mut self) {
        self.interner_hash = u64::from(self.frames.interned_id().0);
    }

    /// Pushes a frame while maintaining the per-frame identity hash and the
    /// cumulative stack hash in O(1), instead of re-hashing every frame on
    /// each state change (quadratic for deeply nested sources).
    fn push_frame(&mut self, mut frame: Frame) {
        frame.identity_hash = frame.compute_identity_hash();
        frame.stack_hash = fnv64_mix_u64(
            self.frames
                .last()
                .map_or(0xcbf2_9ce4_8422_2325, |parent| parent.stack_hash),
            frame.identity_hash,
        );
        let mut state_hash = self
            .frames
            .last()
            .map_or(0x811c9dc5, |parent| parent.state_hash);
        state_hash = fnv_mix(state_hash, u32::from(frame.grammar_id.0));
        state_hash = fnv_mix(state_hash, u32::from(frame.base_grammar_id.0));
        state_hash = fnv_mix(state_hash, frame.rule_id.0);
        state_hash = fnv_mix_opt_str(state_hash, frame.scope_prefix.as_deref());
        state_hash = fnv_mix_opt_str(state_hash, frame.name.as_deref());
        state_hash = fnv_mix_opt_str(state_hash, frame.content_name.as_deref());
        state_hash = fnv_mix_opt_str(state_hash, frame.end_pattern.as_deref());
        state_hash = fnv_mix_opt_str(state_hash, frame.while_pattern.as_deref());
        frame.state_hash = state_hash;
        frame.interned_stack_id = intern_frame_stack_node(self.frames.interned_id(), &frame);
        self.frames.push(frame);
        self.refresh_interner_hash();
    }

    fn pop_frame(&mut self) {
        self.frames.pop();
        self.refresh_interner_hash();
    }

    fn truncate_frames(&mut self, len: usize) {
        self.frames.truncate(len);
        self.refresh_interner_hash();
    }

    fn prefix(&self, len: usize) -> Self {
        let mut state = Self {
            frames: self.frames.prefix(len),
            interner_hash: 0,
        };
        state.refresh_interner_hash();
        state
    }
}

impl PartialEq for TokenizerState {
    fn eq(&self, other: &Self) -> bool {
        self.frames == other.frames
    }
}

impl Eq for TokenizerState {}

impl Hash for TokenizerState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.interner_hash);
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

fn fnv64_mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn fnv64_mix_u64(hash: u64, value: u64) -> u64 {
    fnv64_mix(hash, &value.to_le_bytes())
}

fn fnv64_mix_opt_str(hash: u64, value: Option<&str>) -> u64 {
    let hash = fnv64_mix_u64(hash, value.map_or(u64::MAX, |value| value.len() as u64));
    value.map_or(hash, |value| fnv64_mix(hash, value.as_bytes()))
}

#[derive(Debug, Clone)]
struct Frame {
    grammar_id: GrammarId,
    base_grammar_id: GrammarId,
    rule_id: RuleId,
    scope_prefix: Option<Arc<str>>,
    name: Option<Arc<str>>,
    content_name: Option<Arc<str>>,
    end_pattern: Option<Arc<str>>,
    end_pattern_id: Option<PatternId>,
    while_pattern: Option<Arc<str>>,
    while_pattern_id: Option<PatternId>,
    end_captures: Arc<CaptureSpec>,
    while_captures: Arc<CaptureSpec>,
    patterns: Arc<[RuleRef]>,
    apply_end_pattern_last: bool,
    begin_captured_eol: bool,
    /// Cached hash of this frame's identity fields; maintained by
    /// `TokenizerState::push_frame`.
    identity_hash: u64,
    /// Cumulative hash of the frame stack up to and including this frame.
    /// Prefix truncations therefore keep valid hashes without re-hashing.
    stack_hash: u64,
    /// Cumulative public `StateId` hash up to and including this frame.
    state_hash: u32,
    /// Exact hash-consed identity of the full frame stack ending at this
    /// frame. `TokenizerState` equality uses this id instead of walking every
    /// frame in deep continuations.
    interned_stack_id: InternedFrameStackId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
struct InternedFrameStackId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InternedFrameId(u32);

impl Frame {
    fn compute_identity_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        hash = fnv64_mix_u64(hash, u64::from(self.grammar_id.0));
        hash = fnv64_mix_u64(hash, u64::from(self.base_grammar_id.0));
        hash = fnv64_mix_u64(hash, u64::from(self.rule_id.0));
        hash = fnv64_mix_opt_str(hash, self.scope_prefix.as_deref());
        hash = fnv64_mix_opt_str(hash, self.name.as_deref());
        hash = fnv64_mix_opt_str(hash, self.content_name.as_deref());
        hash = fnv64_mix_opt_str(hash, self.end_pattern.as_deref());
        hash = fnv64_mix_u64(
            hash,
            self.end_pattern_id
                .map_or(u64::MAX, |pattern| u64::from(pattern.0)),
        );
        hash = fnv64_mix_opt_str(hash, self.while_pattern.as_deref());
        hash = fnv64_mix_u64(
            hash,
            self.while_pattern_id
                .map_or(u64::MAX, |pattern| u64::from(pattern.0)),
        );
        hash = fnv64_mix_u64(
            hash,
            u64::from(self.apply_end_pattern_last) | (u64::from(self.begin_captured_eol) << 1),
        );
        hash
    }
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.grammar_id == other.grammar_id
            && self.base_grammar_id == other.base_grammar_id
            && self.rule_id == other.rule_id
            && self.scope_prefix == other.scope_prefix
            && self.name == other.name
            && self.content_name == other.content_name
            && self.end_pattern == other.end_pattern
            && self.end_pattern_id == other.end_pattern_id
            && self.while_pattern == other.while_pattern
            && self.while_pattern_id == other.while_pattern_id
            && self.apply_end_pattern_last == other.apply_end_pattern_last
            && self.begin_captured_eol == other.begin_captured_eol
    }
}

impl Eq for Frame {}

impl Hash for Frame {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Capture specs and nested patterns are immutable payloads of
        // `(grammar_id, rule_id)` and add no state identity. The identity
        // fields themselves are pre-hashed once at push time.
        state.write_u64(self.identity_hash);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FrameIdentityKey {
    grammar_id: GrammarId,
    base_grammar_id: GrammarId,
    rule_id: RuleId,
    scope_prefix: Option<Arc<str>>,
    name: Option<Arc<str>>,
    content_name: Option<Arc<str>>,
    end_pattern: Option<Arc<str>>,
    end_pattern_id: Option<PatternId>,
    while_pattern: Option<Arc<str>>,
    while_pattern_id: Option<PatternId>,
    apply_end_pattern_last: bool,
    begin_captured_eol: bool,
}

impl FrameIdentityKey {
    fn from_frame(frame: &Frame) -> Self {
        Self {
            grammar_id: frame.grammar_id,
            base_grammar_id: frame.base_grammar_id,
            rule_id: frame.rule_id,
            scope_prefix: frame.scope_prefix.clone(),
            name: frame.name.clone(),
            content_name: frame.content_name.clone(),
            end_pattern: frame.end_pattern.clone(),
            end_pattern_id: frame.end_pattern_id,
            while_pattern: frame.while_pattern.clone(),
            while_pattern_id: frame.while_pattern_id,
            apply_end_pattern_last: frame.apply_end_pattern_last,
            begin_captured_eol: frame.begin_captured_eol,
        }
    }

    fn matches_frame(&self, frame: &Frame) -> bool {
        self.grammar_id == frame.grammar_id
            && self.base_grammar_id == frame.base_grammar_id
            && self.rule_id == frame.rule_id
            && self.scope_prefix.as_deref() == frame.scope_prefix.as_deref()
            && self.name.as_deref() == frame.name.as_deref()
            && self.content_name.as_deref() == frame.content_name.as_deref()
            && self.end_pattern.as_deref() == frame.end_pattern.as_deref()
            && self.end_pattern_id == frame.end_pattern_id
            && self.while_pattern.as_deref() == frame.while_pattern.as_deref()
            && self.while_pattern_id == frame.while_pattern_id
            && self.apply_end_pattern_last == frame.apply_end_pattern_last
            && self.begin_captured_eol == frame.begin_captured_eol
    }
}

#[derive(Debug, Clone, Copy)]
struct InternedFrameStackNode {
    parent: InternedFrameStackId,
    frame: Option<InternedFrameId>,
    depth: usize,
}

#[derive(Debug, Clone)]
struct InternedFrameStackScopeData {
    parent: InternedFrameStackId,
    scope_prefix: Option<Arc<str>>,
    name: Option<Arc<str>>,
    content_name: Option<Arc<str>>,
}

#[derive(Debug)]
struct FrameStackInternTable {
    frame_ids_by_hash: HashMap<u64, Vec<InternedFrameId>>,
    frame_keys: Vec<FrameIdentityKey>,
    stack_edges: HashMap<(InternedFrameStackId, InternedFrameId), InternedFrameStackId>,
    stack_nodes: Vec<InternedFrameStackNode>,
}

impl FrameStackInternTable {
    fn new() -> Self {
        Self {
            frame_ids_by_hash: HashMap::new(),
            frame_keys: Vec::new(),
            stack_edges: HashMap::new(),
            stack_nodes: vec![InternedFrameStackNode {
                parent: InternedFrameStackId::default(),
                frame: None,
                depth: 0,
            }],
        }
    }

    fn intern_frame(&mut self, frame: &Frame) -> InternedFrameId {
        if let Some(ids) = self.frame_ids_by_hash.get(&frame.identity_hash) {
            for id in ids {
                if self
                    .frame_keys
                    .get(id.0 as usize)
                    .is_some_and(|key| key.matches_frame(frame))
                {
                    return *id;
                }
            }
        }
        let id = InternedFrameId(self.frame_keys.len() as u32);
        let key = FrameIdentityKey::from_frame(frame);
        self.frame_keys.push(key);
        self.frame_ids_by_hash
            .entry(frame.identity_hash)
            .or_default()
            .push(id);
        id
    }

    fn intern_stack_node(
        &mut self,
        parent: InternedFrameStackId,
        frame: &Frame,
    ) -> InternedFrameStackId {
        let frame_id = self.intern_frame(frame);
        let edge = (parent, frame_id);
        if let Some(id) = self.stack_edges.get(&edge) {
            return *id;
        }
        let parent_depth = self
            .stack_nodes
            .get(parent.0 as usize)
            .map_or(0, |node| node.depth);
        let id = InternedFrameStackId(self.stack_nodes.len() as u32);
        self.stack_nodes.push(InternedFrameStackNode {
            parent,
            frame: Some(frame_id),
            depth: parent_depth + 1,
        });
        self.stack_edges.insert(edge, id);
        id
    }

    fn scope_data(&self, id: InternedFrameStackId) -> Option<InternedFrameStackScopeData> {
        let node = self.stack_nodes.get(id.0 as usize)?;
        let frame_id = node.frame?;
        let frame = self.frame_keys.get(frame_id.0 as usize)?;
        Some(InternedFrameStackScopeData {
            parent: node.parent,
            scope_prefix: frame.scope_prefix.clone(),
            name: frame.name.clone(),
            content_name: frame.content_name.clone(),
        })
    }
}

fn frame_stack_intern_table() -> &'static Mutex<FrameStackInternTable> {
    static TABLE: OnceLock<Mutex<FrameStackInternTable>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(FrameStackInternTable::new()))
}

fn intern_frame_stack_node(parent: InternedFrameStackId, frame: &Frame) -> InternedFrameStackId {
    frame_stack_intern_table()
        .lock()
        .expect("frame stack interner poisoned")
        .intern_stack_node(parent, frame)
}

fn interned_frame_stack_scope_data(
    id: InternedFrameStackId,
) -> Option<InternedFrameStackScopeData> {
    frame_stack_intern_table()
        .lock()
        .expect("frame stack interner poisoned")
        .scope_data(id)
}

// Continuation stacks are immutable parent-linked chunks. Push/pop clone at
// most one small tail chunk and never clone the whole deep stack; exact
// equality is the interned stack id maintained on each frame.
const LINKED_FRAME_CHUNK: usize = 32;

#[derive(Debug, Clone)]
struct FrameStack {
    tail: Option<Arc<FrameChunk>>,
    len: usize,
    interned_id: InternedFrameStackId,
}

impl Default for FrameStack {
    fn default() -> Self {
        Self {
            tail: None,
            len: 0,
            interned_id: InternedFrameStackId::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct FrameChunk {
    parent: Option<Arc<FrameChunk>>,
    frames: Vec<Arc<Frame>>,
    depth: usize,
}

impl FrameStack {
    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    fn last(&self) -> Option<&Frame> {
        self.tail.as_ref()?.frames.last().map(AsRef::as_ref)
    }

    fn chunks_in_order(&self) -> Vec<&FrameChunk> {
        let mut chunks = Vec::new();
        let mut cursor = self.tail.as_deref();
        while let Some(chunk) = cursor {
            chunks.push(chunk);
            cursor = chunk.parent.as_deref();
        }
        chunks.reverse();
        chunks
    }

    fn get(&self, index: usize) -> Option<&Frame> {
        if index >= self.len {
            return None;
        }
        let mut cursor = self.tail.as_deref();
        while let Some(chunk) = cursor {
            let start = chunk.depth - chunk.frames.len();
            if index >= start {
                return chunk.frames.get(index - start).map(AsRef::as_ref);
            }
            cursor = chunk.parent.as_deref();
        }
        None
    }

    #[inline]
    fn push(&mut self, frame: Frame) {
        let interned_id = frame.interned_stack_id;
        let frame = Arc::new(frame);
        if let Some(tail) = self.tail.as_mut()
            && tail.frames.len() < LINKED_FRAME_CHUNK
            && Arc::strong_count(tail) == 1
        {
            let tail = Arc::make_mut(tail);
            tail.frames.push(frame);
            tail.depth += 1;
            self.len += 1;
            self.interned_id = interned_id;
            return;
        }
        let (parent, mut frames) = match self.tail.as_ref() {
            Some(tail) if tail.frames.len() < LINKED_FRAME_CHUNK => {
                (tail.parent.clone(), tail.frames.clone())
            }
            Some(tail) => (
                Some(Arc::clone(tail)),
                Vec::with_capacity(LINKED_FRAME_CHUNK),
            ),
            None => (None, Vec::with_capacity(LINKED_FRAME_CHUNK)),
        };
        frames.push(frame);
        let depth = parent.as_ref().map_or(0, |p| p.depth) + frames.len();
        self.tail = Some(Arc::new(FrameChunk {
            parent,
            frames,
            depth,
        }));
        self.len += 1;
        self.interned_id = interned_id;
    }

    #[inline]
    fn pop(&mut self) {
        let Some(tail) = self.tail.as_ref() else {
            return;
        };
        if tail.frames.len() == 1 {
            self.tail = tail.parent.clone();
        } else {
            let mut frames = tail.frames.clone();
            frames.pop();
            self.tail = Some(Arc::new(FrameChunk {
                parent: tail.parent.clone(),
                frames,
                depth: tail.depth - 1,
            }));
        }
        self.len -= 1;
        self.refresh_interned_id_from_top();
    }

    fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        if len == 0 {
            self.tail = None;
            self.len = 0;
            self.interned_id = InternedFrameStackId::default();
            return;
        }
        let mut tail = self.tail.clone();
        while let Some(chunk) = tail.as_ref() {
            let parent_depth = chunk.parent.as_ref().map_or(0, |parent| parent.depth);
            if len > parent_depth {
                break;
            }
            tail = chunk.parent.clone();
        }
        if let Some(chunk) = tail.as_ref() {
            let parent_depth = chunk.parent.as_ref().map_or(0, |parent| parent.depth);
            let keep = len - parent_depth;
            if keep < chunk.frames.len() {
                tail = Some(Arc::new(FrameChunk {
                    parent: chunk.parent.clone(),
                    frames: chunk.frames[..keep].to_vec(),
                    depth: len,
                }));
            }
        }
        self.tail = tail;
        self.len = len;
        self.refresh_interned_id_from_top();
    }

    fn prefix(&self, len: usize) -> Self {
        let mut s = self.clone();
        s.truncate(len);
        s
    }

    #[inline]
    fn interned_id(&self) -> InternedFrameStackId {
        self.interned_id
    }

    fn refresh_interned_id_from_top(&mut self) {
        self.interned_id = self
            .last()
            .map_or(InternedFrameStackId::default(), |frame| {
                frame.interned_stack_id
            });
    }

    #[cfg(test)]
    fn iter(&self) -> FrameStackIter<'_> {
        let mut frames = Vec::with_capacity(self.len);
        for chunk in self.chunks_in_order() {
            for frame in &chunk.frames {
                frames.push(frame.as_ref());
            }
        }
        FrameStackIter { frames, index: 0 }
    }

    #[inline]
    fn for_each(&self, mut f: impl FnMut(usize, &Frame)) {
        let mut index = 0usize;
        for chunk in self.chunks_in_order() {
            for frame in &chunk.frames {
                f(index, frame);
                index += 1;
            }
        }
    }
}

impl PartialEq for FrameStack {
    fn eq(&self, other: &Self) -> bool {
        self.interned_id == other.interned_id
    }
}
impl Eq for FrameStack {}

#[cfg(test)]
struct FrameStackIter<'a> {
    frames: Vec<&'a Frame>,
    index: usize,
}

#[cfg(test)]
impl<'a> Iterator for FrameStackIter<'a> {
    type Item = &'a Frame;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.frames.get(self.index).copied()?;
        self.index += 1;
        Some(frame)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.frames.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}
#[cfg(test)]
impl ExactSizeIterator for FrameStackIter<'_> {}

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
    injection_selectors: Vec<CompiledInjectionSelector>,
    matcher_cache: HashMap<(GrammarId, PatternId), Arc<CompiledPattern>>,
    dynamic_matcher_cache: HashMap<DynamicMatcherKey, Arc<CompiledPattern>>,
    scope_names: ScopeInterner,
    scope_templates: ScopeTemplateInterner,
    scope_stacks: ScopeStackInterner,
    current_scope_stack_cache: HashMap<CurrentScopeStackKey, CachedCurrentScopeStackIds>,
    resolved_scope_stack_cache: HashMap<ScopeStackId, Arc<[String]>>,
    capture_scope_templates: HashMap<(GrammarId, ScopeId), ScopeTemplateId>,
    state_interner: StateInterner,
    line_cache: LineCache<LineCacheKey, CachedLine>,
    candidate_cache: HashMap<StateId, Arc<CandidateSet>, BuildHasherDefault<StateIdentityHasher>>,
    candidate_blueprint_cache: HashMap<CandidateBlueprintKey, Arc<CandidateBlueprint>>,
    injection_outcomes: InjectionOutcomeInterner,
    inline_candidate_cache: HashMap<InlineCandidateCacheKey, Arc<CandidateSet>>,
    regex_scratch: super::regex::bytecode::BytecodeScratch,
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
        let injection_selectors = compile_injection_selectors(&grammars);
        Self {
            grammars,
            root,
            root_scope_key,
            injection_selectors,
            matcher_cache: HashMap::new(),
            dynamic_matcher_cache: HashMap::new(),
            scope_names: ScopeInterner::default(),
            scope_templates: ScopeTemplateInterner::default(),
            scope_stacks: ScopeStackInterner::default(),
            current_scope_stack_cache: HashMap::new(),
            resolved_scope_stack_cache: HashMap::new(),
            capture_scope_templates: HashMap::new(),
            state_interner: StateInterner::new(),
            line_cache: LineCache::new(0),
            candidate_cache: HashMap::with_hasher(BuildHasherDefault::default()),
            candidate_blueprint_cache: HashMap::new(),
            injection_outcomes: InjectionOutcomeInterner::default(),
            inline_candidate_cache: HashMap::new(),
            regex_scratch: super::regex::bytecode::BytecodeScratch::default(),
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
        let mut lines = Vec::with_capacity(source.len().div_ceil(40).max(1));
        for (line_index, chunk) in LineChunks::new(source).enumerate() {
            let tokenized = self.tokenize_line_compact_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            let fingerprint = if chunk.parse_text.ends_with('\n') {
                tokenized.parse_fingerprint.without_trailing_byte(b'\n')
            } else {
                tokenized.parse_fingerprint
            };
            lines.push(self.build_highlighted_line(chunk.text, fingerprint, &tokenized.tokens));
        }
        self.fallback_call_budget_remaining = previous_budget;
        HighlightedText { lines }
    }

    fn tokenize_viewport_compact(
        &mut self,
        source: &str,
        visible: Range<usize>,
        checkpoints: &mut CheckpointTable,
    ) -> Vec<CompactTokenizedLine> {
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
            let tokenized = self.tokenize_line_compact_at_line(chunk.parse_text, state, line_index);
            state = tokenized.state.clone();
            checkpoints.record_if_boundary(line_index + 1, tokenized.exit_state_id);
            if line_index >= visible.start {
                visible_lines.push(tokenized);
            }
        }
        visible_lines
    }

    pub fn tokenize_viewport_scopes(
        &mut self,
        source: &str,
        visible: Range<usize>,
        checkpoints: &mut CheckpointTable,
    ) -> Vec<TokenizedLine> {
        self.tokenize_viewport_compact(source, visible, checkpoints)
            .into_iter()
            .map(|line| self.resolve_compact_line(line))
            .collect()
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
        let tokenized = self.tokenize_viewport_compact(source, visible, checkpoints);
        self.fallback_call_budget_remaining = previous_budget;
        let lines = tokenized
            .iter()
            .enumerate()
            .filter_map(|(offset, tokenized)| {
                let chunk = chunks.get(visible_start + offset)?;
                let fingerprint = if chunk.parse_text.ends_with('\n') {
                    tokenized.parse_fingerprint.without_trailing_byte(b'\n')
                } else {
                    tokenized.parse_fingerprint
                };
                Some(self.build_highlighted_line(chunk.text, fingerprint, &tokenized.tokens))
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
        state: TokenizerState,
        line_index: usize,
    ) -> TokenizedLine {
        let compact = self.tokenize_line_compact_at_line(parse_text, state, line_index);
        self.resolve_compact_line(compact)
    }

    fn tokenize_line_compact_at_line(
        &mut self,
        parse_text: &str,
        mut state: TokenizerState,
        line_index: usize,
    ) -> CompactTokenizedLine {
        let is_first_line = line_index == 0;
        self.record_line_tokenized();
        // Explicitly invalidate scan-local occurrence cursors even when a
        // caller reuses the same String allocation for different line text.
        // Pointer/length identity alone is insufficient in that API pattern.
        self.regex_scratch.begin_line(parse_text);
        let parse_fingerprint = LineTextFingerprint::from_text(parse_text);
        let entry_state_id = self.intern_state(state.clone());
        if self.fallback_call_budget_remaining == Some(0) {
            self.record_line_skipped();
            self.record_degraded_line();
            let stack = self.current_scope_stack_id(&state, true, None);
            return CompactTokenizedLine {
                tokens: plain_compact_tokens(parse_text, stack).into(),
                state,
                entry_state_id,
                exit_state_id: entry_state_id,
                parse_fingerprint,
            };
        }
        if self
            .max_line_bytes
            .is_some_and(|max_line_bytes| parse_text.len() > max_line_bytes)
        {
            self.record_line_skipped();
            self.record_degraded_line();
            let stack = self.current_scope_stack_id(&state, true, None);
            return CompactTokenizedLine {
                tokens: plain_compact_tokens(parse_text, stack).into(),
                state,
                entry_state_id,
                exit_state_id: entry_state_id,
                parse_fingerprint,
            };
        }
        let cache_key = self.line_cache_key(entry_state_id, parse_fingerprint, is_first_line);
        if self.line_cache.is_enabled() {
            if let Some(cached) = self.line_cache.get(&cache_key) {
                if cached.text.as_ref() == parse_text
                    && let Some(exit_state) = self.state_for_id(cached.exit).cloned()
                {
                    self.record_line_cache_hit();
                    return CompactTokenizedLine {
                        tokens: CompactLineTokens::Shared(cached.tokens),
                        state: exit_state,
                        entry_state_id,
                        exit_state_id: cached.exit,
                        parse_fingerprint,
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
        // Existing frames only need a synthetic restore value when they pop;
        // avoid materializing one `None` per deep frame on every line.
        let line_entry_depth = state.depth();
        let mut frame_anchor_positions = Vec::new();
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
                self.push_token(
                    &mut tokens,
                    cursor..parse_text.len(),
                    candidates.active_stack_id,
                );
                break;
            }
            let Some((candidate_index, result)) = search.best else {
                self.push_token(
                    &mut tokens,
                    cursor..parse_text.len(),
                    candidates.active_stack_id,
                );
                break;
            };
            let state_changes = !matches!(
                candidates.candidates[candidate_index].kind,
                CandidateKind::Match { .. }
            );

            if result.start > cursor {
                self.push_token(
                    &mut tokens,
                    cursor..result.start,
                    candidates.active_stack_id,
                );
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
                line_entry_depth,
                candidates.active_stack_id,
                candidates.end_stack_id,
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
            let stack = self.current_scope_stack_id(&state, true, None);
            self.push_token(&mut tokens, cursor..parse_text.len(), stack);
        }
        if degraded {
            self.record_degraded_line();
        }

        let exit_state_id = self.intern_state(state.clone());
        let tokens = if self.line_cache.is_enabled() {
            let tokens: Arc<[CompactScopedToken]> = tokens.into();
            let evicted = self.line_cache.insert(
                cache_key,
                CachedLine {
                    text: Arc::from(parse_text),
                    tokens: Arc::clone(&tokens),
                    exit: exit_state_id,
                },
            );
            if evicted {
                self.record_line_cache_eviction();
            }
            CompactLineTokens::Shared(tokens)
        } else {
            CompactLineTokens::Owned(tokens)
        };
        CompactTokenizedLine {
            tokens,
            state,
            entry_state_id,
            exit_state_id,
            parse_fingerprint,
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
        self.current_scope_stack_cache.clear();
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
        self.candidate_blueprint_cache.clear();
        self.current_scope_stack_cache.clear();
        self.resolved_scope_stack_cache.clear();
        self.injection_outcomes.clear();
        self.inline_candidate_cache.clear();
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
        if steps > *remaining {
            *remaining = 0;
            false
        } else {
            *remaining -= steps;
            true
        }
    }

    fn line_cache_key(
        &self,
        entry: StateId,
        fingerprint: LineTextFingerprint,
        first_line: bool,
    ) -> LineCacheKey {
        LineCacheKey {
            entry,
            first_line,
            fingerprint,
        }
    }

    fn build_highlighted_line(
        &self,
        text: &str,
        fingerprint: LineTextFingerprint,
        scoped_tokens: &[CompactScopedToken],
    ) -> HighlightedLine {
        let mut line = HighlightedLine {
            fingerprint,
            segments: Vec::with_capacity(scoped_tokens.len()),
        };
        for token in scoped_tokens {
            let start = token.range.start.min(text.len());
            let end = token.range.end.min(text.len());
            if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                continue;
            }
            let class = self.scope_stacks.class(token.stack);
            push_segment(&mut line.segments, start, end, class);
        }
        line
    }

    fn resolve_compact_line(&self, line: CompactTokenizedLine) -> TokenizedLine {
        let tokens = line
            .tokens
            .iter()
            .map(|token| ScopedToken {
                range: token.range.clone(),
                scopes: self.scope_stacks.resolve(token.stack, &self.scope_names),
            })
            .collect::<Vec<_>>()
            .into();
        TokenizedLine {
            tokens,
            state: line.state,
            entry_state_id: line.entry_state_id,
            exit_state_id: line.exit_state_id,
        }
    }

    fn apply_while_continuations(
        &mut self,
        line: &str,
        state: &mut TokenizerState,
        tokens: &mut Vec<CompactScopedToken>,
        cursor: &mut usize,
    ) -> HashSet<(GrammarId, RuleId)> {
        let mut suppressed = HashSet::new();
        let mut while_frames = Vec::new();
        state.frames.for_each(|index, frame| {
            if frame.while_pattern.is_some() {
                while_frames.push(index);
            }
        });
        for index in while_frames {
            let Some(frame) = state.frames.get(index).cloned() else {
                break;
            };
            let Some(pattern) = frame.while_pattern.clone() else {
                continue;
            };
            let ctx = AnchorContext::continuation(*cursor);
            let result = self.find_pattern(
                &pattern,
                frame
                    .while_pattern_id
                    .map(|pattern_id| (frame.grammar_id, pattern_id)),
                line,
                *cursor,
                ctx,
            );
            match result {
                Some(result) if result.start == *cursor => {
                    let frame_state = state.prefix(index + 1);
                    let stack = self.current_scope_stack_id(&frame_state, false, None);
                    self.emit_match(
                        tokens,
                        line,
                        &result,
                        frame.grammar_id,
                        stack,
                        None,
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
                    let mut has_child_end = false;
                    state.frames.for_each(|child_index, child| {
                        has_child_end |= child_index > index && child.end_pattern.is_some();
                    });
                    if has_child_end {
                        suppressed.insert((frame.grammar_id, frame.rule_id));
                    }
                    state.truncate_frames(index);
                    break;
                }
            }
        }
        suppressed
    }

    fn candidates_for_state(
        &self,
        state: &TokenizerState,
        injections: &InjectionOutcome,
    ) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        let mut order = 0usize;

        let (grammar_id, base_grammar_id, refs, end_candidate, apply_end_last) =
            if let Some(frame) = state.frames.last() {
                let end = frame.end_pattern.as_ref().map(|pattern| Candidate {
                    order: 0,
                    base_grammar_id: frame.base_grammar_id,
                    pattern: pattern.to_string(),
                    pattern_id: frame
                        .end_pattern_id
                        .map(|pattern_id| (frame.grammar_id, pattern_id)),
                    scope_prefix: frame.scope_prefix.as_deref().map(str::to_owned),
                    kind: CandidateKind::End {
                        grammar_id: frame.grammar_id,
                        captures: frame.end_captures.as_ref().clone(),
                    },
                });
                (
                    frame.grammar_id,
                    frame.base_grammar_id,
                    frame.patterns.to_vec(),
                    end,
                    frame.apply_end_pattern_last,
                )
            } else {
                let Some(grammar) = self.grammars.grammar(self.root) else {
                    return candidates;
                };
                (self.root, self.root, grammar.top_level.clone(), None, false)
            };

        for injection in &injections.left {
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

        for injection in &injections.right {
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
                            let pattern_id = *pattern;
                            if let Some(pattern) = grammar.pattern(*pattern) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: pattern.to_owned(),
                                    pattern_id: Some((grammar_id, pattern_id)),
                                    scope_prefix: scope_prefix.clone(),
                                    kind: CandidateKind::Match {
                                        grammar_id,
                                        name: scope_name(grammar, *name),
                                        name_template: None,
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
                            let begin_pattern_id = *begin;
                            if let Some(begin) = grammar.pattern(*begin) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: begin.to_owned(),
                                    pattern_id: Some((grammar_id, begin_pattern_id)),
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
                            let begin_pattern_id = *begin;
                            if let Some(begin) = grammar.pattern(*begin) {
                                out.push(Candidate {
                                    order: *order,
                                    base_grammar_id,
                                    pattern: begin.to_owned(),
                                    pattern_id: Some((grammar_id, begin_pattern_id)),
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

    fn injection_outcome(
        &mut self,
        stack: &[String],
    ) -> (InjectionOutcomeId, Arc<InjectionOutcome>) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut seen = HashSet::new();
        for injection in &self.injection_selectors {
            if selector_tokens_match(&injection.selector_tokens, stack) {
                if !seen.insert((
                    injection.priority,
                    injection.grammar_id,
                    injection.patterns.clone(),
                )) {
                    continue;
                }
                let candidate = InjectionCandidate {
                    grammar_id: injection.grammar_id,
                    patterns: injection.patterns.clone(),
                };
                if injection.priority == InjectionPriority::Left {
                    left.push(candidate);
                } else {
                    right.push(candidate);
                }
            }
        }
        let outcome = InjectionOutcome { left, right };
        if self.injection_outcomes.len() >= MAX_INJECTION_OUTCOMES
            && !self.injection_outcomes.contains(&outcome)
        {
            // Blueprint keys contain outcome IDs. Drop them together so an
            // evicted outcome never leaves an ID whose meaning must be
            // reconstructed approximately.
            self.injection_outcomes.clear();
            self.candidate_blueprint_cache.clear();
        }
        self.injection_outcomes.intern(outcome)
    }

    fn cached_candidates_for_state(&mut self, state: &TokenizerState) -> Arc<CandidateSet> {
        let state_id = self.intern_state(state.clone());
        if let Some(candidates) = self.candidate_cache.get(&state_id).cloned() {
            self.record_candidate_cache_hit();
            return candidates;
        }
        self.record_candidate_cache_miss();
        let stacks = self.current_scope_stack_ids(state, None);
        let active_stack_id = stacks.active_stack_id;
        let end_stack_id = stacks.end_stack_id;
        let stack = self.resolve_scope_stack_cached(active_stack_id);
        let (injection_outcome_id, injection_outcome) = self.injection_outcome(stack.as_ref());
        let blueprint_key = CandidateBlueprintKey {
            source: CandidateSourceKey::for_state(self.root, state),
            injection_outcome: injection_outcome_id,
        };
        let blueprint =
            if let Some(blueprint) = self.candidate_blueprint_cache.get(&blueprint_key).cloned() {
                blueprint
            } else {
                let candidates = self.candidates_for_state(state, &injection_outcome);
                let blueprint = Arc::new(self.build_candidate_blueprint(candidates));
                if self.candidate_blueprint_cache.len() >= MAX_CANDIDATE_BLUEPRINTS {
                    self.candidate_blueprint_cache.clear();
                }
                self.candidate_blueprint_cache
                    .insert(blueprint_key, blueprint.clone());
                blueprint
            };
        let candidate_set = Arc::new(CandidateSet {
            blueprint,
            active_stack_id,
            end_stack_id,
        });
        if self.candidate_cache.len() >= MAX_CANDIDATE_SETS {
            self.candidate_cache.clear();
        }
        self.candidate_cache.insert(state_id, candidate_set.clone());
        candidate_set
    }

    fn build_candidate_set(
        &mut self,
        candidates: Vec<Candidate>,
        active_stack_id: ScopeStackId,
        end_stack_id: ScopeStackId,
    ) -> CandidateSet {
        let blueprint = Arc::new(self.build_candidate_blueprint(candidates));
        CandidateSet {
            blueprint,
            active_stack_id,
            end_stack_id,
        }
    }

    fn build_candidate_blueprint(&mut self, mut candidates: Vec<Candidate>) -> CandidateBlueprint {
        for candidate in &mut candidates {
            if let CandidateKind::Match {
                name,
                name_template,
                ..
            } = &mut candidate.kind
                && let Some(name) = name.as_deref().filter(|name| !name.contains('$'))
            {
                *name_template = Some(
                    self.scope_templates
                        .intern_scope_template(name, &mut self.scope_names),
                );
            }
        }
        let mut matchers = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            let live_captures = self.live_captures_for_candidate(candidate);
            let matcher = if let Some((grammar_id, pattern_id)) = candidate.pattern_id {
                self.cached_matcher_with_live_captures(
                    grammar_id,
                    pattern_id,
                    &candidate.pattern,
                    live_captures,
                )
            } else {
                self.cached_dynamic_matcher_with_live_captures(&candidate.pattern, live_captures)
            };
            matchers.push(matcher);
        }
        let pattern_set_search = (matchers.len() > 1).then(|| {
            if let Some(counters) = self.counters_mut() {
                counters.record_pattern_set_construction();
            }
            PatternSetMatcher::from_compiled(&matchers)
        });
        CandidateBlueprint {
            candidates,
            matchers,
            pattern_set_search,
        }
    }

    fn cached_matcher(
        &mut self,
        grammar_id: GrammarId,
        pattern_id: PatternId,
        pattern: &str,
    ) -> Arc<CompiledPattern> {
        let key = (grammar_id, pattern_id);
        if let Some(matcher) = self.matcher_cache.get(&key) {
            return matcher.clone();
        }
        let matcher = Arc::new(CompiledPattern::new(pattern));
        self.matcher_cache.insert(key, matcher.clone());
        if let Some(counters) = self.counters_mut() {
            counters.record_regex_compile(Some(grammar_id.0), Some(pattern_id.0), pattern);
        }
        matcher
    }

    fn cached_matcher_with_live_captures(
        &mut self,
        grammar_id: GrammarId,
        pattern_id: PatternId,
        pattern: &str,
        live_captures: Vec<u32>,
    ) -> Arc<CompiledPattern> {
        let key = (grammar_id, pattern_id);
        if let Some(matcher) = self.matcher_cache.get(&key) {
            return matcher.clone();
        }
        let matcher = Arc::new(CompiledPattern::new_with_live_captures(
            pattern,
            live_captures,
        ));
        self.matcher_cache.insert(key, matcher.clone());
        if let Some(counters) = self.counters_mut() {
            counters.record_regex_compile(Some(grammar_id.0), Some(pattern_id.0), pattern);
        }
        matcher
    }

    fn cached_dynamic_matcher(&mut self, pattern: &str) -> Arc<CompiledPattern> {
        let key = DynamicMatcherKey {
            pattern: pattern.to_owned(),
            live_captures: vec![u32::MAX],
        };
        if let Some(matcher) = self.dynamic_matcher_cache.get(&key) {
            return matcher.clone();
        }
        // Dynamic begin/end substitutions are source-derived and potentially
        // unbounded. Keep them separate from immutable grammar patterns and
        // put a hard ceiling on retained entries.
        if self.dynamic_matcher_cache.len() >= MAX_DYNAMIC_MATCHERS {
            self.dynamic_matcher_cache.clear();
        }
        let matcher = Arc::new(CompiledPattern::new(pattern));
        self.dynamic_matcher_cache.insert(key, matcher.clone());
        if let Some(counters) = self.counters_mut() {
            counters.record_regex_compile(None, None, pattern);
        }
        matcher
    }

    fn cached_dynamic_matcher_with_live_captures(
        &mut self,
        pattern: &str,
        live_captures: Vec<u32>,
    ) -> Arc<CompiledPattern> {
        let key = DynamicMatcherKey {
            pattern: pattern.to_owned(),
            live_captures: live_captures.clone(),
        };
        if let Some(matcher) = self.dynamic_matcher_cache.get(&key) {
            return matcher.clone();
        }
        if self.dynamic_matcher_cache.len() >= MAX_DYNAMIC_MATCHERS {
            self.dynamic_matcher_cache.clear();
        }
        let matcher = Arc::new(CompiledPattern::new_with_live_captures(
            pattern,
            live_captures,
        ));
        self.dynamic_matcher_cache.insert(key, matcher.clone());
        if let Some(counters) = self.counters_mut() {
            counters.record_regex_compile(None, None, pattern);
        }
        matcher
    }

    fn live_captures_for_candidate(&self, candidate: &Candidate) -> Vec<u32> {
        let mut live = Vec::new();
        match &candidate.kind {
            CandidateKind::Match {
                grammar_id,
                name,
                captures,
                ..
            } => {
                add_scope_capture_refs(name.as_deref(), &mut live);
                self.add_capture_spec_refs(*grammar_id, captures, &mut live);
            }
            CandidateKind::BeginEnd {
                grammar_id,
                end,
                begin_captures,
                name,
                content_name,
                ..
            } => {
                add_scope_capture_refs(name.as_deref(), &mut live);
                add_scope_capture_refs(content_name.as_deref(), &mut live);
                self.add_capture_spec_refs(*grammar_id, begin_captures, &mut live);
                if let Some(pattern) = self
                    .grammars
                    .grammar(*grammar_id)
                    .and_then(|grammar| grammar.pattern(*end))
                {
                    add_end_pattern_capture_refs(pattern, &mut live);
                }
            }
            CandidateKind::BeginWhile {
                grammar_id,
                while_pattern,
                begin_captures,
                name,
                content_name,
                ..
            } => {
                add_scope_capture_refs(name.as_deref(), &mut live);
                add_scope_capture_refs(content_name.as_deref(), &mut live);
                self.add_capture_spec_refs(*grammar_id, begin_captures, &mut live);
                if let Some(pattern) = self
                    .grammars
                    .grammar(*grammar_id)
                    .and_then(|grammar| grammar.pattern(*while_pattern))
                {
                    add_end_pattern_capture_refs(pattern, &mut live);
                }
            }
            CandidateKind::End {
                grammar_id,
                captures,
            } => self.add_capture_spec_refs(*grammar_id, captures, &mut live),
        }
        live.sort_unstable();
        live.dedup();
        live
    }

    fn add_capture_spec_refs(
        &self,
        grammar_id: GrammarId,
        captures: &CaptureSpec,
        live: &mut Vec<u32>,
    ) {
        let grammar = self.grammars.grammar(grammar_id);
        for (group, entry) in &captures.entries {
            if entry.name.is_some() || !entry.patterns.is_empty() {
                live.push(*group);
            }
            if let Some(name) = entry
                .name
                .and_then(|name| grammar.and_then(|grammar| grammar.scope(name)))
            {
                add_scope_capture_refs(Some(name), live);
            }
        }
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
        if let Some(counters) = self.counters_mut() {
            counters.record_candidate_search();
        }
        let mut best: Option<(usize, MatchResult)> = None;
        let mut fallback_budget_killed = false;
        let mut fallback_steps = 0u64;

        let suppression_active = suppressed_begin_rules.is_some_and(|rules| !rules.is_empty());
        let unified_search_active = !suppression_active && !self.counters_enabled;
        let ctx = scan_anchor_context(from, is_first_line, anchor_pos);
        if unified_search_active && let Some(pattern_set) = &candidate_set.pattern_set_search {
            if let Some((pattern_index, set_result)) =
                pattern_set.find_with_context_and_scratch(line, from, ctx, &mut self.regex_scratch)
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
                if let Some(counters) = self.counters_mut() {
                    counters.record_candidate_pattern_considered();
                }
                let pattern = self.find_cached_pattern_selection_report(
                    &candidate.pattern,
                    candidate_set.matchers[index].matcher(),
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
            && candidate_set.matchers[*index].needs_capture_replay()
        {
            if let Some(counters) = self.counters_mut() {
                counters.record_capture_replay();
            }
            let ctx = scan_anchor_context(from, is_first_line, anchor_pos);
            let compiled = &candidate_set.matchers[*index];
            let mode = super::regex::backtrack::capture_engine_mode();
            let capture_candidate = compiled.find_live_captures_at(
                line,
                selection_result.start,
                ctx,
                &mut self.regex_scratch,
            );
            let recursive = || {
                compiled
                    .matcher()
                    .find_report_at(line, selection_result.start, ctx)
                    .map(|(result, steps)| (result, steps.unwrap_or(0)))
            };
            let report = match (mode, capture_candidate) {
                (super::regex::backtrack::PositionEngineMode::Candidate, Some(candidate)) => {
                    candidate
                }
                (super::regex::backtrack::PositionEngineMode::Shadow, Some(candidate)) => {
                    let recursive = recursive();
                    let agrees = match (&candidate, &recursive) {
                        (Ok((candidate, _)), Ok((recursive, _))) => candidate == recursive,
                        (
                            Err(FallbackError::BudgetExceeded { .. }),
                            Err(FallbackError::BudgetExceeded { .. }),
                        )
                        | (
                            Err(FallbackError::InvalidStart { .. }),
                            Err(FallbackError::InvalidStart { .. }),
                        ) => true,
                        _ => false,
                    };
                    if !agrees {
                        eprintln!(
                            "MARK_TEXTMATE_CAPTURE_VM_MISMATCH pattern={:?} start={} candidate={candidate:?} recursive={recursive:?}",
                            candidate_set.candidates[*index].pattern, selection_result.start,
                        );
                    }
                    recursive
                }
                _ => recursive(),
            };
            match report {
                Ok((Some(result), steps)) => {
                    let steps = steps as u64;
                    fallback_steps = fallback_steps.saturating_add(steps);
                    best = Some((*index, result));
                }
                Ok((None, steps)) => {
                    fallback_steps = fallback_steps.saturating_add(steps as u64);
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
        if best.is_some()
            && let Some(counters) = self.counters_mut()
        {
            counters.record_candidate_winner();
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
        pattern_id: Option<(GrammarId, PatternId)>,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Option<MatchResult> {
        self.find_pattern_report(pattern, pattern_id, line, from, ctx)
            .result
    }

    fn find_pattern_report(
        &mut self,
        pattern: &str,
        pattern_id: Option<(GrammarId, PatternId)>,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> PatternSearchResult {
        let matcher = match pattern_id {
            Some((grammar_id, pattern_id)) => self.cached_matcher(grammar_id, pattern_id, pattern),
            None => self.cached_dynamic_matcher(pattern),
        };
        self.find_cached_pattern_report(pattern, matcher.matcher(), line, from, ctx)
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
        tokens: &mut Vec<CompactScopedToken>,
        candidate: &Candidate,
        result: &MatchResult,
        anchor_pos: &mut Option<usize>,
        frame_anchor_positions: &mut Vec<Option<usize>>,
        line_entry_depth: usize,
        active_stack: ScopeStackId,
        end_stack: ScopeStackId,
    ) -> usize {
        match &candidate.kind {
            CandidateKind::Match {
                grammar_id,
                name,
                name_template,
                captures,
            } => {
                let consumed_end = specified_outside_capture_end(result, captures);
                let mut stack = active_stack;
                if let Some(prefix) = &candidate.scope_prefix {
                    stack = self.push_scope_prefix_once_id(stack, prefix);
                }
                self.emit_match(
                    tokens,
                    line,
                    result,
                    *grammar_id,
                    stack,
                    name.as_deref(),
                    *name_template,
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
                let mut stack = active_stack;
                if let Some(prefix) = candidate.scope_prefix.clone() {
                    stack = self.push_scope_prefix_once_id(stack, &prefix);
                }
                self.emit_match(
                    tokens,
                    line,
                    result,
                    *grammar_id,
                    stack,
                    name.as_deref(),
                    None,
                    begin_captures,
                );
                let end_pattern = self.substituted_pattern(*grammar_id, *end, line, result);
                let end_pattern_id = end_pattern
                    .as_ref()
                    .and_then(|(_, is_static)| is_static.then_some(*end));
                state.push_frame(Frame {
                    grammar_id: *grammar_id,
                    base_grammar_id: candidate.base_grammar_id,
                    rule_id: *rule_id,
                    scope_prefix: candidate.scope_prefix.clone().map(Arc::from),
                    name: name.map(Arc::from),
                    content_name: content_name.map(Arc::from),
                    end_pattern: end_pattern.map(|(pattern, _)| Arc::from(pattern)),
                    end_pattern_id,
                    while_pattern: None,
                    while_pattern_id: None,
                    end_captures: Arc::new(end_captures.clone()),
                    while_captures: Arc::new(CaptureSpec::default()),
                    patterns: patterns.clone().into(),
                    apply_end_pattern_last: *apply_end_pattern_last,
                    begin_captured_eol: result.end == line.len() && line.ends_with('\n'),
                    identity_hash: 0,
                    stack_hash: 0,
                    state_hash: 0,
                    interned_stack_id: InternedFrameStackId::default(),
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
                let mut stack = active_stack;
                if let Some(prefix) = candidate.scope_prefix.clone() {
                    stack = self.push_scope_prefix_once_id(stack, &prefix);
                }
                if begin_captures.entries.is_empty()
                    && content_name.is_some()
                    && !patterns.is_empty()
                {
                    let mut content_stack = stack;
                    if let Some(name) = &name {
                        content_stack = self.push_scope_text_id(content_stack, name);
                    }
                    if let Some(content_name) = &content_name {
                        content_stack = self.push_scope_text_id(content_stack, content_name);
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
                        name.as_deref(),
                        None,
                        begin_captures,
                    );
                }
                let static_while_pattern_id = *while_pattern;
                let while_pattern =
                    self.substituted_pattern(*grammar_id, static_while_pattern_id, line, result);
                let while_pattern_id = while_pattern
                    .as_ref()
                    .and_then(|(_, is_static)| is_static.then_some(static_while_pattern_id));
                state.push_frame(Frame {
                    grammar_id: *grammar_id,
                    base_grammar_id: candidate.base_grammar_id,
                    rule_id: *rule_id,
                    scope_prefix: candidate.scope_prefix.clone().map(Arc::from),
                    name: name.map(Arc::from),
                    content_name: content_name.map(Arc::from),
                    end_pattern: None,
                    end_pattern_id: None,
                    while_pattern: while_pattern.map(|(pattern, _)| Arc::from(pattern)),
                    while_pattern_id,
                    end_captures: Arc::new(CaptureSpec::default()),
                    while_captures: Arc::new(while_captures.clone()),
                    patterns: patterns.clone().into(),
                    apply_end_pattern_last: false,
                    begin_captured_eol: result.end == line.len() && line.ends_with('\n'),
                    identity_hash: 0,
                    stack_hash: 0,
                    state_hash: 0,
                    interned_stack_id: InternedFrameStackId::default(),
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
                self.emit_match(
                    tokens,
                    line,
                    result,
                    *grammar_id,
                    end_stack,
                    None,
                    None,
                    captures,
                );
                let depth_before_pop = state.depth();
                state.pop_frame();
                *anchor_pos = if depth_before_pop > line_entry_depth {
                    frame_anchor_positions.pop().flatten()
                } else {
                    state
                        .frames
                        .last()
                        .is_some_and(|frame| frame.begin_captured_eol)
                        .then_some(0)
                };
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
    ) -> Option<(String, bool)> {
        let grammar = self.grammars.grammar(grammar_id)?;
        let pattern = grammar.pattern(pattern_id)?;
        let capture_texts = capture_texts(line, &result.captures);
        let substituted =
            substitute_end_pattern(pattern, &capture_texts, MAX_SUBSTITUTED_END_PATTERN_LEN)
                .unwrap_or_else(|_| pattern.to_owned());
        let is_static = substituted == pattern;
        Some((substituted, is_static))
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_match(
        &mut self,
        tokens: &mut Vec<CompactScopedToken>,
        line: &str,
        result: &MatchResult,
        grammar_id: GrammarId,
        mut base_stack: ScopeStackId,
        name: Option<&str>,
        name_template: Option<ScopeTemplateId>,
        captures: &CaptureSpec,
    ) {
        if let Some(template) = name_template {
            base_stack = self.scope_stacks.push_template(
                base_stack,
                template,
                &self.scope_templates,
                &self.scope_names,
            );
        } else if let Some(name) = name {
            base_stack = self.push_scope_text_id(
                base_stack,
                &substitute_scope_text(name, line, &result.captures),
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
            base_stack,
            captures,
            result_captures,
        );
        for (range, entry) in outside {
            let range = range.start.max(match_end)..range.end;
            let mut stack = base_stack;
            if let Some(scope_id) = entry.name {
                let (name, template) =
                    self.capture_scope_application(grammar_id, scope_id, line, result_captures);
                stack = self.push_scope_application(stack, name.as_deref(), template);
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
        tokens: &mut Vec<CompactScopedToken>,
        line: &str,
        range: Range<usize>,
        grammar_id: GrammarId,
        base_stack: ScopeStackId,
        capture_spec: &CaptureSpec,
        captures: &[Option<Range<usize>>],
    ) {
        if range.start >= range.end {
            return;
        }
        if self.grammars.grammar(grammar_id).is_none() {
            self.push_token(tokens, range, base_stack);
            return;
        }
        // Match vscode-textmate's ordered capture handling. Capture groups are
        // semantic events in numeric order, not a geometric range tree:
        // overlapping named captures form a small active stack, while a
        // retokenized capture always starts from the rule/content stack plus
        // that capture's own name. Inheriting unrelated overlapping capture
        // names here adds broad `meta.head.*` scopes to C++ child tokens.
        let mut cursor = range.start;
        let mut active = CaptureScopeStack::default();
        for (group, entry) in &capture_spec.entries {
            let Some(capture_range) = captures.get(*group as usize).and_then(Clone::clone) else {
                continue;
            };
            if capture_range.start >= capture_range.end {
                continue;
            }
            if capture_range.start > range.end {
                break;
            }
            let capture_range = clamp_range(capture_range, range.clone());
            if capture_range.start >= capture_range.end {
                continue;
            }

            while active
                .last()
                .is_some_and(|(_, end)| *end <= capture_range.start)
            {
                let (stack, end) = active.pop().expect("checked active capture");
                let end = end.min(range.end);
                if cursor < end {
                    self.push_token(tokens, cursor..end, stack);
                    cursor = end;
                }
            }
            let current_stack = active.last().map_or(base_stack, |(stack, _)| *stack);
            if cursor < capture_range.start {
                self.push_token(tokens, cursor..capture_range.start, current_stack);
                cursor = capture_range.start;
            }

            let (name, name_template) = entry.name.map_or((None, None), |scope_id| {
                self.capture_scope_application(grammar_id, scope_id, line, captures)
            });
            if !entry.patterns.is_empty() {
                let stack = self.push_scope_application(base_stack, name.as_deref(), name_template);
                self.tokenize_inline_patterns(
                    tokens,
                    line,
                    capture_range.clone(),
                    grammar_id,
                    stack,
                    &entry.patterns,
                    true,
                );
                cursor = cursor.max(capture_range.end);
            } else if entry.name.is_some() {
                let stack =
                    self.push_scope_application(current_stack, name.as_deref(), name_template);
                active.push((stack, capture_range.end));
            }
        }

        while let Some((stack, end)) = active.pop() {
            let end = end.min(range.end);
            if cursor < end {
                self.push_token(tokens, cursor..end, stack);
                cursor = end;
            }
        }
        if cursor < range.end {
            self.push_token(tokens, cursor..range.end, base_stack);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tokenize_inline_patterns(
        &mut self,
        tokens: &mut Vec<CompactScopedToken>,
        line: &str,
        range: Range<usize>,
        grammar_id: GrammarId,
        base_stack: ScopeStackId,
        patterns: &[RuleRef],
        compound_patterns: bool,
    ) {
        let base_stack_id = base_stack;
        let mut state = TokenizerState::default();
        let mut local_candidate_cache = HashMap::<TokenizerState, Arc<CandidateSet>>::new();
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
            let candidate_set = if let Some(cached) = local_candidate_cache.get(&state) {
                cached.clone()
            } else {
                let cache_key = InlineCandidateCacheKey {
                    grammar_id,
                    patterns: patterns.to_vec(),
                    compound_patterns,
                    state: state.clone(),
                    base_stack: base_stack_id,
                };
                let candidate_set = if let Some(cached) =
                    self.inline_candidate_cache.get(&cache_key)
                {
                    cached.clone()
                } else {
                    let (candidates, active_stack_id, end_stack_id) = if state.is_initial() {
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
                        (candidates, base_stack_id, base_stack_id)
                    } else {
                        let stacks = self.current_scope_stack_ids(&state, Some(base_stack_id));
                        let active_scopes = self.resolve_scope_stack_cached(stacks.active_stack_id);
                        let (_, injection_outcome) = self.injection_outcome(active_scopes.as_ref());
                        (
                            self.candidates_for_state(&state, &injection_outcome),
                            stacks.active_stack_id,
                            stacks.end_stack_id,
                        )
                    };
                    let candidate_set = Arc::new(self.build_candidate_set(
                        candidates,
                        active_stack_id,
                        end_stack_id,
                    ));
                    if self.inline_candidate_cache.len() >= MAX_INLINE_CANDIDATE_SETS {
                        self.inline_candidate_cache.clear();
                    }
                    self.inline_candidate_cache
                        .insert(cache_key, candidate_set.clone());
                    if let Some(counters) = self.counters_mut() {
                        counters.record_inline_candidate_set_construction();
                    }
                    candidate_set
                };
                local_candidate_cache.insert(state.clone(), candidate_set.clone());
                candidate_set
            };
            if candidate_set.candidates.is_empty() {
                self.push_token(tokens, cursor..range.end, candidate_set.active_stack_id);
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
                self.push_token(tokens, cursor..range.end, candidate_set.active_stack_id);
                return;
            }
            let Some((candidate_index, result)) = search.best else {
                self.push_token(tokens, cursor..range.end, candidate_set.active_stack_id);
                return;
            };
            if result.start >= range.end || result.end > range.end {
                self.push_token(tokens, cursor..range.end, candidate_set.active_stack_id);
                return;
            }
            if cursor < result.start {
                self.push_token(tokens, cursor..result.start, candidate_set.active_stack_id);
            }
            let candidate = &candidate_set.candidates[candidate_index];
            if !compound_patterns
                && state.is_initial()
                && !matches!(candidate.kind, CandidateKind::Match { .. })
            {
                self.push_token(tokens, result.start..result.end, base_stack_id);
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
                0,
                candidate_set.active_stack_id,
                candidate_set.end_stack_id,
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

    fn current_scope_stack_id(
        &mut self,
        state: &TokenizerState,
        include_top_content: bool,
        base_stack: Option<ScopeStackId>,
    ) -> ScopeStackId {
        let stacks = self.current_scope_stack_ids(state, base_stack);
        if include_top_content {
            stacks.active_stack_id
        } else {
            stacks.end_stack_id
        }
    }

    fn current_scope_stack_ids(
        &mut self,
        state: &TokenizerState,
        base_stack: Option<ScopeStackId>,
    ) -> CachedCurrentScopeStackIds {
        let base_stack = match base_stack {
            Some(base_stack) => base_stack,
            None => self.root_scope_stack_id(),
        };
        self.current_scope_stack_ids_for_stack(state.frames.interned_id(), base_stack)
    }

    fn current_scope_stack_ids_for_stack(
        &mut self,
        frame_stack: InternedFrameStackId,
        base_stack: ScopeStackId,
    ) -> CachedCurrentScopeStackIds {
        let mut cursor = frame_stack;
        let mut missing = Vec::new();
        let mut cached = loop {
            let key = CurrentScopeStackKey {
                root: self.root,
                base_stack,
                frame_stack: cursor,
            };
            if let Some(cached) = self.current_scope_stack_cache.get(&key).copied() {
                break cached;
            }
            if cursor == InternedFrameStackId::default() {
                let cached = CachedCurrentScopeStackIds {
                    active_stack_id: base_stack,
                    end_stack_id: base_stack,
                };
                self.insert_current_scope_stack_cache(key, cached);
                break cached;
            }
            let frame = interned_frame_stack_scope_data(cursor)
                .expect("interned frame stack id has scope data");
            let parent = frame.parent;
            missing.push((cursor, frame));
            cursor = parent;
        };

        while let Some((stack_id, frame)) = missing.pop() {
            cached = self.extend_current_scope_stack_ids(cached, &frame);
            let key = CurrentScopeStackKey {
                root: self.root,
                base_stack,
                frame_stack: stack_id,
            };
            self.insert_current_scope_stack_cache(key, cached);
        }
        cached
    }

    fn extend_current_scope_stack_ids(
        &mut self,
        parent: CachedCurrentScopeStackIds,
        frame: &InternedFrameStackScopeData,
    ) -> CachedCurrentScopeStackIds {
        let mut end_stack = parent.active_stack_id;
        if let Some(prefix) = frame.scope_prefix.as_deref() {
            end_stack = self.push_scope_prefix_once_id(end_stack, prefix);
        }
        if let Some(name) = frame.name.as_deref() {
            end_stack = self.push_scope_text_id(end_stack, name);
        }
        let mut active_stack = end_stack;
        if let Some(content) = frame.content_name.as_deref() {
            active_stack = self.push_scope_text_id(active_stack, content);
        }
        CachedCurrentScopeStackIds {
            active_stack_id: active_stack,
            end_stack_id: end_stack,
        }
    }

    fn insert_current_scope_stack_cache(
        &mut self,
        key: CurrentScopeStackKey,
        value: CachedCurrentScopeStackIds,
    ) {
        if self.current_scope_stack_cache.len() >= MAX_SCOPE_STACK_CACHE_ENTRIES {
            self.current_scope_stack_cache.clear();
        }
        self.current_scope_stack_cache.entry(key).or_insert(value);
    }

    fn resolve_scope_stack_cached(&mut self, stack: ScopeStackId) -> Arc<[String]> {
        if let Some(scopes) = self.resolved_scope_stack_cache.get(&stack).cloned() {
            return scopes;
        }
        if self.resolved_scope_stack_cache.len() >= MAX_SCOPE_STACK_CACHE_ENTRIES {
            self.resolved_scope_stack_cache.clear();
        }
        let scopes = Arc::from(
            self.scope_stacks
                .resolve(stack, &self.scope_names)
                .into_boxed_slice(),
        );
        self.resolved_scope_stack_cache
            .insert(stack, Arc::clone(&scopes));
        scopes
    }

    fn root_scope_stack_id(&mut self) -> ScopeStackId {
        let Some(root_scope) = self
            .grammars
            .grammar(self.root)
            .map(|grammar| grammar.scope_name.clone())
        else {
            return self.scope_stacks.empty();
        };
        let empty = self.scope_stacks.empty();
        let root_scope = self.scope_names.intern(&root_scope);
        self.scope_stacks.push(empty, root_scope, &self.scope_names)
    }

    fn push_scope_text_id(&mut self, stack: ScopeStackId, text: &str) -> ScopeStackId {
        let template = self
            .scope_templates
            .intern_scope_template(text, &mut self.scope_names);
        self.scope_stacks
            .push_template(stack, template, &self.scope_templates, &self.scope_names)
    }

    fn capture_scope_application(
        &mut self,
        grammar_id: GrammarId,
        scope_id: ScopeId,
        line: &str,
        captures: &[Option<Range<usize>>],
    ) -> (Option<String>, Option<ScopeTemplateId>) {
        let key = (grammar_id, scope_id);
        if let Some(template) = self.capture_scope_templates.get(&key) {
            return (None, Some(*template));
        }
        let Some(text) = self
            .grammars
            .grammar(grammar_id)
            .and_then(|grammar| grammar.scope(scope_id))
            .map(str::to_owned)
        else {
            return (None, None);
        };
        if text.contains('$') {
            return (Some(substitute_scope_text(&text, line, captures)), None);
        }
        let template = self
            .scope_templates
            .intern_scope_template(&text, &mut self.scope_names);
        self.capture_scope_templates.insert(key, template);
        (None, Some(template))
    }

    fn push_scope_application(
        &mut self,
        stack: ScopeStackId,
        name: Option<&str>,
        template: Option<ScopeTemplateId>,
    ) -> ScopeStackId {
        if let Some(template) = template {
            self.scope_stacks.push_template(
                stack,
                template,
                &self.scope_templates,
                &self.scope_names,
            )
        } else if let Some(name) = name {
            self.push_scope_text_id(stack, name)
        } else {
            stack
        }
    }

    fn push_scope_prefix_once_id(&mut self, stack: ScopeStackId, text: &str) -> ScopeStackId {
        let template = self
            .scope_templates
            .intern_prefix_template(text, &mut self.scope_names);
        self.scope_stacks.push_template_once(
            stack,
            template,
            &self.scope_templates,
            &self.scope_names,
        )
    }

    fn push_token(
        &self,
        tokens: &mut Vec<CompactScopedToken>,
        mut range: Range<usize>,
        stack: ScopeStackId,
    ) {
        // Token production is monotone. Ordered capture handling can revisit
        // an overlapping group after a nested capture has already emitted its
        // range; vscode-textmate's LineTokens ignores that covered prefix.
        if let Some(last) = tokens.last() {
            range.start = range.start.max(last.range.end);
        }
        if range.start >= range.end {
            return;
        }
        if let Some(last) = tokens.last_mut()
            && last.range.end == range.start
            && last.stack == stack
        {
            last.range.end = range.end;
            return;
        }
        tokens.push(CompactScopedToken { range, stack });
    }
}

#[derive(Debug, Clone)]
struct CandidateSet {
    blueprint: Arc<CandidateBlueprint>,
    active_stack_id: ScopeStackId,
    end_stack_id: ScopeStackId,
}

/// Capture nesting is almost always one or two levels. Keep the common
/// ordered-capture stack inline so capture emission does not allocate per
/// match; pathological grammars retain an unbounded overflow path.
#[derive(Debug, Default)]
struct CaptureScopeStack {
    inline: [(ScopeStackId, usize); 8],
    inline_len: usize,
    overflow: Vec<(ScopeStackId, usize)>,
}

impl CaptureScopeStack {
    fn last(&self) -> Option<&(ScopeStackId, usize)> {
        self.overflow.last().or_else(|| {
            self.inline_len
                .checked_sub(1)
                .map(|index| &self.inline[index])
        })
    }

    fn push(&mut self, value: (ScopeStackId, usize)) {
        if self.inline_len < self.inline.len() && self.overflow.is_empty() {
            self.inline[self.inline_len] = value;
            self.inline_len += 1;
        } else {
            self.overflow.push(value);
        }
    }

    fn pop(&mut self) -> Option<(ScopeStackId, usize)> {
        if let Some(value) = self.overflow.pop() {
            Some(value)
        } else if self.inline_len != 0 {
            self.inline_len -= 1;
            Some(self.inline[self.inline_len])
        } else {
            None
        }
    }
}

impl Deref for CandidateSet {
    type Target = CandidateBlueprint;

    fn deref(&self) -> &Self::Target {
        &self.blueprint
    }
}

#[derive(Debug)]
struct CandidateBlueprint {
    candidates: Vec<Candidate>,
    matchers: Vec<Arc<CompiledPattern>>,
    pattern_set_search: Option<PatternSetMatcher>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateBlueprintKey {
    source: CandidateSourceKey,
    injection_outcome: InjectionOutcomeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CurrentScopeStackKey {
    root: GrammarId,
    base_stack: ScopeStackId,
    frame_stack: InternedFrameStackId,
}

#[derive(Debug, Clone, Copy)]
struct CachedCurrentScopeStackIds {
    active_stack_id: ScopeStackId,
    end_stack_id: ScopeStackId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CandidateSourceKey {
    Root(GrammarId),
    Frame {
        grammar_id: GrammarId,
        base_grammar_id: GrammarId,
        rule_id: RuleId,
        scope_prefix: Option<Arc<str>>,
        end_pattern: Option<Arc<str>>,
        end_pattern_id: Option<PatternId>,
        apply_end_pattern_last: bool,
    },
}

impl CandidateSourceKey {
    fn for_state(root: GrammarId, state: &TokenizerState) -> Self {
        state
            .frames
            .last()
            .map_or(Self::Root(root), |frame| Self::Frame {
                grammar_id: frame.grammar_id,
                base_grammar_id: frame.base_grammar_id,
                rule_id: frame.rule_id,
                scope_prefix: frame.scope_prefix.clone(),
                end_pattern: frame.end_pattern.clone(),
                end_pattern_id: frame.end_pattern_id,
                apply_end_pattern_last: frame.apply_end_pattern_last,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DynamicMatcherKey {
    pattern: String,
    live_captures: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InlineCandidateCacheKey {
    grammar_id: GrammarId,
    patterns: Vec<RuleRef>,
    compound_patterns: bool,
    state: TokenizerState,
    base_stack: ScopeStackId,
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
    pattern_id: Option<(GrammarId, PatternId)>,
    scope_prefix: Option<String>,
    kind: CandidateKind,
}

#[derive(Debug, Clone)]
enum CandidateKind {
    Match {
        grammar_id: GrammarId,
        name: Option<String>,
        name_template: Option<ScopeTemplateId>,
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

#[cfg(test)]
fn candidate_requires_capture_replay(candidate: &Candidate) -> bool {
    match &candidate.kind {
        CandidateKind::Match { name, captures, .. } => {
            !captures.entries.is_empty() || name.as_ref().is_some_and(|name| name.contains('$'))
        }
        CandidateKind::End { captures, .. } => !captures.entries.is_empty(),
        CandidateKind::BeginEnd { .. } | CandidateKind::BeginWhile { .. } => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InjectionCandidate {
    grammar_id: GrammarId,
    patterns: Vec<RuleRef>,
}

#[derive(Debug, Clone)]
struct CompiledInjectionSelector {
    grammar_id: GrammarId,
    priority: InjectionPriority,
    patterns: Vec<RuleRef>,
    selector_tokens: Arc<[SelectorToken]>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
struct InjectionOutcome {
    left: Vec<InjectionCandidate>,
    right: Vec<InjectionCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InjectionOutcomeId(u64);

#[derive(Debug, Clone, Default)]
struct InjectionOutcomeInterner {
    ids: HashMap<InjectionOutcome, InjectionOutcomeId>,
    values: HashMap<InjectionOutcomeId, Arc<InjectionOutcome>>,
    next_id: u64,
}

impl InjectionOutcomeInterner {
    fn len(&self) -> usize {
        self.ids.len()
    }

    fn contains(&self, outcome: &InjectionOutcome) -> bool {
        self.ids.contains_key(outcome)
    }

    fn intern(&mut self, outcome: InjectionOutcome) -> (InjectionOutcomeId, Arc<InjectionOutcome>) {
        if let Some(id) = self.ids.get(&outcome).copied() {
            return (
                id,
                self.values
                    .get(&id)
                    .cloned()
                    .expect("interned injection outcome has a value"),
            );
        }
        let id = InjectionOutcomeId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let value = Arc::new(outcome.clone());
        self.ids.insert(outcome, id);
        self.values.insert(id, value.clone());
        (id, value)
    }

    fn clear(&mut self) {
        self.ids.clear();
        self.values.clear();
    }
}

#[derive(Debug, Clone)]
struct StateInterner {
    states: Vec<TokenizerState>,
    ids: HashMap<TokenizerState, StateId, BuildHasherDefault<StateIdentityHasher>>,
}

#[derive(Debug, Clone, Default)]
struct StateIdentityHasher(u64);

impl Hasher for StateIdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    fn write_u32(&mut self, value: u32) {
        self.0 = u64::from(value);
    }
}

impl StateInterner {
    fn new() -> Self {
        let mut interner = Self {
            states: Vec::new(),
            ids: HashMap::with_hasher(BuildHasherDefault::default()),
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

fn add_scope_capture_refs(scope: Option<&str>, live: &mut Vec<u32>) {
    let Some(scope) = scope.filter(|scope| scope.contains('$')) else {
        return;
    };
    let bytes = scope.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += scope[index..]
                .chars()
                .next()
                .expect("valid scope char")
                .len_utf8();
            continue;
        }
        if index + 1 < bytes.len() && bytes[index + 1] == b'{' {
            if let Some(close_offset) = scope[index + 2..].find('}') {
                let body_start = index + 2;
                let body_end = body_start + close_offset;
                if let Some((group, _)) = parse_scope_placeholder_body(&scope[body_start..body_end])
                {
                    if let Ok(group) = u32::try_from(group) {
                        live.push(group);
                    }
                    index = body_end + 1;
                    continue;
                }
            }
        } else if index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if let Ok(group) = scope[index + 1..end].parse::<u32>() {
                live.push(group);
                index = end;
                continue;
            }
        }
        index += 1;
    }
}

fn add_end_pattern_capture_refs(pattern: &str, live: &mut Vec<u32>) {
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            continue;
        }
        let Some(next @ '1'..='9') = chars.peek().copied() else {
            // Consume the escaped character exactly as substitution does, so
            // `\\\\1` remains a literal backslash followed by `1`.
            chars.next();
            continue;
        };
        let mut digits = String::new();
        digits.push(next);
        chars.next();
        while let Some(digit @ '0'..='9') = chars.peek().copied() {
            digits.push(digit);
            chars.next();
        }
        if let Ok(group) = digits.parse::<u32>() {
            live.push(group);
        }
    }
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

fn fallback_call_budget(source_bytes: usize) -> u64 {
    MIN_FALLBACK_STEPS_PER_CALL.max(
        u64::try_from(source_bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(FALLBACK_STEPS_PER_SOURCE_BYTE),
    )
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

fn plain_compact_tokens(parse_text: &str, stack: ScopeStackId) -> Vec<CompactScopedToken> {
    if parse_text.is_empty() {
        Vec::new()
    } else {
        vec![CompactScopedToken {
            range: 0..parse_text.len(),
            stack,
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

fn compile_injection_selectors(grammars: &GrammarSet) -> Vec<CompiledInjectionSelector> {
    grammars
        .grammars()
        .iter()
        .flat_map(|grammar| {
            grammar
                .injections
                .iter()
                .map(|injection| CompiledInjectionSelector {
                    grammar_id: grammar.id,
                    priority: injection.priority,
                    patterns: injection.patterns.clone(),
                    selector_tokens: tokenize_selector(&injection.selector_body).into(),
                })
        })
        .collect()
}

#[cfg(test)]
fn selector_matches(selector: &str, stack: &[String]) -> bool {
    let tokens = tokenize_selector(selector);
    selector_tokens_match(&tokens, stack)
}

fn selector_tokens_match(tokens: &[SelectorToken], stack: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let mut parser = SelectorParser {
        tokens,
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

    fn continuation_frame(rule: u32) -> Frame {
        Frame {
            grammar_id: GrammarId(1),
            base_grammar_id: GrammarId(2),
            rule_id: RuleId(rule),
            scope_prefix: Some(Arc::from(format!("meta.prefix.{rule}"))),
            name: Some(Arc::from(format!("meta.name.{rule}"))),
            content_name: None,
            end_pattern: Some(Arc::from(format!("end-{rule}"))),
            end_pattern_id: Some(PatternId(rule)),
            while_pattern: None,
            while_pattern_id: None,
            end_captures: Arc::new(CaptureSpec::default()),
            while_captures: Arc::new(CaptureSpec::default()),
            patterns: Arc::from([]),
            apply_end_pattern_last: rule.is_multiple_of(2),
            begin_captured_eol: false,
            identity_hash: 0,
            stack_hash: 0,
            state_hash: 0,
            interned_stack_id: InternedFrameStackId::default(),
        }
    }

    #[test]
    fn parent_linked_frame_stack_preserves_prefixes_hashes_and_exact_equality() {
        let mut state = TokenizerState::default();
        let mut independently_built = TokenizerState::default();
        let mut expected_state_hash = 0x811c9dc5u32;
        for rule in 0..300 {
            let frame = continuation_frame(rule);
            expected_state_hash = fnv_mix(expected_state_hash, 1);
            expected_state_hash = fnv_mix(expected_state_hash, 2);
            expected_state_hash = fnv_mix(expected_state_hash, rule);
            expected_state_hash =
                fnv_mix_opt_str(expected_state_hash, frame.scope_prefix.as_deref());
            expected_state_hash = fnv_mix_opt_str(expected_state_hash, frame.name.as_deref());
            expected_state_hash = fnv_mix_opt_str(expected_state_hash, None);
            expected_state_hash =
                fnv_mix_opt_str(expected_state_hash, frame.end_pattern.as_deref());
            expected_state_hash = fnv_mix_opt_str(expected_state_hash, None);
            state.push_frame(frame);
            independently_built.push_frame(continuation_frame(rule));
        }
        assert_eq!(state.depth(), 300);
        assert_eq!(state.state_id(), StateId(expected_state_hash));
        assert_eq!(state, independently_built);
        assert_eq!(
            state
                .frames
                .iter()
                .map(|frame| frame.rule_id.0)
                .collect::<Vec<_>>(),
            (0..300).collect::<Vec<_>>()
        );

        let prefix = state.prefix(33);
        assert_eq!(prefix.depth(), 33);
        assert_eq!(prefix.frames.last().unwrap().rule_id, RuleId(32));
        let mut changed = state.clone();
        changed.truncate_frames(31);
        changed.push_frame(continuation_frame(500));
        assert_eq!(changed.depth(), 32);
        assert_eq!(changed.frames.last().unwrap().rule_id, RuleId(500));
        assert_eq!(state.depth(), 300, "persistent ancestor was mutated");
        assert_ne!(changed, state);
    }

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
    fn candidate_blueprint_reuses_structure_across_distinct_scope_stacks() {
        let grammar = r##"{
            "scopeName": "source.blueprint-stacks",
            "patterns": [{
                "begin": "^([a-z]+):$",
                "end": "^end$",
                "name": "meta.block.$1.blueprint-stacks",
                "patterns": [
                    {"match":"(x)", "captures":{"1":{"name":"keyword.x.blueprint-stacks"}}},
                    {"match":"y", "name":"keyword.y.blueprint-stacks"}
                ]
            }]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        let alpha = tokenizer.tokenize_line_scopes("alpha:", TokenizerState::default());
        let beta = tokenizer.tokenize_line_scopes("beta:", TokenizerState::default());
        assert_ne!(alpha.exit_state_id, beta.exit_state_id);

        let alpha_body = tokenizer.tokenize_line_scopes("x", alpha.state);
        let beta_body = tokenizer.tokenize_line_scopes("x", beta.state);
        let alpha_set = tokenizer
            .candidate_cache
            .get(&alpha_body.entry_state_id)
            .expect("alpha candidates");
        let beta_set = tokenizer
            .candidate_cache
            .get(&beta_body.entry_state_id)
            .expect("beta candidates");

        assert!(Arc::ptr_eq(&alpha_set.blueprint, &beta_set.blueprint));
        assert_ne!(alpha_set.active_stack_id, beta_set.active_stack_id);
        assert_ne!(alpha_set.end_stack_id, beta_set.end_stack_id);
        assert!(
            alpha_body.tokens[0]
                .scopes
                .contains(&"meta.block.alpha.blueprint-stacks".to_owned())
        );
        assert!(
            beta_body.tokens[0]
                .scopes
                .contains(&"meta.block.beta.blueprint-stacks".to_owned())
        );
        assert!(
            alpha_body.tokens[0]
                .scopes
                .contains(&"keyword.x.blueprint-stacks".to_owned())
        );
        assert!(
            beta_body.tokens[0]
                .scopes
                .contains(&"keyword.x.blueprint-stacks".to_owned())
        );

        let counters = tokenizer.counters();
        assert_eq!(counters.pattern_set_construction_count, 1, "{counters:#?}");
    }

    #[test]
    fn candidate_blueprint_key_keeps_dynamic_end_patterns_exact() {
        let grammar = r##"{
            "scopeName": "source.blueprint-dynamic-end",
            "patterns": [{
                "begin": "^<<([A-Z]+)$",
                "end": "^\\1$",
                "patterns": [{"match":"body", "name":"string.body.blueprint-dynamic-end"}]
            }]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();

        let foo = tokenizer.tokenize_line_scopes("<<FOO", TokenizerState::default());
        let bar = tokenizer.tokenize_line_scopes("<<BAR", TokenizerState::default());
        tokenizer.tokenize_line_scopes("body", foo.state);
        tokenizer.tokenize_line_scopes("body", bar.state);

        let foo_set = tokenizer.candidate_cache.get(&foo.exit_state_id).unwrap();
        let bar_set = tokenizer.candidate_cache.get(&bar.exit_state_id).unwrap();
        assert!(!Arc::ptr_eq(&foo_set.blueprint, &bar_set.blueprint));
        assert_eq!(foo_set.candidates[0].pattern, "^FOO$");
        assert_eq!(bar_set.candidates[0].pattern, "^BAR$");
    }

    #[test]
    fn candidate_blueprint_key_uses_exact_injection_outcome() {
        let grammar = r##"{
            "scopeName": "source.blueprint-injections",
            "patterns": [{
                "begin": "^([a-z]+):$",
                "end": "^end$",
                "name": "meta.$1.blueprint-injections",
                "patterns": [{"match":"!", "name":"plain.bang.blueprint-injections"}]
            }],
            "injections": {
                "L:meta.alpha.blueprint-injections": {
                    "match":"!", "name":"injected.bang.blueprint-injections"
                }
            }
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();

        let alpha = tokenizer.tokenize_line_scopes("alpha:", TokenizerState::default());
        let beta = tokenizer.tokenize_line_scopes("beta:", TokenizerState::default());
        let alpha_body = tokenizer.tokenize_line_scopes("!", alpha.state);
        let beta_body = tokenizer.tokenize_line_scopes("!", beta.state);

        let alpha_set = tokenizer
            .candidate_cache
            .get(&alpha_body.entry_state_id)
            .unwrap();
        let beta_set = tokenizer
            .candidate_cache
            .get(&beta_body.entry_state_id)
            .unwrap();
        assert!(!Arc::ptr_eq(&alpha_set.blueprint, &beta_set.blueprint));
        assert!(
            alpha_body.tokens[0]
                .scopes
                .contains(&"injected.bang.blueprint-injections".to_owned())
        );
        assert!(
            beta_body.tokens[0]
                .scopes
                .contains(&"plain.bang.blueprint-injections".to_owned())
        );
    }

    #[test]
    fn candidate_sets_reuse_compiled_patterns() {
        let grammar = r##"{
            "scopeName": "source.compiled-candidates",
            "patterns": [
                {"match":"alpha", "name":"keyword.alpha.compiled-candidates"},
                {"match":"beta", "name":"keyword.beta.compiled-candidates"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("alpha", TokenizerState::default());
        tokenizer.clear_candidate_cache();
        tokenizer.tokenize_line_scopes("beta", TokenizerState::default());

        let counters = tokenizer.counters();
        assert_eq!(counters.regex_compile_count, 2, "{counters:#?}");
        assert_eq!(counters.pattern_set_construction_count, 2, "{counters:#?}");
    }

    #[test]
    fn warm_candidate_entry_does_not_recompile_or_rebuild_pattern_set() {
        let grammar = r##"{
            "scopeName": "source.warm-candidates",
            "patterns": [
                {"match":"alpha", "name":"keyword.alpha.warm-candidates"},
                {"match":"beta", "name":"keyword.beta.warm-candidates"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("alpha", TokenizerState::default());
        tokenizer.tokenize_line_scopes("beta", TokenizerState::default());

        let counters = tokenizer.counters();
        assert_eq!(counters.regex_compile_count, 2, "{counters:#?}");
        assert_eq!(counters.pattern_set_construction_count, 1, "{counters:#?}");
        assert!(
            counters
                .pattern_compile_counts
                .iter()
                .all(|entry| entry.count == 1),
            "{counters:#?}"
        );
    }

    #[test]
    fn duplicate_pattern_text_keeps_distinct_static_pattern_identities() {
        let grammar = r##"{
            "scopeName": "source.duplicate-pattern-id",
            "patterns": [
                {"match":"x", "name":"keyword.first.duplicate-pattern-id"},
                {"match":"x", "name":"string.second.duplicate-pattern-id"}
            ]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        let line = tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        assert!(
            line.tokens[0]
                .scopes
                .contains(&"keyword.first.duplicate-pattern-id".to_owned())
        );
        assert!(
            !line.tokens[0]
                .scopes
                .contains(&"string.second.duplicate-pattern-id".to_owned())
        );

        let counters = tokenizer.counters();
        assert_eq!(counters.regex_compile_count, 2, "{counters:#?}");
        assert_eq!(counters.pattern_compile_counts.len(), 2, "{counters:#?}");
        assert_ne!(
            counters.pattern_compile_counts[0].pattern_id,
            counters.pattern_compile_counts[1].pattern_id
        );
    }

    #[test]
    fn dynamic_end_cache_reuses_only_equal_substitutions() {
        let grammar = r##"{
            "scopeName": "source.dynamic-compile-cache",
            "patterns": [{
                "begin":"^<<([A-Z]+)$",
                "end":"^\\1$",
                "name":"string.heredoc.dynamic-compile-cache"
            }]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        for marker in ["FOO", "BAR", "FOO"] {
            let begin =
                tokenizer.tokenize_line_scopes(&format!("<<{marker}"), TokenizerState::default());
            let end = tokenizer.tokenize_line_scopes(marker, begin.state);
            assert!(end.state.is_initial());
        }

        let counters = tokenizer.counters();
        assert_eq!(counters.regex_compile_count, 3, "{counters:#?}");
        let dynamic = counters
            .pattern_compile_counts
            .iter()
            .filter(|entry| entry.pattern_id.is_none())
            .collect::<Vec<_>>();
        assert_eq!(dynamic.len(), 2, "{counters:#?}");
        assert!(dynamic.iter().all(|entry| entry.count == 1));
    }

    #[test]
    fn inline_candidate_sets_persist_across_capture_retokenization() {
        let grammar = r##"{
            "scopeName": "source.inline-cache",
            "patterns": [{
                "match":"(x)",
                "captures": {
                    "1": {"patterns": [
                        {"match":"x", "name":"keyword.x.inline-cache"}
                    ]}
                }
            }]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        tokenizer.set_counters_enabled(true);

        tokenizer.tokenize_line_scopes("x", TokenizerState::default());
        tokenizer.tokenize_line_scopes("x", TokenizerState::default());

        let counters = tokenizer.counters();
        assert_eq!(
            counters.inline_candidate_set_construction_count, 1,
            "{counters:#?}"
        );
        assert_eq!(counters.regex_compile_count, 2, "{counters:#?}");
    }

    #[test]
    fn capture_replay_is_skipped_only_for_static_capture_free_matches() {
        let candidate = |name: &str| Candidate {
            order: 0,
            base_grammar_id: GrammarId(0),
            pattern: "pattern".to_owned(),
            pattern_id: None,
            scope_prefix: None,
            kind: CandidateKind::Match {
                grammar_id: GrammarId(0),
                name: Some(name.to_owned()),
                name_template: None,
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
    fn capture_reference_scanners_match_substitution_syntax() {
        let mut live = Vec::new();
        add_scope_capture_refs(
            Some("entity.$1.${2}.${3:/downcase}.${4:/upcase}.$bad"),
            &mut live,
        );
        assert_eq!(live, [1, 2, 3, 4]);

        live.clear();
        add_end_pattern_capture_refs(r"^\1-\12-\\1$", &mut live);
        assert_eq!(live, [1, 12]);
    }

    #[test]
    fn dynamic_matcher_cache_identity_includes_capture_liveness() {
        let mut tokenizer =
            TextMateTokenizer::from_grammar(r#"{"scopeName":"source.live-cache","patterns":[]}"#)
                .unwrap();
        let first = tokenizer.cached_dynamic_matcher_with_live_captures("(x)", vec![1]);
        let reused = tokenizer.cached_dynamic_matcher_with_live_captures("(x)", vec![1]);
        let distinct = tokenizer.cached_dynamic_matcher_with_live_captures("(x)", vec![]);
        assert!(Arc::ptr_eq(&first, &reused));
        assert!(!Arc::ptr_eq(&first, &distinct));
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
    fn retokenized_capture_does_not_inherit_overlapping_capture_scope() {
        let grammar = r##"{
            "scopeName": "source.capture-order",
            "patterns": [{
                "match": "(foo)",
                "captures": {
                    "0": {"name": "meta.head.capture-order"},
                    "1": {"patterns": [
                        {"match": "foo", "name": "entity.name.capture-order"}
                    ]}
                }
            }]
        }"##;
        let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
        let line = tokenizer.tokenize_line_scopes("foo", TokenizerState::default());
        assert_eq!(line.tokens.len(), 1, "{:#?}", line.tokens);
        assert!(
            line.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "entity.name.capture-order"),
            "{:#?}",
            line.tokens
        );
        assert!(
            !line.tokens[0]
                .scopes
                .iter()
                .any(|scope| scope == "meta.head.capture-order"),
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
    fn source_budget_allows_exact_exhaustion_and_zero_step_followups() {
        let mut tokenizer = core_tokenizer("rust");
        tokenizer.fallback_call_budget_remaining = Some(7);

        assert!(tokenizer.consume_fallback_call_budget(7));
        assert_eq!(tokenizer.fallback_call_budget_remaining, Some(0));
        assert!(tokenizer.consume_fallback_call_budget(0));
        assert!(!tokenizer.consume_fallback_call_budget(1));
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
