use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_REQUEST_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const SESSION_COMMAND_CHANNEL_CAPACITY: usize = 64;
pub const DEFAULT_REVIEW_FILES: usize = 200;
pub const MAX_REVIEW_FILES: usize = 200;
pub const DEFAULT_CHANGED_FILES_PER_PAGE: usize = 200;
pub const MAX_CHANGED_FILES_PER_PAGE: usize = 1_000;
pub const MAX_HUNKS_PER_FILE: usize = 32;
pub const DEFAULT_PATCH_BYTES: usize = 64 * 1024;
pub const MIN_PATCH_BYTES: usize = 4;
pub const MAX_PATCH_BYTES: usize = 256 * 1024;
pub const DEFAULT_COMMENTS_PER_PAGE: usize = 50;
pub const MAX_COMMENTS_PER_PAGE: usize = 100;
pub const MAX_COMMENTS_PER_BATCH: usize = 100;
pub const MAX_LIVE_COMMENTS: usize = 5_000;
pub const MAX_SUMMARY_BYTES: usize = 8 * 1024;
pub const MAX_RATIONALE_BYTES: usize = 32 * 1024;
pub const MAX_AUTHOR_BYTES: usize = 256;
pub const MAX_PATH_BYTES: usize = 4 * 1024;
pub const MAX_REQUEST_ID_BYTES: usize = 256;

pub const METHOD_SESSION_GET: &str = "session.get";
pub const METHOD_CONTEXT_GET: &str = "context.get";
pub const METHOD_REVIEW_GET: &str = "review.get";
pub const METHOD_PATCH_GET: &str = "patch.get";
pub const METHOD_NAVIGATE: &str = "navigate";
pub const METHOD_COMMENT_ADD: &str = "comment.add";
pub const METHOD_COMMENT_APPLY: &str = "comment.apply";
pub const METHOD_COMMENT_LIST: &str = "comment.list";
pub const METHOD_COMMENT_REMOVE: &str = "comment.remove";
pub const METHOD_COMMENT_CLEAR: &str = "comment.clear";
pub const METHOD_COMMENT_DISPOSITION: &str = "comment.disposition";
pub const METHOD_PROGRESS_SET: &str = "progress.set";
pub const METHOD_VERDICT_GET: &str = "verdict.get";
pub const METHOD_VERDICT_SET: &str = "verdict.set";
pub const METHOD_VERDICT_CLEAR: &str = "verdict.clear";
pub const METHOD_RELOAD: &str = "reload";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub protocol: u16,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub protocol: u16,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl Response {
    pub fn success(id: impl Into<String>, result: Value) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: impl Into<String>, error: ProtocolError) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl ProtocolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub process_id: u32,
    pub process_identity: String,
    pub protocol: u16,
    pub repository: String,
    pub working_directory: String,
    pub source: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListing {
    #[serde(flatten)]
    pub record: SessionRecord,
    pub responsive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub process_id: u32,
    pub protocol: u16,
    pub repository: String,
    pub working_directory: String,
    pub source: String,
    pub document_generation: u64,
    pub source_changed: bool,
    pub capabilities: Capabilities,
    pub responsive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub methods: Vec<String>,
    pub limits: Limits,
    pub snapshot_default: bool,
    pub automatic_reload: bool,
}

