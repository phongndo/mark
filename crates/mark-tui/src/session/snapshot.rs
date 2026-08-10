use std::cell::RefCell;

use mark_diff::{DiffFile, DiffHunk, DiffLineKind, FileStatus};
use mark_session::{
    DEFAULT_PATCH_BYTES, DEFAULT_REVIEW_FILES, FileSummary, Focus, HunkSummary, MAX_HUNKS_PER_FILE,
    MAX_PATCH_BYTES, MAX_REVIEW_FILES, MIN_PATCH_BYTES, PatchParams, PatchResult, ProtocolError,
    ReviewParams, ReviewResult,
};

use crate::{app::DiffApp, model::UiRow};

use super::{COMMENT_PAGE_RESERVE_BYTES, RESPONSE_RESULT_BUDGET_BYTES, runtime::SessionRuntime};

const MAX_HUNK_HEADER_BYTES: usize = 256;
const DEFAULT_LINE_CONTEXT: usize = 3;

pub(crate) fn focus(app: &DiffApp) -> Option<Focus> {
    let focus_row = app.viewport_focus_row();
    let row = app.document.model.row(focus_row);
    let file_index = app
        .document
        .model
        .file_at_row(focus_row)
        .unwrap_or(app.sidebar.selected_file.get());
    let file = app.document.changeset.files.get(file_index)?;
    let hunk = row
        .and_then(UiRow::typed_hunk_key)
        .filter(|(file, _)| file.get() == file_index)
        .map(|(_, hunk)| hunk.get() + 1)
        .or_else(|| {
            app.focused_hunk_for_viewport(app.viewport.viewport_rows)
                .filter(|(file, _)| file.get() == file_index)
                .map(|(_, hunk)| hunk.get() + 1)
        });
    let (old_line, new_line) = row
        .and_then(|row| focus_coordinates(app, row))
        .unwrap_or((None, None));
    Some(Focus {
        file: file.display_path().to_owned(),
        hunk,
        old_line,
        new_line,
    })
}

fn focus_coordinates(app: &DiffApp, row: UiRow) -> Option<(Option<usize>, Option<usize>)> {
    match row {
        UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
            let line = app
                .document
                .changeset
                .files
                .get(file.get())?
                .hunks()
                .get(hunk.get())?
                .lines
                .get(line.get())?;
            Some((line.old_line(), line.new_line()))
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => {
            let lines = &app
                .document
                .changeset
                .files
                .get(file.get())?
                .hunks()
                .get(hunk.get())?
                .lines;
            let old_line = left
                .get()
                .and_then(|line| lines.get(line.get()))
                .and_then(mark_diff::DiffLine::old_line);
            let new_line = right
                .get()
                .and_then(|line| lines.get(line.get()))
                .and_then(mark_diff::DiffLine::new_line);
            Some((old_line, new_line))
        }
        UiRow::ContextLine {
            old_line, new_line, ..
        } => Some((Some(old_line), Some(new_line))),
        _ => None,
    }
}

pub(crate) fn review(app: &DiffApp, params: ReviewParams) -> Result<ReviewResult, ProtocolError> {
    let max_serialized_bytes = if params.include_comments {
        RESPONSE_RESULT_BUDGET_BYTES.saturating_sub(COMMENT_PAGE_RESERVE_BYTES)
    } else {
        RESPONSE_RESULT_BUDGET_BYTES
    };
    review_with_budget(app, params, max_serialized_bytes)
}

