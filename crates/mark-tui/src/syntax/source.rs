use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use mark_diff::{
    DiffLine, DiffLineKind, DiffOptions, DiffSource, RepoRelativePath, RepoRoot, RevSpec,
};
use mark_syntax::{SyntaxHighlighter, SyntaxLimits};
use tokio::sync::mpsc::Sender;

use super::{
    DiffSide, HighlightedSide, SyntaxKey, SyntaxPriority, SyntaxSkipReason, SyntaxWorkerQueue,
    types::SyntaxSourceKind,
};

#[derive(Debug)]
pub(crate) struct SyntaxJob {
    pub(crate) key: SyntaxKey,
    pub(crate) language: String,
    pub(crate) source: SyntaxJobSource,
    pub(crate) limits: SyntaxLimits,
    pub(crate) queued_source_bytes: u64,
    pub(crate) priority: SyntaxPriority,
    pub(crate) queued_at: Instant,
}

#[derive(Debug)]
pub(crate) struct SyntaxResult {
    pub(crate) key: SyntaxKey,
    pub(crate) language: String,
    pub(crate) source_kind: SyntaxSourceKind,
    pub(crate) priority: SyntaxPriority,
    pub(crate) queue_latency_micros: u128,
    pub(crate) run_latency_micros: u128,
    pub(crate) side: Result<SyntaxSuccess, SyntaxJobFailure>,
}

#[derive(Debug)]
pub(crate) struct SyntaxSuccess {
    pub(crate) side: HighlightedSide,
    pub(crate) source_bytes: Option<u64>,
    pub(crate) source_lines: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum SyntaxJobFailure {
    Unavailable,
    HighlightError,
}

#[derive(Debug)]
pub(crate) struct HunkSource {
    pub(crate) text: String,
    pub(crate) line_map: Vec<Option<usize>>,
    pub(crate) source_lines: usize,
}

#[derive(Debug)]
pub(crate) enum SyntaxJobSource {
    Hunk(HunkSource),
    FullFile(FullFileSource),
}

impl SyntaxJobSource {
    pub(crate) fn known_bytes(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.text.len() as u64),
            // Full-file source sizes may require git subprocesses. Those checks
            // are deliberately performed inside the syntax worker so viewport
            // scrolling never blocks on filesystem/git I/O while queueing jobs.
            Self::FullFile(_) => None,
        }
    }

    pub(crate) fn known_lines(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.source_lines as u64),
            Self::FullFile(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FullFileSource {
    pub(crate) repo: RepoRoot,
    pub(crate) kind: FullFileSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullFileSourceKind {
    Worktree {
        path: RepoRelativePath,
    },
    GitRevision {
        rev: RevSpec,
        path: RepoRelativePath,
    },
    GitMergeBase {
        base: RevSpec,
        head: RevSpec,
        path: RepoRelativePath,
    },
}

pub(crate) fn run_syntax_worker(queue: SyntaxWorkerQueue, result_tx: Sender<SyntaxResult>) {
    let mut highlighter = SyntaxHighlighter::new();
    while let Some(job) = queue.pop() {
        let queue_latency_micros = job.queued_at.elapsed().as_micros();
        let run_start = Instant::now();
        let key = job.key;
        let language = job.language;
        let source_kind = key.source.kind;
        let priority = job.priority;
        let limits = job.limits;
        let source = job.source;
        let side = panic::catch_unwind(AssertUnwindSafe(|| {
            let (source, source_bytes, source_lines) = load_job_source(source, limits)?;
            highlighter
                .highlight(&language, &source)
                .map(|highlighted| SyntaxSuccess {
                    side: HighlightedSide {
                        lines: highlighted.lines,
                    },
                    source_bytes,
                    source_lines,
                })
                .map_err(|_| SyntaxJobFailure::HighlightError)
        }))
        .unwrap_or(Err(SyntaxJobFailure::HighlightError));
        let run_latency_micros = run_start.elapsed().as_micros();
        if result_tx
            .blocking_send(SyntaxResult {
                key,
                language,
                source_kind,
                priority,
                queue_latency_micros,
                run_latency_micros,
                side,
            })
            .is_err()
        {
            break;
        }
    }
}

pub(crate) fn load_job_source(
    source: SyntaxJobSource,
    limits: SyntaxLimits,
) -> Result<(String, Option<u64>, Option<u64>), SyntaxJobFailure> {
    match source {
        SyntaxJobSource::Hunk(source) => Ok((source.text, None, None)),
        SyntaxJobSource::FullFile(source) => {
            let source_bytes =
                full_file_source_size(&source).map_err(|_| SyntaxJobFailure::Unavailable)?;
            if usize::try_from(source_bytes)
                .ok()
                .is_some_and(|bytes| bytes > limits.max_source_bytes)
            {
                return Err(SyntaxJobFailure::Unavailable);
            }
            let text = load_full_file_source(&source).map_err(|_| SyntaxJobFailure::Unavailable)?;
            let source_lines = validate_highlight_source_with_line_count(&text, limits)
                .map_err(|_| SyntaxJobFailure::Unavailable)? as u64;
            Ok((text, Some(source_bytes), Some(source_lines)))
        }
    }
}

pub(crate) fn syntax_path(file: &mark_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path().or(file.new_path()),
        DiffSide::New => file.new_path().or(file.old_path()),
    }
}

pub(crate) fn build_hunk_source(
    lines: &[DiffLine],
    side: DiffSide,
    limits: SyntaxLimits,
) -> Result<HunkSource, SyntaxSkipReason> {
    let mut text = String::new();
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind(), side) {
            continue;
        }
        let line_text = line.text_lossy();
        if line_text.len() > limits.max_line_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        if source_lines > 0 {
            text.push('\n');
        }
        text.push_str(&line_text);
        if text.len() > limits.max_source_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        line_map[index] = Some(source_lines);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(HunkSource {
        text,
        line_map,
        source_lines,
    })
}

