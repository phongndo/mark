use std::collections::HashSet;

use mark_session::{
    CommentAddParams, CommentApplyParams, CommentClearParams, CommentDispositionParams,
    CommentInput, CommentListParams, CommentListResult, CommentMutationResult,
    CommentOrigin as ProtocolOrigin, CommentRemovalResult, CommentRemoveParams, CommentView,
    DEFAULT_COMMENTS_PER_PAGE, GenerationParams, MAX_COMMENTS_PER_PAGE, ProgressResult,
    ProgressSetParams, ProtocolError, VerdictSetParams, VerdictView,
};

use crate::{
    annotation::AnnotationKey,
    app::DiffApp,
    review::{
        CommentOrigin, FinalVerdict, NewAgentComment, ReviewComment, VerdictDestination,
        VerdictKind,
    },
};

use super::{COMMENT_PAGE_RESERVE_BYTES, RESPONSE_RESULT_BUDGET_BYTES, anchors};

pub(crate) struct CommentPage {
    pub(crate) comments: Vec<CommentView>,
    pub(crate) next_cursor: Option<String>,
}

pub(crate) fn add(
    app: &mut DiffApp,
    params: CommentAddParams,
) -> Result<CommentMutationResult, ProtocolError> {
    apply(
        app,
        CommentApplyParams {
            generation: params.generation,
            comments: vec![params.comment],
            focus: params.focus,
        },
    )
}

pub(crate) fn apply(
    app: &mut DiffApp,
    params: CommentApplyParams,
) -> Result<CommentMutationResult, ProtocolError> {
    require_generation(app, params.generation)?;
    if params.comments.is_empty() || params.comments.len() > mark_session::MAX_COMMENTS_PER_BATCH {
        return Err(ProtocolError::new(
            "comment_batch_limit",
            format!(
                "comment batch must contain between 1 and {} comments",
                mark_session::MAX_COMMENTS_PER_BATCH
            ),
        ));
    }
    if app
        .annotations_state
        .annotations
        .len()
        .saturating_add(params.comments.len())
        > mark_session::MAX_LIVE_COMMENTS
    {
        return Err(ProtocolError::new(
            "comment_limit",
            "live comment limit would be exceeded",
        ));
    }

    let mut validated = Vec::with_capacity(params.comments.len());
    for comment in params.comments {
        validated.push(validate_comment(app, comment)?);
    }
    let first_anchor = validated.first().map(|comment| comment.anchor.clone());
    let affected = validated
        .iter()
        .map(|comment| comment.anchor.clone())
        .collect::<HashSet<_>>();
    let ids = app
        .annotations_state
        .annotations
        .insert_agent_batch(validated, params.generation)
        .map_err(|_: crate::review::StoreLimitError| {
            ProtocolError::new(
                "comment_limit",
                "comment batch exceeds the review store limits",
            )
        })?;
    invalidate_comment_geometry(app, affected);
    if params.focus
        && let Some(anchor) = first_anchor
    {
        app.jump_to_annotation(&anchor);
    }
    app.runtime.dirty = true;
    Ok(CommentMutationResult {
        generation: app.document.generation,
        ids,
    })
}

pub(crate) fn list(
    app: &DiffApp,
    params: CommentListParams,
) -> Result<CommentListResult, ProtocolError> {
    if params
        .file
        .as_ref()
        .is_some_and(|file| file.len() > mark_session::MAX_PATH_BYTES)
    {
        return Err(ProtocolError::new(
            "invalid_path",
            "comment path exceeds the byte limit",
        ));
    }
    let page = page_views(
        app,
        params.file.as_deref(),
        params.origin,
        params.cursor.as_deref(),
        params.limit,
        RESPONSE_RESULT_BUDGET_BYTES.saturating_sub(COMMENT_PAGE_RESERVE_BYTES),
    )?;
    Ok(CommentListResult {
        generation: app.document.generation,
        comments: page.comments,
        next_cursor: page.next_cursor,
    })
}

pub(crate) fn page_all_views(
    app: &DiffApp,
    cursor: Option<&str>,
    limit: Option<usize>,
    max_serialized_bytes: usize,
) -> Result<CommentPage, ProtocolError> {
    page_views(
        app,
        None,
        ProtocolOrigin::All,
        cursor,
        limit,
        max_serialized_bytes,
    )
}

