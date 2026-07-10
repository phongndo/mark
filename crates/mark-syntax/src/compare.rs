//! Test-only comparison hooks for the migration.

use crate::{HighlightedText, SyntaxHighlighter};
use mark_core::MarkResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HighlightComparison {
    pub(crate) legacy: HighlightedText,
    pub(crate) candidate: Option<HighlightedText>,
    pub(crate) golden_jsonl: Option<String>,
}

pub(crate) fn compare_legacy_with_candidate(
    language: &str,
    source: &str,
    candidate: Option<HighlightedText>,
    golden_jsonl: Option<String>,
) -> MarkResult<HighlightComparison> {
    let mut highlighter = SyntaxHighlighter::new();
    let legacy = highlighter.highlight(language, source)?;
    Ok(HighlightComparison {
        legacy,
        candidate,
        golden_jsonl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_legacy_comparison_hook() {
        let comparison = compare_legacy_with_candidate("json", "{\"ok\": true}\n", None, None)
            .expect("legacy highlighter should remain available during migration");
        assert_eq!(comparison.legacy.lines.len(), 2);
        assert!(comparison.candidate.is_none());
    }
}
