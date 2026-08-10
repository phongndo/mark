use std::{env, fs, io::Write, path::PathBuf};

use mark_core::MarkError;

use crate::{CliResult, args::SkillCommand, version::CLI_VERSION, write_stdout};

const SKILL: &str = include_str!("../../../assets/skills/agent-review/SKILL.md");

pub(crate) fn skill(command: SkillCommand) -> CliResult<()> {
    match command {
        SkillCommand::Show => write_stdout(format_args!("{SKILL}")),
        SkillCommand::Path => {
            let path = materialize()?;
            write_stdout(format_args!("{}\n", path.display()))
        }
    }
}

fn materialize() -> CliResult<PathBuf> {
    let cache = env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(env::temp_dir);
    let directory = cache
        .join("mark")
        .join("skills")
        .join(CLI_VERSION)
        .join("mark-live-review");
    fs::create_dir_all(&directory)?;
    let path = directory.join("SKILL.md");
    if fs::read(&path).is_ok_and(|contents| contents == SKILL.as_bytes()) {
        return Ok(path);
    }
    let temp = directory.join(format!(
        ".SKILL.md.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(SKILL.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, &path)?;
    let materialized = fs::read_to_string(&path)?;
    if materialized != SKILL {
        return Err(MarkError::Usage("materialized skill did not verify".to_owned()).into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_skill_has_required_session_safety_rules() {
        assert!(SKILL.contains("Never launch `mark`"));
        assert!(SKILL.contains("structure first"));
        assert!(SKILL.contains("stale_generation"));
        assert!(SKILL.contains("untrusted data"));
    }
}
