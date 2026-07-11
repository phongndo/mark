use std::{
    collections::HashSet,
    fs,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};

use mark_syntax::{
    SyntaxClass, SyntaxSegment,
    engine::{
        regex::{AnchorContext, FallbackError, RegexMatcher},
        scopes::ScopeClassifier,
        tokenizer::{GrammarSet, TextMateTokenizer, TokenizedLine, TokenizerState},
    },
};
use serde::Deserialize;

const MANIFEST_PATH: &str = "crates/mark-syntax/tests/fixtures/textmate/cases.toml";
const DIVERGENCES_PATH: &str = "crates/mark-syntax/tests/fixtures/textmate/divergences.toml";

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default, rename = "case")]
    cases: Vec<CaseSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct CaseSpec {
    language: String,
    scope: String,
    grammar: String,
    fixture: String,
    golden: String,
    #[serde(default)]
    embedded: Vec<EmbeddedSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct EmbeddedSpec {
    #[allow(dead_code)]
    scope: String,
    grammar: String,
}

#[derive(Debug, Deserialize, Default)]
struct DivergenceFile {
    #[serde(default, rename = "divergence")]
    divergences: Vec<DivergenceSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct DivergenceSpec {
    language: String,
    grammar: String,
    fixture: String,
    line_start: usize,
    line_end: usize,
    mode: DivergenceMode,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DivergenceMode {
    Exact,
    Coarse,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonMode {
    Exact,
    Coarse,
}

#[derive(Debug)]
struct RuntimeDivergence {
    spec: DivergenceSpec,
    hits: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoldenLine {
    language: String,
    scope_name: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    line_number: Option<usize>,
    line: String,
    tokens: Vec<GoldenToken>,
    #[serde(default)]
    rule_stack: Option<String>,
    #[serde(default)]
    rule_stack_hash: Option<String>,
    #[serde(default)]
    stopped_early: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoldenToken {
    start_index: usize,
    end_index: usize,
    scopes: Vec<String>,
}

fn utf16_range_to_utf8(
    line: &str,
    start_utf16: usize,
    end_utf16: usize,
) -> Option<std::ops::Range<usize>> {
    Some(utf16_offset_to_utf8(line, start_utf16)?..utf16_offset_to_utf8(line, end_utf16)?)
}

fn utf16_offset_to_utf8(line: &str, target: usize) -> Option<usize> {
    let mut utf16 = 0usize;
    for (byte, ch) in line.char_indices() {
        if utf16 == target {
            return Some(byte);
        }
        utf16 += ch.len_utf16();
        if utf16 > target {
            return None;
        }
    }
    // vscode-textmate represents the final token as extending through its
    // synthetic line terminator, so its end index may be one UTF-16 code unit
    // past the input string. Clamp that sentinel to the real byte length.
    (utf16 == target || utf16.checked_add(1) == Some(target)).then_some(line.len())
}

fn coalesce_scope_tokens(
    tokens: impl IntoIterator<Item = (std::ops::Range<usize>, Vec<String>)>,
) -> Vec<(std::ops::Range<usize>, Vec<String>)> {
    let mut coalesced: Vec<(std::ops::Range<usize>, Vec<String>)> = Vec::new();
    for (range, scopes) in tokens {
        if range.start >= range.end {
            continue;
        }
        if let Some((last_range, last_scopes)) = coalesced.last_mut()
            && last_range.end == range.start
            && *last_scopes == scopes
        {
            last_range.end = range.end;
            continue;
        }
        coalesced.push((range, scopes));
    }
    coalesced
}

#[test]
fn parses_golden_jsonl_record() {
    let line: GoldenLine = serde_json::from_str(
        r#"{"language":"json","scopeName":"source.json","file":"fixture.json","lineNumber":7,"line":"{\"ok\":true}","tokens":[{"startIndex":0,"endIndex":1,"scopes":["source.json","punctuation.definition.dictionary.begin.json"]}],"ruleStack":"[(1, source.json, source.json)]","ruleStackHash":"abc","stoppedEarly":false}"#,
    )
    .unwrap();
    assert_eq!(line.language, "json");
    assert_eq!(line.scope_name, "source.json");
    assert_eq!(line.file.as_deref(), Some("fixture.json"));
    assert_eq!(line.line_number, Some(7));
    assert_eq!(line.line, "{\"ok\":true}");
    assert_eq!(line.tokens[0].start_index, 0);
    assert_eq!(line.tokens[0].end_index, 1);
    assert_eq!(line.tokens[0].scopes[0], "source.json");
    assert_eq!(line.rule_stack_hash.as_deref(), Some("abc"));
    assert_eq!(line.stopped_early, Some(false));
}

#[test]
fn converts_utf16_offsets_to_utf8_byte_ranges() {
    let line = "aπ𝌆z";
    assert_eq!(utf16_offset_to_utf8(line, 0), Some(0));
    assert_eq!(utf16_offset_to_utf8(line, 1), Some(1));
    assert_eq!(utf16_offset_to_utf8(line, 2), Some(3));
    assert_eq!(utf16_offset_to_utf8(line, 3), None);
    assert_eq!(utf16_offset_to_utf8(line, 4), Some(7));
    assert_eq!(utf16_range_to_utf8(line, 1, 4), Some(1..7));
    assert_eq!(utf16_offset_to_utf8(line, 6), Some(line.len()));
}

#[test]
fn manifest_golden_cases_match_or_are_allowlisted() {
    let cases = load_manifest();
    assert!(
        !cases.is_empty(),
        "golden manifest should list at least one case"
    );
    let mut divergences = load_divergences();
    validate_divergences(&divergences);

    let mut failures = Vec::new();
    for case in &cases {
        assert_case_files_exist(case);
        let records = load_golden_records(case);
        compare_exact_scopes(case, &records, &mut divergences, &mut failures);
        compare_coarse_highlights(case, &records, &mut divergences, &mut failures);
    }

    for divergence in &divergences {
        if divergence.hits == 0 {
            failures.push(format!(
                "stale divergence: language={} grammar={} fixture={} lines {}-{} mode={:?} reason={}",
                divergence.spec.language,
                divergence.spec.grammar,
                divergence.spec.fixture,
                divergence.spec.line_start,
                divergence.spec.line_end,
                divergence.spec.mode,
                divergence.spec.reason,
            ));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn manifest_golden_cases_have_no_budget_degradation() {
    for case in load_manifest() {
        let mut tokenizer = tokenizer_for_case(&case);
        tokenizer.set_counters_enabled(true);
        let source = fs::read_to_string(repo_path(&case.fixture)).expect("fixture source");
        let _ = tokenizer.tokenize_source(&source);
        let counters = tokenizer.take_counters();
        assert_eq!(
            counters.degraded_lines, 0,
            "{} degraded committed fixture lines",
            case.fixture
        );
        assert_eq!(
            counters.fallback_budget_kills, 0,
            "{} exhausted fallback budget",
            case.fixture
        );
    }
}

#[test]
fn libcxx_cpp_fixture_has_zero_oracle_divergence() {
    let cases = load_manifest();
    let case = cases
        .iter()
        .find(|case| case.fixture.contains("cpp/libcxx_"))
        .expect("manifest should contain a dedicated libc++ C++ fixture");
    let allowlisted: Vec<_> = load_divergences()
        .into_iter()
        .filter(|divergence| divergence.spec.fixture == case.fixture)
        .collect();
    assert!(
        allowlisted.is_empty(),
        "libc++ fixture must not rely on divergence allowlist entries"
    );

    let records = load_golden_records(case);
    let mut divergences = Vec::new();
    let mut failures = Vec::new();
    compare_exact_scopes(case, &records, &mut divergences, &mut failures);
    compare_coarse_highlights(case, &records, &mut divergences, &mut failures);
    assert!(
        failures.is_empty(),
        "libc++ fixture diverged from oracle:\n{}",
        failures.join("\n")
    );
}

#[test]
fn tokenizer_never_panics_on_generated_utf8_inputs() {
    let grammar = fs::read_to_string(repo_path(
        "assets/tm-grammars/languages/json.tmLanguage.json",
    ))
    .expect("json grammar");
    let mut tokenizer = TextMateTokenizer::from_grammar(&grammar).unwrap();
    let mut state = TokenizerState::default();
    for input in generated_utf8_inputs() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let tokenized = tokenizer.tokenize_line_scopes(&input, state.clone());
            assert_token_invariants(&input, &tokenized);
            state = tokenized.state;
        }));
        assert!(result.is_ok(), "tokenizer panicked for input {input:?}");
    }
}

#[test]
fn zero_width_matches_advance_and_do_not_loop() {
    let grammar = r##"{
        "scopeName": "source.zero-width",
        "patterns": [
            {"match":"", "name":"meta.empty.zero-width"},
            {"match":"λ|🚀|[A-Za-z]+", "name":"keyword.visible.zero-width"}
        ]
    }"##;
    let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
    let line = "λ🚀abc";
    let tokenized = tokenizer.tokenize_line_scopes(line, TokenizerState::default());
    assert_token_invariants(line, &tokenized);
    assert!(tokenized.state.is_initial());
}

#[test]
fn parser_state_replay_from_checkpoint_matches_replay_from_zero() {
    let grammar = r##"{
        "scopeName": "source.checkpoint",
        "patterns": [
            {"begin":"/\\*", "end":"\\*/", "name":"comment.block.checkpoint"},
            {"begin":"\"", "end":"\"", "name":"string.quoted.double.checkpoint"},
            {"match":"\\b(let|return)\\b", "name":"keyword.control.checkpoint"}
        ]
    }"##;
    let lines = [
        "let before = 1;",
        "/* comment starts",
        "still in comment λ🚀",
        "ends */ let after = \"ok\";",
        "return after;",
    ];

    let mut full = TextMateTokenizer::from_grammar(grammar).unwrap();
    let mut state = TokenizerState::default();
    let mut full_states = Vec::new();
    let mut full_tokens = Vec::new();
    for line in lines {
        let tokenized = full.tokenize_line_scopes(line, state);
        state = tokenized.state.clone();
        full_tokens.push(tokenized.tokens);
        full_states.push(state.clone());
    }

    let checkpoint_after_line = 1usize;
    let checkpoint_state = full_states[checkpoint_after_line].clone();
    let mut replay = TextMateTokenizer::from_grammar(grammar).unwrap();
    let mut state = checkpoint_state;
    for line_index in checkpoint_after_line + 1..lines.len() {
        let tokenized = replay.tokenize_line_scopes(lines[line_index], state);
        assert_eq!(
            tokenized.tokens, full_tokens[line_index],
            "line {line_index}"
        );
        state = tokenized.state;
    }
    assert_eq!(state, *full_states.last().unwrap());
}

#[test]
fn fallback_budget_kills_pathological_pattern_and_tokenizer_continues_line() {
    let pathological = "a".repeat(256);
    let matcher = RegexMatcher::new(r"(?=(a+)+b)(a+)+b");
    let report = matcher.find_report(&pathological, 0, AnchorContext::line_start());
    assert!(
        matches!(report, Err(FallbackError::BudgetExceeded { .. })),
        "expected fallback budget kill, got {report:?}"
    );

    let grammar = r##"{
        "scopeName": "source.budget",
        "patterns": [
            {"match":"(?=(a+)+b)(a+)+b", "name":"invalid.pathological.budget"},
            {"match":"ok", "name":"keyword.control.budget"}
        ]
    }"##;
    let mut tokenizer = TextMateTokenizer::from_grammar(grammar).unwrap();
    let line = format!("{} ok", "a".repeat(256));
    let tokenized = tokenizer.tokenize_line_scopes(&line, TokenizerState::default());
    assert_token_invariants(&line, &tokenized);
    assert!(
        tokenized
            .tokens
            .iter()
            .any(|token| token.range == (257..259)
                && token
                    .scopes
                    .iter()
                    .any(|scope| scope == "keyword.control.budget")),
        "tokenizer should degrade the pathological prefix but still highlight later safe ranges: {:#?}",
        tokenized.tokens
    );
}

fn compare_exact_scopes(
    case: &CaseSpec,
    records: &[GoldenLine],
    divergences: &mut [RuntimeDivergence],
    failures: &mut Vec<String>,
) {
    let mut tokenizer = tokenizer_for_case(case);
    let mut state = TokenizerState::default();
    for (index, record) in records.iter().enumerate() {
        validate_record(case, record, index, failures);
        if mark_allowed(divergences, case, index + 1, ComparisonMode::Exact) {
            continue;
        }
        // vscode-textmate tokenizes each logical line with a synthetic `\n`;
        // many real grammars close line comments with a literal `\n` pattern.
        let parse_line = format!("{}\n", record.line);
        let tokenized = tokenizer.tokenize_line_scopes_at_line(&parse_line, state, index);
        state = tokenized.state.clone();
        let actual = coalesce_scope_tokens(tokenized.tokens.iter().map(|token| {
            (
                token.range.start.min(record.line.len())..token.range.end.min(record.line.len()),
                token.scopes.clone(),
            )
        }));
        let expected = coalesce_scope_tokens(record.tokens.iter().filter_map(|token| {
            let range = utf16_range_to_utf8(&record.line, token.start_index, token.end_index)
                .or_else(|| (record.line.is_empty() && token.start_index == 0).then_some(0..0))?;
            Some((range, token.scopes.clone()))
        }));
        if actual != expected {
            failures.push(format!(
                "exact mismatch: {} {} line {} ruleStackHash={} expected={} actual={} line={:?}",
                case.language,
                case.fixture,
                index + 1,
                record.rule_stack_hash.as_deref().unwrap_or("<missing>"),
                summarize_scoped(&expected),
                summarize_scoped(&actual),
                record.line,
            ));
        }
    }
}

fn compare_coarse_highlights(
    case: &CaseSpec,
    records: &[GoldenLine],
    divergences: &mut [RuntimeDivergence],
    failures: &mut Vec<String>,
) {
    let source = fs::read_to_string(repo_path(&case.fixture)).expect("fixture source");
    let allowed_lines = records
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            mark_allowed(divergences, case, index + 1, ComparisonMode::Coarse).then_some(index)
        })
        .collect::<HashSet<_>>();
    if allowed_lines.len() == records.len() {
        return;
    }
    let mut tokenizer = tokenizer_for_case(case);
    let highlighted = tokenizer.tokenize_source(&source);
    let expected = expected_coarse_segments(records);
    if highlighted.lines.len() != expected.len() {
        failures.push(format!(
            "coarse line count mismatch: {} {} expected {} actual {}",
            case.language,
            case.fixture,
            expected.len(),
            highlighted.lines.len()
        ));
        return;
    }
    for (index, expected_segments) in expected.iter().enumerate() {
        if allowed_lines.contains(&index) {
            continue;
        }
        let actual_segments = &highlighted.lines[index].segments;
        if actual_segments != expected_segments {
            failures.push(format!(
                "coarse mismatch: {} {} line {} expected={:?} actual={:?} line={:?}",
                case.language,
                case.fixture,
                index + 1,
                expected_segments,
                actual_segments,
                records
                    .get(index)
                    .map(|record| record.line.as_str())
                    .unwrap_or(""),
            ));
        }
    }
}

