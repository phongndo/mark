//! Durable reviews shared by native harness adapters, the JSON CLI, and MCP.
//!
//! A generation identifies a patch snapshot. A revision orders all review
//! changes. Binding a harness conversation never implicitly replaces a snapshot.

use std::{
    env, fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ReviewAnchor,
    checkpoint::{LockedDirectory, invalid, validate_id},
};

mod fingerprint;
mod snapshot;

pub const REVIEW_SCHEMA_VERSION: u16 = 1;
pub const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REVIEW_THREADS: usize = 256;
pub const MAX_REVIEW_MESSAGES: usize = 1024;
pub const MAX_REVIEW_CHANGES: usize = 256;
pub const MAX_MUTATION_RECEIPTS: usize = 512;
pub const MAX_FEEDBACK_SUBMISSIONS: usize = 128;
const MAX_BODY_BYTES: usize = 32 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBinding {
    pub harness: String,
    pub conversation: String,
    pub repository: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenReview {
    pub harness: String,
    pub conversation: String,
    pub repository: String,
    pub source: Value,
    pub patch: String,
    #[serde(default)]
    pub review_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewMessage {
    pub message_id: String,
    pub reply_to: Option<String>,
    pub body: String,
    pub origin: String,
    pub author: Option<String>,
    pub revision: u64,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewThread {
    pub thread_id: String,
    pub anchor: ReviewAnchor,
    pub messages: Vec<ReviewMessage>,
    pub generation: u64,
    pub stale: bool,
    pub disposition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmission {
    pub submission_id: String,
    pub thread_ids: Vec<String>,
    pub message_ids: Vec<String>,
    pub generation: u64,
    pub revision: u64,
    pub acknowledged_by: Vec<String>,
    /// Immutable submitted content: later drafts cannot alter a delivered batch.
    pub threads: Vec<ReviewThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReturn {
    pub outcome: String,
    pub submission_ids: Vec<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewChange {
    pub revision: u64,
    pub kind: String,
    pub entity_id: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationReceipt {
    pub client_mutation_id: String,
    pub request: String,
    /// Refresh adapters generate patch bytes after receiving the client request.
    /// Keep that request identity separate from the complete store mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request: Option<String>,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub schema_version: u16,
    pub review_id: String,
    pub binding: ReviewBinding,
    pub source: Value,
    pub generation: u64,
    pub revision: u64,
    pub snapshot: ReviewSnapshot,
    pub threads: Vec<ReviewThread>,
    pub submissions: Vec<FeedbackSubmission>,
    pub last_return: Option<ReviewReturn>,
    pub changes: Vec<ReviewChange>,
    pub progress: ReviewProgress,
    pub created_at: u64,
    pub updated_at: u64,
    /// Receipts are never evicted: reaching the explicit cap rejects new writes.
    pub mutations: Vec<MutationReceipt>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewProgress {
    pub reviewed_files: Vec<String>,
    /// Repository-relative path and one-based hunk number.
    pub reviewed_hunks: Vec<(String, usize)>,
    pub verdict: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub review_id: String,
    pub binding: ReviewBinding,
    pub source: Value,
    pub generation: u64,
    pub revision: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone)]
pub struct ReviewStore {
    directory: PathBuf,
}

impl ReviewStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn from_env() -> io::Result<Self> {
        let state = env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".local/state"))
            })
            .ok_or_else(|| invalid("state_directory_missing", "set XDG_STATE_HOME or HOME"))?;
        Ok(Self::new(state.join("mark/reviews")))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn open(&self, args: OpenReview) -> io::Result<ReviewRecord> {
        bounded(&args.harness, "harness", 128)?;
        bounded(&args.conversation, "conversation", 1024)?;
        validate_patch(&args.patch)?;
        if serde_json::to_vec(&args.source)?.len() > 16 * 1024 || args.source.is_null() {
            return Err(invalid(
                "invalid_source",
                "source must be non-null and at most 16 KiB",
            ));
        }
        let repository = fs::canonicalize(&args.repository)?;
        if !repository.is_dir() {
            return Err(invalid(
                "invalid_repository",
                "repository must be a directory",
            ));
        }
        let binding = ReviewBinding {
            harness: args.harness,
            conversation: args.conversation,
            repository: repository.to_string_lossy().into_owned(),
        };
        let lock = LockedDirectory::acquire(&self.directory)?;
        if let Some(id) = args.review_id {
            let review = load(&lock, &id)?;
            if review.binding.repository != binding.repository || review.source != args.source {
                return Err(invalid(
                    "review_binding_mismatch",
                    "explicit review belongs to another worktree or source",
                ));
            }
            return Ok(review);
        }
        for id in lock.ids()? {
            let review = load(&lock, &id)?;
            if review.binding == binding && review.source == args.source {
                return Ok(review);
            }
        }
        let now = timestamp();
        let mut review = ReviewRecord {
            schema_version: REVIEW_SCHEMA_VERSION,
            review_id: unique_id(),
            binding,
            source: args.source,
            generation: 1,
            revision: 1,
            snapshot: ReviewSnapshot { patch: args.patch },
            threads: Vec::new(),
            submissions: Vec::new(),
            last_return: None,
            changes: Vec::new(),
            progress: ReviewProgress::default(),
            created_at: now,
            updated_at: now,
            mutations: Vec::new(),
        };
        review.changes.push(ReviewChange {
            revision: 1,
            kind: "review.opened".to_owned(),
            entity_id: None,
            generation: 1,
        });
        persist(&lock, &review)?;
        Ok(review)
    }

    pub fn get(&self, review_id: &str) -> io::Result<ReviewRecord> {
        let lock = LockedDirectory::acquire(&self.directory)?;
        load(&lock, review_id)
    }

    /// Reconcile an uncertain refresh before an adapter reads a newer source.
    /// Only adapter-generated `patch` is excluded from the client identity;
    /// generation, source, and revision conditions still have to match exactly.
    pub fn reconcile_refresh(&self, params: &Value) -> io::Result<Option<Value>> {
        let id = string(params, "review_id")?;
        let mutation_id = string(params, "client_mutation_id")?;
        bounded(mutation_id, "client_mutation_id", 256)?;
        number(params, "generation")?;
        validate_expected_revision(params)?;
        let client_request = refresh_client_fingerprint(params)?;
        let lock = LockedDirectory::acquire(&self.directory)?;
        let review = load(&lock, id)?;
        let Some(receipt) = review
            .mutations
            .iter()
            .find(|receipt| receipt.client_mutation_id == mutation_id)
        else {
            return Ok(None);
        };
        match &receipt.client_request {
            Some(original) if original == &client_request => {
                receipt_result(&review, receipt).map(Some)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "mutation_conflict: refresh client request differs or its legacy receipt cannot prove the original request",
            )),
        }
    }

    pub fn list(&self) -> io::Result<Vec<ReviewSummary>> {
        let lock = LockedDirectory::acquire(&self.directory)?;
        lock.ids()?
            .into_iter()
            .map(|id| {
                let record = load(&lock, &id)?;
                Ok(ReviewSummary {
                    review_id: record.review_id,
                    binding: record.binding,
                    source: record.source,
                    generation: record.generation,
                    revision: record.revision,
                    updated_at: record.updated_at,
                })
            })
            .collect()
    }

    /// The shared command interface. Callers supply patch data; this module never
    /// invokes Git, starts a terminal, or guesses a harness conversation.
    pub fn call(&self, method: &str, params: Value) -> io::Result<Value> {
        match method {
            "review.open" => return value(self.open(parse(params)?)?),
            "review.list" => return Ok(json!({ "reviews": self.list()? })),
            _ => {}
        }
        let id = string(&params, "review_id")?;
        let lock = LockedDirectory::acquire(&self.directory)?;
        let mut review = load(&lock, id)?;
        match method {
            "review.get" => return value(&review),
            "review.patch" => return patch_page(&review, &params),
            "review.changes" => return changes_page(&review, &params),
            "feedback.get" => return feedback_page(&review, &params),
            _ => {}
        }
        if !matches!(
            method,
            "review.refresh"
                | "comment.add"
                | "comment.reply"
                | "comment.edit"
                | "comment.remove"
                | "feedback.submit"
                | "feedback.ack"
                | "review.finish"
                | "review.disposition"
                | "review.progress"
                | "review.reanchor"
                | "review.begin"
        ) {
            return Err(invalid("unknown_method", method));
        }
        let mutation_id = string(&params, "client_mutation_id")?;
        bounded(mutation_id, "client_mutation_id", 256)?;
        let request = request_fingerprint(method, &params)?;
        if let Some(receipt) = review
            .mutations
            .iter()
            .find(|receipt| receipt.client_mutation_id == mutation_id)
        {
            if receipt.request == request {
                return receipt_result(&review, receipt);
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "mutation_conflict: client_mutation_id was used with different input",
            ));
        }
        if review.mutations.len() >= MAX_MUTATION_RECEIPTS {
            return Err(invalid(
                "mutation_limit",
                "review mutation receipt limit reached; open a new review",
            ));
        }
        let generation = number(&params, "generation")?;
        if generation != review.generation {
            return Err(invalid(
                "stale_generation",
                "snapshot generation does not match the review",
            ));
        }
        if validate_expected_revision(&params)?.is_some_and(|expected| expected != review.revision)
        {
            return Err(invalid(
                "stale_revision",
                "review changed after the inspected revision",
            ));
        }
        let next_revision = review
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("revision_limit", "revision counter exhausted"))?;
        let result = match method {
            "review.refresh" => refresh(&mut review, &params, next_revision)?,
            "comment.add" => add_comment(&mut review, &params, next_revision)?,
            "comment.reply" => reply(&mut review, &params, next_revision)?,
            "comment.edit" => edit_comment(&mut review, &params, next_revision, false)?,
            "comment.remove" => edit_comment(&mut review, &params, next_revision, true)?,
            "feedback.submit" => submit(&mut review, &params, next_revision)?,
            "feedback.ack" => acknowledge(&mut review, &params, next_revision)?,
            "review.finish" => finish(&mut review, &params, next_revision)?,
            "review.disposition" => disposition(&mut review, &params, next_revision)?,
            "review.progress" => progress(&mut review, &params, next_revision)?,
            "review.reanchor" => reanchor(&mut review, &params, next_revision)?,
            "review.begin" => {
                review.last_return = None;
                json!({ "review_id": review.review_id, "generation": review.generation, "revision": next_revision })
            }
            _ => unreachable!(),
        };
        review.revision = next_revision;
        review.updated_at = timestamp();
        review.changes.push(ReviewChange {
            revision: next_revision,
            kind: method.to_owned(),
            entity_id: result
                .get("thread")
                .and_then(|thread| thread.get("thread_id"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            generation: review.generation,
        });
        if review.changes.len() > MAX_REVIEW_CHANGES {
            review.changes.remove(0);
        }
        let mut receipt = result.clone();
        if method == "feedback.submit" {
            receipt
                .as_object_mut()
                .expect("mutation result object")
                .remove("submission");
            receipt["_submission_ref"] = result["submission"]["submission_id"].clone();
        }
        review.mutations.push(MutationReceipt {
            client_mutation_id: mutation_id.to_owned(),
            request,
            client_request: if method == "review.refresh" {
                Some(refresh_client_fingerprint(&params)?)
            } else {
                None
            },
            result: receipt,
        });
        persist(&lock, &review)?;
        Ok(result)
    }
}

fn refresh(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    let patch = string(params, "patch")?;
    validate_patch(patch)?;
    if params
        .get("source")
        .is_some_and(|source| source != &review.source)
    {
        return Err(invalid(
            "source_changed",
            "refresh cannot replace a review source",
        ));
    }
    let changed = patch != review.snapshot.patch;
    if changed {
        let retained = snapshot::unchanged_paths(&review.snapshot.patch, patch);
        review.snapshot.patch = patch.to_owned();
        review.generation = review
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("generation_limit", "generation counter exhausted"))?;
        for thread in &mut review.threads {
            if retained.contains(&thread.anchor.file) {
                if !thread.stale {
                    thread.generation = review.generation;
                }
            } else {
                thread.stale = true;
            }
        }
        review
            .progress
            .reviewed_files
            .retain(|file| retained.contains(file));
        review
            .progress
            .reviewed_hunks
            .retain(|(file, _)| retained.contains(file));
        review.progress.verdict = None;
        review.last_return = None;
    }
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "changed": changed }),
    )
}

