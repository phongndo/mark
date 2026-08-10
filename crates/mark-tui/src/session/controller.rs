use mark_session::{
    Capabilities, CommentAddParams, CommentApplyParams, CommentClearParams,
    CommentDispositionParams, CommentListParams, CommentRemoveParams, ContextParams, ContextResult,
    DEFAULT_CHANGED_FILES_PER_PAGE, EmptyParams, GenerationParams, MAX_CHANGED_FILES_PER_PAGE,
    METHOD_COMMENT_ADD, METHOD_COMMENT_APPLY, METHOD_COMMENT_CLEAR, METHOD_COMMENT_DISPOSITION,
    METHOD_COMMENT_LIST, METHOD_COMMENT_REMOVE, METHOD_CONTEXT_GET, METHOD_NAVIGATE,
    METHOD_PATCH_GET, METHOD_PROGRESS_SET, METHOD_RELOAD, METHOD_REVIEW_GET, METHOD_SESSION_GET,
    METHOD_VERDICT_CLEAR, METHOD_VERDICT_GET, METHOD_VERDICT_SET, NavigateParams, PatchParams,
    ProgressSetParams, ProtocolError, Request, Response, SessionCommand, SessionMetadata,
    VerdictSetParams, is_known_method,
};
use serde::de::DeserializeOwned;

use super::{
    RESPONSE_RESULT_BUDGET_BYTES, comments, navigation, reload, runtime::SessionRuntime, snapshot,
};
use crate::app::DiffApp;

const CONTEXT_CHANGED_FILES_BUDGET_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn handle(app: &mut DiffApp, runtime: &SessionRuntime, command: SessionCommand) {
    if command.request.method == METHOD_RELOAD {
        reload::handle(app, command);
        return;
    }
    let id = command.request.id.clone();
    let response = match dispatch(app, runtime, command.request) {
        Ok(result) => Response::success(id, result),
        Err(error) => Response::failure(id, error),
    };
    let _ = command.reply.send(response);
}

fn dispatch(
    app: &mut DiffApp,
    runtime: &SessionRuntime,
    request: Request,
) -> Result<serde_json::Value, ProtocolError> {
    match request.method.as_str() {
        METHOD_SESSION_GET => {
            let _: EmptyParams = params(&request)?;
            value(SessionMetadata {
                session_id: runtime.record.session_id.clone(),
                process_id: runtime.record.process_id,
                protocol: runtime.record.protocol,
                repository: runtime.record.repository.clone(),
                working_directory: runtime.record.working_directory.clone(),
                source: super::runtime::source_label(&app.document.options),
                document_generation: app.document.generation,
                source_changed: app.jobs.source_changed,
                capabilities: session_capabilities(app),
                responsive: true,
            })
        }
        METHOD_CONTEXT_GET => {
            let params: ContextParams = params(&request)?;
            let lifecycle = &app.annotations_state.lifecycle;
            let changed_files = changed_files_page(&lifecycle.changed_files, params)?;
            value(ContextResult {
                generation: app.document.generation,
                source_changed: app.jobs.source_changed,
                focus: snapshot::focus(app),
                comment_count: app.annotations_state.annotations.len(),
                pass: lifecycle.pass,
                moved_comment_count: app
                    .annotations_state
                    .annotations
                    .comments()
                    .filter(|comment| comment.lifecycle == crate::review::CommentLifecycle::Moved)
                    .count(),
                stale_comment_count: app
                    .annotations_state
                    .annotations
                    .comments()
                    .filter(|comment| comment.lifecycle == crate::review::CommentLifecycle::Stale)
                    .count(),
                cleared_comment_count: app
                    .annotations_state
                    .annotations
                    .comments()
                    .filter(|comment| comment.lifecycle == crate::review::CommentLifecycle::Cleared)
                    .count(),
                changed_file_count: lifecycle.changed_files.len(),
                changed_files: changed_files.files,
                changed_files_next_cursor: changed_files.next_cursor,
                reviewed_file_count: lifecycle.reviewed_files.len(),
                verdict: lifecycle.verdict.as_ref().map(comments::verdict_view),
            })
        }
        METHOD_REVIEW_GET => {
            let params: mark_session::ReviewParams = params(&request)?;
            let include_comments = params.include_comments;
            let comments_cursor = params.comments_cursor.clone();
            let comments_limit = params.comments_limit;
            let mut review = snapshot::review(app, params)?;
            if include_comments {
                review.comments = Some(Vec::new());
                let base_bytes = serde_json::to_vec(&review)
                    .map_err(|error| ProtocolError::new("internal_error", error.to_string()))?
                    .len();
                let comment_budget = RESPONSE_RESULT_BUDGET_BYTES
                    .checked_sub(base_bytes)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            "response_too_large",
                            "review structure exceeds the available response budget",
                        )
                    })?;
                let page = comments::page_all_views(
                    app,
                    comments_cursor.as_deref(),
                    comments_limit,
                    comment_budget,
                )?;
                review.comments = Some(page.comments);
                review.comments_next_cursor = page.next_cursor;
            }
            value(review)
        }
        METHOD_PATCH_GET => value(snapshot::session_patch(
            app,
            runtime,
            params::<PatchParams>(&request)?,
        )?),
        METHOD_COMMENT_ADD => value(comments::add(app, params::<CommentAddParams>(&request)?)?),
        METHOD_COMMENT_APPLY => value(comments::apply(
            app,
            params::<CommentApplyParams>(&request)?,
        )?),
        METHOD_COMMENT_LIST => value(comments::list(app, params::<CommentListParams>(&request)?)?),
        METHOD_COMMENT_REMOVE => value(comments::remove(
            app,
            params::<CommentRemoveParams>(&request)?,
        )?),
        METHOD_COMMENT_CLEAR => value(comments::clear(
            app,
            params::<CommentClearParams>(&request)?,
        )?),
        METHOD_COMMENT_DISPOSITION => value(comments::disposition(
            app,
            params::<CommentDispositionParams>(&request)?,
        )?),
        METHOD_PROGRESS_SET => value(comments::set_progress(
            app,
            params::<ProgressSetParams>(&request)?,
        )?),
        METHOD_VERDICT_GET => {
            let _: EmptyParams = params(&request)?;
            value(
                app.annotations_state
                    .lifecycle
                    .verdict
                    .as_ref()
                    .map(comments::verdict_view),
            )
        }
        METHOD_VERDICT_SET => value(comments::set_verdict(
            app,
            params::<VerdictSetParams>(&request)?,
        )?),
        METHOD_VERDICT_CLEAR => value(comments::clear_verdict(
            app,
            params::<GenerationParams>(&request)?,
        )?),
        METHOD_NAVIGATE => value(navigation::navigate(
            app,
            params::<NavigateParams>(&request)?,
        )?),
        method if !is_known_method(method) => Err(ProtocolError::new(
            "unknown_method",
            format!("unknown session method: {method}"),
        )),
        _ => Err(ProtocolError::new(
            "unknown_method",
            "unknown session method",
        )),
    }
}

