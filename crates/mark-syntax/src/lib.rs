#[doc(hidden)]
pub mod engine;
#[doc(hidden)]
pub mod grammars;
mod highlight;
mod language;
mod paths;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub use highlight::detect_language_from_path;
pub use language::{
    add_languages, add_languages_with_options, available_languages, clean_cache, doctor,
    enabled_languages, installed_languages, language_statuses, remove_languages, update_languages,
};
pub use paths::{
    colorscheme_dir, config_path, load_settings, settings_path, settings_read_path,
    settings_write_path,
};
pub use types::*;

#[cfg(test)]
pub(crate) use highlight::*;
#[cfg(test)]
pub(crate) use language::*;
#[cfg(test)]
pub(crate) use paths::*;
pub(crate) use storage::*;

pub fn canonical_language(language: &str) -> Option<String> {
    grammars::canonical_language(language)
}

pub fn has_language(language: &str) -> bool {
    grammars::has_language(language)
}

pub fn classify_scope_name(scope: &str) -> Option<SyntaxClass> {
    engine::scopes::classify_scope_name(scope)
}
