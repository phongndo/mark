use crate::HighlightedText;
use mark_core::{MarkError, MarkResult};

/// Private boundary for the syntax implementation.
///
/// The previous backend was removed intentionally. Keeping the unavailable
/// implementation explicit lets the rest of Mark fall back to plain diff text
/// without pretending that an empty grammar catalog is authoritative.
#[derive(Debug, Default)]
pub(crate) struct SyntaxEngine;

impl SyntaxEngine {
    pub(crate) fn is_available() -> bool {
        false
    }

    pub(crate) fn available_languages() -> Vec<String> {
        Vec::new()
    }

    pub(crate) fn canonical_language(_language: &str) -> Option<String> {
        None
    }

    pub(crate) fn detect_language_from_path(_path: &str) -> Option<String> {
        None
    }

    pub(crate) fn has_language(_language: &str) -> bool {
        false
    }

    pub(crate) fn highlight(
        &mut self,
        language: &str,
        _source: &str,
    ) -> MarkResult<HighlightedText> {
        Err(MarkError::Usage(format!(
            "syntax highlighting backend is unavailable for `{language}`"
        )))
    }
}
