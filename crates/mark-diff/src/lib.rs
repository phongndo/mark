use mark_core::{MarkError, MarkResult};
use std::{
    borrow::Cow,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const STREAM_BUFFER_BYTES: usize = 1024 * 1024;

mod difftool;
mod git_args;
mod git_io;
mod parser;
mod stats;
mod types;

use difftool::difftool_workdir;
use git_args::{
    append_pathspecs, diff_title, git_diff_args, should_include_untracked, validate_options,
};
use git_io::{
    git_diff_bytes_with_untracked_pathspecs, git_diff_to_writer, git_diff_to_writer_with_untracked,
};
pub use parser::{parse_patch, parse_patch_bytes, parse_patch_bytes_limited};
#[cfg(test)]
use stats::parse_patch_stats;
use stats::{patch_stats, render_patch_stats, terminal_safe_text};
pub use types::{
    BranchName, Changeset, CommitSha, DiffFile, DiffFileBody, DiffHunk, DiffLimitExceeded,
    DiffLimits, DiffLine, DiffLineKind, DiffOptions, DiffOutput, DiffPath, DiffRowRef, DiffSource,
    DiffStats, DiffViewModel, DisplayPath, FileChange, FileStatus, HunkLineRanges, LineSpan,
    NewLineNumber, OldLineNumber, PatchLabel, PatchSource, PullRequestId, RefName, RemoteName,
    RepoArg, RepoRelativePath, RepoRoot, RevSpec, ReviewId, UntrackedMode, WorktreePath,
};

pub fn load(options: DiffOptions) -> MarkResult<Changeset> {
    load_changeset(&options, true)
}

pub fn load_review(options: DiffOptions) -> MarkResult<Changeset> {
    load_review_ref(&options)
}

pub fn load_review_ref(options: &DiffOptions) -> MarkResult<Changeset> {
    load_changeset(options, false)
}

pub fn load_review_ref_limited(options: &DiffOptions, limits: DiffLimits) -> MarkResult<Changeset> {
    load_changeset_limited(options, false, limits)
}

pub fn load_review_ref_with_patch_bytes(options: &DiffOptions) -> MarkResult<(Changeset, u64)> {
    load_changeset_with_patch_bytes(options, false)
}

pub fn load_review_ref_path(options: &DiffOptions, path: &Path) -> MarkResult<Changeset> {
    load_changeset_paths(options, &[path.to_path_buf()], false)
}

pub fn load_review_ref_paths(options: &DiffOptions, paths: &[PathBuf]) -> MarkResult<Changeset> {
    load_changeset_paths(options, paths, false)
}

fn load_changeset(options: &DiffOptions, keep_raw_patch: bool) -> MarkResult<Changeset> {
    load_changeset_limited(options, keep_raw_patch, DiffLimits::from_env())
}

fn load_changeset_limited(
    options: &DiffOptions,
    keep_raw_patch: bool,
    limits: DiffLimits,
) -> MarkResult<Changeset> {
    load_changeset_with_patch_bytes_limited(options, keep_raw_patch, limits)
        .map(|(changeset, _)| changeset)
}

fn load_changeset_with_patch_bytes(
    options: &DiffOptions,
    keep_raw_patch: bool,
) -> MarkResult<(Changeset, u64)> {
    load_changeset_with_patch_bytes_limited(options, keep_raw_patch, DiffLimits::from_env())
}

fn load_changeset_with_patch_bytes_limited(
    options: &DiffOptions,
    keep_raw_patch: bool,
    limits: DiffLimits,
) -> MarkResult<(Changeset, u64)> {
    let title = diff_title(options);
    if let DiffSource::Patch(PatchSource::File(path)) = &options.source {
        validate_options(options)?;
        let repo = options
            .repo
            .clone()
            .map(RepoArg::into_path_buf)
            .unwrap_or_default();
        let patch = read_file_arc(path, limits.max_patch_bytes)?;
        let patch_bytes = u64::try_from(patch.len()).unwrap_or(u64::MAX);
        let changeset =
            changeset_from_shared_patch_limited(repo, title, patch, keep_raw_patch, limits)?;
        return Ok((changeset, patch_bytes));
    }
    let (repo, patch) = diff_patch_bytes_limited(options, limits.max_patch_bytes)?;
    let patch_bytes = u64::try_from(patch.len()).unwrap_or(u64::MAX);
    let changeset = changeset_from_patch_limited(repo, title, patch, keep_raw_patch, limits)?;
    Ok((changeset, patch_bytes))
}

fn load_changeset_paths(
    options: &DiffOptions,
    paths: &[PathBuf],
    keep_raw_patch: bool,
) -> MarkResult<Changeset> {
    let title = diff_title(options);
    let limits = DiffLimits::from_env();
    let (repo, patch) = diff_patch_bytes_paths(options, paths, limits.max_patch_bytes)?;
    changeset_from_patch_limited(repo, title, Cow::Owned(patch), keep_raw_patch, limits)
}

fn changeset_from_patch_limited(
    repo: PathBuf,
    title: String,
    patch: Cow<'_, [u8]>,
    keep_raw_patch: bool,
    limits: DiffLimits,
) -> MarkResult<Changeset> {
    let patch: Arc<[u8]> = match patch {
        Cow::Borrowed(bytes) => Arc::from(bytes),
        Cow::Owned(bytes) => Arc::from(bytes),
    };
    changeset_from_shared_patch_limited(repo, title, patch, keep_raw_patch, limits)
}

fn changeset_from_shared_patch_limited(
    repo: PathBuf,
    title: String,
    patch: Arc<[u8]>,
    keep_raw_patch: bool,
    limits: DiffLimits,
) -> MarkResult<Changeset> {
    if let Some(max_patch_bytes) = limits.max_patch_bytes
        && patch.len() > max_patch_bytes
    {
        return Err(MarkError::Usage(
            DiffLimitExceeded::new("patch bytes", max_patch_bytes, patch.len()).to_string(),
        ));
    }
    let files = parse_patch_bytes_limited(Arc::clone(&patch), limits)
        .map_err(|error| MarkError::Usage(error.to_string()))?;
    let raw_patch = if keep_raw_patch {
        patch
    } else {
        Changeset::empty_raw_patch()
    };

    Ok(Changeset {
        repo: repo.into(),
        title,
        files,
        raw_patch,
    })
}

fn read_file_arc(path: &Path, max_patch_bytes: Option<usize>) -> MarkResult<Arc<[u8]>> {
    if max_patch_bytes.is_some() {
        return read_patch_input_limited(fs::File::open(path)?, max_patch_bytes);
    }

    let mut file = fs::File::open(path)?;
    let len = usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::other("patch file is too large for this platform"))?;
    let mut raw = Arc::<[u8]>::new_uninit_slice(len);
    let uninit = Arc::get_mut(&mut raw).expect("new Arc should be uniquely owned");
    // SAFETY: the slice covers the same allocation as `uninit`; `read_exact`
    // initializes every byte before `assume_init` below.
    let bytes = unsafe { std::slice::from_raw_parts_mut(uninit.as_mut_ptr().cast::<u8>(), len) };
    file.read_exact(bytes)?;
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        // The file grew after metadata was read. Preserve fs::read semantics
        // on this rare race rather than returning a truncated patch.
        return Ok(Arc::from(fs::read(path)?));
    }
    // SAFETY: `read_exact` succeeded for the complete allocation.
    Ok(unsafe { raw.assume_init() })
}