fn page_views(
    app: &DiffApp,
    file: Option<&str>,
    origin: ProtocolOrigin,
    cursor: Option<&str>,
    limit: Option<usize>,
    max_serialized_bytes: usize,
) -> Result<CommentPage, ProtocolError> {
    let start = parse_cursor(cursor)?;
    let limit = limit
        .unwrap_or(DEFAULT_COMMENTS_PER_PAGE)
        .clamp(1, MAX_COMMENTS_PER_PAGE);
    let total = app
        .annotations_state
        .annotations
        .comments()
        .filter(|comment| comment_matches(comment, file, origin))
        .count();
    if start > total {
        return Err(ProtocolError::new(
            "invalid_cursor",
            "comment cursor is outside the filtered comment list",
        ));
    }

    let mut comments = Vec::with_capacity(limit.min(total.saturating_sub(start)));
    let mut serialized_bytes = 0usize;
    for comment in app
        .annotations_state
        .annotations
        .comments()
        .filter(|comment| comment_matches(comment, file, origin))
        .skip(start)
        .take(limit)
    {
        let view = comment_view(app, comment);
        let view_bytes = serde_json::to_vec(&view)
            .map_err(|error| ProtocolError::new("internal_error", error.to_string()))?
            .len()
            .saturating_add(usize::from(!comments.is_empty()));
        if serialized_bytes.saturating_add(view_bytes) > max_serialized_bytes {
            if comments.is_empty() {
                return Err(ProtocolError::new(
                    "response_too_large",
                    "one comment does not fit in the available response budget",
                ));
            }
            break;
        }
        serialized_bytes = serialized_bytes.saturating_add(view_bytes);
        comments.push(view);
    }
    let end = start.saturating_add(comments.len());
    Ok(CommentPage {
        comments,
        next_cursor: (end < total).then(|| end.to_string()),
    })
}

