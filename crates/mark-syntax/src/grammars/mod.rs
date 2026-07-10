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

    #[test]
    fn embedded_bundle_parses_and_exposes_catalog() {
        let bundle = embedded_bundle();
        // Phase 4 core-30 public languages; cpp-macro is a private blob only.
        assert_eq!(bundle.languages.len(), 30);
        assert_eq!(bundle.grammar_blobs.len(), 31);
        assert!(
            bundle
                .grammar_blob_for_scope("source.cpp.embedded.macro")
                .is_some()
        );
        assert!(!bundle.has_language("cpp-macro"));
        assert_eq!(bundle.canonical_language("rs"), Some("rust"));
        assert_eq!(bundle.canonical_language("shellscript"), Some("bash"));
        assert_eq!(bundle.canonical_language("docker"), Some("dockerfile"));
        assert_eq!(bundle.detect_language_from_path("src/lib.rs"), Some("rust"));
        assert_eq!(
            bundle.detect_language_from_path("component.tsx"),
            Some("tsx")
        );
        assert_eq!(bundle.detect_language_from_path("include/foo.h"), Some("c"));
        assert_eq!(
            bundle.detect_language_from_path("Dockerfile"),
            Some("dockerfile")
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
}
