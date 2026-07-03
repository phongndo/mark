use std::io::{self, IsTerminal};

use crate::{CliResult, write_stdout};

use super::table::{ascii_output_requested, list_glyphs, render_syntax_statuses, terminal_width};

pub(crate) fn print_syntax_add_result(result: &mark_command::SyntaxAddResult) -> CliResult<()> {
    for language in &result.added {
        write_stdout(format_args!("+ enabled {language}\n"))?;
    }
    for language in &result.already_enabled {
        write_stdout(format_args!("= enabled {language}\n"))?;
    }
    for language in &result.unavailable {
        write_stdout(format_args!(
            "warning {language}: no bundled grammar; diff will render plain text\n"
        ))?;
    }
    for mapping in &result.custom_mappings {
        write_stdout(format_args!("+ mapped {mapping}\n"))?;
    }
    Ok(())
}

pub(crate) fn print_syntax_update_result(
    result: &mark_command::SyntaxUpdateResult,
) -> CliResult<()> {
    if result.bundled.is_empty() && result.unavailable.is_empty() {
        write_stdout(format_args!("no bundled grammars matched\n"))?;
    }
    for language in &result.bundled {
        write_stdout(format_args!("= bundled grammar {language}\n"))?;
    }
    for language in &result.unavailable {
        write_stdout(format_args!(
            "warning {language}: no bundled grammar; diff will render plain text\n"
        ))?;
    }
    Ok(())
}

pub(crate) fn print_syntax_remove_result(
    result: &mark_command::SyntaxRemoveResult,
) -> CliResult<()> {
    for language in &result.removed {
        write_stdout(format_args!("- disabled {language} in config\n"))?;
    }
    for language in &result.kept_core {
        write_stdout(format_args!("= core language remains enabled {language}\n"))?;
    }
    for language in &result.missing {
        write_stdout(format_args!("= not enabled in config {language}\n"))?;
    }
    for mapping in &result.removed_custom_mappings {
        write_stdout(format_args!("- unmapped {mapping}\n"))?;
    }
    Ok(())
}

pub(crate) fn print_syntax_statuses(
    statuses: &[mark_command::SyntaxLanguageStatus],
    detail: bool,
) -> CliResult<()> {
    if statuses.is_empty() {
        write_stdout(format_args!("no syntax languages enabled\n"))?;
        return Ok(());
    }

    let terminal = io::stdout().is_terminal();
    let glyphs = list_glyphs(terminal && !ascii_output_requested());
    write_stdout(format_args!(
        "{}",
        render_syntax_statuses(
            statuses,
            terminal,
            glyphs,
            terminal.then(terminal_width).flatten(),
        )
    ))?;

    if !detail {
        return Ok(());
    }

    Ok(())
}