fn validate_record(case: &CaseSpec, record: &GoldenLine, index: usize, failures: &mut Vec<String>) {
    if record.language != case.language {
        failures.push(format!(
            "golden language mismatch for {} line {}: expected {}, got {}",
            case.fixture,
            index + 1,
            case.language,
            record.language
        ));
    }
    if record.scope_name != case.scope {
        failures.push(format!(
            "golden scope mismatch for {} line {}: expected {}, got {}",
            case.fixture,
            index + 1,
            case.scope,
            record.scope_name
        ));
    }
    if record.line_number != Some(index) {
        failures.push(format!(
            "golden lineNumber mismatch for {} line {}: got {:?}",
            case.fixture,
            index + 1,
            record.line_number
        ));
    }
    if record.stopped_early == Some(true) {
        failures.push(format!(
            "oracle stopped early for {} line {} ruleStack={:?}",
            case.fixture,
            index + 1,
            record.rule_stack
        ));
    }
}

fn expected_coarse_segments(records: &[GoldenLine]) -> Vec<Vec<SyntaxSegment>> {
    let mut classifier = ScopeClassifier::default();
    records
        .iter()
        .map(|record| {
            let mut segments = Vec::new();
            for token in &record.tokens {
                let Some(mut range) =
                    utf16_range_to_utf8(&record.line, token.start_index, token.end_index)
                else {
                    continue;
                };
                range.start = range.start.min(record.line.len());
                range.end = range.end.min(record.line.len());
                if range.start >= range.end {
                    continue;
                }
                let class = classifier.class_for_stack(&token.scopes);
                push_segment(&mut segments, range, class);
            }
            segments
        })
        .collect()
}