pub(crate) fn build_full_file_line_map(
    lines: &[DiffLine],
    side: DiffSide,
) -> Result<Vec<Option<usize>>, SyntaxSkipReason> {
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind(), side) {
            continue;
        }

        let Some(line_number) = diff_line_number(line, side) else {
            continue;
        };
        let Some(source_line) = line_number.checked_sub(1) else {
            continue;
        };
        line_map[index] = Some(source_line);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(line_map)
}

pub(crate) fn diff_line_number(line: &DiffLine, side: DiffSide) -> Option<usize> {
    match side {
        DiffSide::Old => line.old_line(),
        DiffSide::New => line.new_line(),
    }
}

fn range_operand_contains_path_selector(operand: &RevSpec) -> bool {
    // `:/regex` searches commit messages. Its leading colon (and any colons in
    // the regex itself) are part of the revision rather than a path selector.
    if operand.starts_with(":/") {
        return false;
    }

    let mut brace_depth = 0usize;
    for character in operand.chars() {
        match character {
            '{' => brace_depth = brace_depth.saturating_add(1),
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ':' if brace_depth == 0 => return true,
            _ => {}
        }
    }
    false
}

type RangeOperandCacheKey = (PathBuf, RevSpec);
static RANGE_OPERAND_CACHE: OnceLock<Mutex<HashMap<RangeOperandCacheKey, Option<RevSpec>>>> =
    OnceLock::new();

fn range_operand_cache() -> &'static Mutex<HashMap<RangeOperandCacheKey, Option<RevSpec>>> {
    RANGE_OPERAND_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn invalidate_range_operand_revision_cache(repo: &Path, options: &DiffOptions) {
    let DiffSource::Range { left, right } = &options.source else {
        return;
    };
    let Some(cache) = RANGE_OPERAND_CACHE.get() else {
        return;
    };
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    cache.remove(&(repo.to_owned(), left.clone()));
    cache.remove(&(repo.to_owned(), right.clone()));
}