fn diff_patch_bytes_paths(
    options: &DiffOptions,
    paths: &[PathBuf],
    max_patch_bytes: Option<usize>,
) -> MarkResult<(PathBuf, Vec<u8>)> {
    if matches!(
        options.source,
        DiffSource::Patch(_) | DiffSource::Difftool { .. }
    ) {
        return Err(MarkError::Usage(
            "path-scoped reload does not apply to patch or difftool input".to_owned(),
        ));
    }
    if paths.is_empty() {
        return Err(MarkError::Usage(
            "path-scoped reload requires at least one path".to_owned(),
        ));
    }

    let repo = mark_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let mut args = git_diff_args(options, &repo)?;
    append_pathspecs(&mut args, paths);
    let patch = if should_include_untracked(options) {
        git_diff_bytes_with_untracked_pathspecs(&repo, &args, paths, max_patch_bytes)?
    } else {
        git_io::git_diff_bytes_limited(&repo, &args, max_patch_bytes)?
    };

    Ok((repo, patch))
}

fn diff_patch_bytes(options: &DiffOptions) -> MarkResult<(PathBuf, Cow<'_, [u8]>)> {
    diff_patch_bytes_limited(options, DiffLimits::from_env().max_patch_bytes)
}

fn diff_patch_bytes_limited(
    options: &DiffOptions,
    max_patch_bytes: Option<usize>,
) -> MarkResult<(PathBuf, Cow<'_, [u8]>)> {
    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        let repo = options
            .repo
            .clone()
            .map(RepoArg::into_path_buf)
            .unwrap_or_default();
        let patch = patch_source_bytes(source, max_patch_bytes)?;
        return Ok((repo, patch));
    }

    if let DiffSource::Difftool { left, right, path } = &options.source {
        validate_options(options)?;
        let repo = difftool_workdir(options)?;
        let patch = difftool::difftool_patch_bytes_limited(
            &repo,
            left,
            right,
            path.as_deref(),
            max_patch_bytes,
        )?;
        return Ok((repo, Cow::Owned(patch)));
    }

    let repo = mark_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_args(options, &repo)?;
    let patch = if should_include_untracked(options) {
        git_io::git_diff_bytes_with_untracked_limited(&repo, &args, max_patch_bytes)?
    } else {
        git_io::git_diff_bytes_limited(&repo, &args, max_patch_bytes)?
    };

    Ok((repo, Cow::Owned(patch)))
}