fn push_segment(
    segments: &mut Vec<SyntaxSegment>,
    range: Range<usize>,
    class: Option<SyntaxClass>,
) {
    if range.start >= range.end {
        return;
    }
    if let Some(last) = segments.last_mut()
        && last.class == class
        && last.byte_end == range.start
    {
        last.byte_end = range.end;
        return;
    }
    segments.push(SyntaxSegment::new(range.start, range.end, class));
}

fn tokenizer_for_case(case: &CaseSpec) -> TextMateTokenizer {
    let mut set = GrammarSet::new();
    let root_source = fs::read_to_string(repo_path(&case.grammar)).expect("root grammar");
    let root = set.load_and_add(&root_source).expect("root grammar parses");
    for embedded in &case.embedded {
        let source = fs::read_to_string(repo_path(&embedded.grammar)).expect("embedded grammar");
        set.load_and_add(&source).expect("embedded grammar parses");
    }
    TextMateTokenizer::new(set, root)
}

fn load_manifest() -> Vec<CaseSpec> {
    let text = fs::read_to_string(repo_path(MANIFEST_PATH)).expect("textmate cases manifest");
    let manifest: Manifest = toml::from_str(&text).expect("valid textmate cases manifest");
    manifest.cases
}

fn load_divergences() -> Vec<RuntimeDivergence> {
    if std::env::var_os("MARK_TEXTMATE_STRICT").is_some() {
        return Vec::new();
    }
    let text = fs::read_to_string(repo_path(DIVERGENCES_PATH)).expect("textmate divergences");
    let file: DivergenceFile = toml::from_str(&text).expect("valid textmate divergences");
    file.divergences
        .into_iter()
        .map(|spec| RuntimeDivergence { spec, hits: 0 })
        .collect()
}

