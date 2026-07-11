use std::{
    ops::{Deref, DerefMut, Range},
    sync::{Arc, OnceLock},
};
use unicode_segmentation::UnicodeSegmentation;

use super::ast::{
    AnchorKind, Ast, AstPathStep, Backref, CharClass, ClassAtom, LookKind, ParsedRegex,
    PerlClassKind, RegexFlags, has_case_insensitive_scope, parse,
};
use super::bytecode::{BytecodeScratch, CompileError, Program};
use super::prefilter::Prefilter;
use super::{AnchorContext, MatchResult, Matcher};

pub(crate) const DEFAULT_STEP_BUDGET: usize = 100_000;
const STATE_LIMIT: usize = 2048;
// Most branching expressions stay below this empirically measured fanout;
// reserving it on the first split avoids repeated growth in hot alternations.
const INITIAL_FANOUT_CAPACITY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepBudget {
    limit: usize,
    remaining: usize,
}

impl StepBudget {
    pub fn new(steps: usize) -> Self {
        Self {
            limit: steps,
            remaining: steps,
        }
    }

    pub fn step(&mut self) -> Result<(), BudgetExceeded> {
        if self.remaining == 0 {
            return Err(BudgetExceeded);
        }
        self.remaining -= 1;
        Ok(())
    }

    pub fn used(&self) -> usize {
        self.limit - self.remaining
    }

    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetExceeded;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackError {
    BudgetExceeded { steps: usize },
    InvalidStart { from: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackReport {
    pub result: Option<MatchResult>,
    pub steps: usize,
}

#[derive(Debug, Clone)]
pub struct FallbackMatcher {
    parsed: Arc<ParsedRegex>,
    bytecode: OnceLock<Option<Arc<Program>>>,
    prefilter: Prefilter,
    start_bytes: Option<StartByteSet>,
    /// True when the pattern can match the empty string at a start position.
    /// Used with `start_bytes` so we still try `from` for patterns like `a?`.
    start_nullable: bool,
    start_hint: StartHint,
    budget: usize,
    /// Process-unique id keying scan-local prefilter cursors.
    prefilter_slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartHint {
    Unanchored,
    LineStart,
    TextStart,
    Continuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VmState {
    pos: usize,
    captures: Vec<Option<Range<usize>>>,
}

/// Most AST nodes produce either no state or one state. Keeping that common
/// result inline avoids a heap allocation at every successful VM step while
/// retaining a `Vec` for genuine backtracking fanout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VmStates {
    Empty,
    One(VmState),
    Many(Vec<VmState>),
}

/// Capture-free state set used by selection when captures cannot affect the
/// match. Keeping positions inline avoids carrying and cloning empty capture
/// vectors through the recursive evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PositionStates {
    Empty,
    One(usize),
    Many(Vec<usize>),
}

impl PositionStates {
    fn empty() -> Self {
        Self::Empty
    }

    fn one(position: usize) -> Self {
        Self::One(position)
    }

    fn from_vec(mut positions: Vec<usize>) -> Self {
        match positions.len() {
            0 => Self::Empty,
            1 => Self::One(positions.pop().expect("length checked")),
            _ => Self::Many(positions),
        }
    }

    fn push(&mut self, position: usize) {
        match self {
            Self::Empty => *self = Self::One(position),
            Self::One(_) => {
                let Self::One(first) = std::mem::replace(self, Self::Empty) else {
                    unreachable!("variant checked")
                };
                let mut positions = Vec::with_capacity(INITIAL_FANOUT_CAPACITY);
                positions.push(first);
                positions.push(position);
                *self = Self::Many(positions);
            }
            Self::Many(positions) => positions.push(position),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    fn into_first(self) -> Option<usize> {
        match self {
            Self::Empty => None,
            Self::One(position) => Some(position),
            Self::Many(positions) => positions.into_iter().next(),
        }
    }
}

enum PositionStatesIntoIter {
    Empty,
    One(Option<usize>),
    Many(std::vec::IntoIter<usize>),
}

impl Iterator for PositionStatesIntoIter {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::One(position) => position.take(),
            Self::Many(positions) => positions.next(),
        }
    }
}

impl IntoIterator for PositionStates {
    type Item = usize;
    type IntoIter = PositionStatesIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Empty => PositionStatesIntoIter::Empty,
            Self::One(position) => PositionStatesIntoIter::One(Some(position)),
            Self::Many(positions) => PositionStatesIntoIter::Many(positions.into_iter()),
        }
    }
}

impl VmStates {
    fn empty() -> Self {
        Self::Empty
    }

    fn one(state: VmState) -> Self {
        Self::One(state)
    }

    fn from_vec(mut states: Vec<VmState>) -> Self {
        match states.len() {
            0 => Self::Empty,
            1 => Self::One(states.pop().expect("length checked")),
            _ => Self::Many(states),
        }
    }

    fn push(&mut self, state: VmState) {
        match self {
            Self::Empty => *self = Self::One(state),
            Self::One(_) => {
                let Self::One(first) = std::mem::replace(self, Self::Empty) else {
                    unreachable!("variant checked")
                };
                let mut states = Vec::with_capacity(INITIAL_FANOUT_CAPACITY);
                states.push(first);
                states.push(state);
                *self = Self::Many(states);
            }
            Self::Many(states) => states.push(state),
        }
    }

    fn into_first(self) -> Option<VmState> {
        match self {
            Self::Empty => None,
            Self::One(state) => Some(state),
            Self::Many(states) => states.into_iter().next(),
        }
    }
}

impl Deref for VmStates {
    type Target = [VmState];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Empty => &[],
            Self::One(state) => std::slice::from_ref(state),
            Self::Many(states) => states,
        }
    }
}

impl DerefMut for VmStates {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Empty => &mut [],
            Self::One(state) => std::slice::from_mut(state),
            Self::Many(states) => states,
        }
    }
}

enum VmStatesIntoIter {
    Empty,
    One(Option<VmState>),
    Many(std::vec::IntoIter<VmState>),
}

impl Iterator for VmStatesIntoIter {
    type Item = VmState;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::One(state) => state.take(),
            Self::Many(states) => states.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Empty => (0, Some(0)),
            Self::One(Some(_)) => (1, Some(1)),
            Self::One(None) => (0, Some(0)),
            Self::Many(states) => states.size_hint(),
        }
    }
}

impl IntoIterator for VmStates {
    type Item = VmState;
    type IntoIter = VmStatesIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Empty => VmStatesIntoIter::Empty,
            Self::One(state) => VmStatesIntoIter::One(Some(state)),
            Self::Many(states) => VmStatesIntoIter::Many(states.into_iter()),
        }
    }
}

impl FallbackMatcher {
    pub fn new(pattern: &str) -> Self {
        Self::with_budget(pattern, DEFAULT_STEP_BUDGET)
    }

    pub fn with_budget(pattern: &str, budget: usize) -> Self {
        Self::from_parsed(Arc::new(parse(pattern)), budget)
    }

