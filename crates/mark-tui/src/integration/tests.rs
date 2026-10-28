use super::*;
use crate::{app::SyntaxStartupMode, controls::DiffLayoutMode};
use mark_diff::{Changeset, DiffOptions, RepoRoot};

const PATCH: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,3 @@\n first\n-old\n+new\n last\n";

fn fixture() -> (tempfile::TempDir, ReviewStore, Value, DiffApp) {
    let directory = tempfile::tempdir().unwrap();
    let store = ReviewStore::new(directory.path().canonicalize().unwrap().join("state"));
    let record=store.call("review.open",json!({"harness":"test","conversation":"one","repository":directory.path(),"source":{"kind":"worktree"},"patch":PATCH})).unwrap();
    let app = app(PATCH);
    (directory, store, record, app)
}

fn app(patch: &str) -> DiffApp {
    DiffApp::new_with_syntax(
        DiffOptions::default(),
        Changeset {
            repo: RepoRoot::new("/repo"),
            title: "test".to_owned(),
            files: mark_diff::parse_patch(patch),
            raw_patch: std::sync::Arc::from(patch.as_bytes()),
        },
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
    )
}

#[test]
fn back_checkpoints_human_notes_and_reopen_restores_them() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    attach_with_store(&mut view, record.clone(), store.clone()).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key, "Please explain this change".to_owned(), 1)
        .unwrap();
    finish(&mut view).unwrap();
    let record = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(record["last_return"]["outcome"], "back");
    assert_eq!(record["threads"][0]["messages"][0]["origin"], "human");
    assert!(record["submissions"].as_array().unwrap().is_empty());
    let mut reopened = app(PATCH);
    attach_with_store(&mut reopened, record, store).unwrap();
    assert_eq!(reopened.annotations_state.annotations.len(), 1);
    assert_eq!(
        reopened
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap()
            .summary,
        "Please explain this change"
    );
}

#[test]
fn submit_returns_only_explicit_feedback_and_polls_agent_replies() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap().to_owned();
    attach_with_store(&mut view, record, store.clone()).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::Old, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key, "Why remove this?".to_owned(), 1)
        .unwrap();
    submit(&mut view).unwrap();
    finish(&mut view).unwrap();
    let record = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(record["last_return"]["outcome"], "submitted");
    assert_eq!(
        record["submissions"][0]["threads"][0]["anchor"]["old_line"],
        2
    );
    let thread = &record["threads"][0];
    store.call("comment.reply",json!({"review_id":id,"generation":1,"client_mutation_id":"reply","thread_id":thread["thread_id"],"reply_to":thread["messages"][0]["message_id"],"body":"The replacement handles this case.","origin":"agent","author":"test"})).unwrap();
    view.runtime.integration.as_mut().unwrap().last_poll = Instant::now() - Duration::from_secs(1);
    poll(&mut view);
    assert_eq!(view.annotations_state.annotations.len(), 2);
    assert_eq!(
        view.annotations_state
            .annotations
            .comments()
            .filter(|comment| comment.origin == CommentOrigin::Agent)
            .count(),
        1
    );
    checkpoint_or_notice(&mut view);
    assert_eq!(
        store.call("review.get", json!({"review_id":id})).unwrap()["threads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reload_persists_moved_anchor_using_existing_review_transition() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap().to_owned();
    attach_with_store(&mut view, record, store.clone()).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key, "Explain new".to_owned(), 1)
        .unwrap();
    checkpoint_or_notice(&mut view);
    let moved = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,4 @@\n+intro\n first\n-old\n+new\n last\n";
    let next = app(moved).document.changeset;
    view.replace_loaded_diff(DiffOptions::default(), next);
    with_integration(&mut view, |review, app| review.save(app)).unwrap();
    let record = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(record["generation"], 2);
    assert_eq!(record["threads"][0]["anchor"]["new_line"], 3);
    assert_eq!(record["threads"][0]["stale"], false);
}