fn add_comment(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    if review.threads.len() >= MAX_REVIEW_THREADS {
        return Err(invalid("thread_limit", "review thread limit reached"));
    }
    message_capacity(review)?;
    let anchor: ReviewAnchor = parse(
        params
            .get("anchor")
            .cloned()
            .ok_or_else(|| invalid("invalid_params", "anchor is required"))?,
    )?;
    validate_anchor(&anchor)?;
    let message = new_message(params, revision, None)?;
    let thread = ReviewThread {
        thread_id: format!("thread-{}", unique_id()),
        anchor,
        messages: vec![message.clone()],
        generation: review.generation,
        stale: false,
        disposition: "open".to_owned(),
    };
    review.threads.push(thread.clone());
    Ok(comment_result(review, &thread, &message, revision))
}

fn reply(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    message_capacity(review)?;
    let thread_id = string(params, "thread_id")?;
    let reply_to = string(params, "reply_to")?;
    let thread = review
        .threads
        .iter_mut()
        .find(|thread| thread.thread_id == thread_id)
        .ok_or_else(|| missing("thread_not_found", thread_id))?;
    if !thread
        .messages
        .iter()
        .any(|message| message.message_id == reply_to)
    {
        return Err(invalid(
            "invalid_reply",
            "reply_to must name a message in this thread",
        ));
    }
    let message = new_message(params, revision, Some(reply_to.to_owned()))?;
    thread.messages.push(message.clone());
    let thread = thread.clone();
    Ok(comment_result(review, &thread, &message, revision))
}

fn new_message(
    params: &Value,
    revision: u64,
    reply_to: Option<String>,
) -> io::Result<ReviewMessage> {
    let body = string(params, "body")?;
    bounded(body, "body", MAX_BODY_BYTES)?;
    let origin = optional_string(params, "origin")?.unwrap_or("agent");
    if !matches!(origin, "human" | "agent") {
        return Err(invalid("invalid_origin", "origin must be human or agent"));
    }
    let author = optional_string(params, "author")?;
    if let Some(author) = author {
        bounded(author, "author", 256)?;
    }
    Ok(ReviewMessage {
        message_id: format!("message-{}", unique_id()),
        reply_to,
        body: body.to_owned(),
        origin: origin.to_owned(),
        author: author.map(str::to_owned),
        revision,
        deleted: false,
    })
}

