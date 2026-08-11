use std::{
    io,
    path::{Path, PathBuf},
};

use mark_core::{MarkError, MarkResult};

use crate::args::{CompareArgs, DiffArgs, DifftoolArgs, PatchArgs, ReviewArgs, ShowArgs};

fn diff_output(stat: bool) -> mark_command::DiffOutput {
    if stat {
        mark_command::DiffOutput::Stat
    } else {
        mark_command::DiffOutput::Patch
    }
}

pub(crate) fn diff_options(args: DiffArgs) -> MarkResult<mark_command::DiffOptions> {
    Ok(mark_command::DiffOptions {
        repo: args.repo.repo.map(Into::into),
        source: mark_command::DiffSource::Worktree,
        local_untracked: mark_command::UntrackedMode::from_include(!args.no_untracked),
        output: diff_output(args.display.stat),
    })
}

pub(crate) fn compare_options(args: CompareArgs) -> MarkResult<mark_command::DiffOptions> {
    let source = match args.revs.as_slice() {
        [base] => mark_command::DiffSource::Base(base.clone().into()),
        [left, right] => mark_command::DiffSource::Range {
            left: left.clone().into(),
            right: right.clone().into(),
        },
        _ => {
            return Err(MarkError::Usage(
                "compare expects one or two revisions".to_owned(),
            ));
        }
    };

    Ok(mark_command::DiffOptions {
        repo: args.repo.repo.map(Into::into),
        source,
        local_untracked: mark_command::UntrackedMode::from_include(!args.no_untracked),
        output: diff_output(args.display.stat),
    })
}

pub(crate) fn show_options(args: ShowArgs) -> MarkResult<mark_command::DiffOptions> {
    Ok(mark_command::DiffOptions {
        repo: args.repo.repo.map(Into::into),
        source: mark_command::DiffSource::Show(
            args.rev.unwrap_or_else(|| "HEAD".to_owned()).into(),
        ),
        local_untracked: mark_command::UntrackedMode::Exclude,
        output: diff_output(args.display.stat),
    })
}

pub(crate) fn review_options(args: ReviewArgs) -> MarkResult<mark_command::DiffOptions> {
    mark_command::review_diff_options(args.repo.repo, &args.target, args.display.stat)
}

pub(crate) fn difftool_options(args: DifftoolArgs) -> MarkResult<mark_command::DiffOptions> {
    Ok(mark_command::DiffOptions {
        repo: args.repo.repo.map(Into::into),
        source: mark_command::DiffSource::Difftool {
            left: args.left.into(),
            right: args.right.into(),
            path: args.path.map(Into::into),
        },
        local_untracked: mark_command::UntrackedMode::Exclude,
        output: diff_output(args.display.stat),
    })
}

pub(crate) fn patch_options(args: PatchArgs) -> MarkResult<mark_command::DiffOptions> {
    Ok(mark_command::DiffOptions {
        repo: args.repo.repo.map(Into::into),
        source: patch_source(args.path)?,
        local_untracked: mark_command::UntrackedMode::Exclude,
        output: diff_output(args.display.stat),
    })
}

pub(crate) fn patch_source(path: PathBuf) -> MarkResult<mark_command::DiffSource> {
    if path == Path::new("-") {
        let max_patch_bytes = mark_diff::DiffLimits::from_env().max_patch_bytes;
        let patch = mark_diff::read_patch_input_limited(io::stdin().lock(), max_patch_bytes)?;
        return Ok(mark_command::DiffSource::Patch(
            mark_command::PatchSource::Stdin(patch),
        ));
    }

    Ok(mark_command::DiffSource::Patch(
        mark_command::PatchSource::File(path),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{DiffWatchArgs, DisplayArgs, RepoArgs};

    fn compare_args(revs: &[&str]) -> CompareArgs {
        CompareArgs {
            revs: revs.iter().map(|rev| (*rev).to_owned()).collect(),
            repo: RepoArgs::default(),
            no_untracked: false,
            watch: DiffWatchArgs::default(),
            display: DisplayArgs::default(),
        }
    }

    #[test]
    fn diff_always_selects_all_local_changes() {
        let options = diff_options(DiffArgs::default()).expect("diff options should build");
        assert_eq!(options.source, mark_command::DiffSource::Worktree);
    }

    #[test]
    fn compare_one_revision_selects_the_current_workspace() {
        let options =
            compare_options(compare_args(&["main"])).expect("compare options should build");
        assert_eq!(
            options.source,
            mark_command::DiffSource::Base("main".into())
        );
    }

    #[test]
    fn compare_two_revisions_selects_an_exact_comparison() {
        let options = compare_options(compare_args(&["main", "feature"]))
            .expect("compare options should build");
        assert_eq!(
            options.source,
            mark_command::DiffSource::Range {
                left: "main".into(),
                right: "feature".into(),
            }
        );
    }
}
