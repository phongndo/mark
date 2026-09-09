use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use mark_core::MarkError;

use crate::{
    CliResult,
    args::{SkillAgent, SkillCommand, SkillInstallArgs},
    version::CLI_VERSION,
    write_stdout,
};

const SKILL: &str = include_str!("../../../assets/skills/agent-review/SKILL.md");
const SKILL_NAME: &str = "mark-live-review";

pub(crate) fn skill(command: Option<SkillCommand>) -> CliResult<()> {
    match command.unwrap_or(SkillCommand::Show) {
        SkillCommand::Show => write_stdout(format_args!("{SKILL}")),
        SkillCommand::Path => {
            let path = materialize()?;
            write_stdout(format_args!("{}\n", path.display()))
        }
        SkillCommand::Install(args) => {
            let path = install(args)?;
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
    let path = cache
        .join("mark")
        .join("skills")
        .join(CLI_VERSION)
        .join(SKILL_NAME)
        .join("SKILL.md");
    write_skill(&path)?;
    Ok(path)
}

fn install(args: SkillInstallArgs) -> CliResult<PathBuf> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            MarkError::Usage("cannot install the skill because HOME is not set".to_owned())
        })?;
    let skill_root = match args.agent {
        SkillAgent::Pi => home.join(".pi").join("agent").join("skills"),
        SkillAgent::Codex => home.join(".agents").join("skills"),
        SkillAgent::Claude => home.join(".claude").join("skills"),
        SkillAgent::Cursor => home.join(".cursor").join("skills"),
        SkillAgent::Antigravity => home.join(".gemini").join("config").join("skills"),
        SkillAgent::Copilot => home.join(".copilot").join("skills"),
        SkillAgent::Opencode => env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode")
            .join("skills"),
    };
    let path = skill_root.join(SKILL_NAME).join("SKILL.md");
    write_skill(&path)?;
    Ok(path)
}

fn write_skill(path: &Path) -> CliResult<()> {
    if fs::read(path).is_ok_and(|contents| contents == SKILL.as_bytes()) {
        return Ok(());
    }
    let directory = path
        .parent()
        .ok_or_else(|| MarkError::Usage("skill path has no parent directory".to_owned()))?;
    fs::create_dir_all(directory)?;
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
    fs::rename(&temp, path)?;
    let installed = fs::read(path)?;
    if installed != SKILL.as_bytes() {
        return Err(MarkError::Usage("written skill did not verify".to_owned()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercise the actual bundled examples, rather than just checking prose.
    // Keep examples simple shell commands; this intentionally is not a shell parser.
    #[test]
    fn skill_shell_examples_parse_with_repo_and_pinned_session() {
        use clap::Parser;

        let mut count = 0;
        for block in SKILL.split("```sh\n").skip(1) {
            let body = block.split("```").next().expect("closed shell block");
            for line in body.lines() {
                let command = line.strip_prefix("cat <<'JSON' | ").unwrap_or(line);
                if !command.starts_with("mark ") {
                    continue;
                }
                let args = command.split_whitespace().collect::<Vec<_>>();
                crate::args::Cli::try_parse_from(&args)
                    .unwrap_or_else(|error| panic!("invalid skill example {command}: {error}"));
                if let Some(index) = args.iter().position(|arg| *arg == "--repo") {
                    let mut pinned = args.clone();
                    pinned.splice(index..index + 2, ["example-session"]);
                    crate::args::Cli::try_parse_from(&pinned).unwrap_or_else(|error| {
                        panic!("invalid pinned skill example {pinned:?}: {error}")
                    });
                }
                count += 1;
            }
        }
        assert!(
            count >= 8,
            "expected discovery, reads, write, and reload examples"
        );
    }

    #[test]
    fn skill_comment_batch_example_matches_protocol() {
        let json = SKILL
            .split("--stdin --json\n")
            .nth(1)
            .expect("comment batch example")
            .split("\nJSON")
            .next()
            .expect("heredoc terminator");
        let batch: mark_session::CommentApplyParams =
            serde_json::from_str(json).expect("example must match the wire protocol");
        assert_eq!(batch.generation, 1);
        assert_eq!(batch.comments.len(), 1);
        assert_eq!(batch.comments[0].anchor.new_line, Some(42));
        assert!(!batch.focus, "examples must leave the human viewport alone");
    }

    #[test]
    fn embedded_skill_has_required_session_safety_rules() {
        assert!(SKILL.contains("Never launch `mark`"));
        assert!(SKILL.contains("structure first"));
        assert!(SKILL.contains("stale_generation"));
        assert!(SKILL.contains("untrusted data"));
        assert!(SKILL.contains("Never run `mark session navigate`"));
        assert!(SKILL.contains("same file and line"));
    }
}
