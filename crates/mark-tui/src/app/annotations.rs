use super::{
    DiffApp, HunkFocusScrollBehavior, POST_EDITOR_QUIT_KEY_IGNORE, create_annotation_scratch_file,
    normalize_annotation_editor_contents, viewport_center_offset,
};
use crate::annotation::{
    AnnotationDraft, AnnotationKey, AnnotationKeyIndex, AnnotationScope, AnnotationSide,
};
use crate::editor::{configured_editor, open_text_in_editor};
use crate::keymap::{AnnotationMenuAction, GlobalAction, MenuAction};
use crate::model::{DiffLineIndex, FileIndex, HunkIndex, UiRow};
use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows_at_scroll};
use crate::review::FindingDisposition;
use crate::selector::{SelectorController, SelectorMovement};
use crate::syntax::DiffSide;
use crate::text_input::{TextInputKeyResult, handle_text_input_key};
use crossterm::event::{KeyCode, KeyEvent};
use mark_core::MarkResult;
use mark_diff::FileStatus;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone)]
pub(crate) struct AnnotationMenuItem {
    pub(crate) key: AnnotationKey,
    pub(crate) model_row: usize,
    pub(crate) anchor_scroll: usize,
    pub(crate) status: FileStatus,
    pub(crate) label: String,
    pub(crate) preview: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationEditMode {
    Inline,
    External,
}

impl DiffApp {
    pub(crate) fn open_annotation_menu(&mut self) {
        if self.annotations_state.annotations.is_empty() {
            self.set_notice("no annotations");
            return;
        }
        self.close_color_scheme_picker();
        self.overlays.annotation_menu.reset();
        self.overlays.open_annotation_menu();
        self.overlays.hide_diff_menu();
        self.overlays.hide_options_menu();
        self.close_branch_menu();
        self.close_review_input();
        self.close_commit_menu();
        self.runtime.dirty = true;
    }

