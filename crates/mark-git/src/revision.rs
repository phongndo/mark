use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

use mark_core::{MarkError, MarkResult};

pub fn show_target(repo: &Path, rev: &str) -> MarkResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-t", "--end-of-options"])
        .arg(rev)
        .output()?;

    if !output.status.success() {
        return Ok(rev.to_owned());
    }

    if String::from_utf8_lossy(&output.stdout).trim() == "tag" {
        Ok(format!("{rev}^{{}}"))
    } else {
        Ok(rev.to_owned())
    }
}

pub fn existing_commitish_revision(repo: &Path, rev: &str, kind: &str) -> MarkResult<String> {
    existing_revision(repo, rev, kind, "commit")
}

pub fn existing_object_revision(repo: &Path, rev: &str) -> MarkResult<String> {
    if revision_expression_exists(repo, rev)? {
        return Ok(rev.to_owned());
    }

    Err(unknown_revision_error(rev, ""))
}

pub fn revision_is_treeish(repo: &Path, rev: &str) -> MarkResult<bool> {
    let Some(object) = resolve_revision(repo, rev)? else {
        return Ok(false);
    };
    revision_object_matches(repo, &object, "tree")
}

pub fn revision_expression_exists(repo: &Path, rev: &str) -> MarkResult<bool> {
    let output = rev_parse_verify(repo, rev)?;
    // `rev-parse --verify` exits non-zero for expressions that expand to
    // multiple objects, but still writes the resolved objects. `git diff`
    // accepts those expressions as range operands.
    if output.status.success() || !output_stdout_is_empty(&output) {
        return Ok(true);
    }

    multi_revision_expression_exists(repo, rev)
}

pub fn range_right_operand_is_pathspec(repo: &Path, left: &str, right: &str) -> MarkResult<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        // Keep the right operand ambiguous here so Git decides whether it is a
        // pathspec in the same way it would for `git diff <rev> <path>`.
        .args(["diff", "--quiet", "--no-ext-diff", "--end-of-options"])
        .arg(left)
        .arg(right)
        .output()?;

    Ok(matches!(output.status.code(), Some(0) | Some(1)))
}

pub fn worktree_base_revision(repo: &Path) -> MarkResult<String> {
    if has_head(repo)? {
        Ok("HEAD".to_owned())
    } else {
        empty_tree_revision(repo)
    }
}

pub fn merge_base_revision(repo: &Path, base: &str) -> MarkResult<String> {
    if !commitish_exists(repo, base)? {
        return Err(MarkError::Usage(format!("unknown base revision `{base}`")));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--end-of-options", base, "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(crate::git_error(
            "failed to derive branch merge base",
            &output,
        ));
    }

    let revision = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if revision.is_empty() {
        return Err(MarkError::Usage(
            "git returned an empty merge base revision".to_owned(),
        ));
    }
    Ok(revision)
}

fn existing_revision(repo: &Path, rev: &str, kind: &str, object_kind: &str) -> MarkResult<String> {
    if revision_exists(repo, rev, object_kind)? {
        return Ok(rev.to_owned());
    }

    Err(unknown_revision_error(rev, kind))
}

fn unknown_revision_error(rev: &str, kind: &str) -> MarkError {
    if kind.is_empty() {
        MarkError::Usage(format!("unknown revision `{rev}`"))
    } else {
        MarkError::Usage(format!("unknown {kind} revision `{rev}`"))
    }
}

fn commitish_exists(repo: &Path, rev: &str) -> MarkResult<bool> {
    revision_exists(repo, rev, "commit")
}

fn revision_exists(repo: &Path, rev: &str, object_kind: &str) -> MarkResult<bool> {
    let Some(object) = resolve_revision(repo, rev)? else {
        return Ok(false);
    };

    revision_object_matches(repo, &object, object_kind)
}

fn resolve_revision(repo: &Path, rev: &str) -> MarkResult<Option<String>> {
    let output = rev_parse_verify(repo, rev)?;
    if !output.status.success() {
        return Ok(None);
    }

    let revision = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if revision.is_empty() {
        Ok(None)
    } else {
        Ok(Some(revision))
    }
}

fn rev_parse_verify(repo: &Path, rev: &str) -> MarkResult<Output> {
    Ok(git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(rev)
        .output()?)
}

fn multi_revision_expression_exists(repo: &Path, rev: &str) -> MarkResult<bool> {
    let output = git_command(repo)
        .args(["rev-list", "--no-walk", "--quiet", "--end-of-options"])
        .arg(rev)
        .output()?;
    Ok(output.status.success())
}

fn output_stdout_is_empty(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

fn revision_object_matches(repo: &Path, object: &str, object_kind: &str) -> MarkResult<bool> {
    let output = git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{object}^{{{object_kind}}}"))
        .output()?;
    Ok(output.status.success())
}

fn empty_tree_revision(repo: &Path) -> MarkResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", "-t", "tree", "--stdin"])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(crate::git_error(
            "failed to derive empty tree revision",
            &output,
        ));
    }

    let revision = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if revision.is_empty() {
        return Err(MarkError::Usage(
            "git returned an empty tree revision with no object id".to_owned(),
        ));
    }
    Ok(revision)
}

fn has_head(repo: &Path) -> MarkResult<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()?;
    Ok(output.status.success())
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    command
}
