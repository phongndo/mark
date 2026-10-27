//! The existing terminal view attached to one durable, harness-bound review.
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    io,
    time::{Duration, Instant},
};

use mark_core::MarkResult;
use mark_session::review::{ReviewRecord, ReviewStore};
use serde_json::{Value, json};

use crate::{
    annotation::{AnnotationKey, AnnotationScope, AnnotationSide},
    app::DiffApp,
    review::{CommentLifecycle, CommentOrigin, FindingDisposition, ReviewComment},
    session::anchors,
};

#[derive(Debug)]
pub(crate) struct IntegratedReview {
    store: ReviewStore,
    record: ReviewRecord,
    imported: HashMap<String, Imported>,
    next_ui_id: u64,
    last_signature: u64,
    last_poll: Instant,
    submitted: Vec<String>,
    sync_conflict: Option<String>,
    source_options: mark_diff::DiffOptions,
    needs_import: bool,
    pending_mutations: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
struct Imported {
    thread_id: String,
    message_id: String,
    body: String,
    human: bool,
    baseline: ReviewComment,
}

/// Apply the same evidence-based review transition used by a live terminal
/// reload when another presentation requests the next pass.
pub fn refresh_integrated_review(
    options: mark_diff::DiffOptions,
    params: Value,
) -> MarkResult<Value> {
    let store = ReviewStore::from_env()?;
    let record = store.call("review.get", json!({"review_id":params["review_id"]}))?;
    // An uncertain retry is reconciled by the store's original mutation receipt.
    if params["generation"] != record["generation"] {
        return Ok(store.call("review.refresh", params)?);
    }
    let patch = record["snapshot"]["patch"]
        .as_str()
        .ok_or_else(|| io::Error::other("review snapshot is missing"))?;
    let new_patch = params["patch"]
        .as_str()
        .ok_or_else(|| io::Error::other("refresh patch is missing"))?;
    if patch == new_patch {
        return Ok(store.call("review.refresh", params)?);
    }
    let load = |patch: &str| {
        let mut source = options.clone();
        source.source = mark_diff::DiffSource::Patch(mark_diff::PatchSource::Text {
            label: "Mark review".into(),
            patch: std::sync::Arc::from(patch.as_bytes()),
        });
        mark_diff::load_review_ref_with_raw_patch(&source)
    };
    let mut app = DiffApp::new_with_syntax(
        options.clone(),
        load(patch)?,
        crate::controls::DiffLayoutMode::Unified,
        crate::app::SyntaxStartupMode::Disabled,
    );
    attach(&mut app, record)?;
    let loaded = load(new_patch)?;
    app.replace_loaded_diff(options, loaded);
    let result = store.call("review.refresh", params)?;
    with_integration(&mut app, |review, app| {
        review.record = serde_json::from_value(
            store.call("review.get", json!({"review_id":review.record.review_id}))?,
        )
        .map_err(io::Error::other)?;
        app.document.generation = review.record.generation;
        review.save(app)
    })?;
    Ok(result)
}

pub(crate) fn attach(app: &mut DiffApp, value: Value) -> MarkResult<()> {
    attach_with_store(app, value, ReviewStore::from_env()?)
}

fn attach_with_store(app: &mut DiffApp, value: Value, store: ReviewStore) -> MarkResult<()> {
    let record: ReviewRecord = serde_json::from_value(value).map_err(io::Error::other)?;
    app.document.generation = record.generation;
    let mut integration = IntegratedReview {
        store,
        record,
        imported: HashMap::new(),
        next_ui_id: 0,
        last_signature: 0,
        last_poll: Instant::now(),
        submitted: Vec::new(),
        sync_conflict: None,
        source_options: app.document.options.clone(),
        needs_import: false,
        pending_mutations: HashMap::new(),
    };
    integration.import(app, None)?;
    app.annotations_state.lifecycle.pass = integration.record.generation;
    app.annotations_state.lifecycle.reviewed_files = integration
        .record
        .progress
        .reviewed_files
        .iter()
        .cloned()
        .collect();
    app.annotations_state.lifecycle.reviewed_hunks = integration
        .record
        .progress
        .reviewed_hunks
        .iter()
        .cloned()
        .collect();
    if let Some(verdict) = &integration.record.progress.verdict {
        let kind = match verdict["kind"].as_str() {
            Some("approve") => Some(crate::review::VerdictKind::Approve),
            Some("request_changes") => Some(crate::review::VerdictKind::RequestChanges),
            Some("comment") => Some(crate::review::VerdictKind::Comment),
            _ => None,
        };
        if let Some(kind) = kind {
            app.annotations_state.lifecycle.verdict = Some(crate::review::FinalVerdict {
                kind,
                summary: verdict["summary"].as_str().map(str::to_owned),
                destination: crate::review::VerdictDestination::Local,
            });
        }
    }
    app.set_notice("Q sends feedback · q returns to conversation · notes are saved locally");
    integration.last_signature = signature(app);
    app.runtime.integration = Some(Box::new(integration));
    Ok(())
}

pub(crate) fn checkpoint_or_notice(app: &mut DiffApp) {
    if let Err(error) = with_integration(app, |review, app| review.save(app)) {
        app.set_error_log(format!("review not saved: {error}"));
    }
}

pub(crate) fn poll(app: &mut DiffApp) {
    // Keep the editor's original baseline until the draft is committed. The
    // next checkpoint reconciles a concurrent edit before publishing it.
    if app.annotations_state.annotation_draft.is_some() {
        return;
    }
    let due = app
        .runtime
        .integration
        .as_ref()
        .is_some_and(|review| review.last_poll.elapsed() >= Duration::from_millis(500));
    if !due {
        return;
    }
    if let Err(error) = with_integration(app, |review, app| {
        review.last_poll = Instant::now();
        let next: ReviewRecord = serde_json::from_value(
            review
                .store
                .call("review.get", json!({"review_id":review.record.review_id}))?,
        )
        .map_err(io::Error::other)?;
        if next.generation != review.record.generation {
            return Err(io::Error::other(
                "stale_generation: this review changed in another view; return and reopen it",
            )
            .into());
        }
        if next.revision != review.record.revision || review.needs_import {
            if let Err(error) = review.import(app, Some(&next)) {
                review.sync_conflict = Some(error.to_string());
                return Err(error);
            }
            review.record = next;
            review.needs_import = false;
            review.sync_conflict = None;
            app.runtime.dirty = true;
        }
        Ok(())
    }) {
        app.set_error_log(error.to_string());
    }
}

pub(crate) fn submit(app: &mut DiffApp) -> MarkResult<()> {
    with_integration(app, |review, app| {
        review.save(app)?;
        let result = review.mutate("feedback.submit", json!({}))?;
        let id = result["submission"]["submission_id"]
            .as_str()
            .or_else(|| result["submission_id"].as_str())
            .ok_or_else(|| io::Error::other("feedback submission did not return an identity"))?;
        review.submitted.push(id.to_owned());
        Ok(())
    })
}

pub(crate) fn has_feedback(app: &DiffApp) -> bool {
    let Some(review) = app.runtime.integration.as_ref() else {
        return true;
    };
    app.annotations_state
        .annotations
        .comments()
        .filter(|comment| comment.origin == CommentOrigin::Human)
        .any(|comment| {
            let Some(imported) = review.imported.get(&comment.id) else {
                return true;
            };
            if imported.body != comment.summary {
                return true;
            }
            let message = review
                .record
                .threads
                .iter()
                .flat_map(|thread| &thread.messages)
                .find(|message| message.message_id == imported.message_id);
            message.is_some_and(|message| {
                !review
                    .record
                    .submissions
                    .iter()
                    .flat_map(|batch| &batch.threads)
                    .flat_map(|thread| &thread.messages)
                    .any(|sent| {
                        sent.message_id == message.message_id && sent.revision == message.revision
                    })
            })
        })
}

pub(crate) fn finish(app: &mut DiffApp) -> MarkResult<()> {
    with_integration(app, |review, app| {
        review.save(app)?;
        review.mutate("review.finish",json!({"outcome":if review.submitted.is_empty(){"back"}else{"submitted"},"submission_ids":review.submitted}))?;
        Ok(())
    })
}

fn with_integration(
    app: &mut DiffApp,
    operation: impl FnOnce(&mut IntegratedReview, &mut DiffApp) -> MarkResult<()>,
) -> MarkResult<()> {
    let Some(mut review) = app.runtime.integration.take() else {
        return Ok(());
    };
    let result = operation(&mut review, app);
    app.runtime.integration = Some(review);
    result
}

impl IntegratedReview {
    fn mutate(&mut self, method: &str, mut params: Value) -> MarkResult<Value> {
        if self.needs_import {
            return Err(io::Error::other(
                "concurrent_update: synchronize review changes before saving again",
            )
            .into());
        }
        let key = serde_json::to_string(&json!([method, params])).map_err(io::Error::other)?;
        let params = if let Some(pending) = self.pending_mutations.get(&key) {
            pending.clone()
        } else {
            params["review_id"] = json!(self.record.review_id);
            params["generation"] = json!(self.record.generation);
            params["expected_revision"] = json!(self.record.revision);
            params["client_mutation_id"] = json!(format!("tui-{}", mark_session::new_session_id()));
            self.pending_mutations.insert(key.clone(), params.clone());
            params
        };
        let result = match self.store.call(method, params) {
            Ok(result) => result,
            Err(error) => {
                if error.to_string().starts_with("stale_revision:")
                    || error.to_string().starts_with("stale_generation:")
                {
                    self.pending_mutations.remove(&key);
                }
                return Err(error.into());
            }
        };
        let next: ReviewRecord = serde_json::from_value(
            self.store
                .call("review.get", json!({"review_id":self.record.review_id}))?,
        )
        .map_err(io::Error::other)?;
        self.pending_mutations.remove(&key);
        self.needs_import = result["revision"].as_u64() != Some(next.revision);
        if result["generation"].as_u64() != Some(next.generation) {
            self.sync_conflict = Some(
                "stale_generation: this review changed in another view; reopen it to continue"
                    .to_owned(),
            );
        }
        self.record = next;
        Ok(result)
    }