    pub(crate) fn close_annotation_menu(&mut self) {
        if self.overlays.close_annotation_menu() {
            self.runtime.hit_map.annotation_menu_area = None;
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn annotation_menu_items(&self) -> Vec<AnnotationMenuItem> {
        let mut items = self
            .annotations_state
            .annotations
            .iter()
            .filter_map(|(key, text)| {
                let model_row = self.annotation_model_row(key)?;
                let status = self
                    .document
                    .changeset
                    .files
                    .iter()
                    .find(|file| {
                        file.old_path() == Some(key.path.as_str())
                            || file.new_path() == Some(key.path.as_str())
                    })
                    .map(|file| file.status())
                    .unwrap_or(FileStatus::Unknown);
                Some(AnnotationMenuItem {
                    key: key.clone(),
                    model_row,
                    anchor_scroll: self.annotation_anchor_visual_scroll(model_row),
                    status,
                    label: self
                        .annotation_label(key)
                        .unwrap_or_else(|| format!("{}", key.line)),
                    preview: text
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .to_owned(),
                    text: text.clone(),
                })
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| (a.anchor_scroll, &a.label).cmp(&(b.anchor_scroll, &b.label)));
        items
    }

    pub(crate) fn filtered_annotation_menu_items(&self) -> Vec<AnnotationMenuItem> {
        let query = self
            .overlays
            .annotation_menu
            .input
            .trim()
            .to_ascii_lowercase();
        self.annotation_menu_items()
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.label.to_ascii_lowercase().contains(&query)
                    || item.preview.to_ascii_lowercase().contains(&query)
            })
            .collect()
    }

    pub(crate) fn move_annotation_menu_selection(&mut self, delta: isize) {
        let len = self.filtered_annotation_menu_items().len();
        if SelectorController::new(&mut self.overlays.annotation_menu, len)
            .move_by(delta, SelectorMovement::Saturating)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_annotation_menu_selection(&mut self, selected: usize) {
        let len = self.filtered_annotation_menu_items().len();
        if SelectorController::new(&mut self.overlays.annotation_menu, len).set_selected(selected) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn ensure_annotation_menu_selection_visible(&mut self, visible_rows: usize) {
        let len = self.filtered_annotation_menu_items().len();
        self.overlays
            .annotation_menu
            .ensure_selected_visible(len, visible_rows);
    }

    pub(crate) fn handle_annotation_menu_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.config.keymap.matches_menu(MenuAction::Close, key) {
            self.close_annotation_menu();
            return Ok(false);
        }
        if self.config.keymap.matches_menu(MenuAction::Down, key) {
            self.move_annotation_menu_selection(1);
        } else if self.config.keymap.matches_menu(MenuAction::Up, key) {
            self.move_annotation_menu_selection(-1);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Jump, key)
        {
            self.edit_selected_annotation(AnnotationEditMode::Inline);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::EditExternal, key)
        {
            self.edit_selected_annotation(AnnotationEditMode::External);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Remove, key)
        {
            self.remove_selected_annotation();
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Accept, key)
        {
            self.set_selected_agent_disposition(FindingDisposition::Accepted);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Dismiss, key)
        {
            self.set_selected_agent_disposition(FindingDisposition::Dismissed);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Blocking, key)
        {
            self.set_selected_agent_disposition(FindingDisposition::Blocking);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::NonBlocking, key)
        {
            self.set_selected_agent_disposition(FindingDisposition::NonBlocking);
        } else if self
            .config
            .keymap
            .matches_annotation_menu(AnnotationMenuAction::Fixed, key)
        {
            self.set_selected_agent_disposition(FindingDisposition::Fixed);
        } else {
            let len = self.filtered_annotation_menu_items().len();
            let outcome = SelectorController::new(&mut self.overlays.annotation_menu, len)
                .apply_input_key(key);
            if outcome.handled() {
                if outcome.changed() {
                    self.runtime.dirty = true;
                }
                return Ok(false);
            }
            match key.code {
                KeyCode::PageDown => self.move_annotation_menu_selection(10),
                KeyCode::PageUp => self.move_annotation_menu_selection(-10),
                KeyCode::Home => self.set_annotation_menu_selection(0),
                KeyCode::End => self.set_annotation_menu_selection(usize::MAX),
                _ => {}
            }
        }
        Ok(false)
    }

    fn selected_annotation_menu_item(&self) -> Option<AnnotationMenuItem> {
        self.filtered_annotation_menu_items()
            .get(self.overlays.annotation_menu.selected)
            .cloned()
    }

    fn edit_selected_annotation(&mut self, mode: AnnotationEditMode) {
        let Some(item) = self.selected_annotation_menu_item() else {
            return;
        };
        self.close_annotation_menu();
        self.jump_to_annotation(&item.key);
        if !self.open_annotation_draft_for_key(item.key, item.model_row) {
            return;
        }
        if mode == AnnotationEditMode::External {
            self.open_annotation_draft_in_editor();
        }
        self.runtime.dirty = true;
    }

    fn set_selected_agent_disposition(&mut self, disposition: FindingDisposition) {
        let Some(item) = self.selected_annotation_menu_item() else {
            return;
        };
        let changed = self
            .annotations_state
            .annotations
            .set_agents_disposition_at(&item.key, disposition);
        if changed == 0 {
            self.set_notice("no agent findings at annotation");
            return;
        }
        self.annotations_state.annotation_block_scroll = None;
        self.annotations_state
            .annotation_rows
            .borrow_mut()
            .remove(&item.key);
        *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
        self.annotations_state
            .annotation_heights
            .borrow_mut()
            .remove(&item.key);
        let len = self.filtered_annotation_menu_items().len();
        self.overlays.annotation_menu.clamp(len);
        if len == 0 {
            self.close_annotation_menu();
        }
        self.set_scroll_with_grep_sync(
            self.viewport.scroll,
            false,
            HunkFocusScrollBehavior::Preserve,
        );
        self.sync_annotation_cursor_to_viewport();
        self.set_notice(match disposition {
            FindingDisposition::Accepted => "finding accepted",
            FindingDisposition::Dismissed => "finding dismissed",
            FindingDisposition::Blocking => "finding marked blocking",
            FindingDisposition::NonBlocking => "finding marked non-blocking",
            FindingDisposition::Fixed => "finding marked fixed",
            FindingDisposition::Open => "finding reopened",
        });
        self.runtime.dirty = true;
    }

    fn remove_selected_annotation(&mut self) {
        let Some(item) = self.selected_annotation_menu_item() else {
            return;
        };
        if self
            .annotations_state
            .annotations
            .remove(&item.key)
            .is_none()
        {
            return;
        }
        self.annotations_state.annotation_block_scroll = None;
        if !self.annotations_state.annotations.contains_key(&item.key) {
            self.annotations_state
                .annotation_rows
                .borrow_mut()
                .remove(&item.key);
            *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
        }
        self.annotations_state
            .annotation_heights
            .borrow_mut()
            .remove(&item.key);
        let len = self.filtered_annotation_menu_items().len();
        self.overlays.annotation_menu.clamp(len);
        if len == 0 {
            self.close_annotation_menu();
        }
        self.set_scroll_with_grep_sync(
            self.viewport.scroll,
            false,
            HunkFocusScrollBehavior::Preserve,
        );
        self.sync_annotation_cursor_to_viewport();
        self.runtime.dirty = true;
    }

    pub(crate) fn jump_to_annotation(&mut self, key: &AnnotationKey) {
        let Some(target_model_row) = self.annotation_model_row(key) else {
            return;
        };
        let target_anchor = self.annotation_anchor_visual_scroll(target_model_row);
        let target_scroll =
            target_anchor.saturating_sub(viewport_center_offset(self.viewport.viewport_rows));
        let target_scroll = self.scroll_with_model_row_rendered(target_scroll, target_model_row);
        self.set_scroll_with_grep_sync(
            target_scroll.min(self.max_scroll()),
            false,
            HunkFocusScrollBehavior::Preserve,
        );
    }

    pub(crate) fn cache_annotation_model_rows(&self) {
        let mut missing = {
            let cached = self.annotations_state.annotation_rows.borrow();
            self.annotations_state
                .annotations
                .keys()
                .filter(|key| !cached.contains_key(*key))
                .cloned()
                .collect::<HashSet<_>>()
        };
        if missing.is_empty() {
            return;
        }
        if missing.len() == 1 {
            let key = missing.into_iter().next().expect("one missing annotation");
            let _ = self.annotation_model_row(&key);
            return;
        }

        let mut found = Vec::with_capacity(missing.len());
        for (index, row) in self.document.model.iter_rows().enumerate() {
            for key in AnnotationKey::candidates_from_ui_row(&self.document.changeset, row) {
                if missing.remove(&key) {
                    found.push((key, Some(index)));
                    if missing.is_empty() {
                        break;
                    }
                }
            }
            if missing.is_empty() {
                break;
            }
        }
        if !missing.is_empty() {
            let file_indices_by_side_path =
                annotation_file_indices_by_side_path(&self.document.changeset);
            found.extend(missing.into_iter().map(|key| {
                let model_row = file_indices_by_side_path
                    .get(&(key.side, key.path.as_str()))
                    .and_then(|file_indices| {
                        file_indices.iter().find_map(|file_index| {
                            let file = self.document.changeset.files.get(*file_index)?;
                            self.find_annotation_model_row_in_file(
                                &key,
                                FileIndex::new(*file_index),
                                file,
                            )
                        })
                    });
                (key, model_row)
            }));
        }
        self.annotations_state
            .annotation_rows
            .borrow_mut()
            .extend(found);
        *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
    }

    fn cache_annotation_keys_by_model_row(&self) {
        if self
            .annotations_state
            .annotation_keys_by_row
            .borrow()
            .is_some()
        {
            return;
        }
        self.cache_annotation_model_rows();
        let mut index = AnnotationKeyIndex::default();
        let annotation_rows = self.annotations_state.annotation_rows.borrow();
        for key in self.annotations_state.annotations.keys() {
            let Some(model_row) = annotation_rows.get(key).copied().flatten() else {
                continue;
            };
            index
                .anchors_by_model_row
                .entry(model_row)
                .or_default()
                .push(key.clone());
        }
        for keys in index.anchors_by_model_row.values_mut() {
            keys.sort_unstable();
        }
        *self.annotations_state.annotation_keys_by_row.borrow_mut() = Some(index);
    }

    pub(crate) fn annotation_keys_at_model_row(
        &self,
        model_row: usize,
        row: UiRow,
    ) -> Vec<AnnotationKey> {
        let mut keys = AnnotationKey::candidates_from_ui_row(&self.document.changeset, row);
        if let Some(draft) = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .filter(|draft| draft.model_row_index == model_row)
            && !keys.contains(&draft.key)
        {
            keys.push(draft.key.clone());
        }
        self.cache_annotation_keys_by_model_row();
        if let Some(anchored_keys) = self
            .annotations_state
            .annotation_keys_by_row
            .borrow()
            .as_ref()
            .and_then(|index| index.anchors_by_model_row.get(&model_row))
        {
            for key in anchored_keys {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        keys
    }

    pub(crate) fn annotation_model_row(&self, key: &AnnotationKey) -> Option<usize> {
        if let Some(model_row) = self.annotations_state.annotation_rows.borrow().get(key) {
            return *model_row;
        }

        let model_row = self.find_annotation_model_row(key);
        self.annotations_state
            .annotation_rows
            .borrow_mut()
            .insert(key.clone(), model_row);
        *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
        model_row
    }

    fn find_annotation_model_row(&self, key: &AnnotationKey) -> Option<usize> {
        self.document
            .changeset
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                AnnotationKey::path_for_side(file, key.side) == Some(key.path.as_str())
            })
            .find_map(|(file_index, file)| {
                self.find_annotation_model_row_in_file(key, FileIndex::new(file_index), file)
            })
    }

    fn find_annotation_model_row_in_file(
        &self,
        key: &AnnotationKey,
        file_index: FileIndex,
        file: &mark_diff::DiffFile,
    ) -> Option<usize> {
        match key.scope {
            AnnotationScope::File => return self.document.model.file_start_row(file_index.get()),
            AnnotationScope::Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
            } => {
                let hunk = file.hunks().iter().position(|hunk| {
                    hunk.old_start() == old_start
                        && hunk.old_count() == old_count
                        && hunk.new_start() == new_start
                        && hunk.new_count() == new_count
                })?;
                let hunk_index = HunkIndex::new(hunk);
                return self
                    .document
                    .model
                    .hunk_header_row(file_index, hunk_index)
                    .map(crate::model::ModelRow::get)
                    .or_else(|| {
                        self.document
                            .model
                            .hunk_row_range(file_index.get(), hunk_index.get())
                            .and_then(|range| (!range.is_empty()).then_some(range.start))
                    });
            }
            AnnotationScope::Line => {}
        }
        let model_row =
            self.model_row_for_source_coordinate(file_index, file, key.side, key.line)?;
        let row = self.document.model.row(model_row)?;
        AnnotationKey::candidates_from_ui_row(&self.document.changeset, row)
            .contains(key)
            .then_some(model_row)
    }

    fn model_row_for_source_coordinate(
        &self,
        file_index: FileIndex,
        file: &mark_diff::DiffFile,
        side: AnnotationSide,
        line: usize,
    ) -> Option<usize> {
        for (hunk_index, hunk) in file.hunks().iter().enumerate() {
            let line_index = hunk.lines.iter().position(|candidate| match side {
                AnnotationSide::Old => candidate.old_line() == Some(line),
                AnnotationSide::New => candidate.new_line() == Some(line),
            });
            let Some(line_index) = line_index else {
                continue;
            };
            if let Some(model_row) = self.document.model.diff_line_row(
                file_index,
                HunkIndex::new(hunk_index),
                DiffLineIndex::new(line_index),
            ) {
                return Some(model_row.get());
            }
        }
        let side = match side {
            AnnotationSide::Old => DiffSide::Old,
            AnnotationSide::New => DiffSide::New,
        };
        self.document
            .model
            .context_line_row_for_side(file_index, side, line)
            .map(crate::model::ModelRow::get)
    }

    pub(crate) fn move_annotation(&mut self, delta: isize) {
        if self.annotations_state.annotations.is_empty() {
            self.set_notice("no annotations");
            return;
        }

        let mut targets = self
            .annotations_state
            .annotations
            .keys()
            .filter_map(|key| {
                let row = self.annotation_model_row(key)?;
                Some((self.annotation_anchor_visual_scroll(row), row, key.clone()))
            })
            .collect::<Vec<_>>();
        targets.sort_unstable_by(|left, right| {
            (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2))
        });

        if targets.is_empty() {
            self.set_notice("annotations are hidden");
            return;
        }

        let focus_scroll = self.annotation_navigation_focus_scroll();
        let selected = self
            .annotation_cursor_target()
            .and_then(|current| targets.iter().position(|target| target.2 == current.key));
        let target = if let Some(selected) = selected {
            let index = if delta < 0 {
                selected.checked_sub(1).unwrap_or(targets.len() - 1)
            } else {
                selected.saturating_add(1) % targets.len()
            };
            targets[index].clone()
        } else if delta < 0 {
            targets
                .iter()
                .rev()
                .find(|target| target.0 < focus_scroll)
                .cloned()
                .unwrap_or_else(|| {
                    targets
                        .last()
                        .expect("annotation targets should not be empty")
                        .clone()
                })
        } else {
            targets
                .iter()
                .find(|target| target.0 > focus_scroll)
                .cloned()
                .unwrap_or_else(|| targets[0].clone())
        };
        let (target_anchor, target_model_row, target_key) = target;
        let target_scroll =
            target_anchor.saturating_sub(viewport_center_offset(self.viewport.viewport_rows));
        let target_scroll = self.scroll_with_model_row_rendered(target_scroll, target_model_row);

        self.set_scroll_with_grep_sync(
            target_scroll.min(self.max_scroll()),
            false,
            HunkFocusScrollBehavior::Preserve,
        );
        if self.annotation_cursor_enabled() {
            self.select_annotation_cursor(&target_key);
        }
    }

