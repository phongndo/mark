use std::collections::BTreeMap;

use mark_diff::{DiffFile, DiffHunk};

use crate::{
    annotation::{AnnotationKey, AnnotationScope, AnnotationSide},
    app::DiffApp,
    review::{ReviewAnchorEvidence, ReviewComment},
};

const MAX_ANCHOR_EVIDENCE_BYTES: usize = 64 * 1024;
const MAX_TRANSITION_EVIDENCE_BYTES: usize = 2 * 1024 * 1024;

struct CapturedComment {
    comment: ReviewComment,
    evidence: ReviewAnchorEvidence,
}

pub(crate) struct ReviewTransition {
    file_fingerprints: BTreeMap<String, String>,
    comments: Vec<CapturedComment>,
}

impl ReviewTransition {
    pub(crate) fn capture(app: &DiffApp) -> Self {
        let mut evidence_bytes = 0usize;
        let comments = app
            .annotations_state
            .annotations
            .comments()
            .map(|comment| {
                let mut comment = comment.clone();
                let mut evidence = comment
                    .original_anchor_evidence
                    .take()
                    .unwrap_or_else(|| anchor_evidence(app, &comment.anchor));
                let bytes = anchor_evidence_bytes(&evidence);
                if evidence_bytes.saturating_add(bytes) > MAX_TRANSITION_EVIDENCE_BYTES {
                    evidence = ReviewAnchorEvidence::default();
                } else {
                    evidence_bytes = evidence_bytes.saturating_add(bytes);
                }
                CapturedComment { comment, evidence }
            })
            .collect();
        Self {
            file_fingerprints: file_fingerprints(app),
            comments,
        }
    }

    pub(crate) fn apply(self, app: &mut DiffApp) {
        let current_files = file_fingerprints(app);
        let changed = changed_files(&self.file_fingerprints, &current_files);
        if changed.is_empty() {
            return;
        }
        let lifecycle = &mut app.annotations_state.lifecycle;
        lifecycle.pass = lifecycle.pass.saturating_add(1);
        lifecycle.changed_files = changed.clone();
        lifecycle
            .reviewed_files
            .retain(|path| !changed.contains(path));
        lifecycle.reviewed_hunks.retain(|key| {
            key.split_once('\0')
                .is_some_and(|(path, _)| !changed.contains(path))
        });
        lifecycle.verdict = None;
        let generation = app.document.generation;
        let comments = self
            .comments
            .into_iter()
            .map(|captured| {
                let anchor_changed = changed.contains(&captured.comment.anchor.path);
                restore_comment(app, captured, generation, anchor_changed)
            })
            .collect();
        let restore_result = app.annotations_state.annotations.restore_comments(comments);
        invalidate_annotation_geometry(app);
        if restore_result.is_err() {
            app.annotations_state.annotations = Default::default();
            app.set_error_log("could not re-anchor review comments after reload");
        }
    }
}

pub(crate) fn reset_review(app: &mut DiffApp) {
    app.annotations_state.annotations = Default::default();
    app.annotations_state.lifecycle = Default::default();
    app.annotations_state.annotation_draft = None;
    app.annotations_state.sticky_annotation_draft = false;
    invalidate_annotation_geometry(app);
}

fn invalidate_annotation_geometry(app: &mut DiffApp) {
    app.annotations_state.annotation_block_scroll = None;
    app.annotations_state.annotation_rows.borrow_mut().clear();
    *app.annotations_state.annotation_keys_by_row.borrow_mut() = None;
    app.annotations_state
        .annotation_heights
        .borrow_mut()
        .clear();
}

fn restore_comment(
    app: &DiffApp,
    captured: CapturedComment,
    generation: u64,
    anchor_changed: bool,
) -> ReviewComment {
    let CapturedComment {
        mut comment,
        evidence,
    } = captured;
    let (anchor, lifecycle) = if !anchor_changed {
        (comment.anchor.clone(), comment.lifecycle)
    } else {
        reanchor(app, &comment.anchor, &evidence).unwrap_or_else(|| {
            let lifecycle = if find_file(app, &comment.anchor.path).is_some() {
                super::CommentLifecycle::Stale
            } else {
                super::CommentLifecycle::Cleared
            };
            (comment.anchor.clone(), lifecycle)
        })
    };
    comment.anchor = anchor;
    comment.lifecycle = lifecycle;
    comment.document_generation = generation;
    comment.original_anchor_evidence = Some(evidence);
    comment
}

