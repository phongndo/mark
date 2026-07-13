mod commands;
mod inspect;
mod options;
mod output;
mod table;

pub(crate) use commands::syntax;
pub(crate) use inspect::inspect;
pub(crate) use options::{
    diff_options, difftool_options, patch_options, review_options, show_options,
};
pub(crate) use output::{
    print_syntax_add_result, print_syntax_remove_result, print_syntax_statuses,
    print_syntax_update_result,
};
#[cfg(test)]
pub(crate) use table::{display_width, list_glyphs, render_syntax_statuses};

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        language: &str,
        enabled: bool,
        available: bool,
        has_highlights: bool,
    ) -> mark_command::SyntaxLanguageStatus {
        let runtime = if available {
            let grammar = mark_command::SyntaxGrammarInfo::bundled("0.0.0");
            if has_highlights {
                mark_command::SyntaxLanguageRuntimeState::Ready(grammar)
            } else {
                mark_command::SyntaxLanguageRuntimeState::MissingHighlights(grammar)
            }
        } else {
            mark_command::SyntaxLanguageRuntimeState::MissingGrammar
        };
        let state = if enabled {
            mark_command::SyntaxLanguageState::enabled(runtime)
        } else {
            let runtime = runtime
                .into_available()
                .expect("disabled test status should have an available grammar");
            mark_command::SyntaxLanguageState::disabled(runtime)
        };
        mark_command::SyntaxLanguageStatus {
            language: language.to_owned(),
            state,
        }
    }

    #[test]
    fn syntax_status_output_uses_compact_table() {
        let output = render_syntax_statuses(
            &[
                status("rust", true, true, true),
                status("typescript", true, true, false),
                status("elixir", false, true, true),
            ],
            false,
            list_glyphs(false),
            None,
        );

        assert!(output.contains("language"));
        assert!(output.contains("status"));
        assert!(output.contains("version"));
        let headers = output
            .lines()
            .next()
            .expect("header should render")
            .split_whitespace()
            .collect::<Vec<_>>();
        assert_eq!(headers, ["language", "status", "source", "version"]);
        assert!(output.contains("rust"));
        assert!(output.contains("ok"));
        assert!(output.contains("typescript"));
        assert!(output.contains("!"));
        assert!(output.contains("elixir"));
        assert!(output.contains("-"));
        assert!(!output.contains("enabled"));
        assert!(!output.contains("syntax"));
    }

    #[test]
    fn syntax_status_output_centers_unicode_status() {
        let output = render_syntax_statuses(
            &[status("rust", true, true, true)],
            false,
            list_glyphs(true),
            None,
        );

        let rust_line = output
            .lines()
            .find(|line| line.starts_with("rust"))
            .expect("rust status row should render");

        assert!(rust_line.contains("  ✓   "));
    }

    #[test]
    fn syntax_status_output_truncates_to_terminal_width() {
        let output = render_syntax_statuses(
            &[status("very-long-language-name", true, true, true)],
            false,
            list_glyphs(false),
            Some(31),
        );

        for line in output.lines() {
            assert!(display_width(line) <= 31, "line too wide: {line}");
        }
    }
}
