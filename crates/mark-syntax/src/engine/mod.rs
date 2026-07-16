//! Rust-native TextMate grammar engine.

pub mod cache;
pub mod checkpoint;
pub mod counters;
pub mod grammar;
pub(crate) mod hashing;
pub mod line;
pub mod regex;
pub mod scopes;
pub mod state;
pub mod tokenizer;

#[cfg(test)]
mod closure_parity_tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use mark_core::{MarkError, MarkResult};

use crate::{HighlightedText, grammars};
use counters::EngineCounters;
use grammar::{CompiledGrammar, RuleBody, RuleRef};
use tokenizer::{GrammarSet, TextMateTokenizer};

#[derive(Debug)]
struct Runtime {
    tokenizers: BTreeMap<String, TextMateTokenizer>,
    counters_enabled: bool,
}

impl Runtime {
    fn load() -> MarkResult<Self> {
        Ok(Self {
            tokenizers: BTreeMap::new(),
            counters_enabled: false,
        })
    }

    fn tokenizer(language: &str, counters_enabled: bool) -> MarkResult<TextMateTokenizer> {
        let (grammars, root) = shared_grammar_set(language)?;
        let mut tokenizer = TextMateTokenizer::new(grammars, root);
        tokenizer.configure_limits(crate::SyntaxLimits::default());
        tokenizer.set_counters_enabled(counters_enabled);
        Ok(tokenizer)
    }

    fn highlight(&mut self, language: &str, source: &str) -> MarkResult<HighlightedText> {
        let canonical = grammars::canonical_language(language)
            .ok_or_else(|| MarkError::Usage(format!("unknown TextMate grammar `{language}`")))?;
        if !self.tokenizers.contains_key(&canonical) {
            let tokenizer = Self::tokenizer(&canonical, self.counters_enabled)?;
            self.tokenizers.insert(canonical.clone(), tokenizer);
        }
        Ok(self
            .tokenizers
            .get_mut(&canonical)
            .expect("tokenizer inserted before highlighting")
            .tokenize_source(source))
    }

    fn set_counters_enabled(&mut self, enabled: bool) {
        self.counters_enabled = enabled;
        for tokenizer in self.tokenizers.values_mut() {
            tokenizer.set_counters_enabled(enabled);
        }
    }

    fn take_counters(&mut self) -> EngineCounters {
        let mut counters = EngineCounters::default();
        for tokenizer in self.tokenizers.values_mut() {
            counters.merge(tokenizer.take_counters());
        }
        counters
    }
}

fn shared_grammar_set(language: &str) -> MarkResult<(GrammarSet, state::GrammarId)> {
    use std::sync::{Arc, Mutex, OnceLock};

    type LoadedGrammarSet = (GrammarSet, state::GrammarId);
    type CacheEntry = OnceLock<Result<LoadedGrammarSet, String>>;
    type Cache = Mutex<std::collections::HashMap<String, Arc<CacheEntry>>>;
    static CACHE: OnceLock<Option<Cache>> = OnceLock::new();

    let disabled = || {
        matches!(
            std::env::var("MARK_TEXTMATE_GRAMMAR_CACHE").as_deref(),
            Ok("off" | "0" | "false")
        )
    };
    let Some(cache) = CACHE
        .get_or_init(|| (!disabled()).then(|| Mutex::new(std::collections::HashMap::new())))
        .as_ref()
    else {
        return load_grammar_set(language);
    };

    // A per-language OnceLock lets unrelated grammar closures compile in
    // parallel while ensuring syntax workers racing on the same visible file
    // parse and compile that closure only once.
    let entry = {
        let mut cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            cache
                .entry(language.to_owned())
                .or_insert_with(|| Arc::new(OnceLock::new())),
        )
    };
    match entry.get_or_init(|| load_grammar_set(language).map_err(|error| error.to_string())) {
        Ok((grammars, root)) => Ok((grammars.clone(), *root)),
        Err(error) => Err(MarkError::Usage(error.clone())),
    }
}

