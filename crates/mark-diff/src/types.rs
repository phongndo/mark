use std::{borrow::Cow, num::NonZeroU64, ops::Range, path::PathBuf, sync::Arc};

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

string_newtype!(RevSpec);
string_newtype!(BranchName);
string_newtype!(CommitSha);
string_newtype!(RefName);
string_newtype!(ReviewId);
string_newtype!(RemoteName);
string_newtype!(DiffPath);
string_newtype!(RepoRelativePath);
string_newtype!(PatchLabel);

macro_rules! path_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(PathBuf);

        impl $name {
            pub fn new(path: impl Into<PathBuf>) -> Self {
                Self(path.into())
            }

            pub fn as_path(&self) -> &std::path::Path {
                &self.0
            }

            pub fn into_path_buf(self) -> PathBuf {
                self.0
            }
        }

        impl From<PathBuf> for $name {
            fn from(path: PathBuf) -> Self {
                Self(path)
            }
        }

        impl From<&std::path::Path> for $name {
            fn from(path: &std::path::Path) -> Self {
                Self(path.to_path_buf())
            }
        }

        impl AsRef<std::path::Path> for $name {
            fn as_ref(&self) -> &std::path::Path {
                self.as_path()
            }
        }

        impl std::ops::Deref for $name {
            type Target = std::path::Path;

            fn deref(&self) -> &Self::Target {
                self.as_path()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.as_path().display().fmt(formatter)
            }
        }
    };
}

path_newtype!(RepoArg);
path_newtype!(WorktreePath);
path_newtype!(DisplayPath);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PullRequestId(NonZeroU64);

