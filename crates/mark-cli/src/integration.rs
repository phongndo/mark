//! Version-matched native packages that work without a source checkout.

use std::{
    collections::BTreeMap,
    env, io,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::{CliResult, version::CLI_VERSION, write_stdout};

mod files;
#[cfg(test)]
mod tests;

include!(concat!(env!("OUT_DIR"), "/integration_assets.rs"));
const SKILL: &str = include_str!("../../../assets/skills/annotation-review/SKILL.md");
const LICENSE: &str = include_str!("../../../LICENSE");
const INTRO: &str = include_str!("../../../assets/integrations/README.md");

#[derive(Debug, Subcommand)]
pub(crate) enum IntegrationCommand {
    #[command(about = "Materialize the private version-matched package and print its path")]
    Path(AgentArgs),
    #[command(about = "Export a self-contained native package without replacing edited files")]
    Export {
        #[command(flatten)]
        agent: AgentArgs,
        #[arg(long, value_name = "DIR")]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
pub(crate) struct AgentArgs {
    #[arg(long, value_enum)]
    agent: IntegrationAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum IntegrationAgent {
    Pi,
    Claude,
    Codex,
    Opencode,
    Fx,
}

impl IntegrationAgent {
    fn name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Fx => "fx",
        }
    }

    fn manifest(self) -> &'static str {
        match self {
            Self::Claude => ".claude-plugin/plugin.json",
            Self::Codex => ".codex-plugin/plugin.json",
            _ => "package.json",
        }
    }
}

pub(crate) fn run(command: IntegrationCommand) -> CliResult<()> {
    let (agent, output) = match command {
        IntegrationCommand::Path(args) => (args.agent, cache_path(args.agent)?),
        IntegrationCommand::Export { agent, output } => (agent.agent, output),
    };
    let output = materialize(agent, &output)?;
    write_stdout(format_args!("{}\n", output.display()))
}

fn cache_path(agent: IntegrationAgent) -> io::Result<PathBuf> {
    let cache = env::var_os("XDG_CACHE_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "integration_cache_missing: set XDG_CACHE_HOME or HOME",
            )
        })?;
    Ok(cache
        .join("mark/integrations")
        .join(CLI_VERSION)
        .join(agent.name()))
}

fn materialize(agent: IntegrationAgent, output: &Path) -> io::Result<PathBuf> {
    files::write_package(output, &package(agent)?)
}

fn package(agent: IntegrationAgent) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();
    files.insert(PathBuf::from("LICENSE"), LICENSE.as_bytes().to_vec());
    files.insert(PathBuf::from("README.md"), INTRO.as_bytes().to_vec());
    let prefix = format!("{}/", agent.name());
    for (name, bytes) in EMBEDDED {
        let relative = if let Some(relative) = name.strip_prefix(&prefix) {
            relative
        } else if name.starts_with("common/") {
            name
        } else {
            continue;
        };
        let mut bytes = bytes.to_vec();
        let path = PathBuf::from(relative);
        if !runtime_asset(&path) {
            continue;
        }
        if path.components().count() == 1
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("mjs" | "js")
            )
        {
            bytes = text(&bytes)?
                .replace("\"../common/", "\"./common/")
                .into_bytes();
        }
        files.insert(path, bytes);
    }
    if matches!(agent, IntegrationAgent::Codex | IntegrationAgent::Fx) {
        let skill_dir = PathBuf::from("skills/mark");
        files.insert(skill_dir.join("SKILL.md"), SKILL.as_bytes().to_vec());
        for name in ["HOST.md", "open-review.mjs", "LICENSE", "README.md"] {
            let bytes = files
                .get(Path::new(name))
                .ok_or_else(|| invalid("annotation skill asset is missing"))?
                .clone();
            files.insert(skill_dir.join(name), bytes);
        }
        let common = files
            .iter()
            .filter(|(path, _)| path.starts_with("common"))
            .map(|(path, bytes)| (skill_dir.join(path), bytes.clone()))
            .collect::<Vec<_>>();
        files.extend(common);
    }
    let manifest_path = PathBuf::from(agent.manifest());
    let mut manifest: Value = serde_json::from_slice(
        files
            .get(&manifest_path)
            .ok_or_else(|| invalid("embedded package manifest is missing"))?,
    )?;
    manifest["version"] = json!(env!("CARGO_PKG_VERSION"));
    files.insert(manifest_path, json_bytes(&manifest)?);
    if agent == IntegrationAgent::Claude {
        files.insert(PathBuf::from(".claude-plugin/marketplace.json"), json_bytes(&json!({
            "name": "mark-local", "owner": { "name": "Mark" },
            "plugins": [{ "name": manifest["name"], "source": "./", "description": manifest["description"] }]
        }))?);
    }
    if agent == IntegrationAgent::Codex {
        let mut portable = json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": manifest["name"], "version": manifest["version"], "description": manifest["description"],
            "license": "MIT", "repository": "https://github.com/phongndo/mark"
        });
        for key in [
            "name",
            "version",
            "description",
            "author",
            "homepage",
            "repository",
            "license",
            "keywords",
        ] {
            if let Some(value) = manifest.get(key).filter(|value| !value.is_null()) {
                portable[key] = value.clone();
            }
        }
        files.insert(PathBuf::from("plugin.json"), json_bytes(&portable)?);
        files.insert(PathBuf::from(".agents/plugins/marketplace.json"), json_bytes(&json!({
            "name": "mark-local", "interface": { "displayName": "Mark" },
            "plugins": [{ "name": manifest["name"], "source": { "source": "local", "path": "./" },
                "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }, "category": "Productivity" }]
        }))?);
    }
    let names: Vec<_> = files
        .keys()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    files.insert(
        PathBuf::from(".mark-integration.json"),
        json_bytes(&json!({ "agent": agent.name(), "version": CLI_VERSION, "files": names }))?,
    );
    Ok(files)
}

fn text(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
fn runtime_asset(path: &Path) -> bool {
    path.components().all(|component| {
        let name = component.as_os_str().to_string_lossy();
        !matches!(
            name.as_ref(),
            "tests"
                | "__tests__"
                | "node_modules"
                | ".git"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | ".DS_Store"
        ) && !name.contains(".test.")
            && !name.contains(".spec.")
    })
}
fn json_bytes(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