fn comment_result(
    review: &ReviewRecord,
    thread: &ReviewThread,
    message: &ReviewMessage,
    revision: u64,
) -> Value {
    json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision,
        "thread": { "thread_id": thread.thread_id, "anchor": thread.anchor, "generation": thread.generation, "stale": thread.stale, "disposition": thread.disposition },
        "message": message })
}

fn receipt_result(review: &ReviewRecord, receipt: &MutationReceipt) -> io::Result<Value> {
    let mut result = receipt.result.clone();
    if let Some(id) = result.get("_submission_ref").and_then(Value::as_str) {
        let mut submission = review
            .submissions
            .iter()
            .find(|submission| submission.submission_id == id)
            .cloned()
            .ok_or_else(|| {
                invalid(
                    "invalid_checkpoint",
                    "receipt refers to a missing submission",
                )
            })?;
        // The initial submit response precedes all delivery acknowledgements.
        submission.acknowledged_by.clear();
        result
            .as_object_mut()
            .expect("receipt object")
            .remove("_submission_ref");
        result["submission"] = serde_json::to_value(submission)?;
    }
    Ok(result)
}

fn edit_comment(
    review: &mut ReviewRecord,
    params: &Value,
    revision: u64,
    remove: bool,
) -> io::Result<Value> {
    let id = string(params, "message_id")?;
    let thread = review
        .threads
        .iter_mut()
        .find(|thread| {
            thread
                .messages
                .iter()
                .any(|message| message.message_id == id)
        })
        .ok_or_else(|| missing("message_not_found", id))?;
    let index = thread
        .messages
        .iter()
        .position(|message| message.message_id == id)
        .expect("located message");
    if thread.messages[index].origin != "human" {
        return Err(invalid(
            "human_message_required",
            "only human messages may be edited or removed",
        ));
    }
    if !remove
        && thread.messages[index + 1..]
            .iter()
            .any(|message| message.origin == "human" && !message.deleted)
    {
        return Err(invalid(
            "latest_message_required",
            "only the latest human message in a thread may be edited",
        ));
    }
    let message = &mut thread.messages[index];
    if remove {
        message.body.clear();
        message.deleted = true;
    } else {
        if message.deleted {
            return Err(invalid(
                "message_deleted",
                "removed messages cannot be edited",
            ));
        }
        let body = string(params, "body")?;
        bounded(body, "body", MAX_BODY_BYTES)?;
        message.body = body.to_owned();
    }
    message.revision = revision;
    let message = message.clone();
    let thread = thread.clone();
    Ok(comment_result(review, &thread, &message, revision))
}

fn submit(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    if review.submissions.len() >= MAX_FEEDBACK_SUBMISSIONS {
        return Err(invalid(
            "submission_limit",
            "feedback submission limit reached",
        ));
    }
    let selected = string_list(params, "thread_ids")?;
    if let Some(ids) = &selected
        && ids
            .iter()
            .any(|id| !review.threads.iter().any(|thread| &thread.thread_id == id))
    {
        return Err(invalid(
            "thread_not_found",
            "selected thread is not in this review",
        ));
    }
    let previously_submitted: std::collections::HashSet<(&str, u64)> = review
        .submissions
        .iter()
        .flat_map(|submission| submission.threads.iter())
        .flat_map(|thread| {
            thread
                .messages
                .iter()
                .map(|message| (message.message_id.as_str(), message.revision))
        })
        .collect();
    let threads: Vec<_> = review
        .threads
        .iter()
        .filter(|thread| {
            selected
                .as_ref()
                .is_none_or(|ids| ids.contains(&thread.thread_id))
                && thread.messages.iter().any(|message| {
                    message.origin == "human"
                        && !message.deleted
                        && !previously_submitted
                            .contains(&(message.message_id.as_str(), message.revision))
                })
        })
        .cloned()
        .collect();
    if threads.is_empty() {
        return Err(invalid(
            "no_feedback",
            "there is no new human feedback to submit",
        ));
    }
    let submission = FeedbackSubmission {
        submission_id: format!("submission-{}", unique_id()),
        thread_ids: threads
            .iter()
            .map(|thread| thread.thread_id.clone())
            .collect(),
        message_ids: threads
            .iter()
            .flat_map(|thread| {
                thread
                    .messages
                    .iter()
                    .map(|message| message.message_id.clone())
            })
            .collect(),
        generation: review.generation,
        revision,
        acknowledged_by: Vec::new(),
        threads,
    };
    review.submissions.push(submission.clone());
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "submission": submission }),
    )
}

fn acknowledge(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    let id = string(params, "submission_id")?;
    let consumer = optional_string(params, "consumer")?.unwrap_or("agent");
    bounded(consumer, "consumer", 256)?;
    let submission = review
        .submissions
        .iter_mut()
        .find(|submission| submission.submission_id == id)
        .ok_or_else(|| missing("submission_not_found", id))?;
    if !submission
        .acknowledged_by
        .iter()
        .any(|name| name == consumer)
    {
        if submission.acknowledged_by.len() >= 32 {
            return Err(invalid(
                "consumer_limit",
                "submission consumer limit reached",
            ));
        }
        submission.acknowledged_by.push(consumer.to_owned());
    }
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "submission_id": id, "consumer": consumer, "acknowledged": true }),
    )
}

fn finish(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    let outcome = string(params, "outcome")?;
    if !matches!(outcome, "back" | "submitted" | "cancelled") {
        return Err(invalid(
            "invalid_outcome",
            "outcome must be back, submitted, or cancelled",
        ));
    }
    let ids = string_list(params, "submission_ids")?.unwrap_or_default();
    if ids.iter().any(|id| {
        !review.submissions.iter().any(|submission| {
            submission.submission_id == *id && submission.generation == review.generation
        })
    }) {
        return Err(invalid(
            "invalid_submission",
            "submission must belong to this review generation",
        ));
    }
    if outcome == "submitted" && ids.is_empty() {
        return Err(invalid(
            "invalid_submission",
            "submitted outcome requires a feedback submission",
        ));
    }
    if outcome != "submitted" && !ids.is_empty() {
        return Err(invalid(
            "invalid_submission",
            "only submitted outcome may include submissions",
        ));
    }
    review.last_return = Some(ReviewReturn {
        outcome: outcome.to_owned(),
        submission_ids: ids.clone(),
        revision,
    });
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "outcome": outcome, "submission_ids": ids }),
    )
}

fn disposition(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    let id = string(params, "thread_id")?;
    let disposition = string(params, "disposition")?;
    if !matches!(
        disposition,
        "open" | "accepted" | "dismissed" | "blocking" | "non_blocking" | "fixed"
    ) {
        return Err(invalid(
            "invalid_disposition",
            "unknown finding disposition",
        ));
    }
    let thread = review
        .threads
        .iter_mut()
        .find(|thread| thread.thread_id == id)
        .ok_or_else(|| missing("thread_not_found", id))?;
    thread.disposition = disposition.to_owned();
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "thread_id": id, "disposition": disposition }),
    )
}