fn load_golden_records(case: &CaseSpec) -> Vec<GoldenLine> {
    let text = fs::read_to_string(repo_path(&case.golden))
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", case.golden));
    text.lines()
        .map(|line| serde_json::from_str::<GoldenLine>(line).expect("golden record"))
        .collect()
}

fn assert_case_files_exist(case: &CaseSpec) {
    for file in [&case.grammar, &case.fixture, &case.golden] {
        assert!(
            repo_path(file).exists(),
            "manifest path should exist: {file}"
        );
    }
    for embedded in &case.embedded {
        assert!(
            repo_path(&embedded.grammar).exists(),
            "embedded grammar path should exist: {}",
            embedded.grammar
        );
    }
}

fn validate_divergences(divergences: &[RuntimeDivergence]) {
    for divergence in divergences {
        let spec = &divergence.spec;
        assert!(
            !spec.language.trim().is_empty(),
            "divergence language required"
        );
        assert!(
            !spec.grammar.trim().is_empty(),
            "divergence grammar required"
        );
        assert!(
            !spec.fixture.trim().is_empty(),
            "divergence fixture required"
        );
        assert!(!spec.reason.trim().is_empty(), "divergence reason required");
        assert!(spec.line_start > 0, "divergence line_start is 1-based");
        assert!(
            spec.line_start <= spec.line_end,
            "divergence line range must be ordered: {spec:?}"
        );
    }
}