    pub(crate) fn from_parsed(parsed: Arc<ParsedRegex>, budget: usize) -> Self {
        let prefilter = Prefilter::from_regex(&parsed);
        let (start_bytes, start_nullable) =
            if parsed.flags.case_insensitive || has_case_insensitive_scope(&parsed.ast) {
                (None, false)
            } else {
                match first_start_bytes(&parsed.ast) {
                    Some(info) if !info.bytes.is_empty() && info.bytes.len() < 128 => {
                        (Some(info.bytes), info.nullable)
                    }
                    Some(info) => (None, info.nullable),
                    None => (None, false),
                }
            };
        let start_hint = start_hint(&parsed.ast);
        static NEXT_PREFILTER_SLOT: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0);
        let prefilter_slot = if prefilter.is_enabled() {
            NEXT_PREFILTER_SLOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        } else {
            u32::MAX
        };
        Self {
            parsed,
            bytecode: OnceLock::new(),
            prefilter,
            start_bytes,
            start_nullable,
            start_hint,
            budget,
            prefilter_slot,
        }
    }

    pub fn parsed(&self) -> &ParsedRegex {
        &self.parsed
    }

    fn bytecode(&self) -> Option<&Program> {
        self.bytecode
            .get_or_init(|| {
                Program::is_beneficial(&self.parsed)
                    .then(|| {
                        // Subroutine calls need capture slots and routine
                        // entries even for position selection; compile with
                        // the minimal internal layout so those patterns still
                        // avoid the recursive VM. Selection discards captures.
                        // Backreference-only patterns measured faster on the
                        // recursive path and keep it.
                        Program::compile(&self.parsed)
                            .or_else(|error| match error {
                                CompileError::Subroutine | CompileError::Conditional => {
                                    Program::compile_captures(&self.parsed, &[])
                                }
                                other => Err(other),
                            })
                            .ok()
                            .map(Arc::new)
                    })
                    .flatten()
            })
            .as_deref()
    }

    fn active_bytecode(&self) -> Option<&Program> {
        (position_engine_mode() != PositionEngineMode::Recursive)
            .then(|| self.bytecode())
            .flatten()
    }

    pub fn prefilter_may_match(&self, line: &str, from: usize) -> Option<bool> {
        self.prefilter
            .is_enabled()
            .then(|| self.prefilter.may_match(line, from))
    }

    pub(crate) fn restricted_start_bytes(&self) -> Option<Vec<u8>> {
        let bytes = self.start_bytes.as_ref().filter(|_| !self.start_nullable)?;
        Some((0u8..=0x7f).filter(|byte| bytes.contains(*byte)).collect())
    }

    pub fn try_find(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Result<FallbackReport, FallbackError> {
        self.try_find_with_capture_count(line, from, ctx, self.parsed.capture_count as usize + 1)
    }

    pub(crate) fn try_find_for_selection(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
    ) -> Result<FallbackReport, FallbackError> {
        let capture_count = if self.active_bytecode().is_some() {
            0
        } else if self.parsed.features.backreference
            || self.parsed.features.conditional
            || self.parsed.features.subroutine
        {
            self.parsed.capture_count as usize + 1
        } else {
            0
        };
        let mut report = self.try_find_with_capture_count(line, from, ctx, capture_count)?;
        if let Some(result) = &mut report.result {
            result.captures.clear();
        }
        Ok(report)
    }

    fn try_find_with_capture_count(
        &self,
        line: &str,
        from: usize,
        ctx: AnchorContext,
        capture_count: usize,
    ) -> Result<FallbackReport, FallbackError> {
        if !line.is_char_boundary(from) {
            return Err(FallbackError::InvalidStart { from });
        }
        if !self.prefilter.may_match(line, from) {
            return Ok(FallbackReport {
                result: None,
                steps: 0,
            });
        }
        let mut budget = StepBudget::new(self.budget);
        if self.start_hint == StartHint::Unanchored
            && let Some(start_bytes) = &self.start_bytes
        {
            // Nullable patterns (e.g. `a?`, optional prefixes) can match empty
            // at `from` even when no start-byte candidate exists later.
            if self.start_nullable {
                if let Some(result) = self.try_match_at_start_with_capture_count(
                    line,
                    from,
                    ctx,
                    &mut budget,
                    capture_count,
                )? {
                    return Ok(FallbackReport {
                        result: Some(result),
                        steps: budget.used(),
                    });
                }
            }
            for (offset, byte) in line
                .as_bytes()
                .get(from..)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let start = from + offset;
                if self.start_nullable && start == from {
                    continue;
                }
                if !start_bytes.contains(*byte) || !line.is_char_boundary(start) {
                    continue;
                }
                if let Some(result) = self.try_match_at_start_with_capture_count(
                    line,
                    start,
                    ctx,
                    &mut budget,
                    capture_count,
                )? {
                    return Ok(FallbackReport {
                        result: Some(result),
                        steps: budget.used(),
                    });
                }
            }
            if has_zero_width_line_end_branch(&self.parsed.ast) {
                let line_end = line.strip_suffix('\n').map_or(line.len(), str::len);
                if line_end >= from
                    && let Some(result) = self.try_match_at_start_with_capture_count(
                        line,
                        line_end,
                        ctx,
                        &mut budget,
                        capture_count,
                    )?
                {
                    return Ok(FallbackReport {
                        result: Some(result),
                        steps: budget.used(),
                    });
                }
            }
            return Ok(FallbackReport {
                result: None,
                steps: budget.used(),
            });
        }
        let positions = self.start_positions(line, from, ctx);
        for start in positions {
            if let Some(result) = self.try_match_at_start_with_capture_count(
                line,
                start,
                ctx,
                &mut budget,
                capture_count,
            )? {
                return Ok(FallbackReport {
                    result: Some(result),
                    steps: budget.used(),
                });
            }
        }
        Ok(FallbackReport {
            result: None,
            steps: budget.used(),
        })
    }

    /// Search-only variant used while selecting among a set of rules. Capture
    /// extraction is replayed only for the winning pattern by the tokenizer.
    pub(crate) fn try_find_at_without_captures_with_scratch(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        scratch: &mut BytecodeScratch,
    ) -> Result<FallbackReport, FallbackError> {
        let capture_count = if self.active_bytecode().is_some() {
            0
        } else if self.parsed.features.backreference
            || self.parsed.features.conditional
            || self.parsed.features.subroutine
        {
            self.parsed.capture_count as usize + 1
        } else {
            0
        };
        self.try_find_at_with_capture_count_and_scratch(
            line,
            start,
            ctx,
            capture_count,
            Some(scratch),
        )
    }

    pub(crate) fn try_find_at(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
    ) -> Result<FallbackReport, FallbackError> {
        self.try_find_at_with_capture_count_and_scratch(
            line,
            start,
            ctx,
            self.parsed.capture_count as usize + 1,
            None,
        )
    }

    fn try_find_at_with_capture_count_and_scratch(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        capture_count: usize,
        scratch: Option<&mut BytecodeScratch>,
    ) -> Result<FallbackReport, FallbackError> {
        if !line.is_char_boundary(start) {
            return Err(FallbackError::InvalidStart { from: start });
        }
        let mut scratch = scratch;
        let prefilter_viable = match scratch.as_deref_mut() {
            Some(scratch) => scratch.prefilter_cursors().may_match(
                self.prefilter_slot,
                &self.prefilter,
                line,
                start,
            ),
            None => self.prefilter.may_match(line, start),
        };
        if !prefilter_viable {
            return Ok(FallbackReport {
                result: None,
                steps: 0,
            });
        }
        match self.start_hint {
            StartHint::LineStart if start != 0 => {
                return Ok(FallbackReport {
                    result: None,
                    steps: 0,
                });
            }
            StartHint::TextStart if start != 0 || !ctx.allow_a => {
                return Ok(FallbackReport {
                    result: None,
                    steps: 0,
                });
            }
            StartHint::Continuation if !ctx.allow_g || start != ctx.g_pos => {
                return Ok(FallbackReport {
                    result: None,
                    steps: 0,
                });
            }
            _ => {}
        }
        if let Some(bytes) = &self.start_bytes
            && !self.start_nullable
            && line
                .as_bytes()
                .get(start)
                .is_none_or(|byte| !bytes.contains(*byte))
        {
            return Ok(FallbackReport {
                result: None,
                steps: 0,
            });
        }
        let mut budget = StepBudget::new(self.budget);
        let result = if capture_count == 0
            && let Some(program) = self.active_bytecode()
        {
            let mut local_scratch = BytecodeScratch::default();
            let scratch = scratch.unwrap_or(&mut local_scratch);
            let end = match position_engine_mode() {
                PositionEngineMode::Recursive => {
                    recursive_position_end(&self.parsed, line, start, ctx, &mut budget)?
                }
                PositionEngineMode::Candidate => program
                    .execute(line, start, ctx, &mut budget, scratch)
                    .map_err(|_| FallbackError::BudgetExceeded {
                        steps: budget.used(),
                    })?,
                PositionEngineMode::Shadow => {
                    let mut candidate_budget = StepBudget::new(self.budget);
                    let candidate =
                        program.execute(line, start, ctx, &mut candidate_budget, scratch);
                    let recursive =
                        recursive_position_end(&self.parsed, line, start, ctx, &mut budget)?;
                    if candidate.as_ref().ok().copied() != Some(recursive) {
                        eprintln!(
                            "MARK_TEXTMATE_VM_MISMATCH pattern={:?} start={} recursive={:?} candidate={:?}",
                            self.parsed.source, start, recursive, candidate
                        );
                    }
                    recursive
                }
            };
            end.map(|end| MatchResult {
                start,
                end,
                captures: Vec::new(),
            })
        } else {
            self.try_match_at_start_with_capture_count(
                line,
                start,
                ctx,
                &mut budget,
                capture_count,
            )?
        };
        Ok(FallbackReport {
            result,
            steps: budget.used(),
        })
    }

    fn start_positions(&self, line: &str, from: usize, ctx: AnchorContext) -> Vec<usize> {
        match self.start_hint {
            StartHint::Unanchored => char_boundaries_from(line, from),
            StartHint::LineStart if from == 0 => vec![0],
            StartHint::TextStart if ctx.allow_a && from == 0 => vec![0],
            StartHint::Continuation if ctx.allow_g && ctx.g_pos >= from => vec![ctx.g_pos],
            StartHint::LineStart | StartHint::TextStart | StartHint::Continuation => Vec::new(),
        }
    }

    fn try_match_at_start_with_capture_count(
        &self,
        line: &str,
        start: usize,
        ctx: AnchorContext,
        budget: &mut StepBudget,
        capture_count: usize,
    ) -> Result<Option<MatchResult>, FallbackError> {
        if capture_count == 0
            && let Some(program) = self.active_bytecode()
        {
            let end = match position_engine_mode() {
                PositionEngineMode::Recursive => {
                    recursive_position_end(&self.parsed, line, start, ctx, budget)?
                }
                PositionEngineMode::Candidate => program
                    .execute(line, start, ctx, budget, &mut BytecodeScratch::default())
                    .map_err(|_| FallbackError::BudgetExceeded {
                        steps: budget.used(),
                    })?,
                PositionEngineMode::Shadow => {
                    let mut candidate_budget = StepBudget::new(self.budget);
                    let candidate = program.execute(
                        line,
                        start,
                        ctx,
                        &mut candidate_budget,
                        &mut BytecodeScratch::default(),
                    );
                    let recursive = recursive_position_end(&self.parsed, line, start, ctx, budget)?;
                    if candidate.as_ref().ok().copied() != Some(recursive) {
                        eprintln!(
                            "MARK_TEXTMATE_VM_MISMATCH pattern={:?} start={} recursive={:?} candidate={:?}",
                            self.parsed.source, start, recursive, candidate
                        );
                    }
                    recursive
                }
            };
            return Ok(end.map(|end| MatchResult {
                start,
                end,
                captures: Vec::new(),
            }));
        }
        if capture_count == 0 && position_only_eligible(&self.parsed) {
            let end = recursive_position_end(&self.parsed, line, start, ctx, budget)?;
            return Ok(end.map(|end| MatchResult {
                start,
                end,
                captures: Vec::new(),
            }));
        }
        let captures = vec![None; capture_count];
        let state = VmState {
            pos: start,
            captures,
        };
        let matches = match_node(
            &self.parsed.ast,
            line,
            state,
            ctx,
            self.parsed.flags,
            budget,
            &self.parsed,
        )
        .map_err(|_| FallbackError::BudgetExceeded {
            steps: budget.used(),
        })?;
        Ok(matches.into_first().map(|mut matched| {
            if let Some(whole) = matched.captures.get_mut(0) {
                *whole = Some(start..matched.pos);
            }
            MatchResult {
                start,
                end: matched.pos,
                captures: matched.captures,
            }
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartByteSet {
    lo: u64,
    hi: u64,
    len: usize,
}

impl StartByteSet {
    fn empty() -> Self {
        Self {
            lo: 0,
            hi: 0,
            len: 0,
        }
    }

    fn insert(&mut self, byte: u8) {
        if !self.contains(byte) {
            if byte < 64 {
                self.lo |= 1u64 << byte;
            } else {
                self.hi |= 1u64 << (byte - 64);
            }
            self.len += 1;
        }
    }

    fn extend(&mut self, other: &Self) {
        for byte in 0..=0x7f {
            if other.contains(byte) {
                self.insert(byte);
            }
        }
    }

    fn contains(&self, byte: u8) -> bool {
        if byte < 64 {
            self.lo & (1u64 << byte) != 0
        } else if byte < 128 {
            self.hi & (1u64 << (byte - 64)) != 0
        } else {
            false
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn len(&self) -> usize {
        self.len
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartBytes {
    bytes: StartByteSet,
    nullable: bool,
}

fn first_start_bytes(ast: &Ast) -> Option<StartBytes> {
    match ast {
        Ast::Empty | Ast::Anchor(_) => Some(StartBytes {
            bytes: StartByteSet::empty(),
            nullable: true,
        }),
        Ast::Literal(literal) => {
            let Some(ch) = literal.chars().next() else {
                return Some(StartBytes {
                    bytes: StartByteSet::empty(),
                    nullable: true,
                });
            };
            let mut bytes = StartByteSet::empty();
            if ch.is_ascii() {
                bytes.insert(ch as u8);
                Some(StartBytes {
                    bytes,
                    nullable: false,
                })
            } else {
                None
            }
        }
        Ast::Class(class) => class_start_bytes(class).map(|bytes| StartBytes {
            bytes,
            nullable: false,
        }),
        Ast::Concat(nodes) => concat_start_bytes(nodes),
        Ast::Alternation(branches) => alternation_start_bytes(branches),
        Ast::Repeat { node, min, max, .. } => {
            if *max == Some(0) {
                return Some(StartBytes {
                    bytes: StartByteSet::empty(),
                    nullable: true,
                });
            }
            let mut info = first_start_bytes(node)?;
            info.nullable = *min == 0 || info.nullable;
            Some(info)
        }
        Ast::Group { child, .. } | Ast::Flags { child, .. } => first_start_bytes(child),
        Ast::Look {
            kind: LookKind::Ahead,
            child,
        } => {
            let mut info = first_start_bytes(child)?;
            info.nullable = true;
            Some(info)
        }
        Ast::Look { .. } => Some(StartBytes {
            bytes: StartByteSet::empty(),
            nullable: true,
        }),
        Ast::Dot
        | Ast::Grapheme
        | Ast::Backref(_)
        | Ast::Conditional { .. }
        | Ast::Subroutine(_)
        | Ast::Unsupported(_) => None,
    }
}

fn has_zero_width_line_end_branch(ast: &Ast) -> bool {
    match ast {
        Ast::Anchor(AnchorKind::LineEnd) => true,
        Ast::Group { child, .. } | Ast::Flags { child, .. } => {
            has_zero_width_line_end_branch(child)
        }
        Ast::Alternation(branches) => branches.iter().any(has_zero_width_line_end_branch),
        _ => false,
    }
}

fn concat_start_bytes(nodes: &[Ast]) -> Option<StartBytes> {
    let mut out = StartBytes {
        bytes: StartByteSet::empty(),
        nullable: true,
    };
    for node in nodes {
        let info = first_start_bytes(node)?;
        out.bytes.extend(&info.bytes);
        out.nullable &= info.nullable;
        if !info.nullable {
            return Some(out);
        }
    }
    Some(out)
}

fn alternation_start_bytes(branches: &[Ast]) -> Option<StartBytes> {
    let mut out = StartBytes {
        bytes: StartByteSet::empty(),
        nullable: false,
    };
    for branch in branches {
        let info = first_start_bytes(branch)?;
        out.bytes.extend(&info.bytes);
        out.nullable |= info.nullable;
    }
    Some(out)
}

fn class_start_bytes(class: &CharClass) -> Option<StartByteSet> {
    if class.negated {
        return None;
    }
    let mut bytes = StartByteSet::empty();
    for atom in &class.atoms {
        match atom {
            ClassAtom::Char(ch) if ch.is_ascii() => bytes.insert(*ch as u8),
            ClassAtom::Char(_) => return None,
            ClassAtom::Range(start, end) if start.is_ascii() && end.is_ascii() => {
                let start = *start as u8;
                let end = *end as u8;
                for byte in start.min(end)..=start.max(end) {
                    bytes.insert(byte);
                }
            }
            ClassAtom::Range(..) => return None,
            ClassAtom::Perl(kind) => insert_perl_start_bytes(&mut bytes, *kind)?,
            ClassAtom::Posix { name, negated } => {
                insert_posix_start_bytes(&mut bytes, name, *negated)?
            }
            ClassAtom::Unicode { .. } => return None,
            ClassAtom::Nested(_) => return None,
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

fn insert_perl_start_bytes(bytes: &mut StartByteSet, kind: PerlClassKind) -> Option<()> {
    match kind {
        PerlClassKind::Digit => insert_range(bytes, b'0', b'9'),
        // The evaluator intentionally uses Unicode word/whitespace semantics.
        // An ASCII-only byte hint would reject valid non-ASCII starts.
        PerlClassKind::Word | PerlClassKind::Space => return None,
        PerlClassKind::HorizontalSpace => {
            insert_range(bytes, b'0', b'9');
            insert_range(bytes, b'A', b'F');
            insert_range(bytes, b'a', b'f');
        }
        PerlClassKind::VerticalSpace => {
            for byte in [b'\n', 0x0b, 0x0c, b'\r'] {
                bytes.insert(byte);
            }
        }
        PerlClassKind::NotDigit
        | PerlClassKind::NotWord
        | PerlClassKind::NotSpace
        | PerlClassKind::NotHorizontalSpace
        | PerlClassKind::NotVerticalSpace
        | PerlClassKind::NotNewline => return None,
    }
    Some(())
}

fn insert_posix_start_bytes(bytes: &mut StartByteSet, name: &str, negated: bool) -> Option<()> {
    if negated {
        return None;
    }
    match name {
        "digit" => insert_range(bytes, b'0', b'9'),
        "xdigit" => {
            insert_range(bytes, b'0', b'9');
            insert_range(bytes, b'A', b'F');
            insert_range(bytes, b'a', b'f');
        }
        // These predicates are Unicode-aware in `posix_class_contains`.
        "alpha" | "alnum" | "lower" | "upper" | "word" | "space" => return None,
        "blank" => {
            bytes.insert(b'\t');
            bytes.insert(b' ');
        }
        "ascii" => insert_range(bytes, 0, 0x7f),
        _ => return None,
    }
    Some(())
}

fn insert_range(bytes: &mut StartByteSet, start: u8, end: u8) {
    for byte in start..=end {
        bytes.insert(byte);
    }
}

fn position_only_eligible(parsed: &ParsedRegex) -> bool {
    let features = &parsed.features;
    parsed.capture_count > 0
        && !features.backreference
        && !features.subroutine
        && !features.possessive_or_atomic
        && !features.conditional
        && !features.unsupported_escape
}

fn match_position_node(
    ast: &Ast,
    line: &str,
    position: usize,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
) -> Result<PositionStates, BudgetExceeded> {
    budget.step()?;
    match ast {
        Ast::Empty => Ok(PositionStates::one(position)),
        Ast::Literal(literal) => {
            let end = position.saturating_add(literal.len());
            let Some(candidate) = line.get(position..end) else {
                return Ok(PositionStates::empty());
            };
            let matched = if flags.case_insensitive {
                candidate.eq_ignore_ascii_case(literal)
            } else {
                candidate == literal
            };
            Ok(if matched {
                PositionStates::one(end)
            } else {
                PositionStates::empty()
            })
        }
        Ast::Dot => {
            let Some((ch, next)) = char_at(line, position) else {
                return Ok(PositionStates::empty());
            };
            Ok(if ch != '\n' || flags.dot_matches_new_line {
                PositionStates::one(next)
            } else {
                PositionStates::empty()
            })
        }
        Ast::Grapheme => Ok(
            grapheme_end(line, position).map_or_else(PositionStates::empty, PositionStates::one)
        ),
        Ast::Class(class) => {
            let Some((ch, next)) = char_at(line, position) else {
                return Ok(PositionStates::empty());
            };
            Ok(if class_contains(class, ch, flags) {
                PositionStates::one(next)
            } else {
                PositionStates::empty()
            })
        }
        Ast::Anchor(anchor) => Ok(if anchor_matches(*anchor, line, position, ctx) {
            PositionStates::one(position)
        } else {
            PositionStates::empty()
        }),
        Ast::Concat(nodes) => {
            let mut positions = PositionStates::one(position);
            for node in nodes {
                let mut next = PositionStates::empty();
                for position in positions {
                    let matches = match_position_node(node, line, position, ctx, flags, budget)?;
                    push_limited_positions(&mut next, matches);
                }
                if next.is_empty() {
                    return Ok(PositionStates::empty());
                }
                positions = next;
            }
            Ok(positions)
        }
        Ast::Alternation(branches) => {
            let mut out = PositionStates::empty();
            for branch in branches {
                let matches = match_position_node(branch, line, position, ctx, flags, budget)?;
                push_limited_positions(&mut out, matches);
            }
            Ok(out)
        }
        Ast::Repeat {
            node,
            min,
            max,
            greedy,
            possessive,
            atomic,
        } => match_position_repeat(
            node,
            *min,
            *max,
            *greedy,
            *possessive,
            *atomic,
            line,
            position,
            ctx,
            flags,
            budget,
        ),
        Ast::Group { child, .. } => match_position_node(child, line, position, ctx, flags, budget),
        Ast::Flags {
            flags: local,
            child,
        } => match_position_node(child, line, position, ctx, *local, budget),
        Ast::Look { kind, child } => match kind {
            LookKind::Ahead => {
                let matches = match_position_node(child, line, position, ctx, flags, budget)?;
                Ok(if matches.is_empty() {
                    PositionStates::empty()
                } else {
                    PositionStates::one(position)
                })
            }
            LookKind::NotAhead => {
                let matches = match_position_node(child, line, position, ctx, flags, budget)?;
                Ok(if matches.is_empty() {
                    PositionStates::one(position)
                } else {
                    PositionStates::empty()
                })
            }
            LookKind::Behind | LookKind::NotBehind => {
                let matched =
                    position_lookbehind_matches(child, line, position, ctx, flags, budget)?;
                let accepted = if *kind == LookKind::Behind {
                    matched
                } else {
                    !matched
                };
                Ok(if accepted {
                    PositionStates::one(position)
                } else {
                    PositionStates::empty()
                })
            }
        },
        Ast::Backref(_) | Ast::Conditional { .. } | Ast::Subroutine(_) | Ast::Unsupported(_) => {
            Ok(PositionStates::empty())
        }
    }
}

#[cfg(test)]
pub(crate) fn recursive_position_span(
    parsed: &ParsedRegex,
    line: &str,
    start: usize,
    ctx: AnchorContext,
) -> Option<std::ops::Range<usize>> {
    let mut budget = StepBudget::new(DEFAULT_STEP_BUDGET);
    match_position_node(&parsed.ast, line, start, ctx, parsed.flags, &mut budget)
        .ok()?
        .into_first()
        .map(|end| start..end)
}

fn recursive_position_end(
    parsed: &ParsedRegex,
    line: &str,
    start: usize,
    ctx: AnchorContext,
    budget: &mut StepBudget,
) -> Result<Option<usize>, FallbackError> {
    match_position_node(&parsed.ast, line, start, ctx, parsed.flags, budget)
        .map(PositionStates::into_first)
        .map_err(|_| FallbackError::BudgetExceeded {
            steps: budget.used(),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PositionEngineMode {
    Recursive,
    Candidate,
    Shadow,
}

pub(crate) fn position_engine_mode() -> PositionEngineMode {
    static MODE: OnceLock<PositionEngineMode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("MARK_TEXTMATE_VM").as_deref() {
        Ok("candidate") => PositionEngineMode::Candidate,
        Ok("shadow") => PositionEngineMode::Shadow,
        Ok("recursive") => PositionEngineMode::Recursive,
        _ => PositionEngineMode::Candidate,
    })
}

pub(crate) fn capture_engine_mode() -> PositionEngineMode {
    static MODE: OnceLock<PositionEngineMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let capture = std::env::var("MARK_TEXTMATE_CAPTURE_VM").ok();
        let position = std::env::var("MARK_TEXTMATE_VM").ok();
        match capture.as_deref().or(position.as_deref()) {
            Some("recursive") => PositionEngineMode::Recursive,
            Some("shadow") => PositionEngineMode::Shadow,
            Some("candidate") | None => PositionEngineMode::Candidate,
            Some(_) => PositionEngineMode::Candidate,
        }
    })
}

fn position_lookbehind_matches(
    child: &Ast,
    line: &str,
    position: usize,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
) -> Result<bool, BudgetExceeded> {
    if let Some(literal) = ast_exact_literal(child) {
        let start = position.saturating_sub(literal.len());
        return Ok(position >= literal.len()
            && line.is_char_boundary(start)
            && line.get(start..position).is_some_and(|candidate| {
                if flags.case_insensitive {
                    candidate.eq_ignore_ascii_case(&literal)
                } else {
                    candidate == literal
                }
            }));
    }

    let mut matches_from = |start| -> Result<bool, BudgetExceeded> {
        if !line.is_char_boundary(start) {
            return Ok(false);
        }
        Ok(match_position_node(child, line, start, ctx, flags, budget)?
            .into_iter()
            .any(|end| end == position))
    };

    if let Some((min_width, max_width)) = lookbehind_byte_width_bounds(child) {
        let Some(latest_start) = position.checked_sub(min_width) else {
            return Ok(false);
        };
        let earliest_start = position.saturating_sub(max_width);
        for start in earliest_start..=latest_start {
            if matches_from(start)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    for start in char_boundaries_until(line, position) {
        if matches_from(start)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn match_position_repeat(
    node: &Ast,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    possessive: bool,
    atomic: bool,
    line: &str,
    position: usize,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
) -> Result<PositionStates, BudgetExceeded> {
    // With an exact repetition count there is no repetition-count choice to
    // make possessive. Oniguruma still permits backtracking inside the atom.
    let possessive = possessive && (atomic || max != Some(min));
    if is_simple_repeat_atom(node) {
        let max = max.unwrap_or(usize::MAX);
        let mut positions = Vec::new();
        positions.push(position);
        let mut current = position;
        let mut count = 0usize;
        while count < max {
            budget.step()?;
            let Some(next) = simple_repeat_next(node, line, current, flags) else {
                break;
            };
            if next == current {
                break;
            }
            positions.push(next);
            current = next;
            count += 1;
        }
        if count < min {
            return Ok(PositionStates::empty());
        }
        let accepted = &positions[min..];
        if possessive {
            return Ok(accepted
                .last()
                .copied()
                .map_or_else(PositionStates::empty, PositionStates::one));
        }
        let mut out = Vec::with_capacity(accepted.len().min(STATE_LIMIT));
        if greedy {
            out.extend(accepted.iter().rev().copied().take(STATE_LIMIT));
        } else {
            out.extend(accepted.iter().copied().take(STATE_LIMIT));
        }
        return Ok(PositionStates::from_vec(out));
    }

    if possessive {
        let max = max.unwrap_or_else(|| line.len().saturating_sub(position).saturating_add(1));
        let mut current = position;
        let mut count = 0usize;
        while count < max && (greedy || count < min) {
            budget.step()?;
            let Some(next) = match_position_node(node, line, current, ctx, flags, budget)?
                .into_iter()
                .next()
            else {
                break;
            };
            count += 1;
            if next == current {
                current = next;
                break;
            }
            current = next;
        }
        return Ok(if count >= min {
            PositionStates::one(current)
        } else {
            PositionStates::empty()
        });
    }

    let max = max.unwrap_or_else(|| line.len().saturating_sub(position).saturating_add(1));
    // Preference-ordered DFS emission; see `match_repeat` for the rationale.
    enum Work {
        Visit(usize, usize),
        Accept(usize),
    }
    let mut stack = vec![Work::Visit(0, position)];
    let mut accepted: Vec<usize> = Vec::new();
    while let Some(work) = stack.pop() {
        budget.step()?;
        let (count, current) = match work {
            Work::Accept(position) => {
                if !accepted.contains(&position) {
                    accepted.push(position);
                    if accepted.len() >= STATE_LIMIT {
                        break;
                    }
                }
                continue;
            }
            Work::Visit(count, position) => (count, position),
        };
        if greedy && count >= min {
            stack.push(Work::Accept(current));
        }
        if count < max && stack.len() < STATE_LIMIT {
            let next_positions = match_position_node(node, line, current, ctx, flags, budget)?;
            let push = |next: usize, stack: &mut Vec<Work>| {
                if next == current {
                    if count < min {
                        stack.push(Work::Visit(count + 1, next));
                    }
                    return;
                }
                stack.push(Work::Visit(count + 1, next));
            };
            match next_positions {
                PositionStates::Empty => {}
                PositionStates::One(next) => push(next, &mut stack),
                PositionStates::Many(positions) => {
                    for next in positions.into_iter().rev() {
                        push(next, &mut stack);
                    }
                }
            }
        }
        if !greedy && count >= min {
            stack.push(Work::Accept(current));
        }
    }
    Ok(PositionStates::from_vec(accepted))
}

fn push_limited_positions(target: &mut PositionStates, states: PositionStates) {
    for position in states {
        if match target {
            PositionStates::Empty => 0,
            PositionStates::One(_) => 1,
            PositionStates::Many(positions) => positions.len(),
        } >= STATE_LIMIT
        {
            break;
        }
        target.push(position);
    }
}

impl Matcher for FallbackMatcher {
    fn find(&self, line: &str, from: usize, ctx: AnchorContext) -> Option<MatchResult> {
        self.try_find(line, from, ctx).ok()?.result
    }
}

fn match_node(
    ast: &Ast,
    line: &str,
    state: VmState,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
    parsed: &ParsedRegex,
) -> Result<VmStates, BudgetExceeded> {
    budget.step()?;
    match ast {
        Ast::Empty => Ok(VmStates::one(state)),
        Ast::Literal(literal) => match_literal(literal, line, state, flags, budget),
        Ast::Dot => match_dot(line, state, flags),
        Ast::Class(class) => match_class(class, line, state, flags),
        Ast::Grapheme => Ok(
            grapheme_end(line, state.pos).map_or_else(VmStates::empty, |end| {
                VmStates::one(VmState { pos: end, ..state })
            }),
        ),
        Ast::Anchor(anchor) => match_anchor(*anchor, line, state, ctx),
        Ast::Concat(nodes) => match_concat(nodes, line, state, ctx, flags, budget, parsed),
        Ast::Alternation(branches) => {
            let mut out = VmStates::empty();
            for branch in branches {
                let branch_states =
                    match_node(branch, line, state.clone(), ctx, flags, budget, parsed)?;
                push_limited(&mut out, branch_states);
            }
            Ok(out)
        }
        Ast::Repeat {
            node,
            min,
            max,
            greedy,
            possessive,
            atomic,
        } => match_repeat(
            node,
            *min,
            *max,
            *greedy,
            *possessive,
            *atomic,
            line,
            state,
            ctx,
            flags,
            budget,
            parsed,
        ),
        Ast::Group { index, child, .. } => {
            let start = state.pos;
            let mut out = match_node(child, line, state, ctx, flags, budget, parsed)?;
            if let Some(index) = index.and_then(|index| usize::try_from(index).ok()) {
                for state in out.iter_mut() {
                    if index < state.captures.len() {
                        state.captures[index] = Some(start..state.pos);
                    }
                }
            }
            Ok(out)
        }
        Ast::Look { kind, child } => {
            match_look(*kind, child, line, state, ctx, flags, budget, parsed)
        }
        Ast::Backref(backref) => match_backref(backref, line, state, parsed, flags, budget),
        Ast::Conditional {
            condition,
            matched,
            unmatched,
        } => {
            let group = match condition {
                Backref::Number(group) => usize::try_from(*group).ok(),
                Backref::Name(name) => parsed
                    .named_captures
                    .get(name)
                    .and_then(|group| usize::try_from(*group).ok()),
            }
            .and_then(|group| state.captures.get(group))
            .is_some_and(Option::is_some);
            match_node(
                if group { matched } else { unmatched },
                line,
                state,
                ctx,
                flags,
                budget,
                parsed,
            )
        }
        Ast::Subroutine(call) => {
            let group = call
                .target_path
                .as_deref()
                .and_then(|path| ast_at_path(&parsed.ast, path))
                .or_else(|| find_subroutine_group(&parsed.ast, &call.target, parsed));
            let Some(group) = group else {
                return Ok(VmStates::empty());
            };
            match_node(group, line, state, ctx, flags, budget, parsed)
        }
        Ast::Flags {
            flags: local,
            child,
        } => match_node(child, line, state, ctx, *local, budget, parsed),
        Ast::Unsupported(_) => Ok(VmStates::empty()),
    }
}

fn match_concat(
    nodes: &[Ast],
    line: &str,
    state: VmState,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
    parsed: &ParsedRegex,
) -> Result<VmStates, BudgetExceeded> {
    let mut states = VmStates::one(state);
    for node in nodes {
        let mut next = VmStates::empty();
        for state in states {
            let states = match_node(node, line, state, ctx, flags, budget, parsed)?;
            push_limited(&mut next, states);
        }
        if next.is_empty() {
            return Ok(VmStates::empty());
        }
        states = next;
    }
    Ok(states)
}

fn match_literal(
    literal: &str,
    line: &str,
    state: VmState,
    flags: RegexFlags,
    _budget: &mut StepBudget,
) -> Result<VmStates, BudgetExceeded> {
    let end = state.pos.saturating_add(literal.len());
    let Some(candidate) = line.get(state.pos..end) else {
        return Ok(VmStates::empty());
    };
    let matched = if flags.case_insensitive {
        candidate.eq_ignore_ascii_case(literal)
    } else {
        candidate == literal
    };
    if matched {
        Ok(VmStates::one(VmState { pos: end, ..state }))
    } else {
        Ok(VmStates::empty())
    }
}

fn match_dot(line: &str, state: VmState, flags: RegexFlags) -> Result<VmStates, BudgetExceeded> {
    let Some((ch, next)) = char_at(line, state.pos) else {
        return Ok(VmStates::empty());
    };
    if ch == '\n' && !flags.dot_matches_new_line {
        return Ok(VmStates::empty());
    }
    Ok(VmStates::one(VmState { pos: next, ..state }))
}

fn match_class(
    class: &CharClass,
    line: &str,
    state: VmState,
    flags: RegexFlags,
) -> Result<VmStates, BudgetExceeded> {
    let Some((ch, next)) = char_at(line, state.pos) else {
        return Ok(VmStates::empty());
    };
    if class_contains(class, ch, flags) {
        Ok(VmStates::one(VmState { pos: next, ..state }))
    } else {
        Ok(VmStates::empty())
    }
}

fn match_anchor(
    anchor: AnchorKind,
    line: &str,
    state: VmState,
    ctx: AnchorContext,
) -> Result<VmStates, BudgetExceeded> {
    let matches = anchor_matches(anchor, line, state.pos, ctx);
    if matches {
        Ok(VmStates::one(state))
    } else {
        Ok(VmStates::empty())
    }
}

pub(crate) fn anchor_matches(
    anchor: AnchorKind,
    line: &str,
    pos: usize,
    ctx: AnchorContext,
) -> bool {
    match anchor {
        AnchorKind::LineStart => pos == 0,
        AnchorKind::LineEnd => is_line_end_position(line, pos),
        AnchorKind::TextEnd => pos == line.len(),
        AnchorKind::TextEndOrFinalNewline => {
            pos == line.len() || line.get(pos..).is_some_and(|tail| tail == "\n")
        }
        AnchorKind::TextStart => ctx.allow_a && pos == 0,
        AnchorKind::Continuation => ctx.allow_g && pos == ctx.g_pos,
        AnchorKind::WordBoundary => is_word_boundary(line, pos),
        AnchorKind::NotWordBoundary => !is_word_boundary(line, pos),
    }
}

fn is_line_end_position(line: &str, pos: usize) -> bool {
    pos == line.len() || (line.as_bytes().get(pos).copied() == Some(b'\n') && pos + 1 == line.len())
}

#[allow(clippy::too_many_arguments)]
fn match_repeat(
    node: &Ast,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    possessive: bool,
    atomic: bool,
    line: &str,
    state: VmState,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
    parsed: &ParsedRegex,
) -> Result<VmStates, BudgetExceeded> {
    // With an exact repetition count there is no repetition-count choice to
    // make possessive. Oniguruma still permits backtracking inside the atom.
    let possessive = possessive && (atomic || max != Some(min));
    if let Some(states) = match_simple_repeat(
        node, min, max, greedy, possessive, line, &state, flags, budget,
    )? {
        return Ok(states);
    }
    if possessive {
        let max = max.unwrap_or_else(|| line.len().saturating_sub(state.pos).saturating_add(1));
        let mut current = state;
        let mut count = 0usize;
        while count < max && (greedy || count < min) {
            budget.step()?;
            let Some(next) = match_node(node, line, current.clone(), ctx, flags, budget, parsed)?
                .into_iter()
                .next()
            else {
                break;
            };
            count += 1;
            let zero_width = next.pos == current.pos && next.captures == current.captures;
            current = next;
            if zero_width {
                break;
            }
        }
        return Ok(if count >= min {
            VmStates::one(current)
        } else {
            VmStates::empty()
        });
    }
    let max = max.unwrap_or_else(|| line.len().saturating_sub(state.pos).saturating_add(1));
    // Emit exit states in Oniguruma preference order with an explicit DFS:
    // a greedy repeat prefers another iteration over exiting, a lazy repeat
    // prefers exiting, and the body's own state order is preserved either
    // way. Sorting by position here would invert the preference of lazy or
    // ordered-alternation bodies inside the repeat.
    enum Work {
        Visit(usize, VmState),
        Accept(VmState),
    }
    let mut stack = vec![Work::Visit(0, state)];
    let mut accepted: Vec<VmState> = Vec::new();
    while let Some(work) = stack.pop() {
        budget.step()?;
        let (count, current) = match work {
            Work::Accept(state) => {
                let duplicate = accepted
                    .iter()
                    .any(|seen| seen.pos == state.pos && seen.captures == state.captures);
                if !duplicate {
                    accepted.push(state);
                    if accepted.len() >= STATE_LIMIT {
                        break;
                    }
                }
                continue;
            }
            Work::Visit(count, state) => (count, state),
        };
        if greedy && count >= min {
            // Popped after the children below, so exiting stays the least
            // preferred continuation of this iteration count.
            stack.push(Work::Accept(current.clone()));
        }
        let mut lazy_accept = (!greedy && count >= min).then(|| current.clone());
        if count < max && stack.len() < STATE_LIMIT {
            let next_states = match_node(node, line, current.clone(), ctx, flags, budget, parsed)?;
            let push = |next: VmState, stack: &mut Vec<Work>| {
                // Zero-width quantified atoms are legal in Oniguruma, but
                // cannot be allowed to loop forever. They must still run
                // enough times to satisfy a finite minimum, and a
                // capture-changing zero-width iteration is meaningful once
                // before it stabilizes.
                if next.pos == current.pos && next.captures == current.captures {
                    if count < min {
                        stack.push(Work::Visit(count + 1, next));
                    }
                    return;
                }
                stack.push(Work::Visit(count + 1, next));
            };
            match next_states {
                VmStates::Empty => {}
                VmStates::One(next) => push(next, &mut stack),
                VmStates::Many(states) => {
                    // Reverse so the body's first-preference state pops first.
                    for next in states.into_iter().rev() {
                        push(next, &mut stack);
                    }
                }
            }
        }
        if let Some(state) = lazy_accept.take() {
            // Pushed after the children, so exiting pops first for lazy.
            stack.push(Work::Accept(state));
        }
    }
    Ok(VmStates::from_vec(accepted))
}

#[allow(clippy::too_many_arguments)]
fn match_simple_repeat(
    node: &Ast,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    possessive: bool,
    line: &str,
    state: &VmState,
    flags: RegexFlags,
    budget: &mut StepBudget,
) -> Result<Option<VmStates>, BudgetExceeded> {
    if !is_simple_repeat_atom(node) {
        return Ok(None);
    }
    let max = max.unwrap_or(usize::MAX);
    let mut positions = Vec::new();
    positions.push(state.pos);
    let mut pos = state.pos;
    let mut count = 0usize;
    while count < max {
        budget.step()?;
        let Some(next) = simple_repeat_next(node, line, pos, flags) else {
            break;
        };
        if next == pos {
            break;
        }
        positions.push(next);
        pos = next;
        count += 1;
    }
    if count < min {
        return Ok(Some(VmStates::empty()));
    }
    let accepted_positions = &positions[min..];
    if possessive {
        let Some(pos) = accepted_positions.last().copied() else {
            return Ok(Some(VmStates::empty()));
        };
        return Ok(Some(VmStates::one(VmState {
            pos,
            captures: state.captures.clone(),
        })));
    }
    let mut states = Vec::with_capacity(accepted_positions.len().min(STATE_LIMIT));
    if greedy {
        for pos in accepted_positions.iter().rev().copied().take(STATE_LIMIT) {
            states.push(VmState {
                pos,
                captures: state.captures.clone(),
            });
        }
    } else {
        for pos in accepted_positions.iter().copied().take(STATE_LIMIT) {
            states.push(VmState {
                pos,
                captures: state.captures.clone(),
            });
        }
    }
    Ok(Some(VmStates::from_vec(states)))
}

fn is_simple_repeat_atom(node: &Ast) -> bool {
    matches!(node, Ast::Literal(literal) if !literal.is_empty())
        || matches!(node, Ast::Class(_) | Ast::Dot)
}

fn simple_repeat_next(node: &Ast, line: &str, pos: usize, flags: RegexFlags) -> Option<usize> {
    match node {
        Ast::Literal(literal) => {
            let end = pos.checked_add(literal.len())?;
            let candidate = line.get(pos..end)?;
            let matched = if flags.case_insensitive {
                candidate.eq_ignore_ascii_case(literal)
            } else {
                candidate == literal
            };
            matched.then_some(end)
        }
        Ast::Class(class) => {
            let (ch, next) = char_at(line, pos)?;
            class_contains(class, ch, flags).then_some(next)
        }
        Ast::Dot => {
            let (ch, next) = char_at(line, pos)?;
            (ch != '\n' || flags.dot_matches_new_line).then_some(next)
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn match_look(
    kind: LookKind,
    child: &Ast,
    line: &str,
    state: VmState,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
    parsed: &ParsedRegex,
) -> Result<VmStates, BudgetExceeded> {
    if kind == LookKind::Ahead {
        let position = state.pos;
        let mut states = match_node(child, line, state, ctx, flags, budget, parsed)?;
        for matched in states.iter_mut() {
            matched.pos = position;
        }
        return Ok(states);
    }
    match kind {
        LookKind::Ahead => unreachable!("positive lookahead returned above"),
        LookKind::NotAhead => {
            let states = match_node(child, line, state.clone(), ctx, flags, budget, parsed)?;
            Ok(if states.is_empty() {
                VmStates::one(state)
            } else {
                VmStates::empty()
            })
        }
        LookKind::Behind | LookKind::NotBehind => {
            let end = state.pos;
            let matched = if let Some((min_width, max_width)) = lookbehind_byte_width_bounds(child)
            {
                if let Some(latest_start) = end.checked_sub(min_width) {
                    let earliest_start = end.saturating_sub(max_width);
                    lookbehind_state_in_window(
                        child,
                        line,
                        earliest_start,
                        latest_start,
                        end,
                        &state,
                        ctx,
                        flags,
                        budget,
                        parsed,
                    )?
                } else {
                    None
                }
            } else {
                lookbehind_state_in_window(
                    child, line, 0, end, end, &state, ctx, flags, budget, parsed,
                )?
            };
            match (kind, matched) {
                (LookKind::Behind, Some(mut matched)) => {
                    matched.pos = end;
                    Ok(VmStates::one(matched))
                }
                (LookKind::NotBehind, None) => Ok(VmStates::one(state)),
                (LookKind::Behind, None) | (LookKind::NotBehind, Some(_)) => Ok(VmStates::empty()),
                (LookKind::Ahead | LookKind::NotAhead, _) => unreachable!(),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lookbehind_state_in_window(
    child: &Ast,
    line: &str,
    earliest_start: usize,
    latest_start: usize,
    end: usize,
    state: &VmState,
    ctx: AnchorContext,
    flags: RegexFlags,
    budget: &mut StepBudget,
    parsed: &ParsedRegex,
) -> Result<Option<VmState>, BudgetExceeded> {
    // Oniguruma probes variable-width lookbehind from the closest viable
    // boundary backwards. This affects both alternation priority and captures.
    for start in (earliest_start..=latest_start).rev() {
        if !line.is_char_boundary(start) {
            continue;
        }
        let probe = VmState {
            pos: start,
            captures: state.captures.clone(),
        };
        let states = match_node(child, line, probe, ctx, flags, budget, parsed)?;
        if let Some(matched) = states.into_iter().find(|end_state| end_state.pos == end) {
            return Ok(Some(matched));
        }
    }
    Ok(None)
}

fn lookbehind_byte_width_bounds(ast: &Ast) -> Option<(usize, usize)> {
    match ast {
        Ast::Empty | Ast::Anchor(_) | Ast::Look { .. } => Some((0, 0)),
        Ast::Literal(literal) => Some((literal.len(), literal.len())),
        Ast::Dot | Ast::Class(_) => Some((1, 4)),
        Ast::Grapheme => None,
        Ast::Concat(nodes) => {
            let mut min = 0usize;
            let mut max = 0usize;
            for node in nodes {
                let (node_min, node_max) = lookbehind_byte_width_bounds(node)?;
                min = min.saturating_add(node_min);
                max = max.saturating_add(node_max);
            }
            Some((min, max))
        }
        Ast::Alternation(branches) => {
            let mut bounds = branches.iter().map(lookbehind_byte_width_bounds);
            let (mut min, mut max) = bounds.next().unwrap_or(Some((0, 0)))?;
            for bound in bounds {
                let (branch_min, branch_max) = bound?;
                min = min.min(branch_min);
                max = max.max(branch_max);
            }
            Some((min, max))
        }
        Ast::Repeat { node, min, max, .. } => {
            let max = (*max)?;
            let (node_min, node_max) = lookbehind_byte_width_bounds(node)?;
            Some((node_min.saturating_mul(*min), node_max.saturating_mul(max)))
        }
        Ast::Group { child, .. } | Ast::Flags { child, .. } => lookbehind_byte_width_bounds(child),
        Ast::Backref(_) | Ast::Conditional { .. } | Ast::Subroutine(_) | Ast::Unsupported(_) => {
            None
        }
    }
}

fn subroutine_index(subroutine: &Backref, parsed: &ParsedRegex) -> Option<u32> {
    Some(match subroutine {
        Backref::Number(index) => *index,
        Backref::Name(name) => *parsed.named_captures.get(name)?,
    })
}

fn find_subroutine_group<'a>(
    ast: &'a Ast,
    subroutine: &Backref,
    parsed: &ParsedRegex,
) -> Option<&'a Ast> {
    let wanted = subroutine_index(subroutine, parsed)?;
    match ast {
        Ast::Group {
            index: Some(index), ..
        } if *index == wanted => Some(ast),
        Ast::Concat(nodes) | Ast::Alternation(nodes) => nodes
            .iter()
            .find_map(|node| find_subroutine_group(node, subroutine, parsed)),
        Ast::Conditional {
            matched, unmatched, ..
        } => find_subroutine_group(matched, subroutine, parsed)
            .or_else(|| find_subroutine_group(unmatched, subroutine, parsed)),
        Ast::Repeat { node, .. }
        | Ast::Group { child: node, .. }
        | Ast::Look { child: node, .. }
        | Ast::Flags { child: node, .. } => find_subroutine_group(node, subroutine, parsed),
        _ => None,
    }
}

fn ast_at_path<'a>(mut ast: &'a Ast, path: &[AstPathStep]) -> Option<&'a Ast> {
    for step in path {
        ast = match (step, ast) {
            (AstPathStep::Branch(index), Ast::Concat(nodes) | Ast::Alternation(nodes)) => {
                nodes.get(*index)?
            }
            (
                AstPathStep::Branch(index),
                Ast::Conditional {
                    matched, unmatched, ..
                },
            ) => match index {
                0 => matched,
                1 => unmatched,
                _ => return None,
            },
            (
                AstPathStep::Child,
                Ast::Repeat { node, .. }
                | Ast::Group { child: node, .. }
                | Ast::Look { child: node, .. }
                | Ast::Flags { child: node, .. },
            ) => node,
            _ => return None,
        };
    }
    Some(ast)
}

fn match_backref(
    backref: &Backref,
    line: &str,
    state: VmState,
    parsed: &ParsedRegex,
    flags: RegexFlags,
    _budget: &mut StepBudget,
) -> Result<VmStates, BudgetExceeded> {
    let index = match backref {
        Backref::Number(index) => *index as usize,
        Backref::Name(name) => parsed.named_captures.get(name).copied().unwrap_or(0) as usize,
    };
    let Some(Some(range)) = state.captures.get(index) else {
        return Ok(VmStates::empty());
    };
    let Some(captured) = line.get(range.clone()) else {
        return Ok(VmStates::empty());
    };
    let end = state.pos.saturating_add(captured.len());
    let Some(candidate) = line.get(state.pos..end) else {
        return Ok(VmStates::empty());
    };
    let matched = if flags.case_insensitive {
        candidate.eq_ignore_ascii_case(captured)
    } else {
        candidate == captured
    };
    if matched {
        Ok(VmStates::one(VmState { pos: end, ..state }))
    } else {
        Ok(VmStates::empty())
    }
}

fn ast_exact_literal(ast: &Ast) -> Option<String> {
    match ast {
        Ast::Empty => Some(String::new()),
        Ast::Literal(literal) => Some(literal.clone()),
        Ast::Concat(nodes) => {
            let mut out = String::new();
            for node in nodes {
                out.push_str(&ast_exact_literal(node)?);
            }
            Some(out)
        }
        Ast::Group { child, .. } | Ast::Flags { child, .. } => ast_exact_literal(child),
        _ => None,
    }
}

fn push_limited(out: &mut VmStates, states: VmStates) {
    for state in states {
        if out.len() >= STATE_LIMIT {
            break;
        }
        out.push(state);
    }
}

pub(crate) fn class_contains(class: &CharClass, ch: char, flags: RegexFlags) -> bool {
    let matched = class
        .atoms
        .iter()
        .any(|atom| atom_contains(atom, ch, flags));
    if class.negated { !matched } else { matched }
}

fn atom_contains(atom: &ClassAtom, ch: char, flags: RegexFlags) -> bool {
    match atom {
        ClassAtom::Char(expected) => char_eq(*expected, ch, flags),
        ClassAtom::Range(start, end) => {
            let ch = fold_char(ch, flags);
            let start = fold_char(*start, flags);
            let end = fold_char(*end, flags);
            start <= ch && ch <= end
        }
        ClassAtom::Perl(kind) => perl_class_contains(*kind, ch),
        ClassAtom::Posix { name, negated } => {
            let contains = posix_class_contains(name, ch);
            if *negated { !contains } else { contains }
        }
        ClassAtom::Unicode { name, negated } => {
            let contains = unicode_class_contains(name, ch);
            if *negated { !contains } else { contains }
        }
        ClassAtom::Nested(class) => class_contains(class, ch, flags),
    }
}

fn perl_class_contains(kind: PerlClassKind, ch: char) -> bool {
    match kind {
        PerlClassKind::Digit => ch.is_ascii_digit(),
        PerlClassKind::NotDigit => !ch.is_ascii_digit(),
        PerlClassKind::Space => ch.is_whitespace(),
        PerlClassKind::NotSpace => !ch.is_whitespace(),
        PerlClassKind::Word => is_word_char(ch),
        PerlClassKind::NotWord => !is_word_char(ch),
        PerlClassKind::HorizontalSpace => ch.is_ascii_hexdigit(),
        PerlClassKind::NotHorizontalSpace => !ch.is_ascii_hexdigit(),
        PerlClassKind::VerticalSpace => matches!(ch, '\n' | '\r' | '\u{000B}' | '\u{000C}'),
        PerlClassKind::NotVerticalSpace => !matches!(ch, '\n' | '\r' | '\u{000B}' | '\u{000C}'),
        PerlClassKind::NotNewline => ch != '\n',
    }
}

fn posix_class_contains(name: &str, ch: char) -> bool {
    if name.eq_ignore_ascii_case("alnum") {
        ch.is_alphanumeric()
    } else if name.eq_ignore_ascii_case("alpha") {
        ch.is_alphabetic()
    } else if name.eq_ignore_ascii_case("ascii") {
        ch.is_ascii()
    } else if name.eq_ignore_ascii_case("blank") {
        matches!(ch, '\t' | ' ')
    } else if name.eq_ignore_ascii_case("cntrl") {
        ch.is_control()
    } else if name.eq_ignore_ascii_case("digit") {
        ch.is_ascii_digit()
    } else if name.eq_ignore_ascii_case("graph") {
        !ch.is_whitespace() && !ch.is_control()
    } else if name.eq_ignore_ascii_case("lower") {
        ch.is_lowercase()
    } else if name.eq_ignore_ascii_case("print") {
        !ch.is_control()
    } else if name.eq_ignore_ascii_case("punct") {
        ch.is_ascii_punctuation()
    } else if name.eq_ignore_ascii_case("space") {
        ch.is_whitespace()
    } else if name.eq_ignore_ascii_case("upper") {
        ch.is_uppercase()
    } else if name.eq_ignore_ascii_case("word") {
        is_word_char(ch)
    } else if name.eq_ignore_ascii_case("xdigit") {
        ch.is_ascii_hexdigit()
    } else {
        false
    }
}

fn unicode_class_contains(name: &str, ch: char) -> bool {
    if name.eq_ignore_ascii_case("l") || name.eq_ignore_ascii_case("letter") {
        ch.is_alphabetic()
    } else if name.eq_ignore_ascii_case("alnum") {
        ch.is_alphanumeric()
    } else if name.eq_ignore_ascii_case("alpha") {
        ch.is_alphabetic()
    } else if name.eq_ignore_ascii_case("ascii") {
        ch.is_ascii()
    } else if name.eq_ignore_ascii_case("blank") {
        matches!(ch, '\t' | ' ')
    } else if name.eq_ignore_ascii_case("cntrl") {
        ch.is_control()
    } else if name.eq_ignore_ascii_case("digit") {
        ch.is_ascii_digit()
    } else if name.eq_ignore_ascii_case("graph") {
        !ch.is_whitespace() && !ch.is_control()
    } else if name.eq_ignore_ascii_case("lower") {
        ch.is_lowercase()
    } else if name.eq_ignore_ascii_case("print") {
        !ch.is_control()
    } else if name.eq_ignore_ascii_case("punct") {
        ch.is_ascii_punctuation()
    } else if name.eq_ignore_ascii_case("space") {
        ch.is_whitespace()
    } else if name.eq_ignore_ascii_case("upper") {
        ch.is_uppercase()
    } else if name.eq_ignore_ascii_case("xdigit") {
        ch.is_ascii_hexdigit()
    } else if name.eq_ignore_ascii_case("n")
        || name.eq_ignore_ascii_case("number")
        || name.eq_ignore_ascii_case("nd")
        || name.eq_ignore_ascii_case("decimal_number")
    {
        ch.is_numeric()
    } else if name.eq_ignore_ascii_case("z") || name.eq_ignore_ascii_case("separator") {
        ch.is_whitespace()
    } else if name.eq_ignore_ascii_case("word") {
        is_word_char(ch)
    } else {
        false
    }
}

fn char_eq(expected: char, actual: char, flags: RegexFlags) -> bool {
    if flags.case_insensitive {
        fold_char(expected, flags) == fold_char(actual, flags)
    } else {
        expected == actual
    }
}

fn fold_char(ch: char, flags: RegexFlags) -> char {
    if flags.case_insensitive {
        ch.to_ascii_lowercase()
    } else {
        ch
    }
}

pub(crate) fn char_at(line: &str, pos: usize) -> Option<(char, usize)> {
    let ch = line.get(pos..)?.chars().next()?;
    Some((ch, pos + ch.len_utf8()))
}

fn grapheme_end(line: &str, position: usize) -> Option<usize> {
    let grapheme = line.get(position..)?.graphemes(true).next()?;
    Some(position + grapheme.len())
}

fn char_boundaries_from(line: &str, from: usize) -> Vec<usize> {
    line.char_indices()
        .map(|(index, _)| index)
        .filter(|index| *index >= from)
        .chain(std::iter::once(line.len()))
        .collect()
}

fn start_hint(ast: &Ast) -> StartHint {
    match ast {
        Ast::Anchor(AnchorKind::LineStart) => StartHint::LineStart,
        Ast::Anchor(AnchorKind::TextStart) => StartHint::TextStart,
        Ast::Anchor(AnchorKind::Continuation) => StartHint::Continuation,
        Ast::Concat(nodes) => nodes
            .iter()
            .find(|node| !matches!(node, Ast::Empty))
            .map_or(StartHint::Unanchored, start_hint),
        Ast::Group { child, .. } | Ast::Flags { child, .. } => start_hint(child),
        _ => StartHint::Unanchored,
    }
}

fn char_boundaries_until(line: &str, until: usize) -> Vec<usize> {
    line.char_indices()
        .map(|(index, _)| index)
        .take_while(move |index| *index <= until)
        .chain(std::iter::once(until))
        .collect()
}

fn is_word_boundary(line: &str, pos: usize) -> bool {
    let before = previous_char(line, pos).is_some_and(is_word_char);
    let after = char_at(line, pos)
        .map(|(ch, _)| ch)
        .is_some_and(is_word_char);
    before != after
}

fn previous_char(line: &str, pos: usize) -> Option<char> {
    line.get(..pos)?.chars().next_back()
}

fn is_word_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> AnchorContext {
        AnchorContext {
            allow_a: true,
            allow_g: false,
            g_pos: 0,
        }
    }

    #[test]
    fn matches_literals_and_captures() {
        let matcher = FallbackMatcher::new(r"(foo)\1");
        let report = matcher.try_find("xxfoofoo", 0, ctx()).unwrap();
        let result = report.result.as_ref().unwrap();
        assert_eq!(result.start..result.end, 2..8);
        assert_eq!(result.captures[1], Some(2..5));
    }

    #[test]
    fn line_end_matches_before_trailing_newline() {
        let matcher = FallbackMatcher::new(r"foo$");
        let report = matcher.try_find("foo\n", 0, ctx()).unwrap();
        let result = report.result.as_ref().unwrap();
        assert_eq!(result.start..result.end, 0..3);
    }

    #[test]
    fn newline_sequence_escape_handles_crlf_and_unicode_newlines() {
        for line in ["\r\n", "\u{0085}", "\u{2028}", "\u{2029}"] {
            let matcher = FallbackMatcher::new(r"^\R$");
            let result = matcher.find(line, 0, ctx()).unwrap();
            assert_eq!(result.start..result.end, 0..line.len(), "{line:?}");
        }
    }

    #[test]
    fn nullable_start_filter_does_not_skip_a_later_line_end() {
        let matcher = FallbackMatcher::new(r#"($|(?="""))"#);
        let result = matcher.find("# comment\n", 1, ctx()).unwrap();
        assert_eq!(result.start..result.end, 9..9);
    }

    #[test]
    fn supports_named_backrefs() {
        let matcher = FallbackMatcher::new(r"(?<x>a)\k<x>");
        let result = matcher.find("zaa", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 1..3);
    }

    #[test]
    fn supports_recursive_oniguruma_subroutine_calls() {
        let matcher = FallbackMatcher::new(r"(?<parens>\((?:[^()]|\g<parens>)*\))");
        let result = matcher.find("x((a)(b))y", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 1..9);
    }

    #[test]
    fn supports_oniguruma_print_property() {
        let matcher = FallbackMatcher::new(r"^\p{print}+$");
        assert!(matcher.find("café λ🚀", 0, ctx()).is_some());
        assert!(matcher.find("bad\0", 0, ctx()).is_none());
    }

    #[test]
    fn scoped_case_insensitive_flags_do_not_get_case_sensitive_start_bytes() {
        let matcher = FallbackMatcher::new(r"(?i:DOCTYPE)");
        assert_eq!(matcher.find("doctype", 0, ctx()).unwrap().start, 0);
    }

    #[test]
    fn supports_lookahead() {
        let matcher = FallbackMatcher::new(r"foo(?=bar)");
        let result = matcher.find("xxfoobar", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 2..5);
    }

    #[test]
    fn positive_lookahead_preserves_captures() {
        let matcher = FallbackMatcher::new(r"(^|\G)(\s*)(`{3,}|~{3,})\s*(?=([^`]*)?$)");
        let result = matcher.find("```text\n", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 0..3);
        assert_eq!(result.captures[4], Some(3..8));
    }

    #[test]
    fn supports_lookbehind() {
        let matcher = FallbackMatcher::new(r"(?<=foo)bar");
        let result = matcher.find("xxfoobar", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 5..8);
    }

    #[test]
    fn positive_lookbehind_preserves_captures_and_scoped_flags() {
        let captured = FallbackMatcher::new(r"(?<=(a))b")
            .find("ab", 0, ctx())
            .unwrap();
        assert_eq!(captured.start..captured.end, 1..2);
        assert_eq!(captured.captures[1], Some(0..1));

        let variable = FallbackMatcher::new(r"(?<=(a|aa))b")
            .find("aab", 0, ctx())
            .unwrap();
        assert_eq!(variable.start..variable.end, 2..3);
        assert_eq!(variable.captures[1], Some(1..2));

        let backref = FallbackMatcher::new(r"(?<=(a))\1")
            .find("aa", 0, ctx())
            .unwrap();
        assert_eq!(backref.start..backref.end, 1..2);
        assert_eq!(backref.captures[1], Some(0..1));

        let scoped = FallbackMatcher::new(r"(?<=(?i:foo))bar")
            .find("FOObar", 0, ctx())
            .unwrap();
        assert_eq!(scoped.start..scoped.end, 3..6);
    }

    #[test]
    fn exact_lookbehind_honors_case_insensitive_flag() {
        let matcher = FallbackMatcher::new(r"(?i)(?<=foo)bar");
        let result = matcher.find("xxFOObar", 0, ctx()).unwrap();

        assert_eq!(result.start..result.end, 5..8);
    }

    #[test]
    fn extended_mode_ignores_unescaped_whitespace_and_comments() {
        let spaced = FallbackMatcher::new("(?x:a b)")
            .find("ab", 0, ctx())
            .unwrap();
        assert_eq!(spaced.start..spaced.end, 0..2);

        let commented = FallbackMatcher::new("(?x:a # comment\n b)")
            .find("ab", 0, ctx())
            .unwrap();
        assert_eq!(commented.start..commented.end, 0..2);

        let escaped = FallbackMatcher::new(r"(?x:a\ b)")
            .find("a b", 0, ctx())
            .unwrap();
        assert_eq!(escaped.start..escaped.end, 0..3);
    }

    #[test]
    fn bounded_lookbehind_searches_only_width_window() {
        let matcher = FallbackMatcher::new(r"(?<=[A-Z]|return|case)foo");
        let report = matcher
            .try_find("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa returnfoo", 0, ctx())
            .unwrap();
        let result = report.result.as_ref().unwrap();

        assert_eq!(result.start..result.end, 39..42);
        assert!(report.steps < 80, "{report:#?}");
    }

    #[test]
    fn bounded_lookbehind_handles_multibyte_character_width() {
        let matcher = FallbackMatcher::new(r"(?<=.)bar");
        let result = matcher.find("ébar", 0, ctx()).unwrap();

        assert_eq!(result.start..result.end, 2..5);
    }

    #[test]
    fn bounded_negative_lookbehind_succeeds_when_prefix_is_too_short() {
        let matcher = FallbackMatcher::new(r"(?<!foo)bar");
        let result = matcher.find("bar", 0, ctx()).unwrap();

        assert_eq!(result.start..result.end, 0..3);
    }

    #[test]
    fn simple_repeat_fast_path_preserves_greedy_order() {
        let matcher = FallbackMatcher::new(r"a*ab");
        let result = matcher.find("aaab", 0, ctx()).unwrap();

        assert_eq!(result.start..result.end, 0..4);
    }

    #[test]
    fn possessive_simple_repeat_does_not_backtrack() {
        let matcher = FallbackMatcher::new(r"a*+ab");
        let report = matcher.try_find("aaab", 0, ctx()).unwrap();

        assert_eq!(report.result, None);
    }

    #[test]
    fn atomic_and_compound_possessive_repeats_commit_ordered_paths() {
        assert!(
            FallbackMatcher::new(r"(?>a|ab)c")
                .try_find("abc", 0, ctx())
                .unwrap()
                .result
                .is_none()
        );
        let committed = FallbackMatcher::new(r"(?>ab|a)c")
            .find("abc", 0, ctx())
            .unwrap();
        assert_eq!(committed.start..committed.end, 0..3);
        assert!(
            FallbackMatcher::new(r"(a|ab)++c")
                .try_find("abc", 0, ctx())
                .unwrap()
                .result
                .is_none()
        );
        let control = FallbackMatcher::new(r"(a|ab)+c")
            .find("abc", 0, ctx())
            .unwrap();
        assert_eq!(control.start..control.end, 0..3);

        let exact = FallbackMatcher::new(r"(a|ab){1}+c")
            .find("abc", 0, ctx())
            .unwrap();
        assert_eq!(exact.start..exact.end, 0..3);
        assert_eq!(exact.captures[1], Some(0..2));

        let zero_width = FallbackMatcher::new(r"(a?){2}+a")
            .find("a", 0, ctx())
            .unwrap();
        assert_eq!(zero_width.start..zero_width.end, 0..1);
        assert_eq!(zero_width.captures[1], Some(0..0));
    }

    #[test]
    fn h_class_matches_hex_digits() {
        let matcher = FallbackMatcher::new(r"\h+");
        let result = matcher.find("xx c0ffee", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 3..9);

        let matcher = FallbackMatcher::new(r"\H+");
        let result = matcher.find("c0ffee tail", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 6..8);
    }

    #[test]
    fn g_anchor_uses_context() {
        let matcher = FallbackMatcher::new(r"\Gfoo");
        let result = matcher
            .find(
                "xxfoo",
                0,
                AnchorContext {
                    allow_a: false,
                    allow_g: true,
                    g_pos: 2,
                },
            )
            .unwrap();
        assert_eq!(result.start..result.end, 2..5);
    }

    #[test]
    fn budget_kills_pathological_pattern() {
        let matcher = FallbackMatcher::with_budget(r"(a+)+b", 10);
        let error = matcher.try_find("aaaaaaaaaaaa", 0, ctx()).unwrap_err();
        assert!(matches!(error, FallbackError::BudgetExceeded { .. }));
    }

    #[test]
    fn prefilter_skips_fallback_without_vm_steps() {
        let matcher = FallbackMatcher::new(r"foo(?=bar)");
        let report = matcher.try_find("no match here", 0, ctx()).unwrap();

        assert_eq!(report.result, None);
        assert_eq!(report.steps, 0);
        assert_eq!(matcher.prefilter_may_match("no match here", 0), Some(false));
    }

    #[test]
    fn start_byte_hint_skips_to_positive_lookahead_candidates() {
        let matcher = FallbackMatcher::new(r"(?=[;)])(?<!\\)");
        let report = matcher.try_find("aaaaaaaaaaaaaaaa;", 0, ctx()).unwrap();
        let result = report.result.as_ref().unwrap();

        assert_eq!(result.start..result.end, 16..16);
        assert!(report.steps < 12, "{report:#?}");
    }

    #[test]
    fn start_byte_hint_handles_nullable_prefix_before_literal() {
        let matcher = FallbackMatcher::new(r"(?=[\t ]*#)(?<!\\)");
        let report = matcher.try_find("abc   # comment", 0, ctx()).unwrap();
        let result = report.result.as_ref().unwrap();

        assert_eq!(result.start..result.end, 3..3);
        assert!(report.steps < 32, "{report:#?}");
    }

    #[test]
    fn start_byte_hint_is_disabled_for_case_insensitive_patterns() {
        let matcher = FallbackMatcher::new(r"(?i)foo");
        let result = matcher.find("xxFOO", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 2..5);
    }

    #[test]
    fn unicode_capable_classes_do_not_receive_ascii_only_start_hints() {
        for pattern in [r"\w+", r"\s+", r"[[:alpha:]]+", r"[[:word:]]+"] {
            let line = if pattern == r"\s+" { "\u{2003}" } else { "λ" };
            let matcher = FallbackMatcher::new(pattern);
            let result = matcher.find(line, 0, ctx()).unwrap();
            assert_eq!(result.start..result.end, 0..line.len(), "{pattern}");
            assert!(matcher.restricted_start_bytes().is_none(), "{pattern}");
        }
    }

    #[test]
    fn anchored_fallback_searches_only_anchor_position() {
        let matcher = FallbackMatcher::new(r"^foo(?=bar)");
        let report = matcher.try_find("xfoobar", 0, ctx()).unwrap();

        assert_eq!(report.result, None);
        assert!(report.steps < 10, "{report:#?}");
    }

    #[test]
    fn returns_utf8_boundary_offsets() {
        let matcher = FallbackMatcher::new("é+");
        let result = matcher.find("xéé", 0, ctx()).unwrap();
        assert_eq!(result.start, 1);
        assert_eq!(result.end, 5);
        assert!("xéé".is_char_boundary(result.start));
        assert!("xéé".is_char_boundary(result.end));
    }

    #[test]
    fn nullable_pattern_matches_empty_without_start_byte() {
        let matcher = FallbackMatcher::new(r"a?");
        let result = matcher.find("xxx", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 0..0);
    }

    #[test]
    fn finite_zero_width_repeats_satisfy_their_minimum() {
        let matcher = FallbackMatcher::new(r"(?:){2}a");
        let result = matcher.find("a", 0, ctx()).unwrap();
        assert_eq!(result.start..result.end, 0..1);
    }

    #[test]
    fn exact_start_replay_preserves_captures() {
        let matcher = FallbackMatcher::new(r"(foo)");
        let report = matcher.try_find_at("xxfoo", 2, ctx()).unwrap();
        let result = report.result.unwrap();

        assert_eq!(result.start..result.end, 2..5);
        assert_eq!(result.captures, vec![Some(2..5), Some(2..5)]);
        assert_eq!(matcher.try_find_at("xxfoo", 1, ctx()).unwrap().result, None);
    }

    #[test]
    fn position_only_selection_matches_capture_vm_spans() {
        for (pattern, line) in [
            (r"(a|aa)*a", "xxaaaa"),
            (r"(ab|a)+?b", "xxaaab"),
            (r"(([A-Z])|[a-z])+[0-9]", "__Abz7"),
            (r"(?:a?)*b", "xxaaab"),
            (r"(?i:(ab|c))+D", "__ABcD"),
            (r"(é|λ)+z", "xéλz"),
            (r"(?=(a|aa)+b)a+b", "xxaaab"),
            (r"(?!foo)([a-z])+[0-9]", "foo bar7"),
            (r"(?<=(a|aa))b", "xxaab"),
            (r"(?<!foo)([a-z])+[0-9]", "foo bar7"),
            (r"(?<par>\((?:[^()]|\g<par>)*\))", "x((a)(b))"),
        ] {
            let matcher = FallbackMatcher::new(pattern);
            let full = matcher.try_find(line, 0, ctx()).unwrap().result;
            let selected = matcher
                .try_find_for_selection(line, 0, ctx())
                .unwrap()
                .result;
            assert_eq!(
                selected.as_ref().map(|result| result.start..result.end),
                full.as_ref().map(|result| result.start..result.end),
                "pattern {pattern:?}"
            );
            assert!(selected.is_none_or(|result| result.captures.is_empty()));
        }
    }
}
