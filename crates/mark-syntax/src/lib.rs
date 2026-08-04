mod highlight;
mod language;
mod paths;
mod scopes;
mod storage;
#[cfg(test)]
mod tests;
pub mod theme;
mod types;

pub use highlight::detect_language_from_path;
pub use language::{
    add_languages, add_languages_with_options, available_languages, clean_cache, doctor,
    enabled_languages, installed_languages, language_statuses, remove_languages, update_languages,
};
pub use paths::{
    colorscheme_dir, config_path, load_settings, load_settings_with_annotation_targeting,
    settings_path, settings_read_path, settings_write_path,
};
#[cfg(feature = "diagnostics")]
pub use syntaxmate::diagnostics::EngineCounters;
pub use types::*;

#[cfg(test)]
pub(crate) use highlight::*;
#[cfg(test)]
pub(crate) use language::*;
#[cfg(test)]
pub(crate) use paths::*;
pub(crate) use storage::*;

pub fn canonical_language(language: &str) -> Option<String> {
    syntaxmate::canonical_language(language)
}

pub fn has_language(language: &str) -> bool {
    syntaxmate::canonical_language(language).is_some()
}

pub fn classify_scope_name(scope: &str) -> Option<SyntaxClass> {
    scopes::classify_scope_name(scope)
}