fn review_with_budget(
    app: &DiffApp,
    params: ReviewParams,
    max_serialized_bytes: usize,
) -> Result<ReviewResult, ProtocolError> {
    let start = parse_cursor(params.cursor.as_deref())?;
    let limit = params
        .limit
        .unwrap_or(DEFAULT_REVIEW_FILES)
        .clamp(1, MAX_REVIEW_FILES);
    let matches = |file: &&DiffFile| {
        !params.changed_only
            || app
                .annotations_state
                .lifecycle
                .changed_files
                .contains(file.display_path())
    };
    let total = app.document.changeset.files.iter().filter(matches).count();
    if start > total {
        return Err(ProtocolError::new(
            "invalid_cursor",
            "review cursor is outside the changeset",
        ));
    }

    let stats = &app.document.total_stats;
    let stats = mark_session::ChangeStats {
        files: stats.files,
        additions: stats.additions,
        deletions: stats.deletions,
        binary_files: stats.binary_files,
    };
    let mut files = Vec::with_capacity(limit.min(total.saturating_sub(start)));
    let mut serialized_files_bytes = 0usize;
    for file in app
        .document
        .changeset
        .files
        .iter()
        .filter(matches)
        .skip(start)
        .take(limit)
    {
        let summary = file_summary(app, file);
        let summary_bytes = serde_json::to_vec(&summary)
            .map_err(|error| ProtocolError::new("internal_error", error.to_string()))?
            .len();
        let candidate_files_bytes = serialized_files_bytes
            .saturating_add(usize::from(!files.is_empty()))
            .saturating_add(summary_bytes);
        let candidate_end = start.saturating_add(files.len()).saturating_add(1);
        let candidate_cursor = (candidate_end < total).then(|| candidate_end.to_string());
        let base_bytes = empty_review_serialized_bytes(
            app.document.generation,
            &stats,
            candidate_cursor,
            params.include_comments,
        )?;
        if base_bytes.saturating_add(candidate_files_bytes) > max_serialized_bytes {
            if files.is_empty() {
                return Err(ProtocolError::new(
                    "response_too_large",
                    "one file summary does not fit in the available response budget",
                ));
            }
            break;
        }
        serialized_files_bytes = candidate_files_bytes;
        files.push(summary);
    }

    let end = start.saturating_add(files.len());
    Ok(ReviewResult {
        generation: app.document.generation,
        stats,
        files,
        next_cursor: (end < total).then(|| end.to_string()),
        comments: params.include_comments.then(Vec::new),
        comments_next_cursor: None,
    })
}

fn empty_review_serialized_bytes(
    generation: u64,
    stats: &mark_session::ChangeStats,
    next_cursor: Option<String>,
    include_comments: bool,
) -> Result<usize, ProtocolError> {
    serde_json::to_vec(&ReviewResult {
        generation,
        stats: stats.clone(),
        files: Vec::new(),
        next_cursor,
        comments: include_comments.then(Vec::new),
        comments_next_cursor: None,
    })
    .map(|bytes| bytes.len())
    .map_err(|error| ProtocolError::new("internal_error", error.to_string()))
}

#[cfg(test)]
fn patch(app: &DiffApp, params: PatchParams) -> Result<PatchResult, ProtocolError> {
    let cache = RefCell::new(None);
    patch_with_cache(app, &cache, params)
}

pub(crate) fn session_patch(
    app: &DiffApp,
    runtime: &SessionRuntime,
    params: PatchParams,
) -> Result<PatchResult, ProtocolError> {
    patch_with_cache(app, &runtime.patch_cache, params)
}

fn patch_with_cache(
    app: &DiffApp,
    cache: &RefCell<Option<PatchCache>>,
    params: PatchParams,
) -> Result<PatchResult, ProtocolError> {
    if params.file.is_empty() || params.file.len() > mark_session::MAX_PATH_BYTES {
        return Err(ProtocolError::new(
            "invalid_path",
            "file path is empty or exceeds the byte limit",
        ));
    }
    let file = find_file(app, &params.file)?;
    let selector_count = usize::from(params.hunk.is_some())
        + usize::from(params.old_line.is_some())
        + usize::from(params.new_line.is_some());
    if selector_count > 1 {
        return Err(ProtocolError::new(
            "invalid_selector",
            "choose only one of hunk, old line, or new line",
        ));
    }
    let offset = parse_cursor(params.cursor.as_deref())?;
    let requested_max_bytes = params.max_bytes.unwrap_or(DEFAULT_PATCH_BYTES);
    if requested_max_bytes < MIN_PATCH_BYTES {
        return Err(ProtocolError::new(
            "invalid_patch_limit",
            format!("patch byte limit must be at least {MIN_PATCH_BYTES}"),
        ));
    }
    let max_bytes = requested_max_bytes.min(MAX_PATCH_BYTES);
    if params.old_line.is_some() || params.new_line.is_some() {
        ensure_patch_work_budget(file, PatchSelection::All)?;
    }
    let selection = patch_selection(file, &params)?;
    let file_path = file.display_path();
    let mut cache = cache.borrow_mut();
    if cache.as_ref().is_none_or(|cached| {
        cached.generation != app.document.generation
            || cached.file_path != file_path
            || cached.selection != selection
    }) {
        *cache = Some(PatchCache {
            generation: app.document.generation,
            file_path: file_path.to_owned(),
            selection,
            plan: PatchPlan::build(file, selection)?,
        });
    }
    let plan = &cache
        .as_ref()
        .expect("patch cache should contain the selected patch")
        .plan;
    if offset > plan.total {
        return Err(ProtocolError::new(
            "invalid_cursor",
            "patch cursor is outside the selected patch",
        ));
    }
    let output = plan.render(file, offset, max_bytes);
    if output.invalid_cursor {
        return Err(ProtocolError::new(
            "invalid_cursor",
            "patch cursor is not on a UTF-8 boundary",
        ));
    }
    let next_offset = output.page_end;
    let truncated = next_offset < output.total;
    Ok(PatchResult {
        generation: app.document.generation,
        file: file_path.to_owned(),
        returned_bytes: output.content.len(),
        total_bytes: output.total,
        next_cursor: truncated.then(|| next_offset.to_string()),
        truncated,
        patch: output.content,
    })
}