fn mark_allowed(
    divergences: &mut [RuntimeDivergence],
    case: &CaseSpec,
    line: usize,
    mode: ComparisonMode,
) -> bool {
    let mut matched = false;
    for divergence in divergences {
        if divergence_matches(&divergence.spec, case, line, mode) {
            divergence.hits += 1;
            matched = true;
        }
    }
    matched
}

fn divergence_matches(
    spec: &DivergenceSpec,
    case: &CaseSpec,
    line: usize,
    mode: ComparisonMode,
) -> bool {
    spec.language == case.language
        && spec.grammar == case.scope
        && spec.fixture == case.fixture
        && spec.line_start <= line
        && line <= spec.line_end
        && matches!(
            (spec.mode, mode),
            (DivergenceMode::Any, _)
                | (DivergenceMode::Exact, ComparisonMode::Exact)
                | (DivergenceMode::Coarse, ComparisonMode::Coarse)
        )
}

fn summarize_scoped(tokens: &[(Range<usize>, Vec<String>)]) -> String {
    const LIMIT: usize = 6;
    let mut parts = tokens
        .iter()
        .take(LIMIT)
        .map(|(range, scopes)| format!("{range:?}:{scopes:?}"))
        .collect::<Vec<_>>();
    if tokens.len() > LIMIT {
        parts.push(format!("… {} more", tokens.len() - LIMIT));
    }
    parts.join(" | ")
}