#[test]
fn editing_or_removing_a_human_note_preserves_thread_identity() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap().to_owned();
    attach_with_store(&mut view, record, store.clone()).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key.clone(), "Draft".to_owned(), 1)
        .unwrap();
    with_integration(&mut view, |review, app| review.save(app)).unwrap();
    let first = store.call("review.get", json!({"review_id":id})).unwrap();
    view.annotations_state
        .annotations
        .insert_human(key.clone(), "Updated draft".to_owned(), 1)
        .unwrap();
    with_integration(&mut view, |review, app| review.save(app)).unwrap();
    let edited = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(
        first["threads"][0]["thread_id"],
        edited["threads"][0]["thread_id"]
    );
    assert_eq!(edited["threads"][0]["messages"][0]["body"], "Updated draft");
    view.annotations_state.annotations.clear_anchor(&key);
    with_integration(&mut view, |review, app| review.save(app)).unwrap();
    let removed = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(removed["threads"][0]["messages"][0]["deleted"], true);
}

#[test]
fn returning_an_already_submitted_review_is_immediate() {
    let (_directory, store, record, mut view) = fixture();
    attach_with_store(&mut view, record, store).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key, "Please explain".to_owned(), 1)
        .unwrap();
    assert!(has_feedback(&view));
    submit(&mut view).unwrap();
    assert!(!has_feedback(&view));
    let outcome = view
        .perform_app_action_with_effects(crate::app::AppAction::SubmitMarks)
        .unwrap();
    assert_eq!(outcome.handled_quit_request(), Some(true));
    assert_eq!(outcome.into_effects(), [crate::app::AppEffect::Quit]);
}

#[test]
fn a_source_switch_cannot_overwrite_the_bound_snapshot() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].clone();
    attach_with_store(&mut view, record, store.clone()).unwrap();
    view.document.options.source = mark_diff::DiffSource::Show("HEAD~1".into());
    let error = with_integration(&mut view, |review, app| review.save(app)).unwrap_err();
    assert!(error.to_string().contains("source_changed"));
    assert_eq!(
        store.call("review.get", json!({"review_id":id})).unwrap()["snapshot"]["patch"],
        PATCH
    );
}

#[test]
fn stale_display_fallbacks_preserve_the_complete_original_anchor() {
    for anchor in [
        json!({"file":"missing.rs","old_line":2,"new_line":5}),
        json!({"file":"missing.rs","hunk":9}),
        json!({"file":"missing.rs","range":{"old":{"start":2,"end":3},"new":{"start":5,"end":6}}}),
    ] {
        let (_directory, store, record, mut view) = fixture();
        let id = record["review_id"].clone();
        store.call("comment.add",json!({"review_id":id,"generation":1,"client_mutation_id":"stale","anchor":anchor,"body":"Keep the original target","origin":"human"})).unwrap();
        let record = store.call("review.get", json!({"review_id":id})).unwrap();
        attach_with_store(&mut view, record, store.clone()).unwrap();
        let key =
            AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
                .unwrap();
        view.annotations_state
            .annotations
            .insert_human(key, "Unrelated note".to_owned(), 1)
            .unwrap();
        with_integration(&mut view, |review, app| review.save(app)).unwrap();
        let stored = store.call("review.get", json!({"review_id":id})).unwrap();
        assert_eq!(stored["threads"][0]["anchor"], anchor);
        assert_eq!(stored["threads"][0]["stale"], true);
    }
}

fn remote_note(
    store: &ReviewStore,
    id: &str,
    mutation: &str,
    line: usize,
    body: &str,
    origin: &str,
) -> Value {
    store.call("comment.add",json!({"review_id":id,"generation":1,"client_mutation_id":mutation,"anchor":{"file":"a.rs","new_line":line},"body":body,"origin":origin})).unwrap()
}

fn force_poll(view: &mut DiffApp) {
    view.runtime.integration.as_mut().unwrap().last_poll = Instant::now() - Duration::from_secs(1);
    poll(view);
}