fn reanchor(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    #[derive(Deserialize)]
    struct AnchorUpdate {
        thread_id: String,
        anchor: ReviewAnchor,
        stale: bool,
        generation: Option<u64>,
    }
    let updates: Vec<AnchorUpdate> = parse(
        params
            .get("threads")
            .cloned()
            .ok_or_else(|| invalid("invalid_params", "threads must be an array"))?,
    )?;
    if updates.len() > MAX_REVIEW_THREADS {
        return Err(invalid("thread_limit", "too many reanchor updates"));
    }
    let count = updates.len();
    let mut seen = std::collections::HashSet::new();
    for update in updates {
        validate_anchor(&update.anchor)?;
        if !seen.insert(update.thread_id.clone()) {
            return Err(invalid("invalid_params", "thread appears more than once"));
        }
        if update
            .generation
            .is_some_and(|generation| generation != review.generation)
        {
            return Err(invalid(
                "stale_generation",
                "thread reanchor belongs to another snapshot",
            ));
        }
        let thread = review
            .threads
            .iter_mut()
            .find(|thread| thread.thread_id == update.thread_id)
            .ok_or_else(|| missing("thread_not_found", &update.thread_id))?;
        thread.anchor = update.anchor;
        thread.stale = update.stale;
        thread.generation = review.generation;
    }
    if let Some(progress_params) = params.get("progress") {
        progress(review, progress_params, revision)?;
    }
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision, "reanchored": count }),
    )
}

fn progress(review: &mut ReviewRecord, params: &Value, revision: u64) -> io::Result<Value> {
    if let Some(file) = optional_string(params, "file")? {
        validate_file(file)?;
        let reviewed = params
            .get("reviewed")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("invalid_params", "reviewed must be a boolean"))?;
        review.progress.reviewed_files.retain(|path| path != file);
        if reviewed {
            review.progress.reviewed_files.push(file.to_owned());
        }
    } else {
        if let Some(files) = string_list(params, "reviewed_files")? {
            review.progress.reviewed_files = files;
        }
        if let Some(hunks) = params.get("reviewed_hunks") {
            review.progress.reviewed_hunks = parse(hunks.clone())?;
        }
        if let Some(verdict) = params.get("verdict") {
            review.progress.verdict = (!verdict.is_null()).then(|| verdict.clone());
        }
    }
    if review.progress.reviewed_files.len() > 4096 || review.progress.reviewed_hunks.len() > 16384 {
        return Err(invalid("progress_limit", "review progress limit reached"));
    }
    for file in &review.progress.reviewed_files {
        validate_file(file)?;
    }
    for (file, hunk) in &review.progress.reviewed_hunks {
        validate_file(file)?;
        if *hunk == 0 {
            return Err(invalid(
                "invalid_progress",
                "reviewed hunk numbers are one-based",
            ));
        }
    }
    if serde_json::to_vec(&review.progress.verdict)?.len() > 8192 {
        return Err(invalid("verdict_limit", "verdict exceeds 8 KiB"));
    }
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": revision,
        "reviewed_file_count": review.progress.reviewed_files.len(), "reviewed_hunk_count": review.progress.reviewed_hunks.len() }),
    )
}

fn patch_page(review: &ReviewRecord, params: &Value) -> io::Result<Value> {
    let patch = &review.snapshot.patch;
    let start = optional_usize(params, "offset")?.unwrap_or(0);
    let limit = optional_usize(params, "max_bytes")?.unwrap_or(64 * 1024);
    if limit == 0 || limit > 256 * 1024 {
        return Err(invalid("invalid_limit", "max_bytes must be 1..262144"));
    }
    if start > patch.len() || !patch.is_char_boundary(start) {
        return Err(invalid(
            "invalid_cursor",
            "patch offset is not a character boundary",
        ));
    }
    let mut end = start.saturating_add(limit).min(patch.len());
    while end > start && !patch.is_char_boundary(end) {
        end -= 1;
    }
    if end == start && start < patch.len() {
        return Err(invalid(
            "invalid_limit",
            "max_bytes cannot hold the next UTF-8 character",
        ));
    }
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "patch": &patch[start..end], "total_bytes": patch.len(), "truncated": end < patch.len(), "next_offset": (end < patch.len()).then_some(end) }),
    )
}

fn changes_page(review: &ReviewRecord, params: &Value) -> io::Result<Value> {
    let limit = optional_usize(params, "limit")?.unwrap_or(100);
    if limit == 0 || limit > MAX_REVIEW_CHANGES {
        return Err(invalid("invalid_limit", "change limit must be 1..256"));
    }
    let after = match optional_string(params, "cursor")? {
        None => 0,
        Some(cursor) => {
            let (id, revision) = cursor
                .rsplit_once(':')
                .ok_or_else(|| invalid("invalid_cursor", "malformed change cursor"))?;
            if id != review.review_id {
                return Err(invalid(
                    "invalid_cursor",
                    "change cursor belongs to another review",
                ));
            }
            revision
                .parse::<u64>()
                .map_err(|_| invalid("invalid_cursor", "malformed change revision"))?
        }
    };
    if after > review.revision {
        return Err(invalid(
            "invalid_cursor",
            "change cursor is ahead of the review",
        ));
    }
    let floor = review
        .changes
        .first()
        .map_or(review.revision, |change| change.revision.saturating_sub(1));
    if after < floor {
        return Ok(
            json!({ "review_id": review.review_id, "generation": review.generation, "revision": review.revision, "changes": [], "next_cursor": format!("{}:{}", review.review_id, review.revision), "has_more": false, "full_refresh_required": true }),
        );
    }
    let changes: Vec<_> = review
        .changes
        .iter()
        .filter(|change| change.revision > after)
        .take(limit)
        .collect();
    let end = changes.last().map_or(after, |change| change.revision);
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": review.revision, "changes": changes, "next_cursor": format!("{}:{end}", review.review_id), "has_more": end < review.revision, "full_refresh_required": false }),
    )
}

fn feedback_page(review: &ReviewRecord, params: &Value) -> io::Result<Value> {
    let consumer = optional_string(params, "consumer")?.unwrap_or("agent");
    bounded(consumer, "consumer", 256)?;
    let submissions: Vec<_> = review
        .submissions
        .iter()
        .filter(|submission| {
            !submission
                .acknowledged_by
                .iter()
                .any(|name| name == consumer)
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    let threads: Vec<_> = submissions
        .iter()
        .rev()
        .flat_map(|submission| submission.threads.iter())
        .filter(|thread| seen.insert(thread.thread_id.as_str()))
        .collect();
    Ok(
        json!({ "review_id": review.review_id, "generation": review.generation, "revision": review.revision,
        "submissions": submissions, "threads": threads, "last_return": review.last_return }),
    )
}

fn load(lock: &LockedDirectory, id: &str) -> io::Result<ReviewRecord> {
    let bytes = lock.read(id).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            missing("review_not_found", id)
        } else {
            error
        }
    })?;
    let review: ReviewRecord = serde_json::from_slice(&bytes)
        .map_err(|error| invalid("invalid_checkpoint", &error.to_string()))?;
    if review.schema_version != REVIEW_SCHEMA_VERSION {
        return Err(invalid(
            "unsupported_schema",
            "checkpoint schema is not supported",
        ));
    }
    if review.review_id != id {
        return Err(invalid(
            "invalid_checkpoint",
            "review ID does not match checkpoint filename",
        ));
    }
    validate_record(&review)?;
    Ok(review)
}