fn repo_path(path: impl AsRef<Path>) -> PathBuf {
    workspace_root().join(path)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn generated_utf8_inputs() -> Vec<String> {
    let mut out = vec![
        String::new(),
        "plain ascii".to_owned(),
        "λ🚀 café".to_owned(),
        "\0\u{1f600}\tcontrol".to_owned(),
        "a/=/regex?/中文/עברית".to_owned(),
    ];
    let mut seed = 0x05ee_dcaf_ed15_ca11_u64;
    let alphabet = [
        'a', 'Z', '0', '_', ' ', '\t', '/', '*', '\\', '"', '\'', '`', '{', '}', '[', ']', '(',
        ')', '<', '>', '=', '+', '-', ':', ';', ',', '.', '#', 'λ', 'π', 'é', '🚀', '中', '✓',
    ];
    for len in [1usize, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233] {
        let mut s = String::new();
        for _ in 0..len {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            s.push(alphabet[(seed as usize) % alphabet.len()]);
        }
        out.push(s);
    }
    out
}

fn assert_token_invariants(line: &str, tokenized: &TokenizedLine) {
    let mut cursor = 0usize;
    for token in tokenized.tokens.iter() {
        assert!(token.range.start <= token.range.end, "bad range {token:?}");
        assert!(token.range.start >= cursor, "overlap in {token:?}");
        assert!(token.range.end <= line.len(), "out of bounds in {token:?}");
        assert!(
            line.is_char_boundary(token.range.start),
            "bad start {token:?}"
        );
        assert!(line.is_char_boundary(token.range.end), "bad end {token:?}");
        cursor = token.range.end;
    }

    let expanded_end = tokenized
        .tokens
        .last()
        .map(|token| token.range.end)
        .unwrap_or(0)
        .max(line.len());
    assert_eq!(expanded_end, line.len(), "expanded spans should cover line");

    let unique_boundaries = tokenized
        .tokens
        .iter()
        .flat_map(|token| [token.range.start, token.range.end])
        .collect::<HashSet<_>>();
    assert!(
        unique_boundaries
            .iter()
            .all(|offset| line.is_char_boundary(*offset))
    );
}