fn range_operand_treeish_revision(repo: &Path, operand: &RevSpec) -> Option<RevSpec> {
    // Reflog date selectors and braced revision-search patterns may contain colons.
    // Reject only a top-level `REV:path` selector before asking Git whether the
    // remaining revision resolves to a commit or tree.
    if range_operand_contains_path_selector(operand) {
        return None;
    }

    let cache = range_operand_cache();
    let key = (repo.to_owned(), operand.clone());
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(revision) = cache.get(&key) {
        return revision.clone();
    }

    // A range operand is shared by every hunk and file in the changeset. Keep
    // the object-kind lookup here so source construction stays cheap after the
    // first lookup instead of running `git rev-parse` per hunk.
    let revision = mark_git::revision_is_treeish(repo, operand)
        .unwrap_or(false)
        .then(|| operand.clone())
        .and_then(|revision| {
            // Appending `:path` directly to `:/regex` is ambiguous to Git.
            // Resolve commit-search expressions first so the full-file blob
            // selector is constructed from an object ID.
            if !revision.starts_with(":/") {
                return Some(revision);
            }
            let output = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
                .arg(revision.as_ref())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            (!resolved.is_empty()).then_some(resolved.into())
        });
    cache.insert(key, revision.clone());
    revision
}

pub(crate) fn full_file_source(
    repo: &Path,
    options: &DiffOptions,
    file: &mark_diff::DiffFile,
    side: DiffSide,
) -> Option<FullFileSource> {
    if matches!(
        options.source,
        DiffSource::Patch(_) | DiffSource::Show(_) | DiffSource::Difftool { .. }
    ) {
        return None;
    }
    if !repo.is_dir() {
        return None;
    }
    let range_revision = if let DiffSource::Range { left, right } = &options.source {
        let operand = match side {
            DiffSide::Old => left,
            DiffSide::New => right,
        };
        // Full-file sources append the diff path, so the operand must resolve
        // to a tree-ish object. Blob objects and existing `REV:path` operands
        // are valid range inputs, but cannot be extended with another path.
        Some(range_operand_treeish_revision(repo, operand)?)
    } else {
        None
    };

    let path: RepoRelativePath = file_path_for_side(file, side)?.into();
    let kind = match (&options.source, side) {
        (DiffSource::Patch(_) | DiffSource::Show(_) | DiffSource::Difftool { .. }, _) => {
            return None;
        }
        (DiffSource::Worktree, DiffSide::Old) => FullFileSourceKind::GitRevision {
            rev: "HEAD".into(),
            path,
        },
        (DiffSource::Worktree, DiffSide::New) => FullFileSourceKind::Worktree { path },
        (DiffSource::Base(base), DiffSide::Old) => FullFileSourceKind::GitMergeBase {
            base: base.clone(),
            head: "HEAD".into(),
            path,
        },
        (DiffSource::Base(_), DiffSide::New) => FullFileSourceKind::Worktree { path },
        (DiffSource::Branch { base, head }, DiffSide::Old) => FullFileSourceKind::GitMergeBase {
            base: base.clone(),
            head: head.clone(),
            path,
        },
        (DiffSource::Branch { head, .. }, DiffSide::New) => FullFileSourceKind::GitRevision {
            rev: head.clone(),
            path,
        },
        (DiffSource::Range { .. }, DiffSide::Old | DiffSide::New) => {
            FullFileSourceKind::GitRevision {
                rev: range_revision.expect("range sources have a validated revision"),
                path,
            }
        }
    };

    Some(FullFileSource {
        repo: repo.to_owned().into(),
        kind,
    })
}

pub(crate) fn file_path_for_side(file: &mark_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path(),
        DiffSide::New => file.new_path(),
    }
}

