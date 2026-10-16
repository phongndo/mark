use crate::{CliResult, write_stdout};

pub(crate) fn config() -> CliResult<()> {
    write_stdout(format_args!(
        "{}\n",
        mark_command::syntax_settings_path()?.display()
    ))?;
    Ok(())
}
