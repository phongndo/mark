use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mark_diff::{DiffFile, DiffHunk, DiffSource, PatchSource};
use serde::{Deserialize, Serialize};

use crate::{
    annotation::{AnnotationKey, AnnotationScope, AnnotationSide},
    app::DiffApp,
    review::{HumanCommentPersistenceBudget, NewAgentComment, ReviewAnchorEvidence, ReviewComment},
};

const SCHEMA_VERSION: u32 = 2;
const MAX_REVIEW_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewPersistence {
    identity: String,
    path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ReviewPersistenceSession {
    root: PathBuf,
    current: ReviewPersistence,
    can_save: bool,
}

pub(crate) struct ReviewTransition {
    fingerprint: String,
    file_fingerprints: BTreeMap<String, String>,
    comments: Vec<PersistedComment>,
}

impl ReviewTransition {
    pub(crate) fn capture(app: &DiffApp) -> Self {
        let file_fingerprints = file_fingerprints(app);
        Self {
            fingerprint: document_fingerprint_from_files(&file_fingerprints),
            file_fingerprints,
            comments: app
                .annotations_state
                .annotations
                .comments()
                .map(|comment| persisted_comment(app, comment))
                .collect(),
        }
    }

    pub(crate) fn apply(self, app: &mut DiffApp) {
        let current_files = file_fingerprints(app);
        let changed = changed_files(&self.file_fingerprints, &current_files);
        if self.fingerprint == document_fingerprint_from_files(&current_files) && changed.is_empty()
        {
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
            .map(|comment| {
                let anchor_changed = changed.contains(&comment.anchor.path);
                restore_comment(app, comment, generation, anchor_changed)
            })
            .collect();
        let restore_result = app.annotations_state.annotations.restore_comments(comments);
        invalidate_annotation_geometry(app);
        if restore_result.is_err() {
            app.set_error_log("could not restore review comments after reload");
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RestoreResult {
    pub(crate) restored: usize,
    pub(crate) snapshot_changed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedReview {
    schema: u32,
    identity: String,
    document_fingerprint: String,
    #[serde(default)]
    file_fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    lifecycle: super::ReviewLifecycleState,
    comments: Vec<PersistedComment>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedComment {
    id: String,
    anchor: crate::annotation::AnnotationKey,
    summary: String,
    rationale: Option<String>,
    author: Option<String>,
    origin: super::CommentOrigin,
    #[serde(default = "open_lifecycle")]
    lifecycle: super::CommentLifecycle,
    #[serde(default = "open_disposition")]
    disposition: super::FindingDisposition,
    document_generation: u64,
    #[serde(default)]
    evidence: ReviewAnchorEvidence,
}

impl DiffApp {
    pub(crate) fn initialize_review_persistence(&mut self) {
        let Some(root) = state_root() else {
            return;
        };
        let current = ReviewPersistence::at(root.clone(), review_identity(self));
        let result = current.load_into(self);
        self.annotations_state.persistence = Some(ReviewPersistenceSession {
            root,
            current,
            can_save: result.is_ok(),
        });
        self.report_review_persistence_restore(result);
    }

    pub(crate) fn prepare_review_persistence_source_change(&mut self) -> io::Result<bool> {
        let Some(session) = self.annotations_state.persistence.take() else {
            return Ok(false);
        };
        let result = if session.can_save {
            session.current.save(self)
        } else {
            Ok(())
        };
        self.annotations_state.persistence = Some(session);
        result.map(|()| true)
    }

    pub(crate) fn finish_review_persistence_source_change(&mut self) {
        let Some(mut session) = self.annotations_state.persistence.take() else {
            return;
        };
        self.annotations_state.lifecycle = super::ReviewLifecycleState::default();
        self.annotations_state.annotations = Default::default();
        invalidate_annotation_geometry(self);

        let current = ReviewPersistence::at(session.root.clone(), review_identity(self));
        let result = current.load_into(self);
        session.current = current;
        session.can_save = result.is_ok();
        self.annotations_state.persistence = Some(session);
        self.report_review_persistence_restore(result);
    }

    pub(crate) fn save_review_persistence(&self) -> io::Result<()> {
        let Some(session) = self.annotations_state.persistence.as_ref() else {
            return Ok(());
        };
        if session.can_save {
            session.current.save(self)
        } else {
            Ok(())
        }
    }

    fn report_review_persistence_restore(&mut self, result: io::Result<RestoreResult>) {
        match result {
            Ok(restored) if restored.snapshot_changed => {
                self.set_warning_notice("restored review from an earlier pass");
            }
            Ok(restored) if restored.restored > 0 => {
                self.set_success_notice(format!("restored {} comments", restored.restored));
            }
            Ok(_) => {}
            Err(error) => self.set_error_log(format!("could not restore saved review: {error}")),
        }
    }
}

impl ReviewPersistence {
    fn at(root: PathBuf, identity: String) -> Self {
        let file = format!("{}.json", fingerprint(identity.as_bytes()));
        Self {
            identity,
            path: root.join("mark").join("reviews").join(file),
        }
    }

    pub(crate) fn load_into(&self, app: &mut DiffApp) -> io::Result<RestoreResult> {
        let Some(review) = self.load()? else {
            return Ok(RestoreResult::default());
        };
        if review.schema == 0 || review.schema > SCHEMA_VERSION || review.identity != self.identity
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted review identity or schema does not match",
            ));
        }
        let current_files = file_fingerprints(app);
        let snapshot_changed =
            review.document_fingerprint != document_fingerprint_from_files(&current_files);
        let mut lifecycle = review.lifecycle;
        if snapshot_changed {
            lifecycle.pass = lifecycle.pass.saturating_add(1);
            lifecycle.changed_files = changed_files(&review.file_fingerprints, &current_files);
            for path in &lifecycle.changed_files {
                lifecycle.reviewed_files.remove(path);
                lifecycle
                    .reviewed_hunks
                    .retain(|hunk| !hunk.starts_with(&format!("{path}\0")));
            }
            lifecycle.verdict = None;
        }

        let generation = app.document.generation;
        let comments = review
            .comments
            .into_iter()
            .map(|comment| {
                let anchor_changed =
                    snapshot_changed && lifecycle.changed_files.contains(&comment.anchor.path);
                restore_comment(app, comment, generation, anchor_changed)
            })
            .collect::<Vec<_>>();
        let restored = comments.len();
        app.annotations_state.lifecycle = lifecycle;
        app.annotations_state
            .annotations
            .restore_comments(comments)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "persisted comments exceed limits",
                )
            })?;
        invalidate_annotation_geometry(app);
        if restored > 0 {
            app.runtime.dirty = true;
        }
        Ok(RestoreResult {
            restored,
            snapshot_changed,
        })
    }

    pub(crate) fn save(&self, app: &DiffApp) -> io::Result<()> {
        if app.annotations_state.annotations.is_empty()
            && app.annotations_state.lifecycle == super::ReviewLifecycleState::default()
        {
            return remove_if_exists(&self.path);
        }
        let review = persisted_review(app, self.identity.clone());
        let bytes = serde_json::to_vec_pretty(&review)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() as u64 > MAX_REVIEW_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted review exceeds the byte limit",
            ));
        }
        let directory = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("persisted review path has no parent"))?;
        ensure_private_directory(directory)?;
        let temp = directory.join(format!(
            ".review-{}-{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        let write_result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            replace_persisted_file(&temp, &self.path)?;
            set_private_file_permissions(&self.path)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write_result
    }

    fn load(&self) -> io::Result<Option<PersistedReview>> {
        let mut file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_REVIEW_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted review is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAX_REVIEW_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_REVIEW_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted review exceeds the byte limit",
            ));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

