use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::app::DiffApp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictKind {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictDestination {
    Local,
    Stdout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FinalVerdict {
    pub(crate) kind: VerdictKind,
    pub(crate) summary: Option<String>,
    pub(crate) destination: VerdictDestination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewLifecycleState {
    pub(crate) pass: u64,
    pub(crate) reviewed_files: BTreeSet<String>,
    pub(crate) reviewed_hunks: BTreeSet<String>,
    pub(crate) changed_files: BTreeSet<String>,
    pub(crate) verdict: Option<FinalVerdict>,
}

impl Default for ReviewLifecycleState {
    fn default() -> Self {
        Self {
            pass: 1,
            reviewed_files: BTreeSet::new(),
            reviewed_hunks: BTreeSet::new(),
            changed_files: BTreeSet::new(),
            verdict: None,
        }
    }
}

impl DiffApp {
    pub(crate) fn set_local_verdict(&mut self, kind: VerdictKind) {
        self.annotations_state.lifecycle.verdict = Some(FinalVerdict {
            kind,
            summary: None,
            destination: VerdictDestination::Local,
        });
        let notice = match kind {
            VerdictKind::Approve => "verdict: approve",
            VerdictKind::RequestChanges => "verdict: request changes",
            VerdictKind::Comment => "verdict: comment",
        };
        self.set_notice(notice);
        self.runtime.dirty = true;
    }

    pub(crate) fn clear_local_verdict(&mut self) {
        self.annotations_state.lifecycle.verdict = None;
        self.set_notice("verdict cleared");
        self.runtime.dirty = true;
    }

    pub(crate) fn toggle_reviewed_progress(&mut self) {
        let focus_row = self.viewport_focus_row();
        let file_index = self
            .document
            .model
            .file_at_row(focus_row)
            .unwrap_or(self.sidebar.selected_file.get());
        let Some(file) = self.document.changeset.files.get(file_index) else {
            return;
        };
        let path = file.display_path().to_owned();
        let hunk = self
            .document
            .model
            .row(focus_row)
            .and_then(|row| row.typed_hunk_key())
            .filter(|(candidate, _)| candidate.get() == file_index)
            .map(|(_, hunk)| hunk.get() + 1);
        let reviewed = if let Some(hunk) = hunk {
            let reviewed = !self.annotations_state.lifecycle.hunk_reviewed(&path, hunk);
            self.annotations_state
                .lifecycle
                .set_hunk_reviewed(&path, hunk, reviewed);
            reviewed
        } else {
            let reviewed = !self.annotations_state.lifecycle.file_reviewed(&path);
            self.annotations_state
                .lifecycle
                .set_file_reviewed(&path, reviewed);
            reviewed
        };
        self.set_notice(if reviewed {
            "marked reviewed"
        } else {
            "marked unreviewed"
        });
        self.runtime.dirty = true;
    }
}

impl ReviewLifecycleState {
    pub(crate) fn mark_file(&mut self, path: &str) {
        self.reviewed_files.insert(path.to_owned());
    }

    pub(crate) fn mark_hunk(&mut self, path: &str, hunk: usize) {
        self.reviewed_hunks.insert(format!("{path}\0{hunk}"));
    }

    pub(crate) fn set_file_reviewed(&mut self, path: &str, reviewed: bool) {
        if reviewed {
            self.mark_file(path);
        } else {
            self.reviewed_files.remove(path);
            self.reviewed_hunks
                .retain(|hunk| !hunk.starts_with(&format!("{path}\0")));
        }
    }

    pub(crate) fn set_hunk_reviewed(&mut self, path: &str, hunk: usize, reviewed: bool) {
        if reviewed {
            self.mark_hunk(path, hunk);
        } else {
            self.reviewed_hunks.remove(&format!("{path}\0{hunk}"));
        }
    }

    pub(crate) fn file_reviewed(&self, path: &str) -> bool {
        self.reviewed_files.contains(path)
    }

    pub(crate) fn hunk_reviewed(&self, path: &str, hunk: usize) -> bool {
        self.reviewed_hunks.contains(&format!("{path}\0{hunk}"))
    }
}