fn session_capabilities(app: &DiffApp) -> Capabilities {
    Capabilities::v1(app.jobs.live_updates.enabled())
}

struct ChangedFilesPage {
    files: Vec<String>,
    next_cursor: Option<String>,
}

fn changed_files_page(
    changed_files: &std::collections::BTreeSet<String>,
    params: ContextParams,
) -> Result<ChangedFilesPage, ProtocolError> {
    let start = params
        .changed_files_cursor
        .as_deref()
        .map(|cursor| {
            cursor.parse::<usize>().map_err(|_| {
                ProtocolError::new(
                    "invalid_cursor",
                    "changed-files cursor is not a valid continuation token",
                )
            })
        })
        .transpose()?
        .unwrap_or_default();
    if start > changed_files.len() {
        return Err(ProtocolError::new(
            "invalid_cursor",
            "changed-files cursor is outside the changed file set",
        ));
    }
    let limit = params
        .changed_files_limit
        .unwrap_or(DEFAULT_CHANGED_FILES_PER_PAGE)
        .clamp(1, MAX_CHANGED_FILES_PER_PAGE);
    let mut files = Vec::with_capacity(limit.min(changed_files.len().saturating_sub(start)));
    let mut serialized_bytes = 0usize;
    for file in changed_files.iter().skip(start).take(limit) {
        let file_bytes = serde_json::to_vec(file)
            .map_err(|error| ProtocolError::new("internal_error", error.to_string()))?
            .len()
            .saturating_add(1);
        if !files.is_empty()
            && serialized_bytes.saturating_add(file_bytes) > CONTEXT_CHANGED_FILES_BUDGET_BYTES
        {
            break;
        }
        serialized_bytes = serialized_bytes.saturating_add(file_bytes);
        files.push(file.clone());
    }
    let end = start.saturating_add(files.len());
    Ok(ChangedFilesPage {
        files,
        next_cursor: (end < changed_files.len()).then(|| end.to_string()),
    })
}

fn params<T: DeserializeOwned>(request: &Request) -> Result<T, ProtocolError> {
    serde_json::from_value(request.params.clone()).map_err(|error| {
        ProtocolError::new(
            "invalid_params",
            format!("invalid parameters for {}: {error}", request.method),
        )
    })
}

fn value(value: impl serde::Serialize) -> Result<serde_json::Value, ProtocolError> {
    serde_json::to_value(value).map_err(|error| {
        ProtocolError::new(
            "internal_error",
            format!("could not encode session response: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use mark_diff::{Changeset, DiffOptions, RepoRoot};

    use crate::{app::LiveUpdatesState, controls::DiffLayoutMode};

    use super::*;

    #[test]
    fn session_capabilities_follow_the_current_live_update_state() {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut app = DiffApp::new(
            DiffOptions::default(),
            Changeset {
                repo: RepoRoot::new("/repo"),
                title: "test".to_owned(),
                files: mark_diff::parse_patch(patch),
                raw_patch: Arc::from(patch.as_bytes()),
            },
            DiffLayoutMode::Unified,
        );
        app.jobs.live_updates = LiveUpdatesState::from_allowed_and_enabled(true, true);
        assert!(session_capabilities(&app).automatic_reload);

        app.jobs.live_updates.set_user_enabled(false);

        assert!(!session_capabilities(&app).automatic_reload);
    }

    #[test]
    fn changed_file_pages_stop_before_the_context_byte_budget() {
        let changed_files = (0..MAX_CHANGED_FILES_PER_PAGE)
            .map(|index| format!("{index:04}-{}", "x".repeat(mark_session::MAX_PATH_BYTES)))
            .collect::<BTreeSet<_>>();
        let page = changed_files_page(
            &changed_files,
            ContextParams {
                changed_files_cursor: None,
                changed_files_limit: Some(MAX_CHANGED_FILES_PER_PAGE),
            },
        )
        .unwrap();
        let serialized_bytes = page
            .files
            .iter()
            .map(|file| serde_json::to_vec(file).unwrap().len().saturating_add(1))
            .sum::<usize>();

        assert!(serialized_bytes <= CONTEXT_CHANGED_FILES_BUDGET_BYTES);
        assert!(page.files.len() < changed_files.len());
        assert_eq!(page.next_cursor, Some(page.files.len().to_string()));
    }
}
