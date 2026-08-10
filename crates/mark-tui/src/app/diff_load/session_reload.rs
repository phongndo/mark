use std::path::PathBuf;

use mark_diff::DiffOptions;
use tokio::sync::oneshot;

use super::super::{AsyncJob, BranchMetadataPolicy, DiffApp, PendingDiffLoad};
use crate::runtime;

fn scoped_paths(
    current: &DiffOptions,
    requested: &DiffOptions,
    pathspecs: &[PathBuf],
) -> Option<Vec<PathBuf>> {
    (!pathspecs.is_empty() && requested == current).then(|| pathspecs.to_vec())
}

impl DiffApp {
    pub(crate) fn start_session_diff_load(
        &mut self,
        options: DiffOptions,
        pathspecs: Vec<PathBuf>,
    ) -> Option<oneshot::Receiver<Result<u64, String>>> {
        if self.jobs.pending_diff_load.is_some() || self.jobs.pending_review_load.is_some() {
            return None;
        }
        self.invalidate_diff_cache();
        self.jobs.pending_review_load = None;
        self.clear_cached_diff_choices();
        let (job_tx, job_rx) = oneshot::channel();
        let load_options = options.clone();
        let scoped_paths = scoped_paths(&self.document.options, &options, &pathspecs);
        let load_scoped = scoped_paths.is_some();
        drop(runtime::spawn_blocking(move || {
            let result = if load_scoped {
                mark_diff::load_review_ref_paths_with_raw_patch(&load_options, &pathspecs)
            } else {
                mark_diff::load_review_ref_with_raw_patch(&load_options)
            };
            let _ = job_tx.send(result);
        }));
        let (completion, response) = oneshot::channel();
        self.jobs.pending_diff_load = Some(PendingDiffLoad {
            options,
            error_prefix: "session reload failed".to_owned(),
            branch_metadata: BranchMetadataPolicy::Refresh,
            scoped_paths,
            completion: Some(completion),
            job: AsyncJob::new(job_rx),
        });
        self.set_success_notice("reloading");
        self.runtime.dirty = true;
        Some(response)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mark_diff::{Changeset, DiffOptions, DiffSource, RepoRoot, RevSpec};

    use crate::controls::DiffLayoutMode;

    use super::*;

    fn changeset(patch: &str) -> Changeset {
        Changeset {
            repo: RepoRoot::new("/repo"),
            title: "test".to_owned(),
            files: mark_diff::parse_patch(patch),
            raw_patch: Arc::from(patch.as_bytes()),
        }
    }

    #[test]
    fn source_changing_session_reload_replaces_the_complete_document() {
        let original = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old a\n+new a\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old b\n+new b\n";
        let replacement =
            "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old b\n+newer b\n";
        let current = DiffOptions::default();
        let mut requested = current.clone();
        requested.source = DiffSource::Show(RevSpec::new("HEAD~1"));
        let paths = vec![PathBuf::from("b.rs")];
        assert_eq!(scoped_paths(&current, &requested, &paths), None);
        let mut app = DiffApp::new(current, changeset(original), DiffLayoutMode::Unified);
        let (_job_tx, job_rx) = oneshot::channel();
        let (completion, _response) = oneshot::channel();
        let pending = PendingDiffLoad {
            options: requested.clone(),
            error_prefix: "session reload failed".to_owned(),
            branch_metadata: BranchMetadataPolicy::Refresh,
            scoped_paths: None,
            completion: Some(completion),
            job: AsyncJob::new(job_rx),
        };

        app.apply_pending_diff_load(&pending, changeset(replacement))
            .unwrap();

        assert_eq!(app.document.options, requested);
        assert_eq!(
            app.document
                .changeset
                .files
                .iter()
                .map(mark_diff::DiffFile::display_path)
                .collect::<Vec<_>>(),
            vec!["b.rs"]
        );
    }

    #[test]
    fn scoped_session_reload_preserves_unselected_files() {
        let original = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old a\n+new a\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old b\n+new b\n";
        let replacement =
            "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old b\n+newer b\n";
        let mut app = DiffApp::new(
            DiffOptions::default(),
            changeset(original),
            DiffLayoutMode::Unified,
        );
        app.annotations_state
            .lifecycle
            .set_hunk_reviewed("a.rs", 0, true);
        let (_job_tx, job_rx) = oneshot::channel();
        let (completion, _response) = oneshot::channel();
        let pending = PendingDiffLoad {
            options: DiffOptions::default(),
            error_prefix: "session reload failed".to_owned(),
            branch_metadata: BranchMetadataPolicy::Refresh,
            scoped_paths: Some(vec![PathBuf::from("b.rs")]),
            completion: Some(completion),
            job: AsyncJob::new(job_rx),
        };

        app.apply_pending_diff_load(&pending, changeset(replacement))
            .unwrap();

        assert_eq!(
            app.document
                .changeset
                .files
                .iter()
                .map(mark_diff::DiffFile::display_path)
                .collect::<Vec<_>>(),
            vec!["a.rs", "b.rs"]
        );
        assert!(String::from_utf8_lossy(&app.document.changeset.raw_patch).contains("+new a"));
        assert!(String::from_utf8_lossy(&app.document.changeset.raw_patch).contains("+newer b"));
        assert!(app.annotations_state.lifecycle.hunk_reviewed("a.rs", 0));
    }

    #[test]
    fn overlapping_session_reload_does_not_replace_pending_completion() {
        let mut app = DiffApp::new(
            DiffOptions::default(),
            Changeset {
                repo: RepoRoot::new("/repo"),
                title: "test".to_owned(),
                files: Vec::new(),
                raw_patch: Changeset::empty_raw_patch(),
            },
            DiffLayoutMode::Unified,
        );
        let (_job_tx, job_rx) = oneshot::channel();
        let (completion, _response) = oneshot::channel();
        app.jobs.pending_diff_load = Some(PendingDiffLoad {
            options: DiffOptions::default(),
            error_prefix: "session reload failed".to_owned(),
            branch_metadata: BranchMetadataPolicy::Refresh,
            scoped_paths: None,
            completion: Some(completion),
            job: AsyncJob::new(job_rx),
        });

        let second = app.start_session_diff_load(DiffOptions::default(), Vec::new());

        assert!(second.is_none());
        assert!(
            app.jobs
                .pending_diff_load
                .as_ref()
                .and_then(|pending| pending.completion.as_ref())
                .is_some()
        );
    }
}