fn reanchor(
    app: &DiffApp,
    anchor: &AnnotationKey,
    evidence: &ReviewAnchorEvidence,
) -> Option<(AnnotationKey, super::CommentLifecycle)> {
    let exact_file = find_file(app, &anchor.path);
    let mut candidates = app
        .document
        .changeset
        .files
        .iter()
        .filter(|file| exact_file.is_none_or(|exact| std::ptr::eq(*file, exact)))
        .filter_map(|file| candidate_anchor(file, anchor, evidence));
    let candidate = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let lifecycle = if candidate == *anchor {
        super::CommentLifecycle::Open
    } else {
        super::CommentLifecycle::Moved
    };
    Some((candidate, lifecycle))
}

fn candidate_anchor(
    file: &DiffFile,
    anchor: &AnnotationKey,
    evidence: &ReviewAnchorEvidence,
) -> Option<AnnotationKey> {
    match anchor.scope {
        AnnotationScope::File => {
            if file.old_path() != Some(anchor.path.as_str())
                && file.new_path() != Some(anchor.path.as_str())
                && evidence
                    .file_fingerprint
                    .as_deref()
                    .is_none_or(|expected| content_fingerprint(file) != expected)
            {
                return None;
            }
            AnnotationKey::for_file(file)
        }
        AnnotationScope::Line => {
            let lines = match anchor.side {
                AnnotationSide::Old => &evidence.old_lines,
                AnnotationSide::New => &evidence.new_lines,
            };
            let line = unique_sequence_start(file, anchor.side, lines)?;
            AnnotationKey::for_file_line(file, anchor.side, line)
        }
        AnnotationScope::Range { .. } => {
            let old = unique_optional_range(file, AnnotationSide::Old, &evidence.old_lines)?;
            let new = unique_optional_range(file, AnnotationSide::New, &evidence.new_lines)?;
            let (side, line) = new
                .map(|(start, count)| (AnnotationSide::New, start + count - 1))
                .or_else(|| old.map(|(start, count)| (AnnotationSide::Old, start + count - 1)))?;
            let (old_start, old_count) = old.unwrap_or((0, 0));
            let (new_start, new_count) = new.unwrap_or((0, 0));
            AnnotationKey::for_range(file, side, line, old_start, old_count, new_start, new_count)
        }
        AnnotationScope::Hunk { .. } => {
            let expected = evidence.hunk_fingerprint.as_deref()?;
            let mut matches = file
                .hunks()
                .iter()
                .filter(|hunk| hunk_fingerprint(hunk) == expected);
            let hunk = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            AnnotationKey::for_hunk(file, hunk)
        }
    }
}

fn anchor_evidence(app: &DiffApp, anchor: &AnnotationKey) -> ReviewAnchorEvidence {
    let Some(file) = find_file(app, &anchor.path) else {
        return ReviewAnchorEvidence::default();
    };
    let file_fingerprint = Some(content_fingerprint(file));
    match anchor.scope {
        AnnotationScope::File => ReviewAnchorEvidence {
            file_fingerprint,
            ..ReviewAnchorEvidence::default()
        },
        AnnotationScope::Line => ReviewAnchorEvidence {
            file_fingerprint,
            old_lines: if anchor.side == AnnotationSide::Old {
                range_lines(file, AnnotationSide::Old, anchor.line, 1)
            } else {
                Vec::new()
            },
            new_lines: if anchor.side == AnnotationSide::New {
                range_lines(file, AnnotationSide::New, anchor.line, 1)
            } else {
                Vec::new()
            },
            hunk_fingerprint: None,
        },
        AnnotationScope::Range {
            old_start,
            old_count,
            new_start,
            new_count,
        } => ReviewAnchorEvidence {
            file_fingerprint,
            old_lines: range_lines(file, AnnotationSide::Old, old_start, old_count),
            new_lines: range_lines(file, AnnotationSide::New, new_start, new_count),
            hunk_fingerprint: None,
        },
        AnnotationScope::Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
        } => ReviewAnchorEvidence {
            file_fingerprint,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            hunk_fingerprint: file
                .hunks()
                .iter()
                .find(|hunk| {
                    hunk.old_start() == old_start
                        && hunk.old_count() == old_count
                        && hunk.new_start() == new_start
                        && hunk.new_count() == new_count
                })
                .map(hunk_fingerprint),
        },
    }
}