pub fn render(options: DiffOptions) -> MarkResult<String> {
    let bytes = render_bytes(options)?;
    String::from_utf8(bytes).map_err(|_| {
        MarkError::Usage("diff output is not valid UTF-8; use byte-preserving output".to_owned())
    })
}

pub fn render_bytes(options: DiffOptions) -> MarkResult<Vec<u8>> {
    if options.is_stat() {
        return render_stat_bytes(&options);
    }
    let (_, patch) = diff_patch_bytes(&options)?;
    Ok(patch.into_owned())
}

pub fn render_to_writer(options: DiffOptions, writer: impl Write) -> MarkResult<()> {
    render_to_writer_ref(&options, writer)
}

pub fn render_to_writer_ref(options: &DiffOptions, mut writer: impl Write) -> MarkResult<()> {
    if options.is_stat() {
        writer.write_all(&render_stat_bytes(options)?)?;
        return Ok(());
    }

    let max_patch_bytes = DiffLimits::from_env().max_patch_bytes;
    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        write_patch_source(source, writer, max_patch_bytes)?;
        return Ok(());
    }

    if let DiffSource::Difftool { left, right, path } = &options.source {
        validate_options(options)?;
        let repo = difftool_workdir(options)?;
        writer.write_all(&difftool::difftool_patch_bytes_limited(
            &repo,
            left,
            right,
            path.as_deref(),
            max_patch_bytes,
        )?)?;
        return Ok(());
    }

    let repo = mark_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_args(options, &repo)?;
    if should_include_untracked(options) {
        git_diff_to_writer_with_untracked(&repo, &args, writer, max_patch_bytes)
    } else {
        git_diff_to_writer(&repo, &args, writer, max_patch_bytes)
    }
}

fn render_stat_bytes(options: &DiffOptions) -> MarkResult<Vec<u8>> {
    let stats = patch_stats(options)?;
    Ok(render_patch_stats(&stats).into_bytes())
}

fn write_patch_source(
    source: &PatchSource,
    mut writer: impl Write,
    max_patch_bytes: Option<usize>,
) -> MarkResult<()> {
    match source {
        PatchSource::File(path) => {
            let mut file = fs::File::open(path)?;
            copy_to_writer_limited(&mut file, &mut writer, max_patch_bytes)?;
        }
        PatchSource::Stdin(patch) => {
            check_patch_byte_limit(patch.len(), max_patch_bytes)?;
            writer.write_all(patch.as_ref())?;
        }
        PatchSource::Text { patch, .. } | PatchSource::Review { patch, .. } => {
            check_patch_byte_limit(patch.len(), max_patch_bytes)?;
            writer.write_all(patch.as_ref())?;
        }
    }
    Ok(())
}

