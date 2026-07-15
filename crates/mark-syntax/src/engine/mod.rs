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

use std::collections::{BTreeMap, BTreeSet};

use mark_core::{MarkError, MarkResult};

use crate::{HighlightedText, grammars};
use counters::EngineCounters;
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
    for decoded in grammar_blob_closure(bundle, &root_scope)? {
        let blob = &bundle.grammar_blobs[decoded.index];
        let source = std::str::from_utf8(&decoded.bytes).map_err(|_| {
            MarkError::Usage(format!(
                "bundled TextMate grammar `{}` is not UTF-8",
                blob.language
            ))
        })?;
        let grammar_id = grammars.load_and_add(source).map_err(|error| {
            MarkError::Usage(format!(
                "failed to load bundled TextMate grammar `{}`: {error}",
                blob.language
            ))
        })?;
        if blob.scope_name == root_scope {
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

struct DecodedGrammarBlob {
    index: usize,
    bytes: Vec<u8>,
}

fn grammar_blob_closure(
    bundle: &grammars::bundle::Bundle,
    root_scope: &str,
) -> MarkResult<Vec<DecodedGrammarBlob>> {
    let mut pending = vec![(root_scope.to_owned(), None::<String>)];
    let mut selected = BTreeSet::new();
    let mut inspected = BTreeSet::new();
    let mut decoded = std::collections::BTreeMap::new();
    while let Some((scope, repository)) = pending.pop() {
        let Some((index, blob)) = bundle
            .grammar_blobs
            .iter()
            .enumerate()
            .find(|(_, blob)| blob.scope_name == scope)
        else {
            continue;
        };
        selected.insert(index);
        if !inspected.insert((index, repository.clone())) {
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = decoded.entry(index) {
            let bytes = blob.decoded_bytes().map_err(|error| {
                MarkError::Usage(format!(
                    "failed to decode bundled TextMate grammar `{}`: {error:?}",
                    blob.language
                ))
            })?;
            let json = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                MarkError::Usage(format!(
                    "failed to inspect bundled TextMate grammar `{}`: {error}",
                    blob.language
                ))
            })?;
            entry.insert((bytes, json));
        }
        let json = &decoded
            .get(&index)
            .expect("decoded grammar inserted before dependency inspection")
            .1;
        collect_external_scopes(
            json,
            &blob.scope_name,
            root_scope,
            repository.as_deref(),
            bundle,
            &mut pending,
        );
    }
    Ok(selected
        .into_iter()
        .map(|index| DecodedGrammarBlob {
            index,
            bytes: decoded
                .remove(&index)
                .expect("selected grammar decoded during dependency inspection")
                .0,
        })
        .collect())
}

fn collect_external_scopes(
    grammar: &serde_json::Value,
    grammar_scope: &str,
    root_scope: &str,
    repository_rule: Option<&str>,
    bundle: &grammars::bundle::Bundle,
    pending: &mut Vec<(String, Option<String>)>,
) {
    let Some(object) = grammar.as_object() else {
        return;
    };
    let repository = object
        .get("repository")
        .and_then(serde_json::Value::as_object);
    // Repository rules form a shared graph, not a tree. Keep completed names
    // visited as well as active ones: removing them after recursion makes the
    // Markdown dependency walk exponential (many fenced-language rules share
    // the same CommonMark repositories).
    let mut visited_local = BTreeSet::new();
    if let Some(name) = repository_rule {
        if let Some(rule) = repository.and_then(|repository| repository.get(name)) {
            collect_rule_dependencies(
                rule,
                grammar_scope,
                root_scope,
                repository,
                bundle,
                pending,
                &mut visited_local,
            );
        }
    } else {
        if let Some(patterns) = object.get("patterns") {
            collect_pattern_dependencies(
                patterns,
                grammar_scope,
                root_scope,
                repository,
                bundle,
                pending,
                &mut visited_local,
            );
        }
        // Inline injections are owned by the root grammar. Dependencies may
        // themselves contain `injections`, but loading them as includes must
        // not activate (or load the closure of) those unrelated rules.
        if grammar_scope == root_scope
            && let Some(injections) = object
                .get("injections")
                .and_then(serde_json::Value::as_object)
        {
            for rule in injections.values() {
                collect_rule_dependencies(
                    rule,
                    grammar_scope,
                    root_scope,
                    repository,
                    bundle,
                    pending,
                    &mut visited_local,
                );
            }
        }
    }
}

fn collect_pattern_dependencies(
    patterns: &serde_json::Value,
    grammar_scope: &str,
    root_scope: &str,
    repository: Option<&serde_json::Map<String, serde_json::Value>>,
    bundle: &grammars::bundle::Bundle,
    pending: &mut Vec<(String, Option<String>)>,
    visited_local: &mut BTreeSet<String>,
) {
    let Some(patterns) = patterns.as_array() else {
        return;
    };
    for rule in patterns {
        collect_rule_dependencies(
            rule,
            grammar_scope,
            root_scope,
            repository,
            bundle,
            pending,
            visited_local,
        );
    }
}

fn collect_rule_dependencies(
    rule: &serde_json::Value,
    grammar_scope: &str,
    root_scope: &str,
    repository: Option<&serde_json::Map<String, serde_json::Value>>,
    bundle: &grammars::bundle::Bundle,
    pending: &mut Vec<(String, Option<String>)>,
    visited_local: &mut BTreeSet<String>,
) {
    let Some(rule) = rule.as_object() else {
        return;
    };
    if let Some(include) = rule.get("include").and_then(serde_json::Value::as_str) {
        if let Some(name) = include.strip_prefix('#') {
            if visited_local.insert(name.to_owned())
                && let Some(local) = repository.and_then(|repository| repository.get(name))
            {
                collect_rule_dependencies(
                    local,
                    grammar_scope,
                    root_scope,
                    repository,
                    bundle,
                    pending,
                    visited_local,
                );
            }
        } else if include == "$self" {
            pending.push((grammar_scope.to_owned(), None));
        } else if include == "$base" {
            pending.push((root_scope.to_owned(), None));
        } else {
            let (scope, repository) = include
                .split_once('#')
                .map_or((include, None), |(scope, repository)| {
                    (scope, Some(repository.to_owned()))
                });
            if bundle.grammar_blob_for_scope(scope).is_some() {
                pending.push((scope.to_owned(), repository));
            }
        }
        return;
    }

    // vscode-textmate's dependency processor follows ordinary rule patterns,
    // but not capture retokenization patterns. A capture-only external include
    // is therefore unresolved unless that grammar is reachable elsewhere.
    if let Some(patterns) = rule.get("patterns") {
        collect_pattern_dependencies(
            patterns,
            grammar_scope,
            root_scope,
            repository,
            bundle,
            pending,
            visited_local,
        );
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