fn load_grammar_set(language: &str) -> MarkResult<(GrammarSet, state::GrammarId)> {
    let mut grammars = GrammarSet::new();
    let mut root = None;
    let bundle = crate::grammars::embedded_bundle();
    let root_blob = bundle.grammar_blob_for_language(language).ok_or_else(|| {
        MarkError::Usage(format!("bundled TextMate grammar `{language}` is missing"))
    })?;
    let root_scope = root_blob.scope_name.clone();
    for grammar in compiled_grammar_closure(bundle, &root_scope)? {
        let is_root = grammar.scope_name == root_scope;
        let grammar_id = grammars.add(grammar);
        if is_root {
            root = Some(grammar_id);
        }
    }

    // Community grammars occasionally retain optional repository includes
    // supplied only by a host editor extension. The tokenizer skips those
    // references rather than disabling the complete bundled backend.
    let root = root.ok_or_else(|| {
        MarkError::Usage(format!("bundled TextMate grammar `{language}` is missing"))
    })?;
    Ok((grammars, root))
}

/// Decode, parse, and compile exactly the external-include closure of one root.
///
/// `CompiledGrammar` retains the complete include graph, so dependency
/// discovery can walk it directly. The previous path first parsed every JSON
/// blob into `serde_json::Value` for discovery and then parsed the same bytes
/// again for compilation. Embedded-heavy grammars paid that duplicate work on
/// their first visible highlight.
fn compiled_grammar_closure(
    bundle: &grammars::bundle::Bundle,
    root_scope: &str,
) -> MarkResult<Vec<CompiledGrammar>> {
    let scope_indexes = bundle
        .grammar_blobs
        .iter()
        .enumerate()
        .map(|(index, blob)| (blob.scope_name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut pending = vec![(root_scope.to_owned(), None::<String>)];
    let mut selected = vec![false; bundle.grammar_blobs.len()];
    let mut inspected = HashSet::new();
    let mut compiled = vec![None::<CompiledGrammar>; bundle.grammar_blobs.len()];

    while let Some((scope, repository)) = pending.pop() {
        let Some(&index) = scope_indexes.get(scope.as_str()) else {
            continue;
        };
        selected[index] = true;
        if !inspected.insert((index, repository.clone())) {
            continue;
        }
        if compiled[index].is_none() {
            let blob = &bundle.grammar_blobs[index];
            let bytes = blob.decoded_bytes().map_err(|error| {
                MarkError::Usage(format!(
                    "failed to decode bundled TextMate grammar `{}`: {error:?}",
                    blob.language
                ))
            })?;
            let source = std::str::from_utf8(&bytes).map_err(|_| {
                MarkError::Usage(format!(
                    "bundled TextMate grammar `{}` is not UTF-8",
                    blob.language
                ))
            })?;
            compiled[index] = Some(
                grammar::load_dev_grammar_from_str(state::GrammarId(0), source).map_err(
                    |error| {
                        MarkError::Usage(format!(
                            "failed to load bundled TextMate grammar `{}`: {error}",
                            blob.language
                        ))
                    },
                )?,
            );
        }
        let grammar = compiled[index]
            .as_ref()
            .expect("selected grammar compiled before dependency inspection");
        collect_compiled_dependencies(grammar, root_scope, repository.as_deref(), &mut pending);
    }

    let mut closure = Vec::new();
    for (index, is_selected) in selected.into_iter().enumerate() {
        if !is_selected {
            continue;
        }
        let mut grammar = compiled[index]
            .take()
            .expect("selected grammar compiled during dependency discovery");
        grammar.id = state::GrammarId(
            u16::try_from(closure.len()).expect("grammar closure fits in GrammarId"),
        );
        closure.push(grammar);
    }
    Ok(closure)
}

fn collect_compiled_dependencies(
    grammar: &CompiledGrammar,
    root_scope: &str,
    repository_rule: Option<&str>,
    pending: &mut Vec<(String, Option<String>)>,
) {
    let mut visited_rules = BTreeSet::new();
    let mut visited_repositories = BTreeSet::new();
    if let Some(name) = repository_rule {
        collect_compiled_rule_ref(
            grammar,
            &RuleRef::Repository(name.to_owned()),
            root_scope,
            pending,
            &mut visited_rules,
            &mut visited_repositories,
        );
        return;
    }

    collect_compiled_rule_refs(
        grammar,
        &grammar.top_level,
        root_scope,
        pending,
        &mut visited_rules,
        &mut visited_repositories,
    );
    // Inline injections belong only to the root. Dependencies can themselves
    // define injections, but loading those grammars as includes must not
    // activate or expand the unrelated injection rules.
    if grammar.scope_name == root_scope {
        for injection in &grammar.injections {
            collect_compiled_rule_refs(
                grammar,
                &injection.patterns,
                root_scope,
                pending,
                &mut visited_rules,
                &mut visited_repositories,
            );
        }
    }
}

fn collect_compiled_rule_refs(
    grammar: &CompiledGrammar,
    refs: &[RuleRef],
    root_scope: &str,
    pending: &mut Vec<(String, Option<String>)>,
    visited_rules: &mut BTreeSet<state::RuleId>,
    visited_repositories: &mut BTreeSet<String>,
) {
    for rule_ref in refs {
        collect_compiled_rule_ref(
            grammar,
            rule_ref,
            root_scope,
            pending,
            visited_rules,
            visited_repositories,
        );
    }
}

fn collect_compiled_rule_ref(
    grammar: &CompiledGrammar,
    rule_ref: &RuleRef,
    root_scope: &str,
    pending: &mut Vec<(String, Option<String>)>,
    visited_rules: &mut BTreeSet<state::RuleId>,
    visited_repositories: &mut BTreeSet<String>,
) {
    match rule_ref {
        RuleRef::Rule(rule_id) => {
            if !visited_rules.insert(*rule_id) {
                return;
            }
            let Some(rule) = grammar.rule(*rule_id) else {
                return;
            };
            let patterns = match &rule.body {
                RuleBody::BeginEnd { patterns, .. }
                | RuleBody::BeginWhile { patterns, .. }
                | RuleBody::IncludeOnly { patterns } => patterns,
                // Match captures are retokenization rules. vscode-textmate's
                // dependency processor does not follow capture-only includes.
                RuleBody::Match { .. } => return,
            };
            collect_compiled_rule_refs(
                grammar,
                patterns,
                root_scope,
                pending,
                visited_rules,
                visited_repositories,
            );
        }
        RuleRef::Repository(name) => {
            // vscode-textmate's dependency processor walks the grammar's
            // top-level repository, but does not expand repositories declared
            // inside an include-only rule. The compiler gives those lexical
            // overlays a collision-free internal name; following them here
            // would load large unrelated closures (notably every fenced
            // language reachable from Wikitext) and change the established
            // bundled-closure contract.
            if name.starts_with("$mark.local.") || !visited_repositories.insert(name.clone()) {
                return;
            }
            if let Some(rule_ref) = grammar.repository.get(name) {
                collect_compiled_rule_ref(
                    grammar,
                    rule_ref,
                    root_scope,
                    pending,
                    visited_rules,
                    visited_repositories,
                );
            }
        }
        RuleRef::SelfRef => pending.push((grammar.scope_name.clone(), None)),
        RuleRef::BaseRef => pending.push((root_scope.to_owned(), None)),
        RuleRef::External { scope, repository } => {
            if let Some(scope) = grammar.scope(*scope) {
                pending.push((scope.to_owned(), repository.clone()));
            }
        }
    }
}

/// Product-facing adapter around the native engine. Heavy grammar parsing and
/// matcher construction are lazy and stay on Mark's dedicated syntax worker.
#[derive(Debug, Default)]
pub(crate) struct SyntaxEngine {
    runtime: Option<Runtime>,
}

impl SyntaxEngine {
    pub(crate) fn is_available() -> bool {
        !grammars::available_languages().is_empty()
    }

    pub(crate) fn available_languages() -> Vec<String> {
        grammars::available_languages()
    }

    pub(crate) fn canonical_language(language: &str) -> Option<String> {
        grammars::canonical_language(language)
    }

    pub(crate) fn detect_language_from_path(path: &str) -> Option<String> {
        grammars::detect_language_from_path(path)
    }

    pub(crate) fn has_language(language: &str) -> bool {
        grammars::has_language(language)
    }

    pub(crate) fn highlight(
        &mut self,
        language: &str,
        source: &str,
    ) -> MarkResult<HighlightedText> {
        if self.runtime.is_none() {
            self.runtime = Some(Runtime::load()?);
        }
        self.runtime
            .as_mut()
            .expect("runtime initialized before highlighting")
            .highlight(language, source)
    }

    pub(crate) fn set_counters_enabled(&mut self, enabled: bool) {
        if self.runtime.is_none() {
            self.runtime = Runtime::load().ok();
        }
        if let Some(runtime) = &mut self.runtime {
            runtime.set_counters_enabled(enabled);
        }
    }

    pub(crate) fn take_counters(&mut self) -> EngineCounters {
        self.runtime
            .as_mut()
            .map_or_else(EngineCounters::default, Runtime::take_counters)
    }
}