fn comment_matches(
    comment: &ReviewComment,
    file: Option<&str>,
    requested_origin: ProtocolOrigin,
) -> bool {
    file.is_none_or(|file| comment.anchor.path == file)
        && match requested_origin {
            ProtocolOrigin::All => true,
            ProtocolOrigin::Agent => comment.origin == CommentOrigin::Agent,
            ProtocolOrigin::Human => comment.origin == CommentOrigin::Human,
        }
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ProtocolError> {
    cursor
        .map(|cursor| {
            cursor.parse::<usize>().map_err(|_| {
                ProtocolError::new("invalid_cursor", "cursor is not a valid continuation token")
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn remove(
    app: &mut DiffApp,
    params: CommentRemoveParams,
) -> Result<CommentRemovalResult, ProtocolError> {
    require_generation(app, params.generation)?;
    let anchor = app
        .annotations_state
        .annotations
        .comments()
        .find(|comment| comment.id == params.id)
        .map(|comment| comment.anchor.clone());
    let removed = match app
        .annotations_state
        .annotations
        .remove_agent_by_id(&params.id)
    {
        Ok(true) => 1,
        Ok(false) => {
            return Err(ProtocolError::new(
                "comment_not_found",
                format!("comment does not exist: {}", params.id),
            ));
        }
        Err(()) => {
            return Err(ProtocolError::new(
                "human_comment_protected",
                "session commands cannot remove human comments",
            ));
        }
    };
    invalidate_comment_geometry(app, anchor.into_iter().collect());
    app.runtime.dirty = true;
    Ok(CommentRemovalResult {
        generation: app.document.generation,
        removed,
    })
}

pub(crate) fn clear(
    app: &mut DiffApp,
    params: CommentClearParams,
) -> Result<CommentRemovalResult, ProtocolError> {
    require_generation(app, params.generation)?;
    if params
        .file
        .as_ref()
        .is_some_and(|file| file.len() > mark_session::MAX_PATH_BYTES)
    {
        return Err(ProtocolError::new(
            "invalid_path",
            "comment path exceeds the byte limit",
        ));
    }
    let affected = app
        .annotations_state
        .annotations
        .comments()
        .filter(|comment| {
            comment.origin == CommentOrigin::Agent
                && params
                    .file
                    .as_ref()
                    .is_none_or(|file| comment.anchor.path == *file)
        })
        .map(|comment| comment.anchor.clone())
        .collect::<HashSet<_>>();
    let removed = app
        .annotations_state
        .annotations
        .clear_agents(params.file.as_deref());
    if removed > 0 {
        invalidate_comment_geometry(app, affected);
        app.runtime.dirty = true;
    }
    Ok(CommentRemovalResult {
        generation: app.document.generation,
        removed,
    })
}

pub(crate) fn disposition(
    app: &mut DiffApp,
    params: CommentDispositionParams,
) -> Result<CommentMutationResult, ProtocolError> {
    require_generation(app, params.generation)?;
    let anchor = app
        .annotations_state
        .annotations
        .comments()
        .find(|comment| comment.id == params.id)
        .map(|comment| comment.anchor.clone())
        .ok_or_else(|| ProtocolError::new("comment_not_found", "comment does not exist"))?;
    app.annotations_state
        .annotations
        .set_disposition(&params.id, disposition_from_protocol(params.disposition))
        .map_err(|()| {
            ProtocolError::new(
                "human_comment_protected",
                "only agent findings can receive a disposition",
            )
        })?;
    invalidate_comment_geometry(app, [anchor].into_iter().collect());
    app.runtime.dirty = true;
    Ok(CommentMutationResult {
        generation: app.document.generation,
        ids: vec![params.id],
    })
}

pub(crate) fn set_progress(
    app: &mut DiffApp,
    params: ProgressSetParams,
) -> Result<ProgressResult, ProtocolError> {
    require_generation(app, params.generation)?;
    if params.file.is_empty() || params.file.len() > mark_session::MAX_PATH_BYTES {
        return Err(ProtocolError::new("invalid_path", "invalid progress path"));
    }
    let file = app
        .document
        .changeset
        .files
        .iter()
        .find(|file| file.display_path() == params.file)
        .ok_or_else(|| ProtocolError::new("path_not_found", "file is not in the changeset"))?;
    if let Some(hunk) = params.hunk {
        if hunk == 0 || hunk > file.hunks().len() {
            return Err(ProtocolError::new(
                "anchor_not_found",
                "hunk does not exist",
            ));
        }
        app.annotations_state
            .lifecycle
            .set_hunk_reviewed(&params.file, hunk, params.reviewed);
    } else {
        app.annotations_state
            .lifecycle
            .set_file_reviewed(&params.file, params.reviewed);
    }
    app.runtime.dirty = true;
    Ok(ProgressResult {
        generation: app.document.generation,
        reviewed_files: app.annotations_state.lifecycle.reviewed_files.len(),
        reviewed_hunks: app.annotations_state.lifecycle.reviewed_hunks.len(),
    })
}

pub(crate) fn set_verdict(
    app: &mut DiffApp,
    params: VerdictSetParams,
) -> Result<VerdictView, ProtocolError> {
    require_generation(app, params.generation)?;
    if params
        .summary
        .as_ref()
        .is_some_and(|summary| summary.len() > mark_session::MAX_SUMMARY_BYTES)
    {
        return Err(ProtocolError::new(
            "verdict_invalid",
            "verdict summary exceeds the byte limit",
        ));
    }
    let verdict = FinalVerdict {
        kind: match params.kind {
            mark_session::VerdictKind::Approve => VerdictKind::Approve,
            mark_session::VerdictKind::RequestChanges => VerdictKind::RequestChanges,
            mark_session::VerdictKind::Comment => VerdictKind::Comment,
        },
        summary: params.summary,
        destination: match params.destination {
            mark_session::VerdictDestination::Local => VerdictDestination::Local,
            mark_session::VerdictDestination::Stdout => VerdictDestination::Stdout,
        },
    };
    app.annotations_state.lifecycle.verdict = Some(verdict.clone());
    app.runtime.dirty = true;
    Ok(verdict_view(&verdict))
}

pub(crate) fn clear_verdict(
    app: &mut DiffApp,
    params: GenerationParams,
) -> Result<serde_json::Value, ProtocolError> {
    require_generation(app, params.generation)?;
    app.annotations_state.lifecycle.verdict = None;
    app.runtime.dirty = true;
    Ok(serde_json::json!({
        "generation": app.document.generation,
        "verdict": null
    }))
}

pub(crate) fn verdict_view(verdict: &FinalVerdict) -> VerdictView {
    VerdictView {
        kind: match verdict.kind {
            VerdictKind::Approve => mark_session::VerdictKind::Approve,
            VerdictKind::RequestChanges => mark_session::VerdictKind::RequestChanges,
            VerdictKind::Comment => mark_session::VerdictKind::Comment,
        },
        summary: verdict.summary.clone(),
        destination: match verdict.destination {
            VerdictDestination::Local => mark_session::VerdictDestination::Local,
            VerdictDestination::Stdout => mark_session::VerdictDestination::Stdout,
        },
    }
}

fn disposition_from_protocol(
    disposition: mark_session::FindingDisposition,
) -> crate::review::FindingDisposition {
    match disposition {
        mark_session::FindingDisposition::Open => crate::review::FindingDisposition::Open,
        mark_session::FindingDisposition::Accepted => crate::review::FindingDisposition::Accepted,
        mark_session::FindingDisposition::Dismissed => crate::review::FindingDisposition::Dismissed,
        mark_session::FindingDisposition::Blocking => crate::review::FindingDisposition::Blocking,
        mark_session::FindingDisposition::NonBlocking => {
            crate::review::FindingDisposition::NonBlocking
        }
        mark_session::FindingDisposition::Fixed => crate::review::FindingDisposition::Fixed,
    }
}

fn validate_comment(
    app: &DiffApp,
    comment: CommentInput,
) -> Result<NewAgentComment, ProtocolError> {
    validate_text(
        "summary",
        &comment.summary,
        mark_session::MAX_SUMMARY_BYTES,
        false,
    )?;
    if let Some(rationale) = comment.rationale.as_deref() {
        validate_text(
            "rationale",
            rationale,
            mark_session::MAX_RATIONALE_BYTES,
            true,
        )?;
    }
    if let Some(author) = comment.author.as_deref() {
        validate_text("author", author, mark_session::MAX_AUTHOR_BYTES, true)?;
    }
    let anchor = anchors::validate(app, &comment.anchor)?;
    let anchor = app.annotations_state.annotations.canonical_anchor(&anchor);
    Ok(NewAgentComment {
        anchor,
        summary: comment.summary,
        rationale: comment.rationale,
        author: comment.author,
    })
}

fn validate_text(
    field: &str,
    text: &str,
    max_bytes: usize,
    empty_allowed: bool,
) -> Result<(), ProtocolError> {
    if (!empty_allowed && text.trim().is_empty()) || text.len() > max_bytes {
        return Err(ProtocolError::new(
            "comment_payload_invalid",
            format!("{field} is empty or exceeds its byte limit"),
        ));
    }
    Ok(())
}

fn require_generation(app: &DiffApp, generation: u64) -> Result<(), ProtocolError> {
    if generation == app.document.generation {
        return Ok(());
    }
    Err(ProtocolError::new(
        "stale_generation",
        format!(
            "request generation {generation} does not match current generation {}",
            app.document.generation
        ),
    ))
}

fn comment_view(app: &DiffApp, comment: &ReviewComment) -> CommentView {
    CommentView {
        id: comment.id.clone(),
        anchor: anchors::to_protocol(app, &comment.anchor),
        summary: comment.summary.clone(),
        rationale: comment.rationale.clone(),
        author: comment.author.clone(),
        origin: match comment.origin {
            CommentOrigin::Human => ProtocolOrigin::Human,
            CommentOrigin::Agent => ProtocolOrigin::Agent,
        },
        lifecycle: match comment.lifecycle {
            crate::review::CommentLifecycle::Open => mark_session::CommentLifecycle::Open,
            crate::review::CommentLifecycle::Moved => mark_session::CommentLifecycle::Moved,
            crate::review::CommentLifecycle::Stale => mark_session::CommentLifecycle::Stale,
            crate::review::CommentLifecycle::Cleared => mark_session::CommentLifecycle::Cleared,
        },
        disposition: match comment.disposition {
            crate::review::FindingDisposition::Open => mark_session::FindingDisposition::Open,
            crate::review::FindingDisposition::Accepted => {
                mark_session::FindingDisposition::Accepted
            }
            crate::review::FindingDisposition::Dismissed => {
                mark_session::FindingDisposition::Dismissed
            }
            crate::review::FindingDisposition::Blocking => {
                mark_session::FindingDisposition::Blocking
            }
            crate::review::FindingDisposition::NonBlocking => {
                mark_session::FindingDisposition::NonBlocking
            }
            crate::review::FindingDisposition::Fixed => mark_session::FindingDisposition::Fixed,
        },
        document_generation: comment.document_generation,
    }
}

fn invalidate_comment_geometry(app: &mut DiffApp, anchors: HashSet<AnnotationKey>) {
    app.annotations_state.annotation_block_scroll = None;
    *app.annotations_state.annotation_keys_by_row.borrow_mut() = None;
    let mut rows = app.annotations_state.annotation_rows.borrow_mut();
    let mut heights = app.annotations_state.annotation_heights.borrow_mut();
    for anchor in anchors {
        rows.remove(&anchor);
        heights.remove(&anchor);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mark_diff::{Changeset, DiffOptions, RepoRoot};
    use mark_session::{CommentApplyParams, CommentInput, ReviewAnchor};

    use crate::{app::DiffApp, controls::DiffLayoutMode, render::diff::build_diff_viewport_lines};

    use super::*;

    fn app() -> DiffApp {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        DiffApp::new(
            DiffOptions::default(),
            Changeset {
                repo: RepoRoot::new("/repo"),
                title: "test".to_owned(),
                files: mark_diff::parse_patch(patch),
                raw_patch: Arc::from(patch.as_bytes()),
            },
            DiffLayoutMode::Unified,
        )
    }

    fn comment(line: usize) -> CommentInput {
        CommentInput {
            anchor: ReviewAnchor {
                file: "src/lib.rs".to_owned(),
                scope: None,
                hunk: None,
                old_line: None,
                new_line: Some(line),
                range: None,
            },
            summary: "finding".to_owned(),
            rationale: None,
            author: Some("agent".to_owned()),
        }
    }

    #[test]
    fn invalid_batch_does_not_partially_mutate_comments() {
        let mut app = app();
        let result = apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: vec![comment(1), comment(999)],
                focus: false,
            },
        );

        assert!(result.is_err());
        assert!(app.annotations_state.annotations.is_empty());
    }

    #[test]
    fn stale_batch_and_agent_clear_preserve_human_comments() {
        let mut app = app();
        let key = anchors::validate(&app, &comment(1).anchor).unwrap();
        app.annotations_state
            .annotations
            .insert_human(key, "human".to_owned(), 0)
            .unwrap();

        let stale = apply(
            &mut app,
            CommentApplyParams {
                generation: 99,
                comments: vec![comment(1)],
                focus: false,
            },
        )
        .unwrap_err();
        assert_eq!(stale.code, "stale_generation");

        let cleared = clear(
            &mut app,
            CommentClearParams {
                generation: 0,
                file: None,
            },
        )
        .unwrap();
        assert_eq!(cleared.removed, 0);
        assert_eq!(app.annotations_state.annotations.len(), 1);
    }

    #[test]
    fn stale_hunk_comments_keep_their_original_ranges_in_comment_list() {
        let mut app = app();
        let anchor = AnnotationKey::for_hunk(
            &app.document.changeset.files[0],
            &app.document.changeset.files[0].hunks()[0],
        )
        .unwrap();
        app.annotations_state
            .annotations
            .restore_comments(vec![ReviewComment {
                id: "agent-1".to_owned(),
                anchor,
                summary: "stale hunk".to_owned(),
                rationale: None,
                author: None,
                origin: CommentOrigin::Agent,
                lifecycle: crate::review::CommentLifecycle::Stale,
                disposition: crate::review::FindingDisposition::Open,
                document_generation: 0,
                original_anchor_evidence: None,
            }])
            .unwrap();
        let changed_patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -2 +2 @@\n-old\n+new\n";
        app.document.changeset.files = mark_diff::parse_patch(changed_patch);

        let result = list(&app, CommentListParams::default()).unwrap();

        assert_eq!(result.comments.len(), 1);
        assert_eq!(
            result.comments[0].lifecycle,
            mark_session::CommentLifecycle::Stale
        );
        assert_eq!(result.comments[0].anchor.hunk, None);
        assert_eq!(
            result.comments[0].anchor.range,
            Some(mark_session::RangeTarget {
                old: Some(mark_session::SourceRange { start: 1, end: 1 }),
                new: Some(mark_session::SourceRange { start: 1, end: 1 }),
            })
        );
    }

    #[test]
    fn batch_inserts_multiple_comments_at_one_anchor_in_input_order() {
        let mut app = app();
        let result = apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: vec![comment(1), comment(1)],
                focus: false,
            },
        )
        .unwrap();

        assert_eq!(result.ids, ["agent-1", "agent-2"]);
        assert_eq!(app.annotations_state.annotations.len(), 2);
        assert_eq!(app.annotations_state.annotations.anchor_count(), 1);
        let rendered = build_diff_viewport_lines(&mut app, 80, 20)
            .into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content.into_owned())
            .collect::<String>();
        assert!(rendered.contains("2 comments"));
        assert!(rendered.contains("Agent (agent): finding"));
    }

    #[test]
    fn agent_author_controls_are_escaped_in_the_card_title() {
        let mut app = app();
        let mut input = comment(1);
        input.author = Some("unsafe\u{1b}]52;c;payload\u{7}".to_owned());
        apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: vec![input],
                focus: false,
            },
        )
        .unwrap();

        let rendered = build_diff_viewport_lines(&mut app, 120, 20)
            .into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content.into_owned())
            .collect::<String>();
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains("unsafe\\u{1b}]52;c;payload\\u{7}"));
    }

    #[test]
    fn agent_summary_controls_are_escaped_in_the_annotation_menu() {
        let mut app = app();
        let mut input = comment(1);
        input.summary = "unsafe\u{1b}]52;c;payload\u{7} and \u{1b}[2J".to_owned();
        apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: vec![input],
                focus: false,
            },
        )
        .unwrap();
        app.open_annotation_menu();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 20)).unwrap();
        terminal
            .draw(|frame| crate::render::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .flat_map(|y| {
                (0..buffer.area.width).map(move |x| {
                    buffer
                        .cell((x, y))
                        .expect("menu cell should exist")
                        .symbol()
                })
            })
            .collect::<String>();

        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains("unsafe\\u{1b}]52;c;payload\\u{7} and \\u{1b}[2J"));
    }

    #[test]
    fn comment_list_returns_stable_bounded_pages() {
        let mut app = app();
        apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: vec![comment(1), comment(1)],
                focus: false,
            },
        )
        .unwrap();

        let first_view = comment_view(
            &app,
            app.annotations_state.annotations.comments().next().unwrap(),
        );
        let byte_budget = serde_json::to_vec(&first_view).unwrap().len();
        let byte_page = page_all_views(&app, None, Some(2), byte_budget).unwrap();
        assert_eq!(byte_page.comments.len(), 1);
        assert_eq!(byte_page.next_cursor.as_deref(), Some("1"));

        let first = list(
            &app,
            CommentListParams {
                limit: Some(1),
                ..CommentListParams::default()
            },
        )
        .unwrap();
        assert_eq!(first.comments[0].id, "agent-1");
        assert_eq!(first.next_cursor.as_deref(), Some("1"));

        let second = list(
            &app,
            CommentListParams {
                cursor: first.next_cursor,
                limit: Some(1),
                ..CommentListParams::default()
            },
        )
        .unwrap();
        assert_eq!(second.comments[0].id, "agent-2");
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn comment_batches_cannot_exceed_the_live_review_budget() {
        let mut app = app();
        let mut large_comment = comment(1);
        large_comment.rationale = Some("x".repeat(mark_session::MAX_RATIONALE_BYTES));
        let batch = vec![large_comment; 25];

        for _ in 0..2 {
            apply(
                &mut app,
                CommentApplyParams {
                    generation: 0,
                    comments: batch.clone(),
                    focus: false,
                },
            )
            .unwrap();
        }
        let error = apply(
            &mut app,
            CommentApplyParams {
                generation: 0,
                comments: batch,
                focus: false,
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "comment_limit");
        assert_eq!(app.annotations_state.annotations.len(), 50);
    }

    #[test]
    fn lifecycle_session_mutations_request_a_redraw() {
        let mut app = app();
        app.runtime.dirty = false;
        set_progress(
            &mut app,
            ProgressSetParams {
                generation: 0,
                file: "src/lib.rs".to_owned(),
                hunk: None,
                reviewed: true,
            },
        )
        .unwrap();
        assert!(app.runtime.dirty);

        app.runtime.dirty = false;
        set_verdict(
            &mut app,
            VerdictSetParams {
                generation: 0,
                kind: mark_session::VerdictKind::Approve,
                summary: None,
                destination: mark_session::VerdictDestination::Local,
            },
        )
        .unwrap();
        assert!(app.runtime.dirty);

        app.runtime.dirty = false;
        clear_verdict(&mut app, GenerationParams { generation: 0 }).unwrap();
        assert!(app.runtime.dirty);
    }
}