fn anchor_evidence_bytes(evidence: &ReviewAnchorEvidence) -> usize {
    evidence.file_fingerprint.as_ref().map_or(0, String::len)
        + evidence.old_lines.iter().map(String::len).sum::<usize>()
        + evidence.new_lines.iter().map(String::len).sum::<usize>()
        + evidence.hunk_fingerprint.as_ref().map_or(0, String::len)
}

fn unique_optional_range(
    file: &DiffFile,
    side: AnnotationSide,
    expected: &[String],
) -> Option<Option<(usize, usize)>> {
    if expected.is_empty() {
        return Some(None);
    }
    unique_sequence_start(file, side, expected).map(|start| Some((start, expected.len())))
}

fn unique_sequence_start(
    file: &DiffFile,
    side: AnnotationSide,
    expected: &[String],
) -> Option<usize> {
    if expected.is_empty() || expected.len() > 128 {
        return None;
    }
    let prefix = sequence_prefix_table(expected);
    let mut matched = 0usize;
    let mut previous_line = None;
    let mut found = None;
    for (line, text) in side_lines(file, side) {
        if previous_line.is_some_and(|previous| line != previous + 1) {
            matched = 0;
        }
        previous_line = Some(line);
        while matched > 0 && expected[matched] != text {
            matched = prefix[matched - 1];
        }
        if expected[matched] == text {
            matched += 1;
        }
        if matched == expected.len() {
            let start = line + 1 - expected.len();
            if found.replace(start).is_some() {
                return None;
            }
            matched = prefix[matched - 1];
        }
    }
    found
}

fn sequence_prefix_table(expected: &[String]) -> Vec<usize> {
    let mut prefix = vec![0; expected.len()];
    let mut matched = 0usize;
    for index in 1..expected.len() {
        while matched > 0 && expected[index] != expected[matched] {
            matched = prefix[matched - 1];
        }
        if expected[index] == expected[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    prefix
}

fn range_lines(file: &DiffFile, side: AnnotationSide, start: usize, count: usize) -> Vec<String> {
    if count == 0 || count > 128 {
        return Vec::new();
    }
    let mut bytes = 0usize;
    let mut lines = Vec::with_capacity(count);
    for (_, text) in side_lines(file, side)
        .filter(|(line, _)| *line >= start && *line < start.saturating_add(count))
    {
        if bytes.saturating_add(text.len()) > MAX_ANCHOR_EVIDENCE_BYTES {
            return Vec::new();
        }
        bytes = bytes.saturating_add(text.len());
        lines.push(text.into_owned());
    }
    lines
}

fn side_lines(
    file: &DiffFile,
    side: AnnotationSide,
) -> impl Iterator<Item = (usize, std::borrow::Cow<'_, str>)> {
    file.hunks()
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter_map(move |line| {
            let number = match side {
                AnnotationSide::Old => line.old_line(),
                AnnotationSide::New => line.new_line(),
            }?;
            Some((number, line.text_lossy()))
        })
}

fn find_file<'a>(app: &'a DiffApp, path: &str) -> Option<&'a DiffFile> {
    app.document
        .changeset
        .files
        .iter()
        .find(|file| file.old_path() == Some(path) || file.new_path() == Some(path))
}

fn hunk_fingerprint(hunk: &DiffHunk) -> String {
    let mut hash = Fingerprinter::new();
    hash.update(hunk.header.as_bytes());
    hash.update(&[0]);
    for line in &hunk.lines {
        hash.update(&[line.kind() as u8]);
        hash.update(line.text_bytes());
        hash.update(&[0]);
    }
    hash.finish()
}

fn file_fingerprints(app: &DiffApp) -> BTreeMap<String, String> {
    let raw_segments = raw_file_segments(&app.document.changeset.raw_patch);
    let segments_match = raw_segments.len() == app.document.changeset.files.len();
    let mut fingerprints = BTreeMap::new();
    for (index, file) in app.document.changeset.files.iter().enumerate() {
        let mut hash = Fingerprinter::new();
        hash.update(file.display_path().as_bytes());
        hash.update(file.status().label().as_bytes());
        for hunk in file.hunks() {
            hash.update(&hunk.old_start().to_le_bytes());
            hash.update(&hunk.old_count().to_le_bytes());
            hash.update(&hunk.new_start().to_le_bytes());
            hash.update(&hunk.new_count().to_le_bytes());
            hash.update(hunk_fingerprint(hunk).as_bytes());
        }
        if segments_match {
            hash.update(raw_segments[index]);
        }
        let fingerprint = hash.finish();
        let aliases = [file.old_path(), file.new_path()];
        if file.old_path().is_none() && file.new_path().is_none() {
            fingerprints.insert(file.display_path().to_owned(), fingerprint);
        } else {
            for path in aliases.into_iter().flatten() {
                fingerprints.insert(path.to_owned(), fingerprint.clone());
            }
        }
    }
    fingerprints
}