fn file_summary(app: &DiffApp, file: &DiffFile) -> FileSummary {
    let path = file.display_path();
    let hunks = file
        .hunks()
        .iter()
        .take(MAX_HUNKS_PER_FILE)
        .enumerate()
        .map(|(index, hunk)| HunkSummary {
            index: index + 1,
            reviewed: app
                .annotations_state
                .lifecycle
                .hunk_reviewed(path, index + 1),
            old_start: hunk.old_start(),
            old_count: hunk.old_count(),
            new_start: hunk.new_start(),
            new_count: hunk.new_count(),
            header: truncate_utf8(&hunk.header, MAX_HUNK_HEADER_BYTES),
        })
        .collect::<Vec<_>>();
    FileSummary {
        change_kind: file.status().label().to_owned(),
        reviewed: app.annotations_state.lifecycle.file_reviewed(path),
        old_path: file.old_path().map(str::to_owned),
        new_path: file.new_path().map(str::to_owned),
        additions: file.additions,
        deletions: file.deletions,
        binary: file.is_binary(),
        hunks_truncated: file.hunks().len() > hunks.len(),
        hunks,
    }
}

fn find_file<'a>(app: &'a DiffApp, path: &str) -> Result<&'a DiffFile, ProtocolError> {
    let mut matches = app.document.changeset.files.iter().filter(|file| {
        file.old_path() == Some(path)
            || file.new_path() == Some(path)
            || file.display_path() == path
    });
    let file = matches.next().ok_or_else(|| {
        ProtocolError::new(
            "path_not_found",
            format!("file is not in the loaded changeset: {path}"),
        )
    })?;
    if matches.next().is_some() {
        return Err(ProtocolError::new(
            "ambiguous_path",
            format!("path matches multiple files in the changeset: {path}"),
        ));
    }
    Ok(file)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchSelection {
    All,
    Hunk {
        index: usize,
        lines: Option<(usize, usize)>,
    },
}

fn patch_selection(file: &DiffFile, params: &PatchParams) -> Result<PatchSelection, ProtocolError> {
    if let Some(hunk) = params.hunk {
        let index = hunk
            .checked_sub(1)
            .ok_or_else(|| ProtocolError::new("invalid_selector", "hunk indexes are one-based"))?;
        if index >= file.hunks().len() {
            return Err(ProtocolError::new(
                "hunk_not_found",
                format!("hunk {hunk} does not exist in {}", file.display_path()),
            ));
        }
        return Ok(PatchSelection::Hunk { index, lines: None });
    }
    let coordinate = params
        .old_line
        .map(|line| (true, line))
        .or_else(|| params.new_line.map(|line| (false, line)));
    let Some((old_side, coordinate)) = coordinate else {
        return Ok(PatchSelection::All);
    };
    for (hunk_index, hunk) in file.hunks().iter().enumerate() {
        if let Some(line_index) = hunk.lines.iter().position(|line| {
            if old_side {
                line.old_line() == Some(coordinate)
            } else {
                line.new_line() == Some(coordinate)
            }
        }) {
            let context = params.context.unwrap_or(DEFAULT_LINE_CONTEXT);
            let (selection_start, selection_end) = if hunk.lines[line_index].kind()
                == DiffLineKind::Context
            {
                (line_index, line_index.saturating_add(1))
            } else {
                let mut start = line_index;
                while start > 0 && hunk.lines[start - 1].kind() != DiffLineKind::Context {
                    start -= 1;
                }
                let mut end = line_index.saturating_add(1);
                while end < hunk.lines.len() && hunk.lines[end].kind() != DiffLineKind::Context {
                    end += 1;
                }
                (start, end)
            };
            let start = selection_start.saturating_sub(context);
            let end = selection_end.saturating_add(context).min(hunk.lines.len());
            return Ok(PatchSelection::Hunk {
                index: hunk_index,
                lines: Some((start, end)),
            });
        }
    }
    let side = if old_side { "old" } else { "new" };
    Err(ProtocolError::new(
        "anchor_not_found",
        format!(
            "no {side}-side line {coordinate} exists in {}",
            file.display_path()
        ),
    ))
}

