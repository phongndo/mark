use std::{
    io,
    path::{Path, PathBuf},
};

use mark_command::{DiffOptions, DiffOutput, DiffSource, PatchSource, UntrackedMode};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Source {
    Worktree {
        #[serde(default = "include_untracked")]
        include_untracked: bool,
    },
    Base {
        base: String,
    },
    Show {
        rev: String,
    },
    Range {
        left: String,
        right: String,
    },
    Patch {
        path: PathBuf,
    },
    Review {
        target: String,
    },
    Difftool {
        left: PathBuf,
        right: PathBuf,
        path: Option<String>,
    },
}

fn include_untracked() -> bool {
    true
}

pub(super) fn options(repository: &Value, source: &Value) -> io::Result<DiffOptions> {
    let repo = repository.as_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid_repository: expected a repository path",
        )
    })?;
    let source: Source = serde_json::from_value(source.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut local_untracked = UntrackedMode::Include;
    let source = match source {
        Source::Worktree { include_untracked } => {
            local_untracked = UntrackedMode::from_include(include_untracked);
            DiffSource::Worktree
        }
        Source::Base { base } => DiffSource::Base(base.into()),
        Source::Show { rev } => DiffSource::Show(rev.into()),
        Source::Range { left, right } => DiffSource::Range {
            left: left.into(),
            right: right.into(),
        },
        Source::Patch { path } => DiffSource::Patch(PatchSource::File(resolve(repo, &path))),
        Source::Review { target } => {
            return mark_command::review_diff_options(Some(PathBuf::from(repo)), &target, false)
                .map_err(|error| io::Error::other(error.to_string()));
        }
        Source::Difftool { left, right, path } => DiffSource::Difftool {
            left: resolve(repo, &left).into(),
            right: resolve(repo, &right).into(),
            path: path.map(|path| PathBuf::from(path).into()),
        },
    };
    Ok(DiffOptions {
        repo: Some(PathBuf::from(repo).into()),
        source,
        local_untracked,
        output: DiffOutput::Patch,
    })
}

fn resolve(repo: &str, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        Path::new(repo).join(path)
    }
}

pub(super) fn patch(options: &DiffOptions) -> io::Result<String> {
    let changeset = mark_diff::load_review_ref_with_raw_patch(options)
        .map_err(|error| io::Error::other(error.to_string()))?;
    String::from_utf8(changeset.raw_patch.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "review patch is not UTF-8"))
}
