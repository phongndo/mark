use std::path::PathBuf;

use crate::{
    SyntaxAddRequest, SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult,
    SyntaxDoctorReport, SyntaxLanguageStatus, SyntaxRemoveResult, SyntaxUpdateResult,
    SyntaxUpdateSelection,
};
use mark_core::MarkResult;

pub fn syntax_add(languages: &[String]) -> MarkResult<SyntaxAddResult> {
    mark_syntax::add_languages(languages)
}

pub fn syntax_add_with_options(request: SyntaxAddRequest) -> MarkResult<SyntaxAddResult> {
    mark_syntax::add_languages_with_options(request)
}

pub fn syntax_update(selection: SyntaxUpdateSelection) -> MarkResult<SyntaxUpdateResult> {
    mark_syntax::update_languages(selection)
}

pub fn syntax_remove(languages: &[String]) -> MarkResult<SyntaxRemoveResult> {
    mark_syntax::remove_languages(languages)
}

pub fn syntax_statuses() -> MarkResult<Vec<SyntaxLanguageStatus>> {
    mark_syntax::language_statuses()
}

pub fn syntax_available_languages(filter: SyntaxAvailableFilter) -> MarkResult<Vec<String>> {
    mark_syntax::available_languages(filter)
}

pub fn syntax_clean_cache() -> MarkResult<SyntaxCleanResult> {
    mark_syntax::clean_cache()
}

pub fn syntax_config_path() -> MarkResult<PathBuf> {
    mark_syntax::config_path()
}

pub fn syntax_settings_path() -> MarkResult<PathBuf> {
    mark_syntax::settings_write_path()
}

pub fn syntax_colorscheme_dir() -> MarkResult<PathBuf> {
    mark_syntax::colorscheme_dir()
}

pub fn syntax_doctor() -> MarkResult<SyntaxDoctorReport> {
    mark_syntax::doctor()
}