pub(super) struct PatchCache {
    generation: u64,
    file_path: String,
    selection: PatchSelection,
    plan: PatchPlan,
}

struct PatchPlan {
    selection: PatchSelection,
    total: usize,
}

impl PatchPlan {
    fn build(file: &DiffFile, selection: PatchSelection) -> Result<Self, ProtocolError> {
        ensure_patch_work_budget(file, selection)?;
        Ok(Self {
            selection,
            total: patch_total_bytes(file, selection),
        })
    }

    fn render(&self, file: &DiffFile, offset: usize, max: usize) -> BoundedPatch {
        let mut output = BoundedPatch::new(offset, max);
        visit_patch_text(file, self.selection, |text| {
            output.push(text);
            !output.stopped
        });
        output.total = self.total;
        output
    }
}

const MAX_PATCH_WORK_UNITS: usize = 100_000;

fn ensure_patch_work_budget(
    file: &DiffFile,
    selection: PatchSelection,
) -> Result<(), ProtocolError> {
    let mut units = 1usize;
    for (hunk_index, hunk) in file.hunks().iter().enumerate() {
        let lines = match selection {
            PatchSelection::All => hunk.lines.len(),
            PatchSelection::Hunk { index, lines } if index == hunk_index => {
                lines.map_or(hunk.lines.len(), |(start, end)| end.saturating_sub(start))
            }
            PatchSelection::Hunk { .. } => continue,
        };
        units = units.saturating_add(1).saturating_add(lines);
        if units > MAX_PATCH_WORK_UNITS {
            return Err(ProtocolError::new(
                "patch_work_limit",
                "selected patch exceeds the synchronous rendering work limit",
            ));
        }
    }
    Ok(())
}

fn patch_total_bytes(file: &DiffFile, selection: PatchSelection) -> usize {
    let mut total = patch_file_header(file).len();
    if file.is_binary() {
        return total.saturating_add("Binary files differ\n".len());
    }
    for (hunk_index, hunk) in file.hunks().iter().enumerate() {
        let lines = match selection {
            PatchSelection::All => None,
            PatchSelection::Hunk { index, lines } if index == hunk_index => lines,
            PatchSelection::Hunk { .. } => continue,
        };
        let (start, end, header_bytes) = match lines {
            Some((start, end)) => (start, end, excerpt_hunk_header(hunk, start, end).len()),
            None => (0, hunk.lines.len(), hunk.header.len()),
        };
        total = total.saturating_add(header_bytes).saturating_add(1);
        for line in &hunk.lines[start..end] {
            total = total
                .saturating_add(1)
                .saturating_add(lossy_utf8_len(line.text_bytes()))
                .saturating_add(1);
        }
    }
    if matches!(file.status(), FileStatus::Unknown) && file.hunks().is_empty() {
        total = total.saturating_add("File metadata changed\n".len());
    }
    total
}

