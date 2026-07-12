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
        let mut grammars = GrammarSet::new();
        let mut root = None;
        let bundle = crate::grammars::embedded_bundle();
        let root_blob = bundle.grammar_blob_for_language(language).ok_or_else(|| {
            MarkError::Usage(format!("bundled TextMate grammar `{language}` is missing"))
        })?;
        let root_scope = root_blob.scope_name.clone();
        let blob_indices = grammar_blob_closure(bundle, &root_scope)?;
        for index in blob_indices {
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

fn grammar_blob_closure(
    bundle: &grammars::bundle::Bundle,
    root_scope: &str,
) -> MarkResult<Vec<usize>> {
    let mut pending = vec![root_scope.to_owned()];
    let mut selected = BTreeSet::new();
    while let Some(scope) = pending.pop() {
        let Some((index, blob)) = bundle
            .grammar_blobs
            .iter()
            .enumerate()
            .find(|(_, blob)| blob.scope_name == scope)
        else {
            continue;
        };
        if !selected.insert(index) {
            continue;
        }
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
        collect_external_scopes(&json, bundle, &mut pending);
    }
    Ok(selected.into_iter().collect())
}

fn collect_external_scopes(
    value: &serde_json::Value,
    bundle: &grammars::bundle::Bundle,
    pending: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(include) = object.get("include").and_then(|value| value.as_str())
                && !include.starts_with('#')
                && !matches!(include, "$self" | "$base")
            {
                let scope = include.split('#').next().unwrap_or(include);
                if bundle.grammar_blob_for_scope(scope).is_some() {
                    pending.push(scope.to_owned());
                }
            }
            for child in object.values() {
                collect_external_scopes(child, bundle, pending);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_external_scopes(child, bundle, pending);
            }
        }
        _ => {}
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