    fn import(&mut self, app: &mut DiffApp, next: Option<&ReviewRecord>) -> MarkResult<()> {
        let record = next.unwrap_or(&self.record);
        let current = app
            .annotations_state
            .annotations
            .comments()
            .cloned()
            .collect::<Vec<_>>();
        let mut ids = current
            .iter()
            .map(|comment| comment.id.clone())
            .collect::<HashSet<_>>();
        let existing = self
            .imported
            .values()
            .map(|value| value.message_id.as_str())
            .collect::<HashSet<_>>();
        let mut next_id = self.next_ui_id;
        let mut imported = HashMap::new();
        let mut comments = Vec::new();
        // Reconcile each already-imported message against its last projection.
        // The remote version becomes the next baseline even when an unrelated
        // unsaved local edit is retained for the next checkpoint.
        for local in &current {
            let Some(previous) = self.imported.get(&local.id) else {
                comments.push(local.clone());
                continue;
            };
            let remote = record.threads.iter().find_map(|thread| {
                thread
                    .messages
                    .iter()
                    .find(|message| message.message_id == previous.message_id && !message.deleted)
                    .map(|message| (thread, message))
            });
            if let Some((thread, message)) = remote {
                let baseline =
                    project_comment(app, thread, message, local.id.clone(), record.generation);
                comments.push(merge_comment(&previous.baseline, local, &baseline)?);
                imported.insert(
                    local.id.clone(),
                    Imported {
                        thread_id: thread.thread_id.clone(),
                        message_id: message.message_id.clone(),
                        body: message.body.clone(),
                        human: message.origin == "human",
                        baseline,
                    },
                );
            } else if comment_changed(&previous.baseline, local) {
                return Err(import_conflict(
                    &local.id,
                    "was deleted in another view while edited locally",
                )
                .into());
            }
        }
        // A local deletion must survive a remote addition or receipt update,
        // while a conflicting remote edit needs an explicit resolution.
        for (id, previous) in &self.imported {
            if ids.contains(id) {
                continue;
            }
            let remote = record.threads.iter().find_map(|thread| {
                thread
                    .messages
                    .iter()
                    .find(|message| message.message_id == previous.message_id && !message.deleted)
                    .map(|message| (thread, message))
            });
            if let Some((thread, message)) = remote {
                let baseline = project_comment(app, thread, message, id.clone(), record.generation);
                if comment_changed(&previous.baseline, &baseline) {
                    return Err(import_conflict(
                        id,
                        "was edited in another view while deleted locally",
                    )
                    .into());
                }
                imported.insert(
                    id.clone(),
                    Imported {
                        thread_id: thread.thread_id.clone(),
                        message_id: message.message_id.clone(),
                        body: message.body.clone(),
                        human: message.origin == "human",
                        baseline,
                    },
                );
            }
        }
        for thread in &record.threads {
            for message in &thread.messages {
                if message.deleted || existing.contains(message.message_id.as_str()) {
                    continue;
                }
                let human = message.origin == "human";
                let id = loop {
                    next_id += 1;
                    let id = format!("{}-{}", if human { "human" } else { "agent" }, next_id);
                    if ids.insert(id.clone()) {
                        break id;
                    }
                };
                let baseline = project_comment(app, thread, message, id.clone(), record.generation);
                comments.push(baseline.clone());
                imported.insert(
                    id,
                    Imported {
                        thread_id: thread.thread_id.clone(),
                        message_id: message.message_id.clone(),
                        body: message.body.clone(),
                        human,
                        baseline,
                    },
                );
            }
        }
        // restore_comments validates before replacing the annotation store.
        // Commit identity allocation only after that validation succeeds.
        app.annotations_state
            .annotations
            .restore_comments(comments)
            .map_err(|_| io::Error::other("restored review exceeds terminal annotation limits"))?;
        self.imported = imported;
        self.next_ui_id = next_id;
        app.forget_all_annotation_layout();
        Ok(())
    }