fn visit_patch_text(
    file: &DiffFile,
    selection: PatchSelection,
    mut visit: impl FnMut(&str) -> bool,
) {
    let file_header = patch_file_header(file);
    if !visit(&file_header) {
        return;
    }
    if file.is_binary() {
        let _ = visit("Binary files differ\n");
        return;
    }
    for (hunk_index, hunk) in file.hunks().iter().enumerate() {
        let lines = match selection {
            PatchSelection::All => None,
            PatchSelection::Hunk { index, lines } if index == hunk_index => lines,
            PatchSelection::Hunk { .. } => continue,
        };
        let (start, end, header) = match lines {
            Some((start, end)) => (start, end, excerpt_hunk_header(hunk, start, end)),
            None => (0, hunk.lines.len(), hunk.header.clone()),
        };
        if !visit(&format!("{header}\n")) {
            return;
        }
        for line in &hunk.lines[start..end] {
            if !visit(line_prefix(line.kind())) {
                return;
            }
            let text = line.text_lossy();
            if !visit(&text) || !visit("\n") {
                return;
            }
        }
    }
    if matches!(file.status(), FileStatus::Unknown) && file.hunks().is_empty() {
        let _ = visit("File metadata changed\n");
    }
}

fn patch_file_header(file: &DiffFile) -> String {
    let old_path = file.old_path();
    let new_path = file.new_path();
    let diff_old_path = old_path.or(new_path).unwrap_or("/dev/null");
    let diff_new_path = new_path.or(old_path).unwrap_or("/dev/null");
    let old_header = old_path.map_or_else(|| "/dev/null".to_owned(), |path| format!("a/{path}"));
    let new_header = new_path.map_or_else(|| "/dev/null".to_owned(), |path| format!("b/{path}"));
    format!("diff --git a/{diff_old_path} b/{diff_new_path}\n--- {old_header}\n+++ {new_header}\n")
}

fn excerpt_hunk_header(hunk: &DiffHunk, start: usize, end: usize) -> String {
    let old_before = hunk.lines[..start]
        .iter()
        .filter(|line| line.old_line().is_some())
        .count();
    let new_before = hunk.lines[..start]
        .iter()
        .filter(|line| line.new_line().is_some())
        .count();
    let lines = &hunk.lines[start..end];
    let old_count = lines
        .iter()
        .filter(|line| line.old_line().is_some())
        .count();
    let new_count = lines
        .iter()
        .filter(|line| line.new_line().is_some())
        .count();
    let old_cursor = hunk.old_start().saturating_add(old_before);
    let new_cursor = hunk.new_start().saturating_add(new_before);
    let old_start = if old_count == 0 {
        old_cursor.saturating_sub(1)
    } else {
        old_cursor
    };
    let new_start = if new_count == 0 {
        new_cursor.saturating_sub(1)
    } else {
        new_cursor
    };
    let suffix = hunk
        .header
        .strip_prefix("@@")
        .and_then(|header| header.split_once("@@"))
        .map_or("", |(_, suffix)| suffix);
    format!(
        "@@ -{} +{} @@{suffix}",
        format_hunk_range(old_start, old_count),
        format_hunk_range(new_start, new_count)
    )
}

fn format_hunk_range(start: usize, count: usize) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

fn line_prefix(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Context => " ",
        DiffLineKind::Addition => "+",
        DiffLineKind::Deletion => "-",
        DiffLineKind::Meta => "\\",
    }
}

fn lossy_utf8_len(mut bytes: &[u8]) -> usize {
    let mut len = 0usize;
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => return len.saturating_add(text.len()),
            Err(error) => {
                len = len
                    .saturating_add(error.valid_up_to())
                    .saturating_add('\u{FFFD}'.len_utf8());
                let invalid_start = error.valid_up_to();
                let Some(invalid_len) = error.error_len() else {
                    return len;
                };
                bytes = &bytes[invalid_start.saturating_add(invalid_len)..];
            }
        }
    }
    len
}

struct BoundedPatch {
    offset: usize,
    max: usize,
    total: usize,
    page_end: usize,
    invalid_cursor: bool,
    stopped: bool,
    content: String,
}

impl BoundedPatch {
    fn new(offset: usize, max: usize) -> Self {
        Self {
            offset,
            max,
            total: 0,
            page_end: offset,
            invalid_cursor: false,
            stopped: false,
            content: String::with_capacity(max.min(16 * 1024)),
        }
    }

