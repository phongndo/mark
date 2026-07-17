use super::{
    AsyncJob, DiffApp, EditorReloadBehavior, EditorReloadNavigation, EditorReloadRequest,
    EditorReloadWorker, EditorScopedReload, EditorViewAnchor, FocusedEditorLaunch,
    HunkFocusScrollBehavior, POST_EDITOR_QUIT_KEY_IGNORE, diff_file_matches_path,
    editor_reload_request_for_file, find_rendered_diff_row_outward, repo_relative_path,
};
use crate::editor::{EditorTarget, configured_editor, open_editor, repo_file_path};
use crate::live_diff::LiveDiff;
use crate::model::{DiffLineIndex, FileIndex, HunkIndex, UiRow};
use crate::runtime;
use mark_diff::DiffSource;
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime};
use tokio::sync::oneshot;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64, u64),
}

impl FileFingerprint {
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec(), metadata.ino()),
        })
    }
}

pub(crate) fn file_changed_since(path: &Path, before: Option<FileFingerprint>) -> bool {
    let after = FileFingerprint::read(path);
    match (before, after) {
        (Some(before), Some(after)) => before != after,
        (None, None) => false,
        _ => true,
    }
}

fn update_closest_editor_row(
    closest: &mut Option<(usize, usize, usize)>,
    row: usize,
    distance: usize,
    priority: usize,
) {
    if closest.is_none_or(|(known_row, known_distance, known_priority)| {
        (distance, priority, row) < (known_distance, known_priority, known_row)
    }) {
        *closest = Some((row, distance, priority));
    }
}

fn closest_hunk_diff_line(
    hunk: &mark_diff::DiffHunk,
    target_line: usize,
) -> Option<(usize, usize)> {
    let first_line = hunk.new_start().max(1);
    let last_line = first_line.saturating_add(hunk.new_count().saturating_sub(1));
    let target_line = target_line.clamp(first_line, last_line);
    let last_index = hunk.lines.len().checked_sub(1)?;

    // A target's diff index is bounded by its new-line offset from each end.
    // Search the entire interval: arbitrarily long deletion or metadata runs
    // can place the target anywhere between these bounds.
    let lower_bound = target_line.saturating_sub(first_line).min(last_index);
    let upper_bound = last_index
        .saturating_sub(last_line.saturating_sub(target_line))
        .max(lower_bound);

    let mut closest: Option<(usize, usize, usize)> = None;
    for index in lower_bound..=upper_bound {
        let Some(line) = hunk.lines.get(index).and_then(|line| line.new_line()) else {
            continue;
        };
        if line == target_line {
            return Some((index, line));
        }
        let distance = line.abs_diff(target_line);
        if closest.is_none_or(|(known_index, _, known_distance)| {
            (distance, index) < (known_distance, known_index)
        }) {
            closest = Some((index, line, distance));
        }
    }

    closest.map(|(index, line, _)| (index, line))
}

impl DiffApp {
    #[cfg(test)]
    pub(crate) fn focused_hunk_editor_target(&self) -> Option<EditorTarget> {
        self.focused_hunk_editor_target_and_anchor()
            .map(|(target, _)| target)
    }

    fn focused_hunk_editor_target_and_anchor(&self) -> Option<(EditorTarget, EditorViewAnchor)> {
        if matches!(
            self.document.options.source,
            DiffSource::Patch(_) | DiffSource::Show(_) | DiffSource::Difftool { .. }
        ) {
            return None;
        }

        let (file, hunk) = self.focused_hunk_for_viewport(self.viewport.viewport_rows)?;
        let file_diff = self.document.changeset.files.get(file.get())?;
        let hunk_diff = file_diff.hunks().get(hunk.get())?;
        let path = file_diff.new_path()?;
        let (line, viewport_row) = self
            .focused_hunk_editor_position(file.get(), hunk.get())
            .unwrap_or_else(|| {
                (
                    hunk_diff.new_start().max(1),
                    self.rendered_viewport_focus_row(self.viewport.viewport_rows)
                        .min(self.viewport.viewport_rows.saturating_sub(1)),
                )
            });

        Some((
            EditorTarget {
                path: repo_file_path(&self.document.changeset.repo, path),
                line,
            },
            EditorViewAnchor { line, viewport_row },
        ))
    }