fn raw_file_segments(raw_patch: &[u8]) -> Vec<&[u8]> {
    const HEADER: &[u8] = b"diff --git ";
    let mut starts = raw_patch
        .windows(HEADER.len())
        .enumerate()
        .filter_map(|(index, window)| {
            (window == HEADER && (index == 0 || raw_patch[index - 1] == b'\n')).then_some(index)
        })
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Vec::new();
    }
    starts.push(raw_patch.len());
    starts
        .windows(2)
        .map(|range| &raw_patch[range[0]..range[1]])
        .collect()
}

fn content_fingerprint(file: &DiffFile) -> String {
    let mut hash = Fingerprinter::new();
    for hunk in file.hunks() {
        hash.update(hunk_fingerprint(hunk).as_bytes());
    }
    hash.finish()
}

fn changed_files(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> std::collections::BTreeSet<String> {
    previous
        .keys()
        .chain(current.keys())
        .filter(|path| previous.get(*path) != current.get(*path))
        .cloned()
        .collect()
}

struct Fingerprinter {
    left: u64,
    right: u64,
}

impl Fingerprinter {
    fn new() -> Self {
        Self {
            left: 0xcbf29ce484222325,
            right: 0x84222325cbf29ce4,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.left = (self.left ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            self.right = (self.right ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> String {
        format!("{:016x}{:016x}", self.left, self.right)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mark_diff::{Changeset, DiffOptions, RepoRoot};

    use crate::{
        annotation::{AnnotationDraft, AnnotationKey, AnnotationSide},
        controls::DiffLayoutMode,
        review::{
            CommentLifecycle, FinalVerdict, ReviewLifecycleState, VerdictDestination, VerdictKind,
        },
    };

    use super::*;

    fn app(patch: &str) -> DiffApp {
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

    fn patch(text: &str) -> String {
        format!(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+{text}\n"
        )
    }

    fn replace_document(target: &mut DiffApp, replacement: DiffApp) {
        target.document.changeset = replacement.document.changeset;
        target.document.generation = target.document.generation.wrapping_add(1);
    }

    #[test]
    fn unchanged_transition_preserves_live_review_state() {
        let mut source = app(&patch("new"));
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.verdict = Some(FinalVerdict {
            kind: VerdictKind::Approve,
            summary: Some("ready".to_owned()),
            destination: VerdictDestination::Local,
        });
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(&patch("new")));

        transition.apply(&mut source);

        assert_eq!(source.annotations_state.lifecycle.pass, 1);
        assert!(
            source
                .annotations_state
                .lifecycle
                .file_reviewed("src/lib.rs")
        );
        assert!(source.annotations_state.lifecycle.verdict.is_some());
        assert!(source.annotations_state.lifecycle.changed_files.is_empty());
    }

    #[test]
    fn changed_metadata_advances_pass_and_invalidates_changed_file_state() {
        let before = "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100755\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let after = "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100700\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut source = app(before);
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.verdict = Some(FinalVerdict {
            kind: VerdictKind::Approve,
            summary: None,
            destination: VerdictDestination::Local,
        });
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(after));

        transition.apply(&mut source);

        assert_eq!(source.annotations_state.lifecycle.pass, 2);
        assert!(
            !source
                .annotations_state
                .lifecycle
                .file_reviewed("src/lib.rs")
        );
        assert!(source.annotations_state.lifecycle.verdict.is_none());
        assert!(
            source
                .annotations_state
                .lifecycle
                .changed_files
                .contains("src/lib.rs")
        );
    }

    #[test]
    fn changed_document_moves_a_comment_only_when_evidence_is_unique() {
        let before = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+target\n";
        let after = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n+insert\n context\n-old\n+target\n";
        let mut source = app(before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            2,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(after));

        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, CommentLifecycle::Moved);
        assert_eq!(comment.anchor.line, 3);
        assert_eq!(source.annotations_state.annotations.keys().count(), 1);
    }

    #[test]
    fn changed_document_marks_ambiguous_comment_stale() {
        let before = patch("target");
        let after = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1,2 @@\n-old\n+target\n+target\n";
        let mut source = app(&before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(after));

        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, CommentLifecycle::Stale);
        assert_eq!(source.annotations_state.annotations.keys().count(), 0);
    }

    #[test]
    fn stale_comment_reuses_original_evidence_on_later_reload() {
        let before = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-old\n+target\n stable\n";
        let unrelated = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-old\n+other\n stable\n";
        let target_moved = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-old\n-stable\n+other\n+target\n";
        let mut source = app(before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(unrelated));
        transition.apply(&mut source);
        assert_eq!(
            source
                .annotations_state
                .annotations
                .comments()
                .next()
                .unwrap()
                .lifecycle,
            CommentLifecycle::Stale
        );

        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(target_moved));
        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, CommentLifecycle::Moved);
        assert_eq!(comment.anchor.line, 2);
    }

    #[test]
    fn removed_file_marks_its_comments_cleared() {
        let mut source = app(&patch("target"));
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(""));

        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, CommentLifecycle::Cleared);
        assert_eq!(source.annotations_state.annotations.keys().count(), 0);
    }

    #[test]
    fn renamed_file_changes_track_both_path_aliases() {
        let before = "diff --git a/old.rs b/new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,2 +1 @@\n context\n-target\n";
        let after = "diff --git a/old.rs b/new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,3 +1 @@\n-extra\n context\n-target\n";
        let mut source = app(before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::Old,
            2,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        replace_document(&mut source, app(after));

        transition.apply(&mut source);

        assert!(
            source
                .annotations_state
                .lifecycle
                .changed_files
                .contains("old.rs")
        );
        assert!(
            source
                .annotations_state
                .lifecycle
                .changed_files
                .contains("new.rs")
        );
        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, CommentLifecycle::Moved);
        assert_eq!(comment.anchor.path, "old.rs");
        assert_eq!(comment.anchor.line, 3);
    }

    #[test]
    fn in_process_transition_uses_files_when_raw_patch_is_stale() {
        let before = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+target\n";
        let after = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n+insert\n context\n-old\n+target\n";
        let mut source = app(before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            2,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        source.document.changeset.files = app(after).document.changeset.files;
        source.document.generation = 1;

        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(source.annotations_state.lifecycle.pass, 2);
        assert_eq!(comment.lifecycle, CommentLifecycle::Moved);
        assert_eq!(comment.anchor.line, 3);
    }

    #[test]
    fn reset_review_discards_all_session_state_for_a_new_source() {
        let mut source = app(&patch("new"));
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();
        source
            .annotations_state
            .annotations
            .insert_human(key.clone(), "question".to_owned(), 0)
            .unwrap();
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.pass = 4;
        source.annotations_state.annotation_draft = Some(AnnotationDraft {
            key: key.clone(),
            model_row_index: 1,
            input: "unfinished question".to_owned(),
            cursor: "unfinished question".len(),
        });
        source.annotations_state.annotation_block_scroll = Some((key, 1));
        source.annotations_state.sticky_annotation_draft = true;

        reset_review(&mut source);

        assert!(source.annotations_state.annotations.is_empty());
        assert_eq!(
            source.annotations_state.lifecycle,
            ReviewLifecycleState::default()
        );
        assert!(source.annotations_state.annotation_draft.is_none());
        assert!(source.annotations_state.annotation_block_scroll.is_none());
        assert!(!source.annotations_state.sticky_annotation_draft);
    }

    #[test]
    fn oversized_source_lines_are_not_captured_as_anchor_evidence() {
        let text = "x".repeat(MAX_ANCHOR_EVIDENCE_BYTES + 1);
        let source = app(&patch(&text));
        let anchor = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();

        let evidence = anchor_evidence(&source, &anchor);

        assert!(evidence.new_lines.is_empty());
    }

    #[test]
    fn raw_file_segments_ignore_diff_headers_inside_payload_lines() {
        let patch = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+diff --git payload\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n";
        let segments = raw_file_segments(patch);
        assert_eq!(segments.len(), 2);
        assert!(segments[0].ends_with(b"+diff --git payload\n"));
    }
}