impl Capabilities {
    pub fn v1(automatic_reload: bool) -> Self {
        Self {
            methods: known_methods()
                .iter()
                .map(|method| (*method).to_owned())
                .collect(),
            limits: Limits::default(),
            snapshot_default: true,
            automatic_reload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub request_frame_bytes: usize,
    pub response_frame_bytes: usize,
    pub command_channel_capacity: usize,
    pub review_files_per_page: usize,
    pub changed_files_per_page: usize,
    pub hunks_per_file: usize,
    pub patch_min_bytes: usize,
    pub patch_bytes: usize,
    pub comments_per_page: usize,
    pub comments_per_batch: usize,
    pub live_comments: usize,
    pub summary_bytes: usize,
    pub rationale_bytes: usize,
    pub author_bytes: usize,
    pub path_bytes: usize,
    pub request_id_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            request_frame_bytes: MAX_REQUEST_FRAME_BYTES,
            response_frame_bytes: MAX_RESPONSE_FRAME_BYTES,
            command_channel_capacity: SESSION_COMMAND_CHANNEL_CAPACITY,
            review_files_per_page: MAX_REVIEW_FILES,
            changed_files_per_page: MAX_CHANGED_FILES_PER_PAGE,
            hunks_per_file: MAX_HUNKS_PER_FILE,
            patch_min_bytes: MIN_PATCH_BYTES,
            patch_bytes: MAX_PATCH_BYTES,
            comments_per_page: MAX_COMMENTS_PER_PAGE,
            comments_per_batch: MAX_COMMENTS_PER_BATCH,
            live_comments: MAX_LIVE_COMMENTS,
            summary_bytes: MAX_SUMMARY_BYTES,
            rationale_bytes: MAX_RATIONALE_BYTES,
            author_bytes: MAX_AUTHOR_BYTES,
            path_bytes: MAX_PATH_BYTES,
            request_id_bytes: MAX_REQUEST_ID_BYTES,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyParams {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files_limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextResult {
    pub generation: u64,
    pub source_changed: bool,
    pub focus: Option<Focus>,
    pub comment_count: usize,
    pub pass: u64,
    pub moved_comment_count: usize,
    pub stale_comment_count: usize,
    pub cleared_comment_count: usize,
    pub changed_file_count: usize,
    pub changed_files: Vec<String>,
    pub changed_files_next_cursor: Option<String>,
    pub reviewed_file_count: usize,
    pub verdict: Option<VerdictView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunk: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_comments: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments_limit: Option<usize>,
    #[serde(default)]
    pub changed_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub generation: u64,
    pub stats: ChangeStats,
    pub files: Vec<FileSummary>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentView>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments_next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeStats {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub binary_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub change_kind: String,
    pub reviewed: bool,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub hunks: Vec<HunkSummary>,
    pub hunks_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkSummary {
    pub index: usize,
    pub reviewed: bool,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchParams {
    pub file: String,
    #[serde(default)]
    pub hunk: Option<usize>,
    #[serde(default)]
    pub old_line: Option<usize>,
    #[serde(default)]
    pub new_line: Option<usize>,
    #[serde(default)]
    pub context: Option<usize>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchResult {
    pub generation: u64,
    pub file: String,
    pub patch: String,
    pub truncated: bool,
    pub returned_bytes: usize,
    pub total_bytes: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAnchor {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ReviewAnchorScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunk: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<RangeTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAnchorScope {
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<SourceRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<SourceRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigateParams {
    #[serde(flatten)]
    pub target: NavigateTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NavigateTarget {
    Anchor { anchor: ReviewAnchor },
    NextComment,
    PreviousComment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationResult {
    pub generation: u64,
    pub focus: Option<Focus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentInput {
    #[serde(flatten)]
    pub anchor: ReviewAnchor,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAddParams {
    pub generation: u64,
    #[serde(flatten)]
    pub comment: CommentInput,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentApplyParams {
    pub generation: u64,
    pub comments: Vec<CommentInput>,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentMutationResult {
    pub generation: u64,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentOrigin {
    Agent,
    Human,
    #[default]
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentListParams {
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub origin: CommentOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentListResult {
    pub generation: u64,
    pub comments: Vec<CommentView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentView {
    pub id: String,
    pub anchor: ReviewAnchor,
    pub summary: String,
    pub rationale: Option<String>,
    pub author: Option<String>,
    pub origin: CommentOrigin,
    pub lifecycle: CommentLifecycle,
    pub disposition: FindingDisposition,
    pub document_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentLifecycle {
    Open,
    Moved,
    Stale,
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingDisposition {
    Open,
    Accepted,
    Dismissed,
    Blocking,
    NonBlocking,
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentDispositionParams {
    pub generation: u64,
    pub id: String,
    pub disposition: FindingDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressSetParams {
    pub generation: u64,
    pub file: String,
    #[serde(default)]
    pub hunk: Option<usize>,
    #[serde(default = "default_true")]
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationParams {
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressResult {
    pub generation: u64,
    pub reviewed_files: usize,
    pub reviewed_hunks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictDestination {
    Local,
    Stdout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictView {
    pub kind: VerdictKind,
    pub summary: Option<String>,
    pub destination: VerdictDestination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictSetParams {
    pub generation: u64,
    pub kind: VerdictKind,
    #[serde(default)]
    pub summary: Option<String>,
    pub destination: VerdictDestination,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRemoveParams {
    pub generation: u64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentClearParams {
    pub generation: u64,
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRemovalResult {
    pub generation: u64,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadParams {
    pub generation: u64,
    pub request: ReloadRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReloadRequest {
    Diff {
        rev: Option<String>,
        #[serde(default)]
        pathspecs: Vec<String>,
    },
    Show {
        rev: Option<String>,
        #[serde(default)]
        pathspecs: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResult {
    pub generation: u64,
}

pub fn known_methods() -> &'static [&'static str] {
    &[
        METHOD_SESSION_GET,
        METHOD_CONTEXT_GET,
        METHOD_REVIEW_GET,
        METHOD_PATCH_GET,
        METHOD_NAVIGATE,
        METHOD_COMMENT_ADD,
        METHOD_COMMENT_APPLY,
        METHOD_COMMENT_LIST,
        METHOD_COMMENT_REMOVE,
        METHOD_COMMENT_CLEAR,
        METHOD_COMMENT_DISPOSITION,
        METHOD_PROGRESS_SET,
        METHOD_VERDICT_GET,
        METHOD_VERDICT_SET,
        METHOD_VERDICT_CLEAR,
        METHOD_RELOAD,
    ]
}

pub fn is_known_method(method: &str) -> bool {
    known_methods().contains(&method)
}
