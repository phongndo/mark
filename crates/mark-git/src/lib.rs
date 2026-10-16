use std::{
    path::{Path, PathBuf},
    process::Command,
};

use mark_core::{MarkError, MarkResult};

mod revision;

pub use revision::{
    existing_commitish_revision, existing_object_revision, merge_base_revision,
    range_right_operand_is_pathspec, revision_expression_exists, revision_is_treeish, show_target,
    worktree_base_revision,
};

pub fn repository_root(repo: Option<&Path>) -> MarkResult<PathBuf> {
    let mut command = Command::new("git");
    if let Some(repo) = repo {
        command.arg("-C").arg(repo);
    }
    command.args(["rev-parse", "--show-toplevel"]);

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_error("failed to find git repository root", &output));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if root.is_empty() {
        return Err(MarkError::Usage("git repository root was empty".to_owned()));
    }

    Ok(PathBuf::from(root))
}

pub fn current_branch(repo: &Path) -> MarkResult<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "--show-current"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read current git branch", &output));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(branch))
    }
}

pub fn remote_url(repo: &Path, remote: &str) -> MarkResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", remote])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to read git remote URL", &output));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if url.is_empty() {
        return Err(MarkError::Usage(format!(
            "git remote {remote} URL was empty"
        )));
    }

    Ok(url)
}

pub fn branch_exists(repo: &Path, branch: &str) -> MarkResult<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_error("failed to check git branch", &output)),
    }
}

pub fn git_path(repo: &Path, path: &str) -> MarkResult<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--git-path", path])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to resolve git path", &output));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Err(MarkError::Usage("git path was empty".to_owned()));
    }

    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

fn git_error(context: &str, output: &std::process::Output) -> MarkError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        MarkError::Usage(context.to_owned())
    } else {
        MarkError::Usage(format!("{context}: {detail}"))
    }
}