    fn push(&mut self, text: &str) {
        let piece_start = self.total;
        self.total = self.total.saturating_add(text.len());
        if self.stopped || self.total <= self.offset {
            return;
        }

        let local_start = self.offset.saturating_sub(piece_start).min(text.len());
        if !text.is_char_boundary(local_start) {
            self.invalid_cursor = true;
            self.stopped = true;
            return;
        }
        let remaining = self.max.saturating_sub(self.content.len());
        let local_end =
            previous_char_boundary(text, local_start.saturating_add(remaining).min(text.len()));
        if local_end > local_start {
            self.content.push_str(&text[local_start..local_end]);
        }
        self.page_end = piece_start.saturating_add(local_end);
        self.stopped = local_end < text.len() || self.content.len() >= self.max;
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

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    let end = previous_char_boundary(text, text.len().min(max_bytes));
    text[..end].to_owned()
}

fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, sync::Arc};

    use mark_diff::{Changeset, DiffLine, DiffOptions, HunkLineRanges, RepoRoot};

    use crate::{app::DiffApp, controls::DiffLayoutMode};

    use super::*;

    fn app() -> DiffApp {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old value\n+new value\n";
        app_from_patch(patch)
    }

    fn app_from_patch(patch: &str) -> DiffApp {
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
    fn context_focus_reports_the_visible_source_line() {
        let mut app = app();
        let added_row = (0..app.document.model.len())
            .find(|row| {
                matches!(
                    app.document.model.row(*row),
                    Some(UiRow::UnifiedLine { line, .. }) if line.get() == 1
                )
            })
            .unwrap();
        app.viewport.viewport_rows = 1;
        app.viewport.scroll = added_row;

        let focus = focus(&app).unwrap();
        assert_eq!(focus.file, "src/lib.rs");
        assert_eq!(focus.hunk, Some(1));
        assert_eq!(focus.old_line, None);
        assert_eq!(focus.new_line, Some(1));
    }

    #[test]
    fn review_returns_structure_without_patch_text() {
        let result = review(
            &app(),
            ReviewParams {
                cursor: None,
                limit: None,
                include_comments: false,
                comments_cursor: None,
                comments_limit: None,
                changed_only: false,
            },
        )
        .unwrap();

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].hunks.len(), 1);
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("new value")
        );
    }

    #[test]
    fn review_pages_are_bounded_by_their_serialized_json_size() {
        let escaped_header = "\u{1}".repeat(200);
        let patch = format!(
            "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@ {escaped_header}\n-old\n+new\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@ {escaped_header}\n-old\n+new\n"
        );
        let app = app_from_patch(&patch);
        let complete = review_with_budget(&app, ReviewParams::default(), usize::MAX).unwrap();
        assert_eq!(complete.files.len(), 2);
        assert!(
            serde_json::to_string(&complete.files[0])
                .unwrap()
                .contains("\\u0001")
        );
        let one_file = ReviewResult {
            generation: complete.generation,
            stats: complete.stats.clone(),
            files: vec![complete.files[0].clone()],
            next_cursor: Some("1".to_owned()),
            comments: None,
            comments_next_cursor: None,
        };
        let budget = serde_json::to_vec(&one_file).unwrap().len() + 64;

        let first = review_with_budget(&app, ReviewParams::default(), budget).unwrap();

        assert_eq!(first.files.len(), 1);
        assert_eq!(first.files[0].new_path.as_deref(), Some("src/a.rs"));
        assert_eq!(first.next_cursor.as_deref(), Some("1"));
        assert!(serde_json::to_vec(&first).unwrap().len() <= budget);

        let second = review_with_budget(
            &app,
            ReviewParams {
                cursor: first.next_cursor,
                ..ReviewParams::default()
            },
            budget,
        )
        .unwrap();
        assert_eq!(second.files.len(), 1);
        assert_eq!(second.files[0].new_path.as_deref(), Some("src/b.rs"));
        assert_eq!(second.next_cursor, None);

        let error = review_with_budget(&app, ReviewParams::default(), 1).unwrap_err();
        assert_eq!(error.code, "response_too_large");
    }

    #[test]
    fn review_filters_changed_files_and_reports_progress() {
        let mut app = app();
        app.annotations_state
            .lifecycle
            .changed_files
            .insert("src/lib.rs".to_owned());
        app.annotations_state.lifecycle.mark_hunk("src/lib.rs", 1);

        let result = review(
            &app,
            ReviewParams {
                changed_only: true,
                ..ReviewParams::default()
            },
        )
        .unwrap();

        assert_eq!(result.files.len(), 1);
        assert!(!result.files[0].reviewed);
        assert!(result.files[0].hunks[0].reviewed);
    }

    #[test]
    fn patch_response_is_bounded_and_continuable() {
        let result = patch(
            &app(),
            PatchParams {
                file: "src/lib.rs".to_owned(),
                max_bytes: Some(24),
                ..PatchParams::default()
            },
        )
        .unwrap();

        assert!(result.returned_bytes <= 24);
        assert!(result.truncated);
        assert!(result.next_cursor.is_some());
        assert!(result.total_bytes > result.returned_bytes);
    }

    #[test]
    fn line_targeted_patch_header_matches_the_emitted_excerpt() {
        let mut excerpt_app = app();
        let hunk = &mut excerpt_app.document.changeset.files[0].hunks_mut()[0];
        hunk.header = "@@ -1,3 +1,3 @@ section".to_owned();
        hunk.ranges = HunkLineRanges::new(1, 3, 1, 3);
        hunk.lines = vec![
            DiffLine::context(1, 1, "first".to_owned()),
            DiffLine::context(2, 2, "target".to_owned()),
            DiffLine::context(3, 3, "last".to_owned()),
        ];

        let excerpt = patch(
            &excerpt_app,
            PatchParams {
                file: "src/lib.rs".to_owned(),
                new_line: Some(2),
                context: Some(0),
                ..PatchParams::default()
            },
        )
        .unwrap();

        assert!(excerpt.patch.contains("@@ -2 +2 @@ section\n target\n"));
        assert!(!excerpt.patch.contains("@@ -1,3 +1,3 @@"));

        let replacement = patch(
            &app(),
            PatchParams {
                file: "src/lib.rs".to_owned(),
                new_line: Some(1),
                context: Some(0),
                ..PatchParams::default()
            },
        )
        .unwrap();
        assert!(
            replacement
                .patch
                .contains("@@ -1 +1 @@\n-old value\n+new value\n")
        );
    }

    #[test]
    fn patch_pages_reuse_the_cached_selection_plan() {
        let app = app();
        let cache = RefCell::new(None);
        let first = patch_with_cache(
            &app,
            &cache,
            PatchParams {
                file: "src/lib.rs".to_owned(),
                max_bytes: Some(24),
                ..PatchParams::default()
            },
        )
        .unwrap();
        let plan = {
            let cached = cache.borrow();
            std::ptr::from_ref(&cached.as_ref().unwrap().plan)
        };

        patch_with_cache(
            &app,
            &cache,
            PatchParams {
                file: "src/lib.rs".to_owned(),
                max_bytes: Some(24),
                cursor: first.next_cursor,
                ..PatchParams::default()
            },
        )
        .unwrap();

        let reused_plan = {
            let cached = cache.borrow();
            std::ptr::from_ref(&cached.as_ref().unwrap().plan)
        };
        assert_eq!(reused_plan, plan);
    }

    #[test]
    fn patch_limit_cannot_be_smaller_than_one_utf8_scalar() {
        let error = patch(
            &app(),
            PatchParams {
                file: "src/lib.rs".to_owned(),
                max_bytes: Some(MIN_PATCH_BYTES - 1),
                ..PatchParams::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "invalid_patch_limit");
    }

    #[test]
    fn lossy_patch_lengths_match_rendered_invalid_utf8() {
        for bytes in [
            b"valid".as_slice(),
            b"bad\xfftext".as_slice(),
            b"cut\xf0\x9f".as_slice(),
        ] {
            assert_eq!(lossy_utf8_len(bytes), String::from_utf8_lossy(bytes).len());
        }
    }

    #[test]
    fn bounded_patch_advances_by_complete_multibyte_characters() {
        let mut first = BoundedPatch::new(0, MIN_PATCH_BYTES);
        first.push("a");
        first.push("🙂");
        first.push("z");
        assert_eq!(first.content, "a");
        assert_eq!(first.page_end, 1);
        assert_eq!(first.total, 6);

        let mut second = BoundedPatch::new(first.page_end, MIN_PATCH_BYTES);
        second.push("a");
        second.push("🙂");
        second.push("z");
        assert_eq!(second.content, "🙂");
        assert_eq!(second.page_end, 5);
        assert!(!second.invalid_cursor);
    }

    #[test]
    fn bounded_patch_rejects_a_cursor_inside_a_multibyte_character() {
        let mut output = BoundedPatch::new(2, MIN_PATCH_BYTES);
        output.push("a🙂z");

        assert!(output.invalid_cursor);
        assert!(output.content.is_empty());
    }
}