    pub(super) fn annotation_navigation_focus_scroll(&self) -> usize {
        let focus_viewport_row = self.rendered_viewport_focus_row(self.viewport.viewport_rows);
        let plans = plan_diff_viewport_rows_at_scroll(
            self,
            self.viewport.scroll,
            self.viewport.viewport_rows.max(1),
        );

        let Some(slot) = plans.get(focus_viewport_row).or_else(|| plans.last()) else {
            return self.viewport.scroll.saturating_add(focus_viewport_row);
        };
        // When the viewport focus lands inside an annotation block, navigate from
        // that block's owner row instead of a raw scroll position hidden by notes.
        match &slot.kind {
            ViewportSlotKind::DiffVisual { visual_scroll, .. } => *visual_scroll,
            ViewportSlotKind::AnnotationCompose { model_row, .. }
            | ViewportSlotKind::AnnotationSaved { model_row, .. } => {
                self.annotation_anchor_visual_scroll(*model_row)
            }
        }
    }

    pub(super) fn reanchor_annotation_draft(&mut self) {
        let Some(key) = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| draft.key.clone())
        else {
            return;
        };
        let Some(model_row_index) = self.annotation_model_row(&key) else {
            self.annotations_state.annotation_draft = None;
            self.annotations_state.sticky_annotation_draft = false;
            self.runtime.dirty = true;
            return;
        };
        if let Some(draft) = self.annotations_state.annotation_draft.as_mut()
            && draft.model_row_index != model_row_index
        {
            draft.model_row_index = model_row_index;
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn handle_annotation_input_key(&mut self, key: KeyEvent) -> bool {
        if self.annotations_state.annotation_draft.is_none() {
            return false;
        }
        if self
            .config
            .keymap
            .matches_single(GlobalAction::CancelMark, key)
        {
            self.annotations_state.annotation_draft = None;
            self.annotations_state.annotation_block_scroll = None;
            self.annotations_state.sticky_annotation_draft = false;
            self.set_scroll_with_grep_sync(
                self.viewport.scroll,
                false,
                HunkFocusScrollBehavior::Preserve,
            );
            self.sync_annotation_cursor_to_viewport();
            self.runtime.dirty = true;
            return true;
        }
        if self
            .config
            .keymap
            .matches_single(GlobalAction::SaveMark, key)
        {
            let draft = self
                .annotations_state
                .annotation_draft
                .take()
                .expect("draft");
            self.commit_annotation_draft(draft);
            return true;
        }
        let Some(draft) = self.annotations_state.annotation_draft.as_mut() else {
            return false;
        };
        let mut keep_visible = false;
        match key.code {
            KeyCode::Enter => {
                draft.input.insert(draft.cursor, '\n');
                draft.cursor += 1;
                self.runtime.dirty = true;
                keep_visible = true;
            }
            _ => match handle_text_input_key(&mut draft.input, &mut draft.cursor, key) {
                TextInputKeyResult::Edited | TextInputKeyResult::Moved => {
                    self.runtime.dirty = true;
                    keep_visible = true;
                }
                TextInputKeyResult::Ignored | TextInputKeyResult::Handled => {}
            },
        }
        if keep_visible {
            self.ensure_annotation_draft_visible();
        }
        true
    }

    pub(super) fn handle_annotation_save_or_cancel_key(&mut self, key: KeyEvent) -> bool {
        if self.annotations_state.annotation_draft.is_none()
            || !(self
                .config
                .keymap
                .matches_single(GlobalAction::CancelMark, key)
                || self
                    .config
                    .keymap
                    .matches_single(GlobalAction::SaveMark, key))
        {
            return false;
        }

        self.handle_annotation_input_key(key)
    }

    pub(super) fn commit_annotation_draft(&mut self, draft: AnnotationDraft) -> bool {
        let draft_key = draft.key.clone();
        self.annotations_state.annotation_block_scroll = None;
        *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
        self.annotations_state
            .annotation_heights
            .borrow_mut()
            .remove(&draft.key);
        self.annotations_state
            .annotation_rows
            .borrow_mut()
            .insert(draft.key.clone(), Some(draft.model_row_index));
        if draft.input.trim().is_empty() {
            self.annotations_state.annotations.remove_human(&draft.key);
            if self.annotations_state.annotations.get(&draft.key).is_none() {
                self.annotations_state
                    .annotation_rows
                    .borrow_mut()
                    .remove(&draft.key);
            }
        } else {
            let error = self
                .annotations_state
                .annotations
                .insert_human(
                    draft.key.clone(),
                    draft.input.clone(),
                    self.document.generation,
                )
                .err()
                .map(|_| "comment exceeds the live review limits".to_owned());
            if let Some(error) = error {
                self.annotations_state.annotation_draft = Some(draft);
                self.ensure_annotation_draft_visible();
                self.set_error_log(error);
                self.runtime.dirty = true;
                return false;
            }
        }
        let sticky = std::mem::take(&mut self.annotations_state.sticky_annotation_draft);
        if sticky && self.annotation_cursor_enabled() {
            // Advance from the draft's preserved cursor origin before viewport
            // synchronization can choose a replacement for that off-screen row.
            self.select_annotation_cursor(&draft_key);
            self.move_annotation_cursor(1);
        }
        self.set_scroll_with_grep_sync(
            self.viewport.scroll,
            false,
            HunkFocusScrollBehavior::Preserve,
        );
        if sticky && !self.annotation_cursor_enabled() {
            self.open_sticky_annotation_target_mode();
        }
        self.sync_annotation_cursor_to_viewport();
        self.runtime.dirty = true;
        true
    }

    pub(crate) fn open_annotation_draft_in_editor(&mut self) {
        let Some(draft) = self.annotations_state.annotation_draft.take() else {
            return;
        };
        let Some(editor) = configured_editor() else {
            self.annotations_state.annotation_draft = Some(draft);
            self.set_warning_notice("set $GIT_EDITOR, $VISUAL, or $EDITOR to edit annotation");
            return;
        };
        let scratch = match create_annotation_scratch_file(&draft.input) {
            Ok(scratch) => scratch,
            Err(error) => {
                self.annotations_state.annotation_draft = Some(draft);
                self.set_error_log(format!("annotation editor failed: {error}"));
                return;
            }
        };
        self.runtime.request_terminal_clear();
        let status_result = open_text_in_editor(&editor, &scratch.path);
        self.jobs.post_editor_quit_key_ignore_until =
            Some(Instant::now() + POST_EDITOR_QUIT_KEY_IGNORE);
        match status_result {
            Ok(status) if status.success() => match fs::read_to_string(&scratch.path) {
                Ok(contents) => {
                    let mut updated = draft;
                    updated.input = normalize_annotation_editor_contents(&contents);
                    updated.cursor = updated.input.len();
                    if self.commit_annotation_draft(updated) {
                        self.set_success_notice("annotation saved");
                    }
                }
                Err(error) => {
                    self.annotations_state.annotation_draft = Some(draft);
                    self.set_error_log(format!("annotation read failed: {error}"));
                }
            },
            Ok(_) => {
                self.annotations_state.annotation_draft = Some(draft);
                self.set_warning_notice("annotation editor closed");
            }
            Err(error) => {
                self.annotations_state.annotation_draft = Some(draft);
                self.set_error_log(format!("annotation editor failed: {error}"));
            }
        }
        self.runtime.dirty = true;
    }
}

fn annotation_file_indices_by_side_path(
    changeset: &mark_diff::Changeset,
) -> HashMap<(AnnotationSide, &str), Vec<usize>> {
    let mut file_indices: HashMap<(AnnotationSide, &str), Vec<usize>> = HashMap::new();
    for (file_index, file) in changeset.files.iter().enumerate() {
        for side in [AnnotationSide::Old, AnnotationSide::New] {
            if let Some(path) = AnnotationKey::path_for_side(file, side) {
                file_indices
                    .entry((side, path))
                    .or_default()
                    .push(file_index);
            }
        }
    }
    file_indices
}