    pub(crate) fn focused_hunk_editor_reload_request(&self) -> Option<EditorReloadRequest> {
        if matches!(
            self.document.options.source,
            DiffSource::Patch(_) | DiffSource::Show(_) | DiffSource::Difftool { .. }
        ) {
            return None;
        }

        let (file, _) = self.focused_hunk_for_viewport(self.viewport.viewport_rows)?;
        editor_reload_request_for_file(self.document.changeset.files.get(file.get())?)
    }

    fn focused_hunk_editor_position(&self, file: usize, hunk: usize) -> Option<(usize, usize)> {
        let rendered_rows = self.rendered_diff_rows_for_viewport(self.viewport.viewport_rows);
        find_rendered_diff_row_outward(
            &rendered_rows,
            self.rendered_viewport_focus_row(self.viewport.viewport_rows),
            |rendered_row| {
                self.editor_line_at_hunk_row(rendered_row.model_row, file, hunk)
                    .map(|line| (line, rendered_row.viewport_row))
            },
        )
    }

    pub(crate) fn editor_line_at_hunk_row(
        &self,
        row_index: usize,
        file: usize,
        hunk: usize,
    ) -> Option<usize> {
        let hunk_diff = self.document.changeset.files.get(file)?.hunks().get(hunk)?;
        match self.document.model.row(row_index)? {
            UiRow::UnifiedLine {
                file: row_file,
                hunk: row_hunk,
                line,
            }
            | UiRow::MetaLine {
                file: row_file,
                hunk: row_hunk,
                line,
            } if row_file.get() == file && row_hunk.get() == hunk => hunk_diff
                .lines
                .get(line.get())?
                .new_line()
                .map(|line| line.max(1)),
            UiRow::SplitLine {
                file: row_file,
                hunk: row_hunk,
                left,
                right,
            } if row_file.get() == file && row_hunk.get() == hunk => right
                .or(left)
                .and_then(|line| hunk_diff.lines.get(line.get()))
                .and_then(|line| line.new_line())
                .map(|line| line.max(1)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn restore_editor_view_for_test(
        &mut self,
        path: &Path,
        line: usize,
        viewport_row: usize,
    ) {
        self.restore_editor_view(path, EditorViewAnchor { line, viewport_row });
    }

    fn restore_editor_view(&mut self, path: &Path, anchor: EditorViewAnchor) {
        let Some(file) = self
            .document
            .changeset
            .files
            .iter()
            .position(|file| diff_file_matches_path(file, path))
        else {
            return;
        };

        let Some(row) = self.closest_editor_view_row(file, anchor.line) else {
            return;
        };

        self.viewport.manual_hunk_focus =
            self.document.model.row(row).and_then(UiRow::typed_hunk_key);
        let scroll = self.scroll_for_model_row_at_viewport_row(row, anchor.viewport_row);
        self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
        self.runtime.dirty = true;
    }

    fn scroll_for_model_row_at_viewport_row(&self, model_row: usize, viewport_row: usize) -> usize {
        let max_scroll = self.max_scroll();
        let target_scroll = self.scroll_for_model_row(model_row).min(max_scroll);
        let viewport_row = viewport_row.min(self.viewport.viewport_rows.saturating_sub(1));
        let preferred_scroll = target_scroll.saturating_sub(viewport_row);
        let mut best = None;

        // Annotation blocks consume viewport slots without advancing the scroll
        // coordinate. Ask the viewport planner where the target actually lands
        // at each nearby scroll instead of treating model and viewport rows as
        // interchangeable.
        for scroll in preferred_scroll..=target_scroll {
            for rendered_row in self
                .rendered_diff_rows_for_viewport_at_scroll(scroll, self.viewport.viewport_rows)
                .into_iter()
                .filter(|rendered_row| rendered_row.model_row == model_row)
            {
                let distance = rendered_row.viewport_row.abs_diff(viewport_row);
                if best.is_none_or(|(known_distance, known_scroll)| {
                    (distance, scroll) < (known_distance, known_scroll)
                }) {
                    best = Some((distance, scroll));
                }
                if distance == 0 {
                    return scroll;
                }
            }
        }

        best.map(|(_, scroll)| scroll)
            .unwrap_or_else(|| self.scroll_with_model_row_rendered(preferred_scroll, model_row))
    }

    fn closest_editor_view_row(&self, file: usize, line: usize) -> Option<usize> {
        let file_diff = self.document.changeset.files.get(file)?;
        let hunks = file_diff.hunks();
        if hunks.is_empty() {
            return None;
        }

        // Hunk new-line ranges are ordered. Only the range immediately before
        // the target and the one immediately after it can contain the nearest
        // rendered line. This avoids materializing every sparse model row.
        let insertion = hunks.partition_point(|hunk| hunk.new_start().max(1) <= line);
        let candidate_hunks = [
            insertion.checked_sub(1),
            (insertion < hunks.len()).then_some(insertion),
        ];
        let mut closest: Option<(usize, usize, usize)> = None;

        for hunk_index in candidate_hunks.into_iter().flatten() {
            let hunk = &hunks[hunk_index];
            let Some(header_row) = self.document.model.hunk_start_row(file, hunk_index) else {
                continue;
            };
            let header_line = hunk.new_start().max(1);
            update_closest_editor_row(&mut closest, header_row, header_line.abs_diff(line), 1);

            if hunk.new_count() == 0 {
                continue;
            }

            let Some((diff_line, diff_line_number)) = closest_hunk_diff_line(hunk, line) else {
                continue;
            };
            let Some(row) = self.document.model.diff_line_row(
                FileIndex::new(file),
                HunkIndex::new(hunk_index),
                DiffLineIndex::new(diff_line),
            ) else {
                continue;
            };
            update_closest_editor_row(&mut closest, row.get(), diff_line_number.abs_diff(line), 0);
        }

        closest.map(|(row, _, _)| row)
    }

    fn editor_reload_navigation(&self) -> EditorReloadNavigation {
        EditorReloadNavigation {
            scroll: self.viewport.scroll,
            selected_file: self.sidebar.selected_file,
            manual_hunk_focus: self.viewport.manual_hunk_focus,
            layout: self.viewport.layout,
            line_wrapping: self.viewport.line_wrapping,
        }
    }

    pub(crate) fn open_focused_hunk_in_editor(&mut self) {
        if let Some(editor) = self.prepare_focused_hunk_editor() {
            self.open_prepared_hunk_in_editor(editor, None);
        }
    }

    pub(crate) fn open_editor_shortcut(&mut self, live_diff: Option<&mut Option<LiveDiff>>) {
        if self.annotations_state.annotation_draft.is_some() {
            self.open_annotation_draft_in_editor();
        } else if let Some(editor) = self.prepare_focused_hunk_editor() {
            self.open_prepared_hunk_in_editor(editor, live_diff);
        }
    }

    pub(super) fn prepare_focused_hunk_editor(&mut self) -> Option<FocusedEditorLaunch> {
        self.prepare_focused_hunk_editor_with(configured_editor())
    }

    pub(super) fn prepare_focused_hunk_editor_with(
        &mut self,
        configured_editor: Option<String>,
    ) -> Option<FocusedEditorLaunch> {
        let Some((target, view_anchor)) = self.focused_hunk_editor_target_and_anchor() else {
            self.set_blocked_notice("no editable focused hunk");
            return None;
        };
        let Some(editor) = configured_editor else {
            self.set_warning_notice("set $GIT_EDITOR, $VISUAL, or $EDITOR to edit focused hunk");
            return None;
        };
        Some(FocusedEditorLaunch {
            target,
            editor,
            view_anchor,
        })
    }

    #[cfg(test)]
    pub(crate) fn prepare_focused_hunk_editor_for_test(
        &mut self,
        configured_editor: Option<String>,
    ) -> bool {
        self.prepare_focused_hunk_editor_with(configured_editor)
            .is_some()
    }

    pub(super) fn open_prepared_hunk_in_editor(
        &mut self,
        editor: FocusedEditorLaunch,
        mut live_diff: Option<&mut Option<LiveDiff>>,
    ) {
        let FocusedEditorLaunch {
            target,
            editor,
            view_anchor,
        } = editor;
        self.close_color_scheme_picker();
        self.overlays.hide_diff_menu();
        self.overlays.hide_options_menu();
        self.close_review_input();
        self.close_branch_menu();
        self.runtime.request_terminal_clear();
        let mut paused_live_diff = false;
        if matches!(self.document.options.source, DiffSource::Worktree)
            && let Some(live_diff) = live_diff.as_mut().and_then(|live_diff| live_diff.as_mut())
        {
            live_diff.set_paused(true);
            paused_live_diff = true;
        }
        let mut scoped_reload = self.focused_hunk_editor_reload_request().or_else(|| {
            repo_relative_path(&self.document.changeset.repo, &target.path).map(|path| {
                let pathspecs = vec![path.clone()];
                EditorReloadRequest {
                    path,
                    pathspecs,
                    view_anchor: None,
                }
            })
        });
        if let Some(request) = &mut scoped_reload {
            request.view_anchor = Some(view_anchor);
        }
        let before = FileFingerprint::read(&target.path);
        let status_result = open_editor(&editor, &target);
        self.jobs.post_editor_quit_key_ignore_until =
            Some(Instant::now() + POST_EDITOR_QUIT_KEY_IGNORE);
        if paused_live_diff
            && let Some(live_diff) = live_diff.as_mut().and_then(|live_diff| live_diff.as_mut())
        {
            live_diff.set_paused(false);
        }

        match status_result {
            Ok(status) if status.success() => {
                let changed = file_changed_since(&target.path, before);
                match self.editor_reload_behavior(
                    changed,
                    scoped_reload.as_ref().map(|request| request.path.as_path()),
                ) {
                    EditorReloadBehavior::None => self.set_notice("editor closed"),
                    EditorReloadBehavior::ScopedAsync => {
                        let request = scoped_reload.expect("scoped reload requires a request");
                        self.queue_editor_scoped_reload(request);
                        self.set_notice("editor closed; refreshing edited file");
                    }
                    EditorReloadBehavior::Sync => match self.reload() {
                        Ok(()) => self.set_notice("editor closed; reloading"),
                        Err(error) => {
                            self.set_error_log(format!("editor closed; reload failed: {error}"))
                        }
                    },
                }
            }
            Ok(status) => {
                self.set_warning_notice(format!("editor exited with {status}"));
            }
            Err(error) => self.set_error_log(format!("editor failed: {error}")),
        }
    }

    pub(crate) fn editor_reload_behavior(
        &self,
        target_changed: bool,
        scoped_path: Option<&Path>,
    ) -> EditorReloadBehavior {
        if !target_changed
            || !matches!(
                self.document.options.source,
                DiffSource::Worktree | DiffSource::Base(_)
            )
        {
            return EditorReloadBehavior::None;
        }

        if scoped_path.is_some() {
            return EditorReloadBehavior::ScopedAsync;
        }

        EditorReloadBehavior::Sync
    }

    pub(crate) fn start_editor_scoped_reload(&mut self, request: EditorReloadRequest) {
        let options = self.document.options.clone();
        let path = request.path;
        let pathspecs = request.pathspecs;
        let view_anchor = request.view_anchor;
        let (tx, rx) = oneshot::channel();
        drop(runtime::spawn_blocking(move || {
            let changeset = mark_diff::load_review_ref_paths(&options, &pathspecs);
            let _ = tx.send(EditorScopedReload {
                path,
                changeset,
                view_anchor,
            });
        }));
        self.jobs.editor_reload = Some(EditorReloadWorker {
            generation: self.document.generation,
            navigation: self.editor_reload_navigation(),
            job: AsyncJob::new(rx),
        });
    }

    pub(crate) fn queue_editor_scoped_reload(&mut self, request: EditorReloadRequest) {
        self.jobs.pending_editor_reload = Some(request);
        self.runtime.dirty = true;
    }

    pub(crate) fn start_pending_editor_reload(&mut self) {
        let Some(request) = self.jobs.pending_editor_reload.take() else {
            return;
        };

        self.start_editor_scoped_reload(request);
    }

    pub(crate) fn drain_editor_reload(&mut self) -> bool {
        let Some(mut worker) = self.jobs.editor_reload.take() else {
            return false;
        };

        match worker.job.try_recv() {
            Ok(reload) => {
                if worker.generation != self.document.generation {
                    return false;
                }

                match reload.changeset {
                    Ok(changeset) => {
                        let restore_view = worker.navigation == self.editor_reload_navigation();
                        self.replace_path_changeset(&reload.path, changeset);
                        if restore_view && let Some(anchor) = reload.view_anchor {
                            self.restore_editor_view(&reload.path, anchor);
                        }
                        self.set_success_notice("edited file reloaded");
                    }
                    Err(error) => self.set_error_log(format!("edited file reload failed: {error}")),
                }
                true
            }
            Err(oneshot::error::TryRecvError::Empty) => {
                self.jobs.editor_reload = Some(worker);
                false
            }
            Err(oneshot::error::TryRecvError::Closed) => {
                self.set_error_log("edited file reload failed");
                true
            }
        }
    }
}
