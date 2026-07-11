//! Grammar asset registry and embedded bundle access.
//!
//! Production highlighting still goes through the legacy backend until the
//! migration cutover, but Phase 4 makes the in-house bundle the authoritative
//! grammar catalog for language metadata and bundle diagnostics.

use std::sync::OnceLock;

pub mod bundle;
pub mod catalog;
pub(crate) mod registry;

use bundle::{Bundle, BundleError, BundleGrammarRegistry, LicenseEntry};

static EMBEDDED_BUNDLE: OnceLock<Bundle> = OnceLock::new();

pub fn embedded_bundle() -> &'static Bundle {
    EMBEDDED_BUNDLE.get_or_init(|| {
        Bundle::parse(embedded_bundle_bytes()).expect("embedded mark syntax bundle should parse")
    })
}

pub fn embedded_bundle_bytes() -> &'static [u8] {
    include_bytes!(env!("MARK_SYNTAX_BUNDLE_PATH"))
}

pub fn embedded_bundle_version() -> &'static str {
    env!("MARK_SYNTAX_BUNDLE_VERSION")
}

pub fn bundle_summary() -> BundleSummary {
    BundleSummary::from_bundle(embedded_bundle())
}

pub fn bundled_licenses() -> &'static [LicenseEntry] {
    &embedded_bundle().licenses
}

pub fn available_languages() -> Vec<String> {
    embedded_bundle()
        .available_languages()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub fn canonical_language(language: &str) -> Option<String> {
    embedded_bundle()
        .canonical_language(language)
        .map(str::to_owned)
}

pub fn has_language(language: &str) -> bool {
    embedded_bundle().has_language(language)
}

pub fn detect_language_from_path(path: &str) -> Option<String> {
    embedded_bundle()
        .detect_language_from_path(path)
        .map(str::to_owned)
}

pub fn grammar_registry() -> BundleGrammarRegistry {
    BundleGrammarRegistry::new(embedded_bundle().clone())
}

pub fn parse_embedded_bundle() -> Result<Bundle, BundleError> {
    Bundle::parse(embedded_bundle_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSummary {
    pub version: String,
    pub source_hash: u64,
    pub grammar_count: usize,
    pub language_count: usize,
    pub scope_count: usize,
    pub license_count: usize,
    pub source_revision: Option<String>,
}

impl BundleSummary {
    pub fn from_bundle(bundle: &Bundle) -> Self {
        let source_revision = bundle.licenses.iter().find_map(|license| {
            (!license.source_revision.is_empty()).then(|| license.source_revision.clone())
        });
        Self {
            version: bundle.version_stamp(),
            source_hash: bundle.source_hash,
            grammar_count: bundle.grammar_blobs.len(),
            language_count: bundle.languages.len(),
            scope_count: bundle.scopes.len(),
            license_count: bundle.licenses.len(),
            source_revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        grammar::load_dev_grammar_from_str,
        state::GrammarId,
        tokenizer::{GrammarSet, TextMateTokenizer},
    };

    #[test]
    fn embedded_bundle_parses_and_exposes_catalog() {
        let bundle = embedded_bundle();
        // Full public catalog plus private dependency blobs. `coverage.toml`
        // decides which embedded blobs are public catalog entries.
        assert_eq!(bundle.languages.len(), 254);
        assert_eq!(bundle.grammar_blobs.len(), 258);
        assert!(
            bundle
                .grammar_blob_for_scope("source.cpp.embedded.macro")
                .is_some()
        );
        assert!(bundle.grammar_blob_for_scope("source.yang").is_some());
        assert!(bundle.grammar_blob_for_scope("source.twig").is_some());
        assert!(bundle.has_language("cpp-macro"));
        assert_eq!(bundle.canonical_language("rs"), Some("rust"));
        assert_eq!(
            bundle.canonical_language("shellscript"),
            Some("shellscript")
        );
        assert_eq!(bundle.canonical_language("docker"), Some("docker"));
        assert_eq!(bundle.detect_language_from_path("src/lib.rs"), Some("rust"));
        assert_eq!(
            bundle.detect_language_from_path("component.tsx"),
            Some("tsx")
        );
        assert_eq!(bundle.detect_language_from_path("include/foo.h"), Some("c"));
        assert_eq!(
            bundle.detect_language_from_path("Dockerfile"),
            Some("docker")
        );
        assert_eq!(bundle.detect_language_from_path("Makefile"), Some("make"));
        assert_eq!(
            bundle.detect_language_from_path("main.tf"),
            Some("terraform")
        );
        assert_eq!(bundle.licenses.len(), bundle.grammar_blobs.len());
    }

    #[test]
    fn embedded_bundle_round_trips_deterministically() {
        let parsed = parse_embedded_bundle().unwrap();
        let reparsed = Bundle::parse(&parsed.to_bytes()).unwrap();
        assert_eq!(parsed.to_bytes(), reparsed.to_bytes());
    }

    #[test]
    fn bundle_registry_decodes_lazily() {
        let mut registry = grammar_registry();
        assert_eq!(registry.cached_grammar_count(), 0);
        let grammar = registry.grammar("json").unwrap();
        assert_eq!(grammar.scope_name, "source.json");
        assert_eq!(registry.cached_grammar_count(), 1);
    }

    #[test]
    fn every_public_language_has_a_smoke_fixture_budget_gate() {
        let bundle = embedded_bundle();
        let mut grammars = GrammarSet::new();
        for (index, blob) in bundle.grammar_blobs.iter().enumerate() {
            let bytes = blob.decoded_bytes().unwrap();
            let source = std::str::from_utf8(&bytes).unwrap();
            let grammar = load_dev_grammar_from_str(GrammarId(index as u16), source)
                .unwrap_or_else(|error| panic!("{} failed to parse: {error}", blob.language));
            grammars.add(grammar);
        }

        let mut failures = Vec::new();
        for language in &bundle.languages {
            let root = grammars
                .grammar_by_scope(&language.scope_name)
                .unwrap_or_else(|| panic!("missing grammar for {}", language.canonical))
                .id;
            let mut tokenizer = TextMateTokenizer::new(grammars.clone(), root);
            tokenizer.set_counters_enabled(true);
            let _ = tokenizer.tokenize_source(language_smoke_fixture(&language.canonical));
            let counters = tokenizer.take_counters();
            if counters.degraded_lines != 0 || counters.fallback_budget_kills != 0 {
                failures.push(format!(
                    "{} degraded={} fallback_budget_kills={}",
                    language.canonical, counters.degraded_lines, counters.fallback_budget_kills
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "public language smoke fixture budget failures:\n{}",
            failures.join("\n")
        );
    }

    fn language_smoke_fixture(language: &str) -> &'static str {
        match language {
            "csv" => "a,b,c\n",
            "tsv" => "a\tb\tc\n",
            "json" | "jsonc" => "{\"key\": 1}\n",
            "xml" | "html" => "<root>text</root>\n",
            "yaml" => "key: value\n",
            _ => "x\n",
        }
    }
}