#[test]
fn polling_updates_remote_edits_dispositions_reanchors_and_deletions() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    let note = remote_note(&store, id, "seed", 2, "Original", "human");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store.clone(),
    )
    .unwrap();
    store.call("comment.edit",json!({"review_id":id,"generation":1,"client_mutation_id":"edit","message_id":note["message"]["message_id"],"body":"Changed in another view"})).unwrap();
    store.call("review.disposition",json!({"review_id":id,"generation":1,"client_mutation_id":"disposition","thread_id":note["thread"]["thread_id"],"disposition":"accepted"})).unwrap();
    store.call("review.reanchor",json!({"review_id":id,"generation":1,"client_mutation_id":"move","threads":[{"thread_id":note["thread"]["thread_id"],"anchor":{"file":"a.rs","new_line":3},"stale":false}]})).unwrap();
    force_poll(&mut view);
    let comment = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap();
    assert_eq!(comment.summary, "Changed in another view");
    assert_eq!(comment.anchor.line, 3);
    assert_eq!(comment.disposition, FindingDisposition::Accepted);
    assert_eq!(comment.lifecycle, CommentLifecycle::Moved);
    store.call("comment.remove",json!({"review_id":id,"generation":1,"client_mutation_id":"remove","message_id":note["message"]["message_id"]})).unwrap();
    force_poll(&mut view);
    assert_eq!(view.annotations_state.annotations.len(), 0);
    assert!(
        view.runtime
            .integration
            .as_ref()
            .unwrap()
            .imported
            .is_empty()
    );
    checkpoint_or_notice(&mut view);
    assert_eq!(
        store.call("review.get", json!({"review_id":id})).unwrap()["threads"][0]["messages"][0]["deleted"],
        true
    );
}

#[test]
fn polling_preserves_unsaved_local_text_while_importing_an_agent_answer() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    let note = remote_note(&store, id, "seed", 2, "Original", "human");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store.clone(),
    )
    .unwrap();
    let key = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap()
        .anchor
        .clone();
    view.annotations_state
        .annotations
        .insert_human(key, "Unsaved local correction".into(), 1)
        .unwrap();
    store.call("comment.reply",json!({"review_id":id,"generation":1,"client_mutation_id":"answer","thread_id":note["thread"]["thread_id"],"reply_to":note["message"]["message_id"],"origin":"agent","body":"External answer"})).unwrap();
    force_poll(&mut view);
    assert_eq!(view.annotations_state.annotations.len(), 2);
    assert!(
        view.annotations_state
            .annotations
            .comments()
            .any(|comment| comment.summary == "Unsaved local correction")
    );
    checkpoint_or_notice(&mut view);
    let stored = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(
        stored["threads"][0]["messages"][0]["body"],
        "Unsaved local correction"
    );
    assert_eq!(
        stored["threads"][0]["messages"][1]["body"],
        "External answer"
    );
}

#[test]
fn conflicting_local_and_remote_edits_are_transactional_and_retryable() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    let note = remote_note(&store, id, "seed", 2, "Original", "human");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store.clone(),
    )
    .unwrap();
    let before = view.runtime.integration.as_ref().unwrap().record.revision;
    let key = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap()
        .anchor
        .clone();
    view.annotations_state
        .annotations
        .insert_human(key.clone(), "Local draft".into(), 1)
        .unwrap();
    store.call("comment.edit",json!({"review_id":id,"generation":1,"client_mutation_id":"remote-edit","message_id":note["message"]["message_id"],"body":"Remote draft"})).unwrap();
    let remote_revision =
        store.call("review.get", json!({"review_id":id})).unwrap()["revision"].clone();
    force_poll(&mut view);
    let integration = view.runtime.integration.as_ref().unwrap();
    assert_eq!(integration.record.revision, before);
    assert_eq!(
        integration.imported.values().next().unwrap().body,
        "Original"
    );
    assert!(
        integration
            .sync_conflict
            .as_ref()
            .unwrap()
            .contains("review_conflict")
    );
    assert_eq!(
        view.annotations_state
            .annotations
            .comments()
            .next()
            .unwrap()
            .summary,
        "Local draft"
    );
    checkpoint_or_notice(&mut view);
    let still_remote = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(still_remote["revision"], remote_revision);
    assert_eq!(
        still_remote["threads"][0]["messages"][0]["body"],
        "Remote draft"
    );
    // Reconcile the text locally; the unchanged remote revision is retried.
    view.annotations_state
        .annotations
        .insert_human(key, "Remote draft".into(), 1)
        .unwrap();
    force_poll(&mut view);
    assert!(
        view.runtime
            .integration
            .as_ref()
            .unwrap()
            .sync_conflict
            .is_none()
    );
    assert_eq!(
        view.runtime.integration.as_ref().unwrap().record.revision,
        remote_revision.as_u64().unwrap()
    );
}