impl PullRequestId {
    pub fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(Self)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl From<NonZeroU64> for PullRequestId {
    fn from(value: NonZeroU64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for PullRequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoRoot(PathBuf);

impl RepoRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &std::path::Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl From<PathBuf> for RepoRoot {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl AsRef<std::path::Path> for RepoRoot {
    fn as_ref(&self) -> &std::path::Path {
        self.as_path()
    }
}

impl std::ops::Deref for RepoRoot {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OldLineNumber(u32);

impl OldLineNumber {
    pub fn new(line: usize) -> Self {
        Self(u32::try_from(line).unwrap_or(u32::MAX))
    }

    pub fn get(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for OldLineNumber {
    fn from(line: usize) -> Self {
        Self::new(line)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NewLineNumber(u32);

impl NewLineNumber {
    pub fn new(line: usize) -> Self {
        Self(u32::try_from(line).unwrap_or(u32::MAX))
    }

    pub fn get(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for NewLineNumber {
    fn from(line: usize) -> Self {
        Self::new(line)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DiffSource {
    #[default]
    Worktree,
    Show(RevSpec),
    Base(RevSpec),
    Branch {
        base: RevSpec,
        head: RevSpec,
    },
    Range {
        left: RevSpec,
        right: RevSpec,
    },
    Difftool {
        left: WorktreePath,
        right: WorktreePath,
        path: Option<DisplayPath>,
    },
    Patch(PatchSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    File(PathBuf),
    Stdin(Arc<[u8]>),
    Text { label: PatchLabel, patch: Arc<[u8]> },
    Review { label: PatchLabel, patch: Arc<[u8]> },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UntrackedMode {
    #[default]
    Include,
    Exclude,
}

impl UntrackedMode {
    pub fn from_include(include: bool) -> Self {
        if include {
            Self::Include
        } else {
            Self::Exclude
        }
    }

    pub fn includes(self) -> bool {
        matches!(self, Self::Include)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffOutput {
    #[default]
    Patch,
    Stat,
}

impl DiffOutput {
    pub fn is_stat(self) -> bool {
        matches!(self, Self::Stat)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOptions {
    pub repo: Option<RepoArg>,
    pub source: DiffSource,
    pub local_untracked: UntrackedMode,
    pub output: DiffOutput,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffLimits {
    pub max_patch_bytes: Option<usize>,
    pub max_diff_rows: Option<usize>,
    pub max_files: Option<usize>,
    pub max_hunks: Option<usize>,
    pub max_line_bytes: Option<usize>,
}

impl DiffLimits {
    pub fn from_env() -> Self {
        Self {
            max_patch_bytes: env_usize("MARK_MAX_PATCH_BYTES"),
            max_diff_rows: env_usize("MARK_MAX_DIFF_ROWS"),
            max_files: env_usize("MARK_MAX_DIFF_FILES"),
            max_hunks: env_usize("MARK_MAX_DIFF_HUNKS"),
            max_line_bytes: env_usize("MARK_MAX_DIFF_LINE_BYTES"),
        }
    }

    pub fn is_unlimited(self) -> bool {
        self.max_patch_bytes.is_none()
            && self.max_diff_rows.is_none()
            && self.max_files.is_none()
            && self.max_hunks.is_none()
            && self.max_line_bytes.is_none()
    }
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLimitExceeded {
    pub limit: &'static str,
    pub max: usize,
    pub actual: usize,
}

impl DiffLimitExceeded {
    pub(crate) fn new(limit: &'static str, max: usize, actual: usize) -> Self {
        Self { limit, max, actual }
    }
}

impl std::fmt::Display for DiffLimitExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "diff exceeds {} limit: {} > {}",
            self.limit, self.actual, self.max
        )
    }
}

impl std::error::Error for DiffLimitExceeded {}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            repo: None,
            source: DiffSource::default(),
            local_untracked: UntrackedMode::Include,
            output: DiffOutput::Patch,
        }
    }
}

impl DiffOptions {
    pub fn include_untracked(&self) -> bool {
        self.local_untracked.includes()
    }

    pub fn is_stat(&self) -> bool {
        self.output.is_stat()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    pub repo: RepoRoot,
    pub title: String,
    pub files: Vec<DiffFile>,
    pub raw_patch: Arc<[u8]>,
}

impl Changeset {
    pub fn empty_raw_patch() -> Arc<[u8]> {
        Arc::<[u8]>::from(Vec::<u8>::new())
    }

    pub fn stats(&self) -> DiffStats {
        let mut stats = DiffStats {
            files: self.files.len(),
            ..DiffStats::default()
        };
        for file in &self.files {
            stats.additions += file.additions;
            stats.deletions += file.deletions;
            if file.is_binary() {
                stats.binary_files += 1;
            }
        }
        stats
    }

    pub fn estimated_model_bytes(&self) -> usize {
        let retained_patch_bytes = self
            .files
            .iter()
            .flat_map(|file| file.hunks())
            .flat_map(|hunk| &hunk.lines)
            .find_map(DiffLine::backing_patch_bytes)
            .unwrap_or(self.raw_patch.len());
        std::mem::size_of::<Self>()
            .saturating_add(retained_patch_bytes)
            .saturating_add(self.files.len() * std::mem::size_of::<DiffFile>())
            .saturating_add(
                self.files
                    .iter()
                    .map(DiffFile::estimated_model_bytes)
                    .sum::<usize>(),
            )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub binary_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub change: FileChange,
    pub additions: usize,
    pub deletions: usize,
    pub body: DiffFileBody,
}

impl DiffFile {
    pub fn display_path(&self) -> &str {
        self.new_path().or(self.old_path()).unwrap_or("/dev/null")
    }

    pub fn old_path(&self) -> Option<&str> {
        self.change.old_path()
    }

    pub fn new_path(&self) -> Option<&str> {
        self.change.new_path()
    }

    pub fn status(&self) -> FileStatus {
        self.change.status()
    }

    pub fn is_binary(&self) -> bool {
        matches!(self.body, DiffFileBody::Binary)
    }

    pub fn has_textual_changes(&self) -> bool {
        matches!(&self.body, DiffFileBody::Text { hunks } if !hunks.is_empty())
    }

    pub fn has_no_textual_changes(&self) -> bool {
        matches!(&self.body, DiffFileBody::NoTextualChanges)
            || matches!(&self.body, DiffFileBody::Text { hunks } if hunks.is_empty())
    }

    pub fn hunks(&self) -> &[DiffHunk] {
        match &self.body {
            DiffFileBody::Text { hunks } => hunks,
            DiffFileBody::Binary | DiffFileBody::NoTextualChanges => &[],
        }
    }

    pub fn hunks_mut(&mut self) -> &mut Vec<DiffHunk> {
        if matches!(self.body, DiffFileBody::NoTextualChanges) {
            self.body = DiffFileBody::Text { hunks: Vec::new() };
        }
        match &mut self.body {
            DiffFileBody::Text { hunks } => hunks,
            DiffFileBody::Binary => panic!("binary diff files do not have text hunks"),
            DiffFileBody::NoTextualChanges => unreachable!(),
        }
    }

    pub fn estimated_model_bytes(&self) -> usize {
        self.change
            .estimated_model_bytes()
            .saturating_add(match &self.body {
                DiffFileBody::Text { hunks } => {
                    hunks.len() * std::mem::size_of::<DiffHunk>()
                        + hunks
                            .iter()
                            .map(DiffHunk::estimated_model_bytes)
                            .sum::<usize>()
                }
                DiffFileBody::Binary | DiffFileBody::NoTextualChanges => 0,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Modified {
        old_path: DiffPath,
        new_path: DiffPath,
    },
    Added {
        path: DiffPath,
    },
    Deleted {
        path: DiffPath,
    },
    Renamed {
        old_path: DiffPath,
        new_path: DiffPath,
    },
    Copied {
        old_path: DiffPath,
        new_path: DiffPath,
    },
    TypeChanged {
        old_path: DiffPath,
        new_path: DiffPath,
    },
    Unknown {
        old_path: Option<DiffPath>,
        new_path: Option<DiffPath>,
    },
}

impl FileChange {
    pub fn modified(path: impl Into<DiffPath>) -> Self {
        let path = path.into();
        Self::Modified {
            old_path: path.clone(),
            new_path: path,
        }
    }

    pub fn from_status(
        status: FileStatus,
        old_path: Option<String>,
        new_path: Option<String>,
    ) -> Self {
        let old_path = old_path.map(DiffPath::from);
        let new_path = new_path.map(DiffPath::from);
        match status {
            FileStatus::Modified => match (old_path, new_path) {
                (Some(old_path), Some(new_path)) => Self::Modified { old_path, new_path },
                (Some(path), None) | (None, Some(path)) => Self::Modified {
                    old_path: path.clone(),
                    new_path: path,
                },
                (None, None) => Self::Unknown {
                    old_path: None,
                    new_path: None,
                },
            },
            FileStatus::Added => match new_path {
                Some(path) => Self::Added { path },
                None => Self::Unknown {
                    old_path,
                    new_path: None,
                },
            },
            FileStatus::Deleted => match old_path {
                Some(path) => Self::Deleted { path },
                None => Self::Unknown {
                    old_path: None,
                    new_path,
                },
            },
            FileStatus::Renamed => match (old_path, new_path) {
                (Some(old_path), Some(new_path)) => Self::Renamed { old_path, new_path },
                (old_path, new_path) => Self::Unknown { old_path, new_path },
            },
            FileStatus::Copied => match (old_path, new_path) {
                (Some(old_path), Some(new_path)) => Self::Copied { old_path, new_path },
                (old_path, new_path) => Self::Unknown { old_path, new_path },
            },
            FileStatus::TypeChanged => match (old_path, new_path) {
                (Some(old_path), Some(new_path)) => Self::TypeChanged { old_path, new_path },
                (Some(path), None) | (None, Some(path)) => Self::TypeChanged {
                    old_path: path.clone(),
                    new_path: path,
                },
                (None, None) => Self::Unknown {
                    old_path: None,
                    new_path: None,
                },
            },
            FileStatus::Unknown => Self::Unknown { old_path, new_path },
        }
    }

    pub fn old_path(&self) -> Option<&str> {
        match self {
            Self::Modified { old_path, .. } => Some(old_path.as_str()),
            Self::Deleted { path } => Some(path.as_str()),
            Self::Added { .. } => None,
            Self::Renamed { old_path, .. }
            | Self::Copied { old_path, .. }
            | Self::TypeChanged { old_path, .. } => Some(old_path.as_str()),
            Self::Unknown { old_path, .. } => old_path.as_ref().map(DiffPath::as_str),
        }
    }

    pub fn new_path(&self) -> Option<&str> {
        match self {
            Self::Modified { new_path, .. } => Some(new_path.as_str()),
            Self::Added { path } => Some(path.as_str()),
            Self::Deleted { .. } => None,
            Self::Renamed { new_path, .. }
            | Self::Copied { new_path, .. }
            | Self::TypeChanged { new_path, .. } => Some(new_path.as_str()),
            Self::Unknown { new_path, .. } => new_path.as_ref().map(DiffPath::as_str),
        }
    }

    pub fn status(&self) -> FileStatus {
        match self {
            Self::Modified { .. } => FileStatus::Modified,
            Self::Added { .. } => FileStatus::Added,
            Self::Deleted { .. } => FileStatus::Deleted,
            Self::Renamed { .. } => FileStatus::Renamed,
            Self::Copied { .. } => FileStatus::Copied,
            Self::TypeChanged { .. } => FileStatus::TypeChanged,
            Self::Unknown { .. } => FileStatus::Unknown,
        }
    }

    pub fn estimated_model_bytes(&self) -> usize {
        fn path_bytes(path: &DiffPath) -> usize {
            path.as_str().len()
        }
        match self {
            Self::Modified { old_path, new_path }
            | Self::Renamed { old_path, new_path }
            | Self::Copied { old_path, new_path }
            | Self::TypeChanged { old_path, new_path } => {
                path_bytes(old_path) + path_bytes(new_path)
            }
            Self::Added { path } | Self::Deleted { path } => path_bytes(path),
            Self::Unknown { old_path, new_path } => old_path
                .as_ref()
                .map(path_bytes)
                .unwrap_or_default()
                .saturating_add(new_path.as_ref().map(path_bytes).unwrap_or_default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffFileBody {
    Text { hunks: Vec<DiffHunk> },
    Binary,
    NoTextualChanges,
}

impl Default for DiffFileBody {
    fn default() -> Self {
        Self::Text { hunks: Vec::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unknown,
}

impl FileStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type-changed",
            Self::Unknown => "changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub ranges: HunkLineRanges,
    pub lines: Vec<DiffLine>,
}

impl DiffHunk {
    pub fn old_start(&self) -> usize {
        self.ranges.old_start()
    }

    pub fn old_count(&self) -> usize {
        self.ranges.old_count()
    }

    pub fn new_start(&self) -> usize {
        self.ranges.new_start()
    }

    pub fn new_count(&self) -> usize {
        self.ranges.new_count()
    }

    pub fn estimated_model_bytes(&self) -> usize {
        self.header
            .len()
            .saturating_add(self.lines.len() * std::mem::size_of::<DiffLine>())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkLineRanges {
    old: LineSpan<OldLineNumber>,
    new: LineSpan<NewLineNumber>,
}

impl HunkLineRanges {
    pub fn new(old_start: usize, old_count: usize, new_start: usize, new_count: usize) -> Self {
        Self {
            old: LineSpan::new(OldLineNumber::new(old_start), old_count),
            new: LineSpan::new(NewLineNumber::new(new_start), new_count),
        }
    }

    pub fn old(&self) -> LineSpan<OldLineNumber> {
        self.old
    }

    pub fn new_lines(&self) -> LineSpan<NewLineNumber> {
        self.new
    }

    pub fn old_start(&self) -> usize {
        self.old.start().get()
    }

    pub fn old_count(&self) -> usize {
        self.old.count()
    }

    pub fn new_start(&self) -> usize {
        self.new.start().get()
    }

    pub fn new_count(&self) -> usize {
        self.new.count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan<N> {
    start: N,
    count: usize,
}

impl<N: Copy> LineSpan<N> {
    pub fn new(start: N, count: usize) -> Self {
        Self { start, count }
    }

    pub fn start(self) -> N {
        self.start
    }

    pub fn count(self) -> usize {
        self.count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Context {
        old_line: OldLineNumber,
        new_line: NewLineNumber,
        text: DiffLineText,
    },
    Addition {
        new_line: NewLineNumber,
        text: DiffLineText,
    },
    Deletion {
        old_line: OldLineNumber,
        text: DiffLineText,
    },
    Meta {
        text: DiffLineText,
    },
}

#[derive(Debug, Clone)]
pub struct DiffLineText {
    storage: DiffLineTextStorage,
}

/// A task-local reference-count root for line spans into one raw patch.
///
/// The extra Arc layer keeps the hot per-line clone counter local to one parser
/// task. Parallel parsers therefore do not contend on the raw patch's single
/// atomic reference count, and the sized inner Arc also makes each stored
/// backing pointer thin.
#[derive(Debug, Clone)]
pub(crate) struct DiffLineTextBacking(Arc<Arc<[u8]>>);

impl DiffLineTextBacking {
    pub(crate) fn new(raw: Arc<[u8]>) -> Self {
        Self(Arc::new(raw))
    }

    fn raw(&self) -> &Arc<[u8]> {
        self.0.as_ref()
    }
}

#[derive(Debug, Clone)]
enum DiffLineTextStorage {
    Owned(String),
    Span {
        backing: DiffLineTextBacking,
        offset: u32,
        len: u32,
    },
}

impl PartialEq for DiffLineText {
    fn eq(&self, other: &Self) -> bool {
        match (&self.storage, &other.storage) {
            (DiffLineTextStorage::Owned(left), DiffLineTextStorage::Owned(right)) => left == right,
            (
                DiffLineTextStorage::Span {
                    backing: left_backing,
                    offset: left_offset,
                    len: left_len,
                },
                DiffLineTextStorage::Span {
                    backing: right_backing,
                    offset: right_offset,
                    len: right_len,
                },
            ) if left_offset == right_offset
                && left_len == right_len
                && Arc::ptr_eq(left_backing.raw(), right_backing.raw()) =>
            {
                true
            }
            _ => self.as_bytes() == other.as_bytes(),
        }
    }
}

impl Eq for DiffLineText {}

impl DiffLineText {
    pub fn owned(text: impl Into<String>) -> Self {
        Self {
            storage: DiffLineTextStorage::Owned(text.into()),
        }
    }

    pub(crate) fn span(backing: DiffLineTextBacking, offset: usize, len: usize) -> Self {
        Self {
            storage: DiffLineTextStorage::Span {
                backing,
                offset: u32::try_from(offset).unwrap_or(u32::MAX),
                len: u32::try_from(len).unwrap_or(u32::MAX),
            },
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match &self.storage {
            DiffLineTextStorage::Owned(text) => text.as_bytes(),
            DiffLineTextStorage::Span {
                backing,
                offset,
                len,
            } => {
                let start = *offset as usize;
                let raw = backing.raw();
                let end = start.saturating_add(*len as usize).min(raw.len());
                raw.get(start..end).unwrap_or_default()
            }
        }
    }

    pub fn as_str(&self) -> &str {
        match &self.storage {
            DiffLineTextStorage::Owned(text) => text.as_str(),
            DiffLineTextStorage::Span { .. } => {
                std::str::from_utf8(self.as_bytes()).unwrap_or("\u{FFFD}")
            }
        }
    }

    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match &self.storage {
            DiffLineTextStorage::Owned(text) => Cow::Borrowed(text.as_str()),
            DiffLineTextStorage::Span { .. } => String::from_utf8_lossy(self.as_bytes()),
        }
    }

    pub fn as_mut_string(&mut self) -> &mut String {
        if !matches!(self.storage, DiffLineTextStorage::Owned(_)) {
            let text = String::from_utf8_lossy(self.as_bytes()).into_owned();
            self.storage = DiffLineTextStorage::Owned(text);
        }
        match &mut self.storage {
            DiffLineTextStorage::Owned(text) => text,
            DiffLineTextStorage::Span { .. } => unreachable!(),
        }
    }

    fn backing_patch_bytes(&self) -> Option<usize> {
        match &self.storage {
            DiffLineTextStorage::Owned(_) => None,
            DiffLineTextStorage::Span { backing, .. } => Some(backing.raw().len()),
        }
    }

    fn span_range(&self) -> Option<(Arc<[u8]>, Range<usize>)> {
        match &self.storage {
            DiffLineTextStorage::Owned(_) => None,
            DiffLineTextStorage::Span {
                backing,
                offset,
                len,
            } => {
                let start = *offset as usize;
                let raw = backing.raw();
                let end = start.saturating_add(*len as usize).min(raw.len());
                Some((Arc::clone(raw), start..end))
            }
        }
    }
}

impl DiffLine {
    pub fn context(old_line: usize, new_line: usize, text: impl Into<String>) -> Self {
        Self::Context {
            old_line: OldLineNumber::new(old_line),
            new_line: NewLineNumber::new(new_line),
            text: DiffLineText::owned(text),
        }
    }

    pub fn addition(new_line: usize, text: impl Into<String>) -> Self {
        Self::Addition {
            new_line: NewLineNumber::new(new_line),
            text: DiffLineText::owned(text),
        }
    }

    pub fn deletion(old_line: usize, text: impl Into<String>) -> Self {
        Self::Deletion {
            old_line: OldLineNumber::new(old_line),
            text: DiffLineText::owned(text),
        }
    }

    pub fn meta(text: impl Into<String>) -> Self {
        Self::Meta {
            text: DiffLineText::owned(text),
        }
    }

    pub(crate) fn context_span(
        old_line: usize,
        new_line: usize,
        backing: DiffLineTextBacking,
        offset: usize,
        len: usize,
    ) -> Self {
        Self::Context {
            old_line: OldLineNumber::new(old_line),
            new_line: NewLineNumber::new(new_line),
            text: DiffLineText::span(backing, offset, len),
        }
    }

    pub(crate) fn addition_span(
        new_line: usize,
        backing: DiffLineTextBacking,
        offset: usize,
        len: usize,
    ) -> Self {
        Self::Addition {
            new_line: NewLineNumber::new(new_line),
            text: DiffLineText::span(backing, offset, len),
        }
    }

    pub(crate) fn deletion_span(
        old_line: usize,
        backing: DiffLineTextBacking,
        offset: usize,
        len: usize,
    ) -> Self {
        Self::Deletion {
            old_line: OldLineNumber::new(old_line),
            text: DiffLineText::span(backing, offset, len),
        }
    }

    pub(crate) fn meta_span(backing: DiffLineTextBacking, offset: usize, len: usize) -> Self {
        Self::Meta {
            text: DiffLineText::span(backing, offset, len),
        }
    }

    pub fn kind(&self) -> DiffLineKind {
        match self {
            Self::Context { .. } => DiffLineKind::Context,
            Self::Addition { .. } => DiffLineKind::Addition,
            Self::Deletion { .. } => DiffLineKind::Deletion,
            Self::Meta { .. } => DiffLineKind::Meta,
        }
    }

    pub fn old_line(&self) -> Option<usize> {
        match self {
            Self::Context { old_line, .. } | Self::Deletion { old_line, .. } => {
                Some(old_line.get())
            }
            Self::Addition { .. } | Self::Meta { .. } => None,
        }
    }

    pub fn new_line(&self) -> Option<usize> {
        match self {
            Self::Context { new_line, .. } | Self::Addition { new_line, .. } => {
                Some(new_line.get())
            }
            Self::Deletion { .. } | Self::Meta { .. } => None,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.as_str(),
        }
    }

    pub fn text_lossy(&self) -> Cow<'_, str> {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.to_string_lossy(),
        }
    }

    pub fn text_bytes(&self) -> &[u8] {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.as_bytes(),
        }
    }

    pub fn text_span_range(&self) -> Option<(Arc<[u8]>, Range<usize>)> {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.span_range(),
        }
    }

    pub fn backing_patch_bytes(&self) -> Option<usize> {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.backing_patch_bytes(),
        }
    }

    pub fn text_mut(&mut self) -> &mut String {
        match self {
            Self::Context { text, .. }
            | Self::Addition { text, .. }
            | Self::Deletion { text, .. }
            | Self::Meta { text } => text.as_mut_string(),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Meta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowRef {
    FileHeader(usize),
    FileBodyNotice(usize),
    HunkHeader {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffViewModel {
    len: usize,
    file_start_rows: Vec<usize>,
    hunk_start_rows: Vec<usize>,
    hunk_rows: Vec<DiffViewHunkRow>,
    file_body_notice_rows: Vec<Option<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffViewHunkRow {
    file: usize,
    hunk: usize,
    line_count: usize,
}

impl DiffViewModel {
    pub fn new(changeset: &Changeset) -> Self {
        let mut len = 0usize;
        let mut file_start_rows = Vec::with_capacity(changeset.files.len());
        let mut hunk_start_rows = Vec::new();
        let mut hunk_rows = Vec::new();
        let mut file_body_notice_rows = Vec::with_capacity(changeset.files.len());

        for (file_index, file) in changeset.files.iter().enumerate() {
            file_start_rows.push(len);
            file_body_notice_rows.push(None);
            len = len.saturating_add(1);

            if file.is_binary() || file.has_no_textual_changes() {
                file_body_notice_rows[file_index] = Some(len);
                len = len.saturating_add(1);
                continue;
            }

            for (hunk_index, hunk) in file.hunks().iter().enumerate() {
                hunk_start_rows.push(len);
                hunk_rows.push(DiffViewHunkRow {
                    file: file_index,
                    hunk: hunk_index,
                    line_count: hunk.lines.len(),
                });
                len = len.saturating_add(1).saturating_add(hunk.lines.len());
            }
        }

        Self {
            len,
            file_start_rows,
            hunk_start_rows,
            hunk_rows,
            file_body_notice_rows,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn row(&self, index: usize) -> Option<DiffRowRef> {
        if index >= self.len {
            return None;
        }
        if let Ok(file) = self.file_start_rows.binary_search(&index) {
            return Some(DiffRowRef::FileHeader(file));
        }
        if let Some(file) = self.file_at_row(index)
            && self.file_body_notice_rows.get(file).copied().flatten() == Some(index)
        {
            return Some(DiffRowRef::FileBodyNotice(file));
        }

        let hunk_index = self
            .hunk_start_rows
            .partition_point(|start| *start <= index);
        let hunk_index = hunk_index.checked_sub(1)?;
        let start = *self.hunk_start_rows.get(hunk_index)?;
        let hunk = *self.hunk_rows.get(hunk_index)?;
        if index == start {
            return Some(DiffRowRef::HunkHeader {
                file: hunk.file,
                hunk: hunk.hunk,
            });
        }
        let line = index.saturating_sub(start).saturating_sub(1);
        (line < hunk.line_count).then_some(DiffRowRef::Line {
            file: hunk.file,
            hunk: hunk.hunk,
            line,
        })
    }

    pub fn file_start_row(&self, file: usize) -> Option<usize> {
        self.file_start_rows.get(file).copied()
    }

    pub fn file_at_row(&self, row: usize) -> Option<usize> {
        if self.file_start_rows.is_empty() {
            return None;
        }
        match self.file_start_rows.binary_search(&row) {
            Ok(index) => Some(index),
            Err(0) => Some(0),
            Err(index) => Some(index - 1),
        }
    }

    pub fn next_hunk_row(&self, row: usize) -> Option<usize> {
        let index = self.hunk_start_rows.partition_point(|start| *start <= row);
        self.hunk_start_rows.get(index).copied()
    }

    pub fn previous_hunk_row(&self, row: usize) -> Option<usize> {
        let index = self.hunk_start_rows.partition_point(|start| *start < row);
        index
            .checked_sub(1)
            .and_then(|index| self.hunk_start_rows.get(index))
            .copied()
    }
}