pub(crate) fn human_comment_persistence_budget(
    app: &DiffApp,
    anchor: &AnnotationKey,
    text: &str,
) -> Result<Option<HumanCommentPersistenceBudget>, serde_json::Error> {
    let anchor = app.annotations_state.annotations.canonical_anchor(anchor);
    let mut review = persisted_review(app, review_identity(app));
    if let Some(comment) = review
        .comments
        .iter_mut()
        .find(|comment| comment.origin == super::CommentOrigin::Human && comment.anchor == anchor)
    {
        comment.summary = text.to_owned();
        comment.document_generation = app.document.generation;
    } else {
        review.comments.push(PersistedComment {
            // The longest possible generated ID makes this a conservative size check.
            id: "human-18446744073709551615".to_owned(),
            anchor: anchor.clone(),
            summary: text.to_owned(),
            rationale: None,
            author: None,
            origin: super::CommentOrigin::Human,
            lifecycle: super::CommentLifecycle::Open,
            disposition: super::FindingDisposition::Open,
            document_generation: app.document.generation,
            evidence: anchor_evidence(app, &anchor),
        });
    }
    let fits = persisted_review_fits(&review)?;
    Ok(fits.then_some(HumanCommentPersistenceBudget::verified()))
}

pub(crate) fn agent_comments_fit_persistence_budget(
    app: &DiffApp,
    comments: &[NewAgentComment],
) -> Result<bool, serde_json::Error> {
    let mut review = persisted_review(app, review_identity(app));
    for comment in comments {
        review.comments.push(PersistedComment {
            // The longest possible generated ID makes this a conservative size check.
            id: "agent-18446744073709551615".to_owned(),
            anchor: comment.anchor.clone(),
            summary: comment.summary.clone(),
            rationale: comment.rationale.clone(),
            author: comment.author.clone(),
            origin: super::CommentOrigin::Agent,
            lifecycle: super::CommentLifecycle::Open,
            disposition: super::FindingDisposition::Open,
            document_generation: app.document.generation,
            evidence: anchor_evidence(app, &comment.anchor),
        });
    }
    persisted_review_fits(&review)
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

fn persisted_review_fits(review: &PersistedReview) -> Result<bool, serde_json::Error> {
    let bytes = serde_json::to_vec_pretty(review)?;
    Ok(bytes.len() as u64 <= MAX_REVIEW_BYTES)
}

fn persisted_review(app: &DiffApp, identity: String) -> PersistedReview {
    let comments = app
        .annotations_state
        .annotations
        .comments()
        .map(|comment| persisted_comment(app, comment))
        .collect::<Vec<_>>();
    let file_fingerprints = file_fingerprints(app);
    PersistedReview {
        schema: SCHEMA_VERSION,
        identity,
        document_fingerprint: document_fingerprint_from_files(&file_fingerprints),
        file_fingerprints,
        lifecycle: app.annotations_state.lifecycle.clone(),
        comments,
    }
}

fn persisted_comment(app: &DiffApp, comment: &ReviewComment) -> PersistedComment {
    PersistedComment {
        id: comment.id.clone(),
        anchor: comment.anchor.clone(),
        summary: comment.summary.clone(),
        rationale: comment.rationale.clone(),
        author: comment.author.clone(),
        origin: comment.origin,
        lifecycle: comment.lifecycle,
        disposition: comment.disposition,
        document_generation: comment.document_generation,
        evidence: comment
            .evidence
            .clone()
            .unwrap_or_else(|| anchor_evidence(app, &comment.anchor)),
    }
}

fn restore_comment(
    app: &DiffApp,
    comment: PersistedComment,
    generation: u64,
    anchor_changed: bool,
) -> ReviewComment {
    let (anchor, lifecycle) = if !anchor_changed {
        (comment.anchor.clone(), comment.lifecycle)
    } else {
        reanchor(app, &comment.anchor, &comment.evidence).unwrap_or_else(|| {
            let lifecycle = if find_file(app, &comment.anchor.path).is_some() {
                super::CommentLifecycle::Stale
            } else {
                super::CommentLifecycle::Cleared
            };
            (comment.anchor.clone(), lifecycle)
        })
    };
    ReviewComment {
        id: comment.id,
        anchor,
        summary: comment.summary,
        rationale: comment.rationale,
        author: comment.author,
        origin: comment.origin,
        lifecycle,
        disposition: comment.disposition,
        document_generation: generation,
        evidence: Some(comment.evidence),
    }
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
    side_lines(file, side)
        .filter(|(line, _)| *line >= start && *line < start.saturating_add(count))
        .map(|(_, text)| text.into_owned())
        .collect()
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

fn open_lifecycle() -> super::CommentLifecycle {
    super::CommentLifecycle::Open
}

fn open_disposition() -> super::FindingDisposition {
    super::FindingDisposition::Open
}

fn review_identity(app: &DiffApp) -> String {
    let source = match &app.document.options.source {
        DiffSource::Worktree => "worktree".to_owned(),
        DiffSource::Show(rev) => format!("show:{}", rev.as_str()),
        DiffSource::Base(rev) => format!("base:{}", rev.as_str()),
        DiffSource::Branch { base, head } => {
            format!("branch:{}:{}", base.as_str(), head.as_str())
        }
        DiffSource::Range { left, right } => {
            format!("range:{}:{}", left.as_str(), right.as_str())
        }
        DiffSource::Difftool { left, right, path } => format!(
            "difftool:{}:{}:{}",
            left.as_path().display(),
            right.as_path().display(),
            path.as_ref()
                .map_or_else(String::new, |path| path.as_path().display().to_string())
        ),
        DiffSource::Patch(PatchSource::File(path)) => {
            let working_directory = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            patch_file_source_identity(path, &working_directory)
        }
        DiffSource::Patch(PatchSource::Stdin(_)) => {
            format!("patch-stdin:{}", document_fingerprint(app))
        }
        DiffSource::Patch(PatchSource::Text { label, .. }) => {
            format!(
                "patch-text:{}:{}",
                label.as_str(),
                document_fingerprint(app)
            )
        }
        DiffSource::Patch(PatchSource::Review { label, .. }) => {
            format!("review:{}:{}", label.as_str(), document_fingerprint(app))
        }
    };
    format!(
        "repo={}\nsource={}\nuntracked={}",
        app.document.changeset.repo.as_path().display(),
        source,
        app.document.options.local_untracked.includes()
    )
}

fn patch_file_source_identity(path: &Path, working_directory: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    let resolved = fs::canonicalize(&absolute).unwrap_or(absolute);
    format!("patch-file:{}", resolved.display())
}

fn document_fingerprint(app: &DiffApp) -> String {
    document_fingerprint_from_files(&file_fingerprints(app))
}

fn document_fingerprint_from_files(files: &BTreeMap<String, String>) -> String {
    let mut hash = Fingerprinter::new();
    for (path, fingerprint) in files {
        hash.update(path.as_bytes());
        hash.update(&[0]);
        hash.update(fingerprint.as_bytes());
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

fn fingerprint(bytes: &[u8]) -> String {
    let mut hash = Fingerprinter::new();
    hash.update(bytes);
    hash.finish()
}

fn state_root() -> Option<PathBuf> {
    env::var_os("MARK_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local").join("state"))
        })
}

#[cfg(not(windows))]
fn replace_persisted_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_persisted_file(source: &Path, destination: &Path) -> io::Result<()> {
    let backup = source.with_extension("backup");
    match fs::rename(destination, &backup) {
        Ok(()) => match fs::rename(source, destination) {
            Ok(()) => fs::remove_file(backup),
            Err(error) => match fs::rename(&backup, destination) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(io::Error::new(
                    error.kind(),
                    format!(
                        "could not replace persisted review: {error}; rollback failed: {rollback_error}"
                    ),
                )),
            },
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(source, destination),
        Err(error) => Err(error),
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "persisted review directory is not a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use mark_diff::{Changeset, DiffOptions, RepoRoot};

    use crate::{
        annotation::{AnnotationKey, AnnotationSide},
        controls::DiffLayoutMode,
        review::NewAgentComment,
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

    #[test]
    fn saving_again_replaces_an_existing_persisted_review() {
        let temp = tempfile::tempdir().unwrap();
        let mut source = app(&patch("new"));
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();
        source.annotations_state.lifecycle.pass = 7;

        persistence.save(&source).unwrap();

        let review = persistence.load().unwrap().unwrap();
        assert_eq!(review.lifecycle.pass, 7);
    }

    #[test]
    fn raw_file_segments_ignore_diff_headers_inside_payload_lines() {
        let patch = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+diff --git payload\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n";
        let segments = raw_file_segments(patch);
        assert_eq!(segments.len(), 2);
        assert!(segments[0].ends_with(b"+diff --git payload\n"));
    }

    #[test]
    fn first_scoped_binary_reload_adopts_the_replacement_raw_patch() {
        let before = "diff --git a/image.bin b/image.bin\nindex 1111111..2222222 100644\nBinary files a/image.bin and b/image.bin differ\n";
        let after = "diff --git a/image.bin b/image.bin\nindex 1111111..3333333 100644\nBinary files a/image.bin and b/image.bin differ\n";
        let mut source = app("");

        source
            .replace_path_changeset(
                Path::new("image.bin"),
                app(before).document.changeset.clone(),
            )
            .unwrap();
        let before_fingerprints = file_fingerprints(&source);

        assert_eq!(
            source.document.changeset.raw_patch.as_ref(),
            before.as_bytes()
        );
        assert_eq!(
            source.document.base_changeset.raw_patch.as_ref(),
            before.as_bytes()
        );

        source
            .replace_path_changeset(
                Path::new("image.bin"),
                app(after).document.changeset.clone(),
            )
            .unwrap();

        assert_eq!(
            source.document.changeset.raw_patch.as_ref(),
            after.as_bytes()
        );
        assert_ne!(file_fingerprints(&source), before_fingerprints);
    }

    #[test]
    fn scoped_binary_reload_persists_the_updated_raw_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let before_binary = "diff --git a/image.bin b/image.bin\nindex 1111111..2222222 100644\nBinary files a/image.bin and b/image.bin differ\n";
        let after_binary = "diff --git a/image.bin b/image.bin\nindex 1111111..3333333 100644\nBinary files a/image.bin and b/image.bin differ\n";
        let text_patch = patch("new");
        let before = format!("{before_binary}{text_patch}");
        let after = format!("{after_binary}{text_patch}");
        let mut source = app(&before);
        let replacement = app(after_binary).document.changeset.clone();

        source
            .replace_path_changeset(Path::new("image.bin"), replacement)
            .unwrap();

        assert_eq!(
            source.document.changeset.raw_patch.as_ref(),
            after.as_bytes()
        );
        assert_eq!(
            source.document.base_changeset.raw_patch.as_ref(),
            after.as_bytes()
        );
        assert!(Arc::ptr_eq(
            &source.document.changeset.raw_patch,
            &source.document.base_changeset.raw_patch
        ));
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.verdict = Some(super::super::FinalVerdict {
            kind: super::super::VerdictKind::Approve,
            summary: Some("ready".to_owned()),
            destination: super::super::VerdictDestination::Local,
        });
        let pass = source.annotations_state.lifecycle.pass;
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut restored = app(&after);
        let result = persistence.load_into(&mut restored).unwrap();

        assert!(!result.snapshot_changed);
        assert_eq!(restored.annotations_state.lifecycle.pass, pass);
        assert!(
            restored
                .annotations_state
                .lifecycle
                .file_reviewed("src/lib.rs")
        );
        assert_eq!(
            restored
                .annotations_state
                .lifecycle
                .verdict
                .as_ref()
                .map(|verdict| verdict.kind),
            Some(super::super::VerdictKind::Approve)
        );
    }

    #[test]
    fn unchanged_scoped_reload_preserves_review_lifecycle() {
        let mut source = app(&patch("new"));
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.verdict = Some(super::super::FinalVerdict {
            kind: super::super::VerdictKind::Approve,
            summary: Some("ready".to_owned()),
            destination: super::super::VerdictDestination::Local,
        });
        let replacement = app(&patch("new")).document.changeset.clone();

        source
            .replace_path_changeset(Path::new("src/lib.rs"), replacement)
            .unwrap();

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
    fn source_transition_saves_outgoing_review_and_loads_destination_review() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut source = app(&patch("source"));
        let source_persistence = ReviewPersistence::at(root.clone(), review_identity(&source));
        source.annotations_state.persistence = Some(ReviewPersistenceSession {
            root: root.clone(),
            current: source_persistence.clone(),
            can_save: true,
        });
        source
            .annotations_state
            .annotations
            .insert_human(
                AnnotationKey::for_file_line(
                    &source.document.changeset.files[0],
                    AnnotationSide::New,
                    1,
                )
                .unwrap(),
                "outgoing".to_owned(),
                0,
            )
            .unwrap();

        let mut destination = app(&patch("destination"));
        destination.document.options.source = DiffSource::Show("HEAD".into());
        destination
            .annotations_state
            .annotations
            .insert_human(
                AnnotationKey::for_file_line(
                    &destination.document.changeset.files[0],
                    AnnotationSide::New,
                    1,
                )
                .unwrap(),
                "destination".to_owned(),
                0,
            )
            .unwrap();
        let destination_persistence = ReviewPersistence::at(root, review_identity(&destination));
        destination_persistence.save(&destination).unwrap();

        source.replace_loaded_diff(
            destination.document.options.clone(),
            destination.document.changeset.clone(),
        );

        assert_eq!(
            source
                .annotations_state
                .annotations
                .comments()
                .map(|comment| comment.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["destination"]
        );
        let mut outgoing = app(&patch("source"));
        source_persistence.load_into(&mut outgoing).unwrap();
        assert_eq!(
            outgoing
                .annotations_state
                .annotations
                .comments()
                .map(|comment| comment.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["outgoing"]
        );
    }

    #[test]
    fn source_reload_changes_the_persistence_destination() {
        let temp = tempfile::tempdir().unwrap();
        let mut source = app(&patch("new"));
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        let initial = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        source.document.options.source = DiffSource::Show("HEAD".into());
        let reloaded = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));

        assert_ne!(initial, reloaded);
        reloaded.save(&source).unwrap();
        assert!(initial.load().unwrap().is_none());
        assert!(reloaded.load().unwrap().is_some());
    }

    #[test]
    fn relative_patch_files_in_different_directories_have_distinct_identities() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("changes.diff"), "first").unwrap();
        fs::write(second.join("changes.diff"), "second").unwrap();

        let first_identity = patch_file_source_identity(Path::new("changes.diff"), &first);
        let second_identity = patch_file_source_identity(Path::new("changes.diff"), &second);

        assert_ne!(first_identity, second_identity);
        assert_eq!(
            first_identity,
            format!(
                "patch-file:{}",
                fs::canonicalize(first.join("changes.diff"))
                    .unwrap()
                    .display()
            )
        );
        assert_eq!(
            second_identity,
            format!(
                "patch-file:{}",
                fs::canonicalize(second.join("changes.diff"))
                    .unwrap()
                    .display()
            )
        );
        assert_ne!(
            patch_file_source_identity(Path::new("missing.diff"), &first),
            patch_file_source_identity(Path::new("missing.diff"), &second)
        );
    }

    #[test]
    fn exact_snapshot_round_trips_comments_and_ids() {
        let temp = tempfile::tempdir().unwrap();
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
        source
            .annotations_state
            .annotations
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: key,
                    summary: "answer".to_owned(),
                    rationale: None,
                    author: Some("agent".to_owned()),
                }],
                0,
            )
            .unwrap();
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut restored = app(&patch("new"));
        let result = persistence.load_into(&mut restored).unwrap();

        assert_eq!(result.restored, 2);
        assert!(!result.snapshot_changed);
        assert_eq!(
            restored
                .annotations_state
                .annotations
                .comments()
                .map(|comment| comment.id.as_str())
                .collect::<Vec<_>>(),
            ["human-1", "agent-2"]
        );
        let key = AnnotationKey::for_file_line(
            &restored.document.changeset.files[0],
            AnnotationSide::New,
            1,
        )
        .unwrap();
        let ids = restored
            .annotations_state
            .annotations
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: key,
                    summary: "follow-up".to_owned(),
                    rationale: None,
                    author: None,
                }],
                0,
            )
            .unwrap();
        assert_eq!(ids, ["agent-3"]);
    }

    #[test]
    fn changed_raw_metadata_invalidates_an_unchanged_text_hunk() {
        let temp = tempfile::tempdir().unwrap();
        let before = "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100755\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let after = "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100700\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let mut source = app(before);
        source.annotations_state.lifecycle.mark_file("src/lib.rs");
        source.annotations_state.lifecycle.verdict = Some(super::super::FinalVerdict {
            kind: super::super::VerdictKind::Approve,
            summary: Some("ready".to_owned()),
            destination: super::super::VerdictDestination::Local,
        });
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut changed = app(after);
        let result = persistence.load_into(&mut changed).unwrap();

        assert!(result.snapshot_changed);
        assert_eq!(changed.annotations_state.lifecycle.pass, 2);
        assert!(
            !changed
                .annotations_state
                .lifecycle
                .file_reviewed("src/lib.rs")
        );
        assert!(changed.annotations_state.lifecycle.verdict.is_none());
        assert!(
            changed
                .annotations_state
                .lifecycle
                .changed_files
                .contains("src/lib.rs")
        );
    }

    #[test]
    fn changed_snapshot_marks_unmatched_comment_stale() {
        let temp = tempfile::tempdir().unwrap();
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
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut changed = app(&patch("different"));
        let result = persistence.load_into(&mut changed).unwrap();

        assert_eq!(result.restored, 1);
        assert!(result.snapshot_changed);
        let comment = changed
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, super::super::CommentLifecycle::Stale);
        assert_eq!(changed.annotations_state.annotations.keys().count(), 0);
        assert_eq!(changed.annotations_state.lifecycle.pass, 2);
        assert!(
            changed
                .annotations_state
                .lifecycle
                .changed_files
                .contains("src/lib.rs")
        );
    }

    #[test]
    fn changed_snapshot_moves_a_comment_only_when_evidence_is_unique() {
        let temp = tempfile::tempdir().unwrap();
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
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut changed = app(after);
        persistence.load_into(&mut changed).unwrap();

        let comment = changed
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(comment.lifecycle, super::super::CommentLifecycle::Moved);
        assert_eq!(comment.anchor.line, 3);
        assert_eq!(changed.annotations_state.annotations.keys().count(), 1);
    }

    #[test]
    fn in_process_transition_uses_file_state_when_raw_patch_is_stale() {
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
            .insert_human(key.clone(), "question".to_owned(), 0)
            .unwrap();
        source.annotations_state.annotation_block_scroll = Some((key.clone(), 1));
        source
            .annotations_state
            .annotation_heights
            .borrow_mut()
            .insert(
                key,
                crate::app::AnnotationHeightCacheEntry {
                    text_ptr: 1,
                    text_len: 1,
                    width: 1,
                    height: 1,
                },
            );
        let transition = ReviewTransition::capture(&source);
        let changed = app(after);
        source.document.changeset.files = changed.document.changeset.files;
        source.document.generation = 1;

        transition.apply(&mut source);

        let comment = source
            .annotations_state
            .annotations
            .comments()
            .next()
            .unwrap();
        assert_eq!(source.annotations_state.lifecycle.pass, 2);
        assert_eq!(comment.lifecycle, super::super::CommentLifecycle::Moved);
        assert_eq!(comment.anchor.line, 3);
        assert!(
            source
                .annotations_state
                .annotation_heights
                .borrow()
                .is_empty()
        );
        assert!(source.annotations_state.annotation_block_scroll.is_none());
    }

    #[test]
    fn invalid_captured_comments_report_an_error_instead_of_panicking() {
        let mut source = app(&patch("new"));
        let transition = ReviewTransition {
            fingerprint: "different".to_owned(),
            file_fingerprints: BTreeMap::new(),
            comments: vec![PersistedComment {
                id: "human-1".to_owned(),
                anchor: AnnotationKey {
                    path: "x".repeat(mark_session::MAX_PATH_BYTES + 1),
                    side: AnnotationSide::New,
                    line: 1,
                    scope: AnnotationScope::Line,
                },
                summary: "question".to_owned(),
                rationale: None,
                author: None,
                origin: crate::review::CommentOrigin::Human,
                lifecycle: crate::review::CommentLifecycle::Open,
                disposition: crate::review::FindingDisposition::Open,
                document_generation: 0,
                evidence: ReviewAnchorEvidence::default(),
            }],
        };

        transition.apply(&mut source);

        assert!(source.annotations_state.annotations.is_empty());
        assert_eq!(
            source.notifications.error_log.as_deref(),
            Some("could not restore review comments after reload")
        );
    }

    #[test]
    fn renamed_file_changes_include_old_and_new_path_aliases() {
        let before = "diff --git a/old.rs b/new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,2 +1 @@\n context\n-target\n";
        let after = "diff --git a/old.rs b/new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,3 +1 @@\n-extra\n context\n-target\n";
        let mut source = app(before);
        let key = AnnotationKey::for_file_line(
            &source.document.changeset.files[0],
            AnnotationSide::Old,
            2,
        )
        .unwrap();
        assert_eq!(key.path, "old.rs");
        source
            .annotations_state
            .annotations
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let transition = ReviewTransition::capture(&source);
        let changed = app(after);
        source.document.changeset.files = changed.document.changeset.files;
        source.document.generation = 1;

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
        assert_eq!(comment.lifecycle, super::super::CommentLifecycle::Moved);
        assert_eq!(comment.anchor.path, "old.rs");
        assert_eq!(comment.anchor.line, 3);
    }

    #[test]
    fn human_comments_require_a_persistence_budget_before_insertion() {
        let mut patch = String::from(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1,70 @@\n",
        );
        for line in 1..=70 {
            patch.push_str(&format!("+line {line}\n"));
        }
        let mut app = app(&patch);
        let text = "x".repeat(mark_session::MAX_RATIONALE_BYTES);
        for line in 1..=50 {
            let anchor = AnnotationKey::for_file_line(
                &app.document.changeset.files[0],
                AnnotationSide::New,
                line,
            )
            .unwrap();
            app.annotations_state
                .annotations
                .insert_human(anchor, text.clone(), 0)
                .unwrap();
        }

        let mut rejected = false;
        for line in 51..=70 {
            let anchor = AnnotationKey::for_file_line(
                &app.document.changeset.files[0],
                AnnotationSide::New,
                line,
            )
            .unwrap();
            let budget = human_comment_persistence_budget(&app, &anchor, &text).unwrap();
            let Some(budget) = budget else {
                rejected = true;
                break;
            };
            app.annotations_state
                .annotations
                .insert_human_with_budget(anchor, text.clone(), 0, budget)
                .unwrap();
        }

        assert!(rejected);
        let review = persisted_review(&app, review_identity(&app));
        assert!(persisted_review_fits(&review).unwrap());
    }

    #[test]
    fn lifecycle_progress_and_verdict_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let mut source = app(&patch("new"));
        source
            .annotations_state
            .lifecycle
            .mark_hunk("src/lib.rs", 1);
        source.annotations_state.lifecycle.verdict = Some(super::super::FinalVerdict {
            kind: super::super::VerdictKind::Approve,
            summary: Some("ready".to_owned()),
            destination: super::super::VerdictDestination::Local,
        });
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        let mut restored = app(&patch("new"));
        persistence.load_into(&mut restored).unwrap();

        assert!(
            restored
                .annotations_state
                .lifecycle
                .hunk_reviewed("src/lib.rs", 1)
        );
        assert_eq!(
            restored
                .annotations_state
                .lifecycle
                .verdict
                .as_ref()
                .map(|verdict| verdict.kind),
            Some(super::super::VerdictKind::Approve)
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_review_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
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
            .insert_human(key, "question".to_owned(), 0)
            .unwrap();
        let persistence = ReviewPersistence::at(temp.path().to_owned(), review_identity(&source));
        persistence.save(&source).unwrap();

        assert_eq!(
            fs::metadata(persistence.path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&persistence.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