#[test]
fn polling_preserves_an_unsaved_local_deletion_during_an_unrelated_remote_addition() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    remote_note(&store, id, "seed", 2, "Delete locally", "human");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store.clone(),
    )
    .unwrap();
    let key = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap()
        .anchor
        .clone();
    view.annotations_state.annotations.clear_anchor(&key);
    remote_note(&store, id, "remote-add", 3, "Independent note", "human");
    force_poll(&mut view);
    assert_eq!(view.annotations_state.annotations.len(), 1);
    assert_eq!(
        view.annotations_state
            .annotations
            .comments()
            .next()
            .unwrap()
            .summary,
        "Independent note"
    );
    checkpoint_or_notice(&mut view);
    let stored = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(stored["threads"][0]["messages"][0]["deleted"], true);
    assert_eq!(
        stored["threads"][1]["messages"][0]["body"],
        "Independent note"
    );
}

#[test]
fn remote_human_import_allocates_around_locally_created_ids() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap().to_owned();
    attach_with_store(&mut view, record, store.clone()).unwrap();
    let key =
        AnnotationKey::for_file_line(&view.document.changeset.files[0], AnnotationSide::New, 2)
            .unwrap();
    view.annotations_state
        .annotations
        .insert_human(key, "Local human-1".into(), 1)
        .unwrap();
    checkpoint_or_notice(&mut view);
    remote_note(&store, &id, "remote-human", 3, "Remote human", "human");
    force_poll(&mut view);
    let comments = view
        .annotations_state
        .annotations
        .comments()
        .collect::<Vec<_>>();
    assert_eq!(comments.len(), 2);
    assert_ne!(comments[0].id, comments[1].id);
    assert!(
        view.runtime
            .integration
            .as_ref()
            .unwrap()
            .sync_conflict
            .is_none()
    );
    assert_eq!(view.runtime.integration.as_ref().unwrap().imported.len(), 2);
}

#[test]
fn a_full_size_agent_reply_preserves_utf8_text_across_summary_and_rationale() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    let body = format!(
        "{}🦀{}",
        "x".repeat(mark_session::MAX_SUMMARY_BYTES - 1),
        "y".repeat(32 * 1024 - mark_session::MAX_SUMMARY_BYTES - 3)
    );
    assert_eq!(body.len(), 32 * 1024);
    remote_note(&store, id, "long-agent", 2, &body, "agent");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store,
    )
    .unwrap();
    let comment = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap();
    assert!(comment.summary.len() <= mark_session::MAX_SUMMARY_BYTES);
    assert_eq!(
        format!(
            "{}{}",
            comment.summary,
            comment.rationale.as_deref().unwrap()
        ),
        body
    );
    assert_eq!(
        view.runtime
            .integration
            .as_ref()
            .unwrap()
            .imported
            .values()
            .next()
            .unwrap()
            .body,
        body
    );
}

#[test]
fn checkpoint_detects_a_remote_conflict_before_the_next_poll() {
    let (_directory, store, record, mut view) = fixture();
    let id = record["review_id"].as_str().unwrap();
    let note = remote_note(&store, id, "seed", 2, "Original", "human");
    attach_with_store(
        &mut view,
        store.call("review.get", json!({"review_id":id})).unwrap(),
        store.clone(),
    )
    .unwrap();
    let key = view
        .annotations_state
        .annotations
        .comments()
        .next()
        .unwrap()
        .anchor
        .clone();
    view.annotations_state
        .annotations
        .insert_human(key, "Local draft".into(), 1)
        .unwrap();
    store.call("comment.edit",json!({"review_id":id,"generation":1,"client_mutation_id":"racing-edit","message_id":note["message"]["message_id"],"body":"Remote draft"})).unwrap();
    let result = with_integration(&mut view, |review, app| review.save(app));
    assert!(
        result.is_err(),
        "checkpoint must merge remote revisions before writing a conflicting local edit"
    );
    let stored = store.call("review.get", json!({"review_id":id})).unwrap();
    assert_eq!(stored["threads"][0]["messages"][0]["body"], "Remote draft");
}