pub(crate) fn load_full_file_source(source: &FullFileSource) -> Result<String, SyntaxSkipReason> {
    let bytes = match &source.kind {
        FullFileSourceKind::Worktree { path } => read_worktree_file(&source.repo, path)?,
        FullFileSourceKind::GitRevision { rev, path } => {
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
        FullFileSourceKind::GitMergeBase { base, head, path } => {
            let rev = git_merge_base(&source.repo, base, head)?;
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
    };

    Ok(match String::from_utf8(bytes) {
        Ok(source) => source,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

pub(crate) fn full_file_source_size(source: &FullFileSource) -> Result<u64, SyntaxSkipReason> {
    match &source.kind {
        FullFileSourceKind::Worktree { path } => worktree_file_size(&source.repo, path),
        FullFileSourceKind::GitRevision { rev, path } => {
            git_blob_size(&source.repo, &format!("{rev}:{path}"))
        }
        FullFileSourceKind::GitMergeBase { base, head, path } => {
            let rev = git_merge_base(&source.repo, base, head)?;
            git_blob_size(&source.repo, &format!("{rev}:{path}"))
        }
    }
}

pub(crate) fn worktree_file_size(repo: &Path, path: &str) -> Result<u64, SyntaxSkipReason> {
    let path = safe_repo_join(repo, path).ok_or(SyntaxSkipReason::NoPath)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| SyntaxSkipReason::NoSource)?;
    if !metadata.file_type().is_file() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(metadata.len())
}

pub(crate) fn read_worktree_file(repo: &Path, path: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let path = safe_repo_join(repo, path).ok_or(SyntaxSkipReason::NoPath)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| SyntaxSkipReason::NoSource)?;
    if !metadata.file_type().is_file() {
        return Err(SyntaxSkipReason::NoSource);
    }
    fs::read(path).map_err(|_| SyntaxSkipReason::NoSource)
}

pub(crate) fn safe_repo_join(repo: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return None;
    }

    let mut joined = repo.to_owned();
    for component in path.components() {
        match component {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(joined)
}

pub(crate) fn git_blob(repo: &Path, object: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "show",
            "--no-ext-diff",
            "--no-color",
            "--end-of-options",
            object,
        ])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(output.stdout)
}

pub(crate) fn git_blob_size(repo: &Path, object: &str) -> Result<u64, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-s", object])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| SyntaxSkipReason::NoSource)
}

pub(crate) fn git_merge_base(
    repo: &Path,
    base: &str,
    head: &str,
) -> Result<String, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--end-of-options", base, head])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }

    let rev = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if rev.is_empty() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(rev)
}

fn validate_highlight_source_with_line_count(
    source: &str,
    limits: SyntaxLimits,
) -> Result<usize, SyntaxSkipReason> {
    if source.len() > limits.max_source_bytes {
        return Err(SyntaxSkipReason::TooLarge);
    }
    let mut lines = 0usize;
    for line in source.lines() {
        if line.len() > limits.max_line_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        lines += 1;
    }
    Ok(lines.max(1))
}

pub(crate) fn split_context_source_lines(source: &str) -> Vec<String> {
    source.lines().map(str::to_owned).collect()
}

pub(crate) fn available_context_lines(
    source_start: usize,
    total: usize,
    source_line_count: usize,
) -> usize {
    let Some(source_index_start) = source_start.checked_sub(1) else {
        return 0;
    };
    source_line_count
        .saturating_sub(source_index_start)
        .min(total)
}

pub(crate) fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn line_belongs_to_side(kind: DiffLineKind, side: DiffSide) -> bool {
    matches!(
        (side, kind),
        (
            DiffSide::Old,
            DiffLineKind::Context | DiffLineKind::Deletion
        ) | (
            DiffSide::New,
            DiffLineKind::Context | DiffLineKind::Addition
        )
    )
}

pub(crate) fn unified_syntax_side(kind: DiffLineKind) -> Option<DiffSide> {
    match kind {
        DiffLineKind::Deletion => Some(DiffSide::Old),
        DiffLineKind::Addition | DiffLineKind::Context => Some(DiffSide::New),
        DiffLineKind::Meta => None,
    }
}