    fn save(&mut self, app: &mut DiffApp) -> MarkResult<()> {
        if app.document.options != self.source_options {
            return Err(io::Error::other("source_changed: this review keeps its original comparison; reopen it from the harness to continue").into());
        }
        if let Some(conflict) = &self.sync_conflict {
            return Err(io::Error::other(conflict.clone()).into());
        }
        let current_signature = signature(app);
        if current_signature == self.last_signature && !self.needs_import {
            return Ok(());
        }
        let latest: ReviewRecord = serde_json::from_value(
            self.store
                .call("review.get", json!({"review_id":self.record.review_id}))?,
        )
        .map_err(io::Error::other)?;
        if latest.generation != self.record.generation {
            return Err(io::Error::other(
                "stale_generation: reopen the review before saving against a changed snapshot",
            )
            .into());
        }
        if latest.revision != self.record.revision || self.needs_import {
            if let Err(error) = self.import(app, Some(&latest)) {
                self.sync_conflict = Some(error.to_string());
                return Err(error);
            }
            self.needs_import = false;
        }
        self.record = latest;
        let patch =
            std::str::from_utf8(&app.document.changeset.raw_patch).map_err(io::Error::other)?;
        let changed = patch != self.record.snapshot.patch;
        if changed {
            self.mutate("review.refresh", json!({"patch":patch}))?;
            app.document.generation = self.record.generation;
        }
        let comments = app
            .annotations_state
            .annotations
            .comments()
            .cloned()
            .collect::<Vec<_>>();
        let live = comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect::<HashSet<_>>();
        let removed = self
            .imported
            .iter()
            .filter(|(id, value)| value.human && !live.contains(id.as_str()))
            .map(|(id, value)| (id.clone(), value.message_id.clone()))
            .collect::<Vec<_>>();
        for (id, message_id) in removed {
            self.mutate("comment.remove", json!({"message_id":message_id}))?;
            self.imported.remove(&id);
        }
        for comment in &comments {
            let anchor = anchors::to_protocol(app, &comment.anchor);
            if let Some(imported) = self.imported.get(&comment.id) {
                if imported.human && imported.body != comment.summary {
                    let message_id = imported.message_id.clone();
                    self.mutate(
                        "comment.edit",
                        json!({"message_id":message_id,"body":comment.summary}),
                    )?;
                    let imported = self
                        .imported
                        .get_mut(&comment.id)
                        .expect("imported comment exists");
                    imported.body = comment.summary.clone();
                    imported.baseline = comment.clone();
                }
            } else {
                let existing = self.record.threads.iter().find(|thread| {
                    serde_json::to_value(&thread.anchor).ok() == serde_json::to_value(&anchor).ok()
                });
                let origin = if comment.origin == CommentOrigin::Human {
                    "human"
                } else {
                    "agent"
                };
                let result = if let Some(thread) = existing {
                    if let Some(last) = thread
                        .messages
                        .iter()
                        .rev()
                        .find(|message| !message.deleted)
                    {
                        self.mutate("comment.reply",json!({"thread_id":thread.thread_id,"reply_to":last.message_id,"body":comment.summary,"origin":origin,"author":comment.author}))?
                    } else {
                        self.mutate("comment.add",json!({"anchor":anchor,"body":comment.summary,"origin":origin,"author":comment.author}))?
                    }
                } else {
                    self.mutate("comment.add",json!({"anchor":anchor,"body":comment.summary,"origin":origin,"author":comment.author}))?
                };
                let imported = Imported {
                    thread_id: result["thread"]["thread_id"]
                        .as_str()
                        .ok_or_else(|| io::Error::other("missing thread identity"))?
                        .to_owned(),
                    message_id: result["message"]["message_id"]
                        .as_str()
                        .ok_or_else(|| io::Error::other("missing message identity"))?
                        .to_owned(),
                    body: comment.summary.clone(),
                    human: comment.origin == CommentOrigin::Human,
                    baseline: comment.clone(),
                };
                self.imported.insert(comment.id.clone(), imported);
            }
        }
        let mut reanchored = Vec::new();
        let mut seen = HashSet::new();
        for comment in &comments {
            let Some(imported) = self.imported.get(&comment.id) else {
                continue;
            };
            if !seen.insert(imported.thread_id.clone()) {
                continue;
            }
            let stale = matches!(
                comment.lifecycle,
                CommentLifecycle::Stale | CommentLifecycle::Cleared
            );
            let anchor = if stale {
                // A display fallback is not evidence for changing a complete
                // durable hunk/range/deleted-line anchor.
                self.record
                    .threads
                    .iter()
                    .find(|thread| thread.thread_id == imported.thread_id)
                    .map(|thread| thread.anchor.clone())
                    .unwrap_or_else(|| anchors::to_protocol(app, &comment.anchor))
            } else {
                anchors::to_protocol(app, &comment.anchor)
            };
            if self.record.threads.iter().any(|thread| {
                thread.thread_id == imported.thread_id
                    && (thread.stale != stale
                        || serde_json::to_value(&thread.anchor).ok()
                            != serde_json::to_value(&anchor).ok())
            }) {
                reanchored
                    .push(json!({"thread_id":imported.thread_id,"anchor":anchor,"stale":stale}));
            }
            let disposition = disposition_name(comment.disposition);
            if self.record.threads.iter().any(|thread| {
                thread.thread_id == imported.thread_id && thread.disposition != disposition
            }) {
                self.mutate(
                    "review.disposition",
                    json!({"thread_id":imported.thread_id,"disposition":disposition}),
                )?;
            }
        }
        if !reanchored.is_empty() {
            self.mutate("review.reanchor", json!({"threads":reanchored}))?;
        }
        let progress = json!({"reviewed_files":app.annotations_state.lifecycle.reviewed_files,"reviewed_hunks":app.annotations_state.lifecycle.reviewed_hunks,"verdict":app.annotations_state.lifecycle.verdict});
        if serde_json::to_value(&self.record.progress).map_err(io::Error::other)? != progress {
            self.mutate("review.progress", progress)?;
        }
        self.last_signature = signature(app);
        Ok(())
    }
}

fn project_comment(
    app: &DiffApp,
    thread: &mark_session::review::ReviewThread,
    message: &mark_session::review::ReviewMessage,
    id: String,
    generation: u64,
) -> ReviewComment {
    let human = message.origin == "human";
    let (anchor, valid) = match anchors::validate(app, &thread.anchor) {
        Ok(key) => (key, true),
        Err(_) => (fallback_key(&thread.anchor), false),
    };
    let mut end = if human {
        message.body.len()
    } else {
        message.body.len().min(mark_session::MAX_SUMMARY_BYTES)
    };
    while !message.body.is_char_boundary(end) {
        end -= 1;
    }
    ReviewComment {
        id,
        anchor,
        summary: message.body[..end].to_owned(),
        rationale: if end < message.body.len() {
            Some(message.body[end..].to_owned())
        } else {
            None
        },
        author: message.author.clone(),
        origin: if human {
            CommentOrigin::Human
        } else {
            CommentOrigin::Agent
        },
        lifecycle: if thread.stale || !valid {
            CommentLifecycle::Stale
        } else {
            CommentLifecycle::Open
        },
        disposition: parse_disposition(&thread.disposition),
        document_generation: generation,
        original_anchor_evidence: None,
    }
}

fn is_stale(comment: &ReviewComment) -> bool {
    matches!(
        comment.lifecycle,
        CommentLifecycle::Stale | CommentLifecycle::Cleared
    )
}

fn comment_changed(base: &ReviewComment, other: &ReviewComment) -> bool {
    base.anchor != other.anchor
        || base.summary != other.summary
        || base.rationale != other.rationale
        || base.author != other.author
        || base.origin != other.origin
        || base.disposition != other.disposition
        || is_stale(base) != is_stale(other)
}

fn import_conflict(id: &str, detail: &str) -> io::Error {
    io::Error::other(format!(
        "review_conflict: comment {id} {detail}; reconcile the local draft before saving"
    ))
}

fn merge_field<T: Clone + PartialEq>(
    base: &T,
    local: &T,
    remote: &T,
    id: &str,
    field: &str,
) -> io::Result<T> {
    if local == base {
        return Ok(remote.clone());
    }
    if remote == base || local == remote {
        return Ok(local.clone());
    }
    Err(import_conflict(
        id,
        &format!("has conflicting local and remote {field}"),
    ))
}

fn merge_comment(
    base: &ReviewComment,
    local: &ReviewComment,
    remote: &ReviewComment,
) -> io::Result<ReviewComment> {
    let mut merged = remote.clone();
    merged.anchor = merge_field(
        &base.anchor,
        &local.anchor,
        &remote.anchor,
        &local.id,
        "anchors",
    )?;
    let (summary, rationale) = merge_field(
        &(base.summary.clone(), base.rationale.clone()),
        &(local.summary.clone(), local.rationale.clone()),
        &(remote.summary.clone(), remote.rationale.clone()),
        &local.id,
        "text",
    )?;
    merged.summary = summary;
    merged.rationale = rationale;
    merged.author = merge_field(
        &base.author,
        &local.author,
        &remote.author,
        &local.id,
        "authors",
    )?;
    merged.origin = merge_field(
        &base.origin,
        &local.origin,
        &remote.origin,
        &local.id,
        "origins",
    )?;
    merged.disposition = merge_field(
        &base.disposition,
        &local.disposition,
        &remote.disposition,
        &local.id,
        "dispositions",
    )?;
    let stale = merge_field(
        &is_stale(base),
        &is_stale(local),
        &is_stale(remote),
        &local.id,
        "lifecycle states",
    )?;
    merged.lifecycle = if stale {
        if local.lifecycle == CommentLifecycle::Cleared {
            CommentLifecycle::Cleared
        } else {
            CommentLifecycle::Stale
        }
    } else if merged.anchor != base.anchor || local.lifecycle == CommentLifecycle::Moved {
        CommentLifecycle::Moved
    } else {
        CommentLifecycle::Open
    };
    if merged.anchor == local.anchor {
        merged.original_anchor_evidence = local.original_anchor_evidence.clone();
    }
    Ok(merged)
}

fn fallback_key(anchor: &mark_session::ReviewAnchor) -> AnnotationKey {
    AnnotationKey {
        path: anchor.file.clone(),
        side: if anchor.old_line.is_some() {
            AnnotationSide::Old
        } else {
            AnnotationSide::New
        },
        line: anchor.old_line.or(anchor.new_line).unwrap_or(0),
        scope: if anchor.scope.is_some() {
            AnnotationScope::File
        } else {
            AnnotationScope::Line
        },
    }
}

fn signature(app: &DiffApp) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    app.document.changeset.raw_patch.as_ptr().hash(&mut hash);
    app.document.generation.hash(&mut hash);
    for comment in app.annotations_state.annotations.comments() {
        comment.id.hash(&mut hash);
        comment.anchor.hash(&mut hash);
        comment.summary.hash(&mut hash);
        (comment.disposition as u8).hash(&mut hash);
        (comment.lifecycle as u8).hash(&mut hash);
    }
    app.annotations_state
        .lifecycle
        .reviewed_files
        .hash(&mut hash);
    app.annotations_state
        .lifecycle
        .reviewed_hunks
        .hash(&mut hash);
    if let Some(verdict) = &app.annotations_state.lifecycle.verdict {
        (verdict.kind as u8).hash(&mut hash);
        verdict.summary.hash(&mut hash);
    }
    hash.finish()
}

fn parse_disposition(value: &str) -> FindingDisposition {
    match value {
        "accepted" => FindingDisposition::Accepted,
        "dismissed" => FindingDisposition::Dismissed,
        "blocking" => FindingDisposition::Blocking,
        "non_blocking" => FindingDisposition::NonBlocking,
        "fixed" => FindingDisposition::Fixed,
        _ => FindingDisposition::Open,
    }
}
fn disposition_name(value: FindingDisposition) -> &'static str {
    match value {
        FindingDisposition::Open => "open",
        FindingDisposition::Accepted => "accepted",
        FindingDisposition::Dismissed => "dismissed",
        FindingDisposition::Blocking => "blocking",
        FindingDisposition::NonBlocking => "non_blocking",
        FindingDisposition::Fixed => "fixed",
    }
}

#[cfg(test)]
mod tests;
