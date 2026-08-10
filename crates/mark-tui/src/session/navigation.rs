use mark_session::{NavigateParams, NavigateTarget, NavigationResult, ProtocolError};

use super::{anchors, snapshot};
use crate::app::{DiffApp, PostFilterNavigation};

pub(crate) fn navigate(
    app: &mut DiffApp,
    params: NavigateParams,
) -> Result<NavigationResult, ProtocolError> {
    match params.target {
        NavigateTarget::Anchor { anchor } => {
            let key = anchors::validate(app, &anchor)?;
            if app.annotation_model_row(&key).is_none() && app.filters.active() {
                app.clear_all_filters();
                app.apply_filters(PostFilterNavigation::Preserve);
            }
            if app.annotation_model_row(&key).is_none() {
                return Err(ProtocolError::new(
                    "anchor_not_visible",
                    "anchor is valid but is not available in the current view",
                ));
            }
            app.jump_to_annotation(&key);
        }
        NavigateTarget::NextComment => move_to_comment(app, 1)?,
        NavigateTarget::PreviousComment => move_to_comment(app, -1)?,
    }
    app.runtime.dirty = true;
    Ok(NavigationResult {
        generation: app.document.generation,
        focus: snapshot::focus(app),
    })
}

fn move_to_comment(app: &mut DiffApp, delta: isize) -> Result<(), ProtocolError> {
    let navigable = app
        .annotations_state
        .annotations
        .keys()
        .any(|key| app.annotation_model_row(key).is_some());
    if !navigable {
        return Err(ProtocolError::new(
            "comment_not_found",
            "the review has no comments available in the current view",
        ));
    }
    app.move_annotation(delta);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mark_diff::{Changeset, DiffOptions, RepoRoot};
    use mark_session::{NavigateParams, NavigateTarget, ReviewAnchor};

    use crate::{
        app::DiffApp,
        controls::DiffLayoutMode,
        review::{FindingDisposition, NewAgentComment},
    };

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

    #[test]
    fn dismissed_comments_are_not_navigable() {
        let mut app = app();
        let anchor = anchors::validate(
            &app,
            &ReviewAnchor {
                file: "src/lib.rs".to_owned(),
                scope: None,
                hunk: None,
                old_line: None,
                new_line: Some(1),
                range: None,
            },
        )
        .unwrap();
        let ids = app
            .annotations_state
            .annotations
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor,
                    summary: "finding".to_owned(),
                    rationale: None,
                    author: None,
                }],
                0,
            )
            .unwrap();
        app.annotations_state
            .annotations
            .set_disposition(&ids[0], FindingDisposition::Dismissed)
            .unwrap();

        let error = navigate(
            &mut app,
            NavigateParams {
                target: NavigateTarget::NextComment,
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "comment_not_found");
    }
}