fn persist(lock: &LockedDirectory, review: &ReviewRecord) -> io::Result<()> {
    validate_record(review)?;
    lock.write(&review.review_id, &serde_json::to_vec(review)?)
}

fn validate_record(review: &ReviewRecord) -> io::Result<()> {
    validate_id(&review.review_id)?;
    validate_patch(&review.snapshot.patch)?;
    bounded(&review.binding.harness, "harness", 128)?;
    bounded(&review.binding.conversation, "conversation", 1024)?;
    if !Path::new(&review.binding.repository).is_absolute()
        || review.source.is_null()
        || serde_json::to_vec(&review.source)?.len() > 16 * 1024
    {
        return Err(invalid(
            "invalid_checkpoint",
            "invalid review binding or source",
        ));
    }
    if review.generation == 0
        || review.revision == 0
        || review.threads.len() > MAX_REVIEW_THREADS
        || review
            .threads
            .iter()
            .map(|thread| thread.messages.len())
            .sum::<usize>()
            > MAX_REVIEW_MESSAGES
        || review.changes.len() > MAX_REVIEW_CHANGES
        || review.mutations.len() > MAX_MUTATION_RECEIPTS
        || review.submissions.len() > MAX_FEEDBACK_SUBMISSIONS
    {
        return Err(invalid(
            "invalid_checkpoint",
            "review counters or limits are invalid",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    let mut all_message_ids = std::collections::HashSet::new();
    for thread in &review.threads {
        validate_entity_id(&thread.thread_id, "thread-")?;
        if !ids.insert(&thread.thread_id) || thread.messages.is_empty() {
            return Err(invalid("invalid_checkpoint", "duplicate or empty thread"));
        }
        validate_anchor(&thread.anchor)?;
        let mut message_ids = std::collections::HashSet::new();
        for message in &thread.messages {
            validate_entity_id(&message.message_id, "message-")?;
            if !all_message_ids.insert(&message.message_id) {
                return Err(invalid(
                    "invalid_checkpoint",
                    "message IDs must be unique across threads",
                ));
            }
            if !message.deleted {
                bounded(&message.body, "body", MAX_BODY_BYTES)?;
            }
            if let Some(author) = &message.author {
                bounded(author, "author", 256)?;
            }
            if message_ids.contains(&message.message_id)
                || message.revision > review.revision
                || !matches!(message.origin.as_str(), "human" | "agent")
                || message
                    .reply_to
                    .as_ref()
                    .is_some_and(|parent| !message_ids.contains(parent))
            {
                return Err(invalid(
                    "invalid_checkpoint",
                    "message identity or reply relationship is invalid",
                ));
            }
            message_ids.insert(&message.message_id);
        }
    }
    let mut submission_ids = std::collections::HashSet::new();
    for submission in &review.submissions {
        validate_entity_id(&submission.submission_id, "submission-")?;
        if !submission_ids.insert(&submission.submission_id)
            || submission.generation == 0
            || submission.generation > review.generation
            || submission.revision > review.revision
            || submission.thread_ids.iter().any(|id| !ids.contains(id))
            || submission
                .message_ids
                .iter()
                .any(|id| !all_message_ids.contains(id))
            || submission.acknowledged_by.len() > 32
        {
            return Err(invalid("invalid_checkpoint", "invalid feedback submission"));
        }
    }
    let mut previous = 0;
    for change in &review.changes {
        if change.revision <= previous
            || change.revision > review.revision
            || change.generation == 0
            || change.generation > review.generation
        {
            return Err(invalid("invalid_checkpoint", "invalid change sequence"));
        }
        previous = change.revision;
    }
    let mut mutation_ids = std::collections::HashSet::new();
    for receipt in &review.mutations {
        bounded(&receipt.client_mutation_id, "client_mutation_id", 256)?;
        if !mutation_ids.insert(&receipt.client_mutation_id) {
            return Err(invalid("invalid_checkpoint", "duplicate mutation receipt"));
        }
    }
    if review.progress.reviewed_files.len() > 4096 || review.progress.reviewed_hunks.len() > 16384 {
        return Err(invalid("invalid_checkpoint", "invalid progress limits"));
    }
    for file in &review.progress.reviewed_files {
        validate_file(file)?;
    }
    for (file, hunk) in &review.progress.reviewed_hunks {
        validate_file(file)?;
        if *hunk == 0 {
            return Err(invalid("invalid_checkpoint", "invalid reviewed hunk"));
        }
    }
    Ok(())
}

fn validate_entity_id(id: &str, prefix: &str) -> io::Result<()> {
    validate_id(
        id.strip_prefix(prefix)
            .ok_or_else(|| invalid("invalid_checkpoint", "invalid entity ID prefix"))?,
    )
}

fn validate_anchor(anchor: &ReviewAnchor) -> io::Result<()> {
    validate_file(&anchor.file)?;
    if anchor.old_line == Some(0) || anchor.new_line == Some(0) || anchor.hunk == Some(0) {
        return Err(invalid(
            "invalid_anchor",
            "line and hunk numbers are one-based",
        ));
    }
    if let Some(range) = &anchor.range {
        for side in [&range.old, &range.new].into_iter().flatten() {
            if side.start == 0 || side.end < side.start {
                return Err(invalid("invalid_anchor", "invalid anchor range"));
            }
        }
    }
    Ok(())
}

fn validate_file(file: &str) -> io::Result<()> {
    bounded(file, "file", 4096)?;
    let path = Path::new(file);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid(
            "invalid_anchor",
            "file must be a normalized repository-relative path",
        ));
    }
    Ok(())
}

fn validate_patch(patch: &str) -> io::Result<()> {
    if patch.len() > MAX_SNAPSHOT_BYTES {
        return Err(invalid(
            "snapshot_limit",
            "patch exceeds 2 MiB snapshot limit",
        ));
    }
    Ok(())
}

fn message_capacity(review: &ReviewRecord) -> io::Result<()> {
    if review
        .threads
        .iter()
        .map(|thread| thread.messages.len())
        .sum::<usize>()
        >= MAX_REVIEW_MESSAGES
    {
        return Err(invalid("message_limit", "review message limit reached"));
    }
    Ok(())
}

fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:012x}{:06x}{:06x}",
        nanos & ((1u128 << 48) - 1),
        std::process::id() & 0x00ff_ffff,
        sequence & 0x00ff_ffff
    )
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn request_fingerprint(method: &str, params: &Value) -> io::Result<String> {
    let mut request = method.as_bytes().to_vec();
    request.push(0);
    request.extend(serde_json::to_vec(params)?);
    Ok(fingerprint::sha256(&request))
}
fn refresh_client_fingerprint(params: &Value) -> io::Result<String> {
    let mut client = params.clone();
    client
        .as_object_mut()
        .ok_or_else(|| invalid("invalid_params", "expected a parameter object"))?
        .remove("patch");
    request_fingerprint("review.refresh", &client)
}
fn validate_expected_revision(params: &Value) -> io::Result<Option<u64>> {
    params
        .get("expected_revision")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                invalid(
                    "invalid_params",
                    "expected_revision must be a nonnegative integer",
                )
            })
        })
        .transpose()
}
fn bounded(text: &str, name: &str, limit: usize) -> io::Result<()> {
    if text.trim().is_empty() || text.len() > limit || text.contains('\0') {
        return Err(invalid(
            "invalid_params",
            &format!("{name} must be nonempty, contain no NUL, and fit {limit} bytes"),
        ));
    }
    Ok(())
}
fn string<'a>(params: &'a Value, key: &str) -> io::Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("invalid_params", &format!("{key} must be a string")))
}
fn optional_string<'a>(params: &'a Value, key: &str) -> io::Result<Option<&'a str>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(invalid(
            "invalid_params",
            &format!("{key} must be a string"),
        )),
    }
}
fn number(params: &Value, key: &str) -> io::Result<u64> {
    params.get(key).and_then(Value::as_u64).ok_or_else(|| {
        invalid(
            "invalid_params",
            &format!("{key} must be a nonnegative integer"),
        )
    })
}
fn optional_usize(params: &Value, key: &str) -> io::Result<Option<usize>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| {
                invalid(
                    "invalid_params",
                    &format!("{key} must be a nonnegative integer"),
                )
            }),
    }
}
fn string_list(params: &Value, key: &str) -> io::Result<Option<Vec<String>>> {
    params
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| parse(value.clone()))
        .transpose()
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> io::Result<T> {
    serde_json::from_value(value).map_err(|error| invalid("invalid_params", &error.to_string()))
}
fn value(value: impl Serialize) -> io::Result<Value> {
    serde_json::to_value(value).map_err(io::Error::other)
}
fn missing(code: &str, message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{code}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, ReviewStore, ReviewRecord) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let store = ReviewStore::new(root.join("state"));
        let review = store
            .open(OpenReview {
                harness: "pi".to_owned(),
                conversation: "conversation-1".to_owned(),
                repository: temp.path().to_string_lossy().into_owned(),
                source: json!({ "kind": "worktree" }),
                patch: "old patch\n".to_owned(),
                review_id: None,
            })
            .unwrap();
        (temp, store, review)
    }

    fn comment(store: &ReviewStore, review: &ReviewRecord, key: &str, origin: &str) -> Value {
        store.call("comment.add", json!({ "review_id": review.review_id, "generation": review.generation,
            "client_mutation_id": key, "anchor": { "file": "src/lib.rs", "new_line": 3 }, "body": "Why this change?", "origin": origin })).unwrap()
    }

    #[test]
    fn review_binding_resumes_snapshot_but_separates_conversations_and_sources() {
        let (temp, store, review) = fixture();
        let mut args = OpenReview {
            harness: "pi".to_owned(),
            conversation: "conversation-1".to_owned(),
            repository: temp.path().to_string_lossy().into_owned(),
            source: json!({ "kind": "worktree" }),
            patch: "new workspace data".to_owned(),
            review_id: None,
        };
        let resumed = store.open(args.clone()).unwrap();
        assert_eq!(resumed.review_id, review.review_id);
        assert_eq!(resumed.snapshot.patch, "old patch\n");
        args.conversation = "conversation-2".to_owned();
        assert_ne!(
            store.open(args.clone()).unwrap().review_id,
            review.review_id
        );
        args.conversation = "conversation-1".to_owned();
        args.source = json!({ "kind": "show", "revision": "HEAD" });
        assert_ne!(store.open(args).unwrap().review_id, review.review_id);
        assert_eq!(store.list().unwrap().len(), 3);
    }

    #[test]
    fn reviews_and_idempotency_receipts_survive_process_lifetime() {
        let (_temp, store, review) = fixture();
        let first = comment(&store, &review, "comment-1", "human");
        let reopened = ReviewStore::new(store.directory());
        assert_eq!(comment(&reopened, &review, "comment-1", "human"), first);
        let current = reopened.get(&review.review_id).unwrap();
        assert_eq!(current.threads.len(), 1);
        assert_eq!(current.revision, 2);
        let error = reopened.call("comment.add", json!({ "review_id": review.review_id, "generation": 1,
            "client_mutation_id": "comment-1", "anchor": { "file": "src/lib.rs", "new_line": 3 }, "body": "Different body", "origin": "human" })).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(reopened.get(&review.review_id).unwrap().revision, 2);
    }

    #[test]
    fn revision_condition_prevents_lost_updates_but_identical_receipts_replay_first() {
        let (_temp, store, review) = fixture();
        let request = json!({ "review_id": review.review_id, "generation": 1, "expected_revision": 1,
            "client_mutation_id": "conditional", "anchor": { "file": "src/lib.rs", "new_line": 3 }, "body": "Conditional note", "origin": "human" });
        let first = store.call("comment.add", request.clone()).unwrap();
        comment(&store, &review, "concurrent", "human");
        assert_eq!(store.get(&review.review_id).unwrap().revision, 3);
        assert_eq!(store.call("comment.add", request.clone()).unwrap(), first);
        let path = store.directory().join(format!("{}.json", review.review_id));
        let before = fs::read(&path).unwrap();
        let mut stale = request;
        stale["client_mutation_id"] = json!("stale-condition");
        let error = store.call("comment.add", stale).unwrap_err();
        assert!(error.to_string().starts_with("stale_revision:"));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn refresh_client_retry_reconciles_before_new_patch_evidence_without_weakening_id_checks() {
        let (_temp, store, review) = fixture();
        let client = json!({ "review_id": review.review_id, "generation": 1, "expected_revision": 1,
            "client_mutation_id": "refresh-client", "source": { "kind": "worktree" } });
        assert!(store.reconcile_refresh(&client).unwrap().is_none());
        let mut internal = client.clone();
        internal["patch"] = json!("first actual Git patch");
        let first = store.call("review.refresh", internal.clone()).unwrap();
        let current = store.get(&review.review_id).unwrap();
        comment(&store, &current, "later-edit", "human");
        assert_eq!(
            store.reconcile_refresh(&client).unwrap(),
            Some(first.clone())
        );
        let mut forged_patch = client.clone();
        forged_patch["patch"] = json!("caller patch is ignored by adapter");
        assert_eq!(store.reconcile_refresh(&forged_patch).unwrap(), Some(first));
        // Direct store mutations still compare the complete request including patch.
        internal["patch"] = json!("second actual Git patch");
        assert!(
            store
                .call("review.refresh", internal)
                .unwrap_err()
                .to_string()
                .starts_with("mutation_conflict:")
        );
        let before =
            fs::read(store.directory().join(format!("{}.json", review.review_id))).unwrap();
        for (key, value) in [
            ("generation", json!(2)),
            ("expected_revision", json!(99)),
            ("source", json!({ "kind": "show", "rev": "HEAD" })),
        ] {
            let mut different = client.clone();
            different[key] = value;
            assert!(
                store
                    .reconcile_refresh(&different)
                    .unwrap_err()
                    .to_string()
                    .starts_with("mutation_conflict:")
            );
        }
        assert_eq!(
            fs::read(store.directory().join(format!("{}.json", review.review_id))).unwrap(),
            before
        );
    }

    #[test]
    fn legacy_receipts_load_but_cannot_blindly_reconcile_a_refresh() {
        let (_temp, store, review) = fixture();
        let request = json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "legacy", "patch": "new patch" });
        let first = store.call("review.refresh", request.clone()).unwrap();
        let path = store.directory().join(format!("{}.json", review.review_id));
        let mut checkpoint: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        checkpoint["mutations"][0]
            .as_object_mut()
            .unwrap()
            .remove("client_request");
        fs::write(&path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        assert!(store.get(&review.review_id).is_ok());
        assert!(
            store
                .reconcile_refresh(&request)
                .unwrap_err()
                .to_string()
                .starts_with("mutation_conflict:")
        );
        assert_eq!(store.call("review.refresh", request).unwrap(), first);
    }

    #[test]
    fn explicit_reply_links_preserve_threads_and_reject_cross_thread_replies() {
        let (_temp, store, review) = fixture();
        let first = comment(&store, &review, "first", "human");
        let other = comment(&store, &review, "other", "human");
        let mut request = json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "reply-1", "thread_id": first["thread"]["thread_id"], "reply_to": other["message"]["message_id"], "body": "An answer" });
        assert!(
            store
                .call("comment.reply", request.clone())
                .unwrap_err()
                .to_string()
                .starts_with("invalid_reply:")
        );
        request["reply_to"] = first["message"]["message_id"].clone();
        let answer = store.call("comment.reply", request).unwrap();
        assert_eq!(answer["thread"]["anchor"], first["thread"]["anchor"]);
        let current = store.get(&review.review_id).unwrap();
        assert_eq!(current.threads[0].messages.len(), 2);
        assert!(answer["thread"].get("messages").is_none());
    }

    #[test]
    fn submission_is_explicit_immutable_and_acknowledged_per_consumer() {
        let (_temp, store, review) = fixture();
        let first = comment(&store, &review, "first", "human");
        assert!(
            store
                .call("feedback.get", json!({ "review_id": review.review_id }))
                .unwrap()["submissions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let sent = store.call("feedback.submit", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "send-1" })).unwrap();
        store.call("comment.edit", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "edit-1", "message_id": first["message"]["message_id"], "body": "A later unsent edit" })).unwrap();
        let pending = store
            .call("feedback.get", json!({ "review_id": review.review_id }))
            .unwrap();
        assert_eq!(
            pending["threads"][0]["messages"][0]["body"],
            "Why this change?"
        );
        let ack = json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "ack-1", "submission_id": sent["submission"]["submission_id"], "consumer": "pi" });
        let result = store.call("feedback.ack", ack.clone()).unwrap();
        assert_eq!(store.call("feedback.ack", ack).unwrap(), result);
        assert!(
            store
                .call(
                    "feedback.get",
                    json!({ "review_id": review.review_id, "consumer": "pi" })
                )
                .unwrap()["submissions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .call(
                    "feedback.get",
                    json!({ "review_id": review.review_id, "consumer": "claude" })
                )
                .unwrap()["submissions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // A changed revision of the same human message is deliverable again.
        assert!(store.call("feedback.submit", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "send-2" })).is_ok());
    }

    #[test]
    fn refresh_rejects_stale_writes_and_retry_never_reapplies_the_snapshot() {
        let (_temp, store, review) = fixture();
        let first = comment(&store, &review, "human", "human");
        let request = json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "refresh-1", "patch": "new patch\n" });
        let refreshed = store.call("review.refresh", request.clone()).unwrap();
        assert_eq!(refreshed["generation"], 2);
        assert_eq!(store.call("review.refresh", request).unwrap(), refreshed);
        let current = store.get(&review.review_id).unwrap();
        assert!(current.threads[0].stale);
        assert_eq!(
            serde_json::to_value(&current.threads[0].anchor).unwrap(),
            first["thread"]["anchor"]
        );
        assert!(store.call("comment.add", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "stale", "anchor": { "file": "src/lib.rs" }, "body": "Outdated" })).unwrap_err().to_string().starts_with("stale_generation:"));
    }

    #[test]
    fn human_removal_keeps_agent_replies_and_edits_cannot_change_agent_messages() {
        let (_temp, store, review) = fixture();
        let first = comment(&store, &review, "human", "human");
        let answer = store.call("comment.reply", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "answer", "thread_id": first["thread"]["thread_id"], "reply_to": first["message"]["message_id"], "body": "Agent answer" })).unwrap();
        assert!(store.call("comment.edit", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "bad-edit", "message_id": answer["message"]["message_id"], "body": "Replace agent answer" })).is_err());
        store.call("comment.remove", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "remove", "message_id": first["message"]["message_id"] })).unwrap();
        let current = store.get(&review.review_id).unwrap();
        assert!(current.threads[0].messages[0].deleted);
        assert_eq!(current.threads[0].messages[1].body, "Agent answer");
    }

    #[test]
    fn change_cursor_is_review_scoped_bounded_and_expires_explicitly() {
        let (_temp, store, review) = fixture();
        let initial = store
            .call(
                "review.changes",
                json!({ "review_id": review.review_id, "limit": 1 }),
            )
            .unwrap();
        let cursor = initial["next_cursor"].as_str().unwrap();
        for index in 0..MAX_REVIEW_CHANGES + 1 {
            store.call("review.progress", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": format!("progress-{index}"), "file": "src/lib.rs", "reviewed": index % 2 == 0 })).unwrap();
        }
        let expired = store
            .call(
                "review.changes",
                json!({ "review_id": review.review_id, "cursor": cursor }),
            )
            .unwrap();
        assert_eq!(expired["full_refresh_required"], true);
        let current = store
            .call(
                "review.changes",
                json!({ "review_id": review.review_id, "cursor": expired["next_cursor"] }),
            )
            .unwrap();
        assert_eq!(current["full_refresh_required"], false);
        assert!(current["changes"].as_array().unwrap().is_empty());
        assert!(
            store
                .call(
                    "review.changes",
                    json!({ "review_id": review.review_id, "cursor": "000000000000000000000000:1" })
                )
                .is_err()
        );
    }

    #[test]
    fn patch_pages_preserve_utf8_and_report_all_continuations() {
        let (_temp, store, review) = fixture();
        store.call("review.refresh", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "utf8", "patch": "aé水z" })).unwrap();
        let first = store
            .call(
                "review.patch",
                json!({ "review_id": review.review_id, "max_bytes": 4 }),
            )
            .unwrap();
        assert_eq!(first["patch"], "aé");
        let second = store.call("review.patch", json!({ "review_id": review.review_id, "offset": first["next_offset"], "max_bytes": 4 })).unwrap();
        assert_eq!(second["patch"], "水z");
        assert_eq!(second["truncated"], false);
        assert!(
            store
                .call(
                    "review.patch",
                    json!({ "review_id": review.review_id, "offset": 2 })
                )
                .is_err()
        );
    }

    #[test]
    fn refresh_preserves_unchanged_files_and_internal_reanchor_restores_changed_threads() {
        let (_temp, store, review) = fixture();
        let stable = "diff --git a/stable.rs b/stable.rs\n--- a/stable.rs\n+++ b/stable.rs\n@@ -1 +1 @@\n-a\n+b\n";
        let changing = "diff --git a/changing.rs b/changing.rs\n--- a/changing.rs\n+++ b/changing.rs\n@@ -1 +1 @@\n-a\n+b\n";
        store.call("review.refresh", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "initial", "patch": format!("{stable}{changing}") })).unwrap();
        let stable_thread = store.call("comment.add", json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "stable", "anchor": { "file": "stable.rs", "new_line": 1 }, "body": "Keep" })).unwrap();
        let changing_thread = store.call("comment.add", json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "changing", "anchor": { "file": "changing.rs", "new_line": 1 }, "body": "Inspect again" })).unwrap();
        store.call("review.progress", json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "progress", "reviewed_files": ["stable.rs", "changing.rs"], "reviewed_hunks": [["stable.rs", 1]], "verdict": { "kind": "approve" } })).unwrap();
        store.call("review.refresh", json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "refresh", "patch": format!("{stable}{}", changing.replace("+b", "+c")) })).unwrap();
        let current = store.get(&review.review_id).unwrap();
        assert!(
            !current
                .threads
                .iter()
                .find(|thread| thread.thread_id == stable_thread["thread"]["thread_id"])
                .unwrap()
                .stale
        );
        assert!(
            current
                .threads
                .iter()
                .find(|thread| thread.thread_id == changing_thread["thread"]["thread_id"])
                .unwrap()
                .stale
        );
        assert_eq!(current.progress.reviewed_files, ["stable.rs"]);
        assert_eq!(
            current.progress.reviewed_hunks,
            [("stable.rs".to_owned(), 1)]
        );
        assert!(current.progress.verdict.is_none());
        store.call("review.reanchor", json!({ "review_id": review.review_id, "generation": 3, "client_mutation_id": "reanchor", "threads": [{ "thread_id": changing_thread["thread"]["thread_id"], "anchor": { "file": "changing.rs", "new_line": 2 }, "stale": false }] })).unwrap();
        let reconciled = store.get(&review.review_id).unwrap();
        let thread = reconciled
            .threads
            .iter()
            .find(|thread| thread.thread_id == changing_thread["thread"]["thread_id"])
            .unwrap();
        assert_eq!(thread.anchor.new_line, Some(2));
        assert!(!thread.stale);
        assert_eq!(thread.generation, 3);
    }

    #[test]
    fn receipts_do_not_duplicate_large_snapshots_or_whole_threads() {
        let (_temp, store, review) = fixture();
        store.call("review.refresh", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "large", "patch": "x".repeat(MAX_SNAPSHOT_BYTES) })).unwrap();
        let current = store.get(&review.review_id).unwrap();
        let first = comment(&store, &current, "human", "human");
        let submitted_request = json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "submit" });
        let submitted = store
            .call("feedback.submit", submitted_request.clone())
            .unwrap();
        store.call("feedback.ack", json!({ "review_id": review.review_id, "generation": 2, "client_mutation_id": "ack", "submission_id": submitted["submission"]["submission_id"] })).unwrap();
        assert_eq!(
            store.call("feedback.submit", submitted_request).unwrap(),
            submitted
        );
        let current = store.get(&review.review_id).unwrap();
        assert_eq!(
            current.threads[0].messages[0].message_id,
            first["message"]["message_id"]
        );
        assert!(
            current
                .mutations
                .iter()
                .all(|receipt| receipt.request.len() < 100)
        );
        assert!(
            current
                .mutations
                .iter()
                .all(|receipt| receipt.result.get("submission").is_none())
        );
        assert!(current.mutations.iter().all(|receipt| {
            receipt
                .result
                .get("thread")
                .and_then(|thread| thread.get("messages"))
                .is_none()
        }));
        assert!(
            fs::metadata(store.directory().join(format!("{}.json", review.review_id)))
                .unwrap()
                .len()
                < MAX_SNAPSHOT_BYTES as u64 + 32 * 1024
        );
    }

    #[test]
    fn failed_mutation_does_not_change_the_checkpoint_and_finish_requires_submission() {
        let (_temp, store, review) = fixture();
        let before =
            fs::read(store.directory().join(format!("{}.json", review.review_id))).unwrap();
        assert!(store.call("review.finish", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "bad-finish", "outcome": "submitted" })).is_err());
        assert!(store.call("comment.add", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "bad-path", "anchor": { "file": "../outside" }, "body": "No" })).is_err());
        assert_eq!(
            fs::read(store.directory().join(format!("{}.json", review.review_id))).unwrap(),
            before
        );
        assert!(store.call("review.finish", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "back", "outcome": "back" })).is_ok());
        assert!(store.get(&review.review_id).unwrap().last_return.is_some());
        store.call("review.begin", json!({ "review_id": review.review_id, "generation": 1, "client_mutation_id": "begin-again" })).unwrap();
        assert!(store.get(&review.review_id).unwrap().last_return.is_none());
        assert!(
            store
                .call("feedback.get", json!({ "review_id": review.review_id }))
                .unwrap()["last_return"]
                .is_null()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_checkpoints_and_store_ancestors_are_rejected() {
        use std::os::unix::fs::symlink;
        let (temp, store, review) = fixture();
        let outside = temp.path().join("outside");
        fs::write(&outside, "untouched").unwrap();
        let checkpoint = store.directory().join(format!("{}.json", review.review_id));
        fs::remove_file(&checkpoint).unwrap();
        symlink(&outside, &checkpoint).unwrap();
        assert!(store.get(&review.review_id).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
        let linked = temp.path().join("linked");
        symlink(store.directory(), &linked).unwrap();
        assert!(ReviewStore::new(linked.join("child")).list().is_err());
        assert!(!store.directory().join("child").exists());
    }

    #[test]
    fn unsupported_checkpoint_schema_is_not_silently_replaced() {
        let (_temp, store, review) = fixture();
        let path = store.directory().join(format!("{}.json", review.review_id));
        let mut raw = serde_json::to_value(&review).unwrap();
        raw["schema_version"] = json!(99);
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert!(
            store
                .get(&review.review_id)
                .unwrap_err()
                .to_string()
                .starts_with("unsupported_schema:")
        );
    }

    #[test]
    fn process_lock_serializes_independent_writers_without_lost_updates() {
        use std::process::{Command, Stdio};
        let (_temp, store, review) = fixture();
        let mut children = Vec::new();
        for _ in 0..4 {
            children.push(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--ignored", "--exact", "review::tests::process_writer"])
                    .env("MARK_REVIEW_TEST_STORE", store.directory())
                    .env("MARK_REVIEW_TEST_ID", &review.review_id)
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        for child in children {
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let current = store.get(&review.review_id).unwrap();
        assert_eq!(current.threads.len(), 32);
        assert_eq!(current.revision, 33);
    }

    #[test]
    #[ignore = "subprocess worker invoked by process_lock_serializes_independent_writers_without_lost_updates"]
    fn process_writer() {
        let directory = env::var_os("MARK_REVIEW_TEST_STORE").unwrap();
        let id = env::var("MARK_REVIEW_TEST_ID").unwrap();
        let store = ReviewStore::new(directory);
        let review = store.get(&id).unwrap();
        for index in 0..8 {
            comment(
                &store,
                &review,
                &format!("{}-{index}", std::process::id()),
                "agent",
            );
        }
    }
}