fn copy_to_writer_limited(
    mut reader: impl Read,
    mut writer: impl Write,
    max_patch_bytes: Option<usize>,
) -> MarkResult<u64> {
    let Some(max) = max_patch_bytes else {
        return copy_to_writer(reader, writer).map_err(Into::into);
    };

    let mut total = 0usize;
    let mut buffer = vec![0; STREAM_BUFFER_BYTES];
    loop {
        let remaining = max.saturating_sub(total);
        let read_capacity = remaining.saturating_add(1).min(buffer.len());
        let read = reader.read(&mut buffer[..read_capacity])?;
        if read == 0 {
            return Ok(u64::try_from(total).unwrap_or(u64::MAX));
        }
        let writable = read.min(remaining);
        writer.write_all(&buffer[..writable])?;
        total = total.saturating_add(read);
        if read > remaining {
            check_patch_byte_limit(total, Some(max))?;
        }
    }
}

fn copy_to_writer(mut reader: impl Read, mut writer: impl Write) -> io::Result<u64> {
    let mut total = 0u64;
    let mut buffer = vec![0; STREAM_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total = total.saturating_add(read as u64);
    }
    Ok(total)
}

pub fn read_patch_input_limited(
    reader: impl Read,
    max_patch_bytes: Option<usize>,
) -> MarkResult<Arc<[u8]>> {
    let patch = read_to_end_limited(reader, max_patch_bytes)?;
    Ok(Arc::from(patch.into_boxed_slice()))
}

pub(crate) fn read_to_end_limited(
    mut reader: impl Read,
    max_patch_bytes: Option<usize>,
) -> MarkResult<Vec<u8>> {
    let mut patch = Vec::new();
    match max_patch_bytes {
        Some(max) => {
            let read_limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
            reader.take(read_limit).read_to_end(&mut patch)?;
            check_patch_byte_limit(patch.len(), Some(max))?;
        }
        None => {
            reader.read_to_end(&mut patch)?;
        }
    }
    Ok(patch)
}

pub(crate) fn check_patch_byte_limit(
    actual: usize,
    max_patch_bytes: Option<usize>,
) -> MarkResult<()> {
    if let Some(max) = max_patch_bytes
        && actual > max
    {
        return Err(MarkError::Usage(
            DiffLimitExceeded::new("patch bytes", max, actual).to_string(),
        ));
    }
    Ok(())
}

fn patch_source_bytes(
    source: &PatchSource,
    max_patch_bytes: Option<usize>,
) -> MarkResult<Cow<'_, [u8]>> {
    let patch = match source {
        PatchSource::File(path) => {
            Cow::Owned(read_to_end_limited(fs::File::open(path)?, max_patch_bytes)?)
        }
        PatchSource::Stdin(patch) => Cow::Borrowed(patch.as_ref()),
        PatchSource::Text { patch, .. } | PatchSource::Review { patch, .. } => {
            Cow::Borrowed(patch.as_ref())
        }
    };
    check_patch_byte_limit(patch.len(), max_patch_bytes)?;
    Ok(patch)
}

pub fn render_stat(changeset: &Changeset) -> String {
    let mut output = String::new();
    for file in &changeset.files {
        output.push_str(&format!(
            "{:>6} {:>6} {}\n",
            file.additions,
            file.deletions,
            terminal_safe_text(file.display_path())
        ));
    }
    let stats = changeset.stats();
    output.push_str(&format!(
        "\n{} files changed, {} insertions(+), {} deletions(-)",
        stats.files, stats.additions, stats.deletions
    ));
    if stats.binary_files > 0 {
        output.push_str(&format!(", {} binary", stats.binary_files));
    }
    output.push('\n');
    output
}

#[cfg(test)]
mod tests;
