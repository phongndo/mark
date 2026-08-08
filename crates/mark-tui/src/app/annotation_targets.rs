use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mark_syntax::AnnotationTargeting;

use super::{AnnotationVisualAnchor, DiffApp, HunkFocusScrollBehavior};
use crate::{
    annotation::{
        AnnotationKey, AnnotationSide, AnnotationTarget, AnnotationTargetMode,
        annotation_hint_codes,
    },
    controls::DiffLayoutMode,
    model::{FileIndex, HunkIndex},
    render::{
        annotation_ranges::annotation_block_body_width,
        annotations::annotation_compose_block_height,
        viewport_plan::{
            ViewportSlotKind, plan_diff_viewport_rows, plan_diff_viewport_rows_at_scroll,
        },
    },
};

const ANNOTATION_CURSOR_SCROLL_OFF: usize = 8;
const MAX_EAGER_DIFF_CURSOR_ROWS: usize = 256;
#[derive(Debug)]
enum CursorTargetDeltaResult {
    Target(AnnotationTarget),
    Unindexed,
}

impl DiffApp {
    pub(crate) fn open_annotation_target_mode(&mut self) {
        self.open_annotation_target_mode_with_sticky(false);
    }

    pub(crate) fn open_sticky_annotation_target_mode(&mut self) {
        self.open_annotation_target_mode_with_sticky(true);
    }

    fn open_annotation_target_mode_with_sticky(&mut self, sticky: bool) {
        if self.annotations_state.annotation_draft.is_some() {
            return;
        }

        if self.annotation_cursor_enabled() {
            self.input.reset_mouse_scroll();
            if self.annotations_state.visual_anchor.is_some() {
                self.open_annotation_draft_at_visual_selection(sticky);
            } else {
                self.open_annotation_draft_at_cursor(sticky);
            }
            return;
        }

        let (targets, _) = self.annotation_hint_targets();
        if targets.is_empty() {
            self.set_notice("no annotatable lines in viewport");
            return;
        }

        self.input.reset_mouse_scroll();
        self.annotations_state.annotation_target_mode = Some(AnnotationTargetMode {
            targets,
            prefix: String::new(),
            sticky,
        });
        self.runtime.dirty = true;
    }

    pub(crate) fn annotation_cursor_enabled(&self) -> bool {
        self.config.interactive && self.config.annotation_targeting == AnnotationTargeting::Cursor
    }

    pub(crate) fn toggle_annotation_visual_mode(&mut self) {
        if !self.annotation_cursor_enabled() {
            self.set_notice("Visual mode requires cursor annotation targeting");
            return;
        }
        if self.annotations_state.annotation_draft.is_some()
            || self.diff_modal_hides_annotation_cursor()
        {
            return;
        }
        if self.close_annotation_visual_mode() {
            return;
        }
        self.ensure_annotation_cursor();
        let Some(target) = self.annotation_cursor_target() else {
            self.set_notice("no cursor line");
            return;
        };
        let anchor_side = target.key.side;
        let Some((first_model_row, last_model_row)) =
            self.annotation_visual_line_bounds(target.model_row_index, anchor_side)
        else {
            self.set_notice("Visual mode requires a code line");
            return;
        };
        self.annotations_state.visual_anchor = Some(AnnotationVisualAnchor {
            model_row: target.model_row_index,
            first_model_row,
            last_model_row,
            side: anchor_side,
        });
        self.runtime.dirty = true;
    }

    pub(crate) fn close_annotation_visual_mode(&mut self) -> bool {
        if self.annotations_state.visual_anchor.take().is_none() {
            return false;
        }
        self.runtime.dirty = true;
        true
    }

    pub(crate) fn annotation_visual_mode_active(&self) -> bool {
        self.annotations_state.visual_anchor.is_some()
    }

    fn annotation_visual_line_bounds(
        &self,
        model_row: usize,
        anchor_side: AnnotationSide,
    ) -> Option<(usize, usize)> {
        let row = self.document.model.row(model_row)?;
        self.annotation_visual_line_coordinate_for_side(row, anchor_side)?;
        let mut range = self.document.model.visual_line_block_at(model_row)?;
        while range.start < range.end
            && self
                .document
                .model
                .row(range.start)
                .and_then(|row| self.annotation_visual_line_coordinate_for_side(row, anchor_side))
                .is_none()
        {
            range.start += 1;
        }
        while range.start < range.end
            && self
                .document
                .model
                .row(range.end - 1)
                .and_then(|row| self.annotation_visual_line_coordinate_for_side(row, anchor_side))
                .is_none()
        {
            range.end -= 1;
        }
        (range.start < range.end && range.contains(&model_row))
            .then_some((range.start, range.end - 1))
    }

    fn annotation_visual_model_bounds(&self) -> Option<(usize, usize)> {
        self.annotations_state
            .visual_anchor
            .map(|anchor| (anchor.first_model_row, anchor.last_model_row))
    }

    fn annotation_visual_selection_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let visual = self.annotations_state.visual_anchor?;
        let head = self
            .annotation_cursor_target()?
            .model_row_index
            .clamp(visual.first_model_row, visual.last_model_row);
        Some(visual.model_row.min(head)..=visual.model_row.max(head))
    }

    pub(crate) fn annotation_active_line_side(
        &self,
        model_row: usize,
        row: crate::model::UiRow,
    ) -> Option<AnnotationSide> {
        if self
            .annotation_visual_selection_range()
            .is_some_and(|selection| selection.contains(&model_row))
        {
            return self
                .annotation_visual_line_coordinate(row)
                .map(|(side, _)| side);
        }
        let draft = self.annotations_state.annotation_draft.as_ref()?;
        self.annotation_key_coordinate_at_model_row(&draft.key, model_row, row)
            .map(|(side, _)| side)
    }

    pub(crate) fn annotation_connectors_at_model_row(
        &self,
        model_row: usize,
        row: crate::model::UiRow,
    ) -> [Option<(AnnotationSide, bool)>; 2] {
        let draft_key = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| &draft.key);
        let saved_keys = self.annotation_connector_keys_at_model_row(model_row, row);
        let mut starts_by_side = [None, None];
        for key in draft_key.into_iter().chain(saved_keys.iter()) {
            if !self.annotation_key_covers_model_row(key, model_row, row) {
                continue;
            }
            let side_index = match key.block_side() {
                AnnotationSide::Old => 0,
                AnnotationSide::New => 1,
            };
            let starts = self.annotation_connector_starts(key, model_row);
            starts_by_side[side_index] = Some(starts_by_side[side_index].unwrap_or(true) && starts);
        }

        let old = starts_by_side[0].map(|starts| (AnnotationSide::Old, starts));
        let new = starts_by_side[1].map(|starts| (AnnotationSide::New, starts));
        // Starting and continuing rails share a column in unified view. Apply a
        // continuing rail last so an overlapping range stays visually open.
        if old.is_some_and(|(_, starts)| !starts) && new.is_some_and(|(_, starts)| starts) {
            [new, old]
        } else {
            [old, new]
        }
    }

    fn annotation_connector_starts(&self, key: &AnnotationKey, model_row: usize) -> bool {
        let mut previous = model_row;
        loop {
            let Some(previous_row) = previous.checked_sub(1) else {
                return true;
            };
            let Some(row) = self.document.model.row(previous_row) else {
                return true;
            };
            // Metadata rows sit between changed lines but never own coordinates.
            // Keep the rail continuous across them instead of restarting. Unified
            // mode stores meta as UnifiedLine, so detect by diff-line kind.
            if self.ui_row_is_metadata(row) {
                previous = previous_row;
                continue;
            }
            return !self.annotation_key_covers_model_row(key, previous_row, row);
        }
    }

    fn annotation_key_covers_model_row(
        &self,
        key: &AnnotationKey,
        model_row: usize,
        row: crate::model::UiRow,
    ) -> bool {
        if self
            .annotation_key_coordinate_at_model_row(key, model_row, row)
            .is_some()
        {
            return true;
        }
        // Bridge metadata rows that sit inside an otherwise continuous range.
        if !self.ui_row_is_metadata(row) {
            return false;
        }
        let previous_covered = (0..model_row).rev().find_map(|candidate| {
            let row = self.document.model.row(candidate)?;
            if self.ui_row_is_metadata(row) {
                return None;
            }
            Some(self.annotation_key_covers_model_row(key, candidate, row))
        });
        let next_covered = ((model_row + 1)..self.document.model.len()).find_map(|candidate| {
            let row = self.document.model.row(candidate)?;
            if self.ui_row_is_metadata(row) {
                return None;
            }
            Some(self.annotation_key_covers_model_row(key, candidate, row))
        });
        previous_covered == Some(true) && next_covered == Some(true)
    }

    fn ui_row_is_metadata(&self, row: crate::model::UiRow) -> bool {
        match row {
            crate::model::UiRow::MetaLine { .. } => true,
            crate::model::UiRow::UnifiedLine { file, hunk, line } => self
                .document
                .changeset
                .files
                .get(file.get())
                .and_then(|file| file.hunks().get(hunk.get()))
                .and_then(|hunk| hunk.lines.get(line.get()))
                .is_some_and(|line| line.kind() == mark_diff::DiffLineKind::Meta),
            _ => false,
        }
    }

    fn annotation_key_coordinate_at_model_row(
        &self,
        key: &AnnotationKey,
        model_row: usize,
        row: crate::model::UiRow,
    ) -> Option<(AnnotationSide, usize)> {
        let file = self
            .document
            .model
            .file_at_row(model_row)
            .and_then(|file| self.document.changeset.files.get(file))?;
        if file.old_path() != Some(key.path.as_str()) && file.new_path() != Some(key.path.as_str())
        {
            return None;
        }
        if key.is_range() {
            return self.annotation_range_covered_coordinate(key, row);
        }
        let coordinate =
            AnnotationKey::line_coordinates_from_ui_row(&self.document.changeset, row, key.side)?;
        key.covers_coordinate(coordinate.0, coordinate.1)
            .then_some(coordinate)
    }

    fn annotation_range_covered_coordinate(
        &self,
        key: &AnnotationKey,
        row: crate::model::UiRow,
    ) -> Option<(AnnotationSide, usize)> {
        // A saved range can outlive the layout that created it. Prefer the
        // current layout's display side, but fall back to any coordinate the
        // persisted source ranges actually cover.
        let preferred_side = if self.viewport.layout == DiffLayoutMode::Split {
            AnnotationSide::New
        } else {
            key.side
        };
        let alternate_side = match preferred_side {
            AnnotationSide::Old => AnnotationSide::New,
            AnnotationSide::New => AnnotationSide::Old,
        };
        [preferred_side, alternate_side]
            .into_iter()
            .filter_map(|side| {
                AnnotationKey::line_coordinates_from_ui_row(&self.document.changeset, row, side)
            })
            .find(|(side, line)| key.covers_coordinate(*side, *line))
    }

    fn annotation_visual_line_coordinate(
        &self,
        row: crate::model::UiRow,
    ) -> Option<(AnnotationSide, usize)> {
        let anchor_side = self.annotations_state.visual_anchor?.side;
        self.annotation_visual_line_coordinate_for_side(row, anchor_side)
    }

    fn annotation_visual_line_coordinate_for_side(
        &self,
        row: crate::model::UiRow,
        anchor_side: AnnotationSide,
    ) -> Option<(AnnotationSide, usize)> {
        // Split selection is row-deterministic: old-only rows use the left
        // side, while rows with new-side content use the right side. Unified
        // context remains on the side that initiated the selection.
        let preferred_side = if self.viewport.layout == DiffLayoutMode::Split {
            AnnotationSide::New
        } else {
            anchor_side
        };
        AnnotationKey::line_coordinates_from_ui_row(&self.document.changeset, row, preferred_side)
    }

    fn visual_annotation_target(&self) -> Result<(AnnotationKey, usize), &'static str> {
        let Some(selection) = self.annotation_visual_selection_range() else {
            return Err("no visual selection");
        };
        let selected_file = self
            .annotations_state
            .visual_anchor
            .and_then(|anchor| self.document.model.file_at_row(anchor.model_row))
            .ok_or("visual selection has no file")?;
        let mut first = None;
        let mut last_model_row = None;
        let mut line_targets = 0usize;
        let mut old_line_targets = 0usize;
        let mut new_line_targets = 0usize;
        let mut old_min = usize::MAX;
        let mut old_max = 0usize;
        let mut new_min = usize::MAX;
        let mut new_max = 0usize;

        for model_row in selection {
            let Some(row) = self.document.model.row(model_row) else {
                continue;
            };
            let Some(file_index) = self.document.model.file_at_row(model_row) else {
                continue;
            };
            if file_index != selected_file {
                continue;
            }
            let Some((side, line)) = self.annotation_visual_line_coordinate(row) else {
                continue;
            };
            if first.is_none() {
                let file = self
                    .document
                    .changeset
                    .files
                    .get(file_index)
                    .ok_or("visual selection has no file")?;
                let key = AnnotationKey::for_file_line(file, side, line)
                    .ok_or("visual selection has no path")?;
                first = Some((key, model_row));
            }
            last_model_row = Some(model_row);
            line_targets = line_targets.saturating_add(1);
            match side {
                AnnotationSide::Old => {
                    old_line_targets = old_line_targets.saturating_add(1);
                    old_min = old_min.min(line);
                    old_max = old_max.max(line);
                }
                AnnotationSide::New => {
                    new_line_targets = new_line_targets.saturating_add(1);
                    new_min = new_min.min(line);
                    new_max = new_max.max(line);
                }
            }
        }

        let Some((anchor, model_row)) = first else {
            return Err("visual selection has no annotatable lines");
        };
        if line_targets == 1 {
            return Ok((anchor, model_row));
        }
        let Some(file) = self.document.changeset.files.get(selected_file) else {
            return Err("visual selection has no file");
        };
        let (old_start, old_count) = source_range(old_min, old_max);
        let (new_start, new_count) = source_range(new_min, new_max);
        if old_count != old_line_targets || new_count != new_line_targets {
            return Err("visual selection has disjoint source lines");
        }
        let key = AnnotationKey::for_range(
            file,
            anchor.side,
            anchor.line,
            old_start,
            old_count,
            new_start,
            new_count,
        )
        .ok_or("visual selection has no path")?;
        Ok((key, last_model_row.unwrap_or(model_row)))
    }

    fn open_annotation_draft_at_visual_selection(&mut self, sticky: bool) {
        let target = self.visual_annotation_target();
        match target {
            Ok((key, model_row)) => {
                if self.open_annotation_draft_for_key(key, model_row) {
                    self.annotations_state.visual_anchor = None;
                    self.annotations_state.sticky_annotation_draft = sticky;
                }
            }
            Err(message) => self.set_notice(message),
        }
    }

    pub(in crate::app) fn reset_annotation_cursor(&mut self, preferred: Option<AnnotationKey>) {
        if !self.annotation_cursor_enabled() {
            self.annotations_state.annotation_cursor = None;
            return;
        }

        if self.document.model.len() > MAX_EAGER_DIFF_CURSOR_ROWS {
            let target = preferred
                .as_ref()
                .and_then(|key| self.annotation_target_for_key(key))
                .or_else(|| self.annotation_target_near_viewport_focus())
                .or_else(|| self.cursor_target_near_viewport_focus())
                .or_else(|| self.cursor_target_for_model_row(self.viewport_focus_row()));
            let mut preferred_keys = HashMap::new();
            if let Some(target) = target
                .as_ref()
                .filter(|target| preferred.as_ref() == Some(&target.key))
            {
                preferred_keys.insert(target.model_row_index, target.key.clone());
            }
            self.annotations_state.annotation_cursor = Some(crate::annotation::AnnotationCursor {
                model_identity: self.document.model.identity(),
                targets: target.into_iter().collect(),
                selected: 0,
                preferred_keys,
                lazy: true,
                previous_exhausted: false,
                next_exhausted: false,
            });
            return;
        }

        let (mut targets, fallback) = self.annotation_cursor_targets();
        let preferred = preferred.as_ref().and_then(|key| {
            let target = self.annotation_target_for_key(key)?;
            targets
                .iter()
                .position(|candidate| candidate.model_row_index == target.model_row_index)
                .map(|selected| (selected, target.key))
        });
        let selected = preferred
            .as_ref()
            .map(|(selected, _)| *selected)
            .unwrap_or(fallback);
        let mut preferred_keys = HashMap::new();
        if let Some((selected, key)) = preferred {
            targets[selected].key = key.clone();
            preferred_keys.insert(targets[selected].model_row_index, key);
        }
        self.annotations_state.annotation_cursor = Some(crate::annotation::AnnotationCursor {
            model_identity: self.document.model.identity(),
            targets,
            selected,
            preferred_keys,
            lazy: false,
            previous_exhausted: false,
            next_exhausted: false,
        });
    }

    pub(in crate::app) fn ensure_annotation_cursor(&mut self) {
        if !self.annotation_cursor_enabled() {
            return;
        }
        let model_identity = self.document.model.identity();
        let cursor_is_current = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.model_identity == model_identity);
        if !cursor_is_current {
            let preferred = self
                .annotation_cursor_target()
                .map(|target| target.key.clone());
            self.reset_annotation_cursor(preferred);
        }
    }

    pub(in crate::app) fn rebuild_annotation_cursor(&mut self) {
        self.annotations_state.annotation_block_scroll = None;
        self.annotations_state.visual_anchor = None;
        let preferred = self
            .annotation_cursor_target()
            .map(|target| target.key.clone());
        self.reset_annotation_cursor(preferred);
    }

    pub(in crate::app) fn refresh_annotation_cursor_target_layout(&mut self) {
        let Some((selected, model_row)) = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .and_then(|cursor| {
                cursor
                    .targets
                    .get(cursor.selected)
                    .map(|target| (cursor.selected, target.model_row_index))
            })
        else {
            return;
        };
        let viewport_row = self.annotation_cursor_viewport_row().unwrap_or(usize::MAX);
        // Viewport planning can discover sparse wrapped blocks and change the
        // target's absolute coordinate, so derive coordinates afterward.
        let visual_scroll = self.scroll_for_model_row(model_row);
        let visual_height = self.annotation_visual_height_for_model_row(model_row);
        if let Some(target) = self
            .annotations_state
            .annotation_cursor
            .as_mut()
            .and_then(|cursor| cursor.targets.get_mut(selected))
        {
            target.visual_scroll = visual_scroll;
            target.visual_height = visual_height;
            target.viewport_row = viewport_row;
        }
    }

    pub(in crate::app) fn sync_annotation_cursor_to_viewport(&mut self) {
        self.ensure_annotation_cursor();
        if !self.annotation_cursor_enabled() {
            return;
        }
        if self.annotations_state.annotation_draft.is_some() || self.annotation_visual_mode_active()
        {
            self.refresh_annotation_cursor_target_layout();
            return;
        }
        let previous = self
            .annotation_cursor_target()
            .map(|target| (target.key.clone(), target.model_row_index));
        let has_target = previous.is_some();
        let target_is_rendered = has_target && self.annotation_cursor_viewport_row().is_some();
        let lazy = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.lazy);
        if (!has_target && lazy) || (has_target && !target_is_rendered) {
            self.annotations_state.annotation_block_scroll = None;
            if lazy {
                if let Some(target) = self.cursor_target_near_viewport_focus() {
                    self.set_lazy_annotation_cursor_target(target);
                } else {
                    self.refresh_annotation_cursor_target_layout();
                }
            } else {
                let preferred = self
                    .cursor_target_near_viewport_focus()
                    .map(|target| target.key);
                self.reset_annotation_cursor(preferred);
            }
        } else {
            self.refresh_annotation_cursor_target_layout();
        }
        let changed = self.annotation_cursor_target().is_some_and(|target| {
            previous.as_ref().is_none_or(|(key, model_row)| {
                key != &target.key || *model_row != target.model_row_index
            })
        });
        if changed {
            // Passive viewport synchronization must not turn inferred cursor
            // placement into explicit hunk focus, but file focus should follow
            // the newly visible target.
            if let Some(file) = self
                .annotation_cursor_target()
                .and_then(|target| self.document.model.file_at_row(target.model_row_index))
            {
                self.sidebar.selected_file = FileIndex::new(file);
                self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            }
            self.runtime.dirty = true;
        }
    }

    pub(in crate::app) fn select_annotation_cursor(&mut self, key: &AnnotationKey) {
        if key.is_cursor_only() {
            self.select_annotation_cursor_model_row(key.line);
            return;
        }
        let Some(target) = self.annotation_target_for_key(key) else {
            return;
        };
        self.select_annotation_cursor_model_row_with_key(target.model_row_index, Some(key));
    }

    pub(crate) fn select_annotation_cursor_model_row(&mut self, model_row: usize) {
        self.select_annotation_cursor_model_row_with_key(model_row, None);
    }

    fn select_annotation_cursor_model_row_with_key(
        &mut self,
        model_row: usize,
        key: Option<&AnnotationKey>,
    ) {
        let visual_target = self
            .annotation_visual_model_bounds()
            .and_then(|(first, last)| {
                let model_row = model_row.clamp(first, last);
                let before = self.annotation_visual_target_at_or_before(model_row, first);
                let after = self.annotation_visual_target_at_or_after(model_row, last);
                match (before, after) {
                    (Some(before), Some(after)) => Some(
                        if before.model_row_index.abs_diff(model_row)
                            <= after.model_row_index.abs_diff(model_row)
                        {
                            before
                        } else {
                            after
                        },
                    ),
                    (Some(target), None) | (None, Some(target)) => Some(target),
                    (None, None) => None,
                }
            });
        let model_row = visual_target
            .as_ref()
            .map(|target| target.model_row_index)
            .unwrap_or(model_row);
        let visual_key = visual_target.as_ref().map(|target| &target.key);
        let key = visual_key.or(key);
        let previous = self
            .annotation_cursor_target()
            .map(|target| (target.key.clone(), target.model_row_index));
        self.ensure_annotation_cursor();
        if let Some(key) = key
            && let Some(cursor) = self.annotations_state.annotation_cursor.as_mut()
        {
            cursor.preferred_keys.insert(model_row, key.clone());
        }
        let lazy = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.lazy);
        if lazy {
            let target = match key {
                Some(key) => self.cursor_target_for_model_row_with_key(model_row, key.clone()),
                None => {
                    let Some(target) = self.cursor_target_for_model_row(model_row) else {
                        return;
                    };
                    target
                }
            };
            self.set_lazy_annotation_cursor_target(target);
        } else {
            let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
                return;
            };
            let Some(selected) = cursor
                .targets
                .iter()
                .position(|target| target.model_row_index == model_row)
            else {
                return;
            };
            cursor.selected = selected;
            if let Some(key) = key {
                cursor.targets[selected].key = key.clone();
            }
        }
        let changed = self.annotation_cursor_target().is_some_and(|target| {
            previous.as_ref().is_none_or(|(key, model_row)| {
                key != &target.key || *model_row != target.model_row_index
            })
        });
        self.refresh_annotation_cursor_target_layout();
        self.focus_hunk_at_annotation_cursor();
        if changed {
            self.annotations_state.annotation_block_scroll = None;
            self.runtime.dirty = true;
        }
    }

    pub(in crate::app) fn select_annotation_cursor_near_model_row_in_rendered_hunk(
        &mut self,
        model_row: usize,
        hunk: (FileIndex, HunkIndex),
    ) {
        self.ensure_annotation_cursor();
        let Some(target) = self.annotation_target_near_model_row_in_rendered_hunk(model_row, hunk)
        else {
            return;
        };
        self.select_annotation_cursor(&target.key);
    }

    pub(in crate::app) fn select_annotation_cursor_near_model_row_in_hunk(
        &mut self,
        model_row: usize,
        hunk: (FileIndex, HunkIndex),
    ) {
        self.ensure_annotation_cursor();
        let lazy = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.lazy);
        let visible_target =
            self.annotation_target_near_model_row_in_rendered_hunk(model_row, hunk);
        if lazy {
            let target = visible_target
                .or_else(|| self.nearest_annotation_target_in_hunk(model_row, hunk))
                .or_else(|| self.nearest_annotation_target(model_row));
            if let Some(target) = target {
                self.set_lazy_annotation_cursor_target(target);
            }
        } else {
            let cursor = self.annotations_state.annotation_cursor.as_ref();
            let visible_selected = visible_target.as_ref().and_then(|target| {
                cursor?.targets.iter().position(|candidate| {
                    candidate.key == target.key
                        && candidate.model_row_index == target.model_row_index
                })
            });
            let hunk_selected = cursor.and_then(|cursor| {
                cursor
                    .targets
                    .iter()
                    .enumerate()
                    .filter(|(_, target)| {
                        self.document
                            .model
                            .row(target.model_row_index)
                            .and_then(|row| row.typed_hunk_key())
                            == Some(hunk)
                    })
                    .min_by_key(|(_, target)| target.model_row_index.abs_diff(model_row))
                    .map(|(selected, _)| selected)
            });
            let selected = visible_selected.or(hunk_selected).or_else(|| {
                cursor.and_then(|cursor| {
                    cursor
                        .targets
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, target)| target.model_row_index.abs_diff(model_row))
                        .map(|(selected, _)| selected)
                })
            });
            let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
                return;
            };
            let Some(selected) = selected else {
                return;
            };
            let changed = cursor.selected != selected;
            cursor.selected = selected;
            if changed {
                self.annotations_state.annotation_block_scroll = None;
                self.runtime.dirty = true;
            }
        }
        self.refresh_annotation_cursor_target_layout();
        self.focus_hunk_at_annotation_cursor();
    }

    pub(in crate::app) fn select_annotation_cursor_near_model_row_in_file(
        &mut self,
        model_row: usize,
        file: FileIndex,
    ) {
        if !self.annotation_cursor_enabled() {
            return;
        }
        self.ensure_annotation_cursor();
        let visible_target = self
            .annotation_target_near_viewport_focus()
            .filter(|target| {
                self.document.model.file_at_row(target.model_row_index) == Some(file.get())
            });
        let target = visible_target
            .or_else(|| {
                self.document
                    .model
                    .file_row_range(file)
                    .and_then(|range| self.nearest_annotation_target_in_range(model_row, range))
            })
            .or_else(|| self.cursor_target_for_model_row(model_row));
        if let Some(target) = target {
            self.select_annotation_cursor(&target.key);
            if !self.keep_annotation_cursor_rendered() {
                self.clear_annotation_cursor_target();
            }
            return;
        }
        self.clear_annotation_cursor_target();
        self.clear_manual_hunk_focus();
    }

    pub(in crate::app) fn clear_annotation_cursor_target(&mut self) {
        if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() {
            cursor.targets.clear();
            cursor.selected = 0;
            cursor.lazy = true;
            cursor.previous_exhausted = false;
            cursor.next_exhausted = false;
        }
        self.annotations_state.annotation_block_scroll = None;
        self.runtime.dirty = true;
    }

    fn annotation_target_for_key(&self, key: &AnnotationKey) -> Option<AnnotationTarget> {
        if key.is_cursor_only() {
            return self
                .cursor_target_for_model_row(key.line)
                .filter(|target| &target.key == key);
        }
        let model_row = self.annotation_model_row(key)?;
        // Structural annotations can intentionally use a content-row anchor
        // when their header is omitted (for example in full-file mode).
        self.document.model.row(model_row)?;
        Some(self.cursor_target_for_model_row_with_key(model_row, key.clone()))
    }

    fn annotation_target_for_model_row(&self, model_row_index: usize) -> Option<AnnotationTarget> {
        let row = self.document.model.row(model_row_index)?;
        let key = AnnotationKey::from_ui_row(&self.document.changeset, row)?;
        Some(self.cursor_target_for_model_row_with_key(model_row_index, key))
    }

    fn cursor_target_for_model_row(&self, model_row_index: usize) -> Option<AnnotationTarget> {
        let row = self.document.model.row(model_row_index)?;
        let candidates = AnnotationKey::candidates_from_ui_row(&self.document.changeset, row);
        let preferred = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .and_then(|cursor| cursor.preferred_keys.get(&model_row_index))
            .filter(|key| {
                candidates.contains(key)
                    || (!key.is_cursor_only()
                        && !key.is_line()
                        && self.annotation_model_row(key) == Some(model_row_index))
            })
            .cloned();
        let existing = {
            let mut annotated = candidates
                .iter()
                .filter(|key| self.annotations_state.annotations.contains_key(*key));
            let first = annotated.next().cloned();
            first.filter(|_| annotated.next().is_none())
        };
        let key = preferred
            .or(existing)
            .or_else(|| AnnotationKey::from_ui_row(&self.document.changeset, row))
            .unwrap_or_else(|| AnnotationKey::cursor_only(model_row_index));
        Some(self.cursor_target_for_model_row_with_key(model_row_index, key))
    }

    fn cursor_target_for_model_row_with_key(
        &self,
        model_row_index: usize,
        key: AnnotationKey,
    ) -> AnnotationTarget {
        AnnotationTarget {
            key,
            model_row_index,
            visual_scroll: self.scroll_for_model_row(model_row_index),
            visual_height: self.annotation_visual_height_for_model_row(model_row_index),
            viewport_row: usize::MAX,
            hint: String::new(),
        }
    }

    fn set_lazy_annotation_cursor_target(&mut self, target: AnnotationTarget) {
        let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
            return;
        };
        let unchanged = cursor.targets.get(cursor.selected).is_some_and(|current| {
            current.key == target.key && current.model_row_index == target.model_row_index
        });
        cursor.targets.clear();
        cursor.targets.push(target);
        cursor.selected = 0;
        if !unchanged {
            cursor.previous_exhausted = false;
            cursor.next_exhausted = false;
            self.annotations_state.annotation_block_scroll = None;
        }
    }

    fn select_discovered_annotation_target(&mut self, target: AnnotationTarget) {
        let lazy = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.lazy);
        if lazy {
            self.set_lazy_annotation_cursor_target(target);
        } else if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut()
            && let Some(selected) = cursor.targets.iter().position(|candidate| {
                candidate.key == target.key && candidate.model_row_index == target.model_row_index
            })
        {
            cursor.selected = selected;
        }
        self.refresh_annotation_cursor_target_layout();
        self.focus_hunk_at_annotation_cursor();
    }

    fn annotation_target_near_model_row_in_rendered_hunk(
        &self,
        model_row: usize,
        hunk: (FileIndex, HunkIndex),
    ) -> Option<AnnotationTarget> {
        plan_diff_viewport_rows_at_scroll(
            self,
            self.viewport.scroll,
            self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX)),
        )
        .into_iter()
        .filter_map(|slot| {
            let ViewportSlotKind::DiffVisual {
                model_row: candidate,
                ..
            } = slot.kind
            else {
                return None;
            };
            if self
                .document
                .model
                .row(candidate)
                .and_then(|row| row.typed_hunk_key())
                != Some(hunk)
            {
                return None;
            }
            self.annotation_target_for_model_row(candidate)
        })
        .min_by_key(|target| target.model_row_index.abs_diff(model_row))
    }

    fn annotation_visual_height_for_model_row(&self, model_row_index: usize) -> usize {
        if self.viewport.line_wrapping {
            self.wrapped_visual_height_for_model_row(model_row_index)
        } else {
            1
        }
    }

    fn cursor_target_near_viewport_focus(&self) -> Option<AnnotationTarget> {
        let visible_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let focus_viewport_row = self.rendered_viewport_focus_row(visible_rows);
        plan_diff_viewport_rows(self, visible_rows)
            .into_iter()
            .enumerate()
            .filter_map(|(viewport_row, slot)| {
                let ViewportSlotKind::DiffVisual { model_row, .. } = slot.kind else {
                    return None;
                };
                let mut target = self.cursor_target_for_model_row(model_row)?;
                target.viewport_row = viewport_row;
                Some((
                    viewport_row.abs_diff(focus_viewport_row),
                    viewport_row,
                    target,
                ))
            })
            .min_by_key(|(distance, viewport_row, _)| (*distance, *viewport_row))
            .map(|(_, _, target)| target)
    }

    fn annotation_target_near_viewport_focus(&self) -> Option<AnnotationTarget> {
        let visible_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let focused_hunk = self.focused_hunk_for_viewport(visible_rows);
        let focus_viewport_row = self.rendered_viewport_focus_row(visible_rows);
        plan_diff_viewport_rows(self, visible_rows)
            .into_iter()
            .enumerate()
            .filter_map(|(viewport_row, slot)| {
                let ViewportSlotKind::DiffVisual { model_row, .. } = slot.kind else {
                    return None;
                };
                let mut target = self.annotation_target_for_model_row(model_row)?;
                target.viewport_row = viewport_row;
                let in_focused_hunk = self
                    .document
                    .model
                    .row(model_row)
                    .and_then(|row| row.typed_hunk_key())
                    .is_some_and(|hunk| Some(hunk) == focused_hunk);
                Some((
                    !in_focused_hunk,
                    viewport_row.abs_diff(focus_viewport_row),
                    viewport_row,
                    target,
                ))
            })
            .min_by_key(|(outside_focus, distance, viewport_row, _)| {
                (*outside_focus, *distance, *viewport_row)
            })
            .map(|(_, _, _, target)| target)
    }

    fn nearest_annotation_target_in_hunk(
        &self,
        model_row: usize,
        hunk: (FileIndex, HunkIndex),
    ) -> Option<AnnotationTarget> {
        let range = self
            .document
            .model
            .hunk_row_range(hunk.0.get(), hunk.1.get())?;
        self.nearest_annotation_target_in_range(model_row, range)
    }

    fn nearest_annotation_target_in_range(
        &self,
        model_row: usize,
        range: std::ops::Range<usize>,
    ) -> Option<AnnotationTarget> {
        if range.is_empty() {
            return None;
        }
        let focus = model_row.clamp(range.start, range.end.saturating_sub(1));
        let mut previous = self
            .annotation_candidate_at_or_before(focus)
            .filter(|candidate| *candidate >= range.start);
        let mut next = focus
            .checked_add(1)
            .and_then(|row| self.annotation_candidate_at_or_after(row))
            .filter(|candidate| *candidate < range.end);
        loop {
            let moving_up = match (previous, next) {
                (Some(previous), Some(next)) => previous.abs_diff(focus) <= next.abs_diff(focus),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => return None,
            };
            let candidate = if moving_up { previous? } else { next? };
            if moving_up {
                previous = candidate
                    .checked_sub(1)
                    .and_then(|row| self.annotation_candidate_at_or_before(row))
                    .filter(|candidate| *candidate >= range.start);
            } else {
                next = candidate
                    .checked_add(1)
                    .and_then(|row| self.annotation_candidate_at_or_after(row))
                    .filter(|candidate| *candidate < range.end);
            }
            if let Some(target) = self.annotation_target_for_model_row(candidate) {
                return Some(target);
            }
        }
    }

    fn annotation_candidate_at_or_after(&self, model_row: usize) -> Option<usize> {
        self.document
            .model
            .annotation_candidate_at_or_after(&self.document.changeset, model_row)
    }

    fn annotation_candidate_at_or_before(&self, model_row: usize) -> Option<usize> {
        self.document
            .model
            .annotation_candidate_at_or_before(&self.document.changeset, model_row)
    }

    fn nearest_annotation_target(&self, model_row: usize) -> Option<AnnotationTarget> {
        self.nearest_annotation_target_in_range(model_row, 0..self.document.model.len())
    }

    fn open_annotation_draft_at_cursor(&mut self, sticky: bool) {
        self.ensure_annotation_cursor();
        let Some(target) = self.annotation_cursor_target().cloned() else {
            self.set_notice("no cursor line");
            return;
        };
        if target.key.is_cursor_only() {
            if self.handle_context_at_row(target.model_row_index) {
                return;
            }
            self.set_notice("this row cannot be annotated");
            return;
        }
        if self.annotation_cursor_viewport_row().is_none() {
            self.set_notice("cursor line is outside the viewport");
            return;
        }
        if self.open_annotation_draft_for_key(target.key, target.model_row_index) {
            self.annotations_state.sticky_annotation_draft = sticky;
        }
    }

    fn annotation_cursor_targets(&self) -> (Vec<AnnotationTarget>, usize) {
        let mut targets = self
            .document
            .model
            .iter_rows()
            .enumerate()
            .filter_map(|(model_row_index, _)| self.cursor_target_for_model_row(model_row_index))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return (targets, 0);
        }

        let visible_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let focused_hunk = self.focused_hunk_for_viewport(visible_rows);
        let focus_viewport_row = self.rendered_viewport_focus_row(visible_rows);
        for (viewport_row, slot) in plan_diff_viewport_rows(self, visible_rows)
            .into_iter()
            .enumerate()
        {
            let ViewportSlotKind::DiffVisual { model_row, .. } = slot.kind else {
                continue;
            };
            let Ok(index) =
                targets.binary_search_by_key(&model_row, |target| target.model_row_index)
            else {
                continue;
            };
            // Wrapped logical rows occupy several slots. Retain whichever
            // visible continuation is closest to viewport focus.
            let previous_viewport_row = targets[index].viewport_row;
            if previous_viewport_row == usize::MAX
                || viewport_row.abs_diff(focus_viewport_row)
                    < previous_viewport_row.abs_diff(focus_viewport_row)
            {
                targets[index].viewport_row = viewport_row;
            }
        }

        let selected = targets
            .iter()
            .enumerate()
            .filter(|(_, target)| target.viewport_row != usize::MAX)
            .map(|(index, target)| {
                let in_focused_hunk = self
                    .document
                    .model
                    .row(target.model_row_index)
                    .and_then(|row| row.typed_hunk_key())
                    .is_some_and(|hunk| Some(hunk) == focused_hunk);
                (
                    index,
                    !target.key.is_line(),
                    !in_focused_hunk,
                    target.viewport_row.abs_diff(focus_viewport_row),
                    target.viewport_row,
                )
            })
            .min_by_key(|(_, structural, outside_focus, distance, viewport_row)| {
                (*structural, *outside_focus, *distance, *viewport_row)
            })
            .map(|(index, _, _, _, _)| index)
            .unwrap_or_else(|| {
                let focus_model_row = self.viewport_focus_row();
                targets
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, target)| {
                        (
                            !target.key.is_line(),
                            target.model_row_index.abs_diff(focus_model_row),
                        )
                    })
                    .map(|(index, _)| index)
                    .unwrap_or_default()
            });
        let selected = if !targets[selected].key.is_line() {
            let focus_model_row = self.viewport_focus_row();
            targets
                .iter()
                .enumerate()
                .filter(|(_, target)| target.key.is_line())
                .min_by_key(|(_, target)| target.model_row_index.abs_diff(focus_model_row))
                .map(|(index, _)| index)
                .unwrap_or(selected)
        } else {
            selected
        };

        (targets, selected)
    }

    fn annotation_hint_targets(&self) -> (Vec<AnnotationTarget>, usize) {
        let visible_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let focused_hunk = self.focused_hunk_for_viewport(visible_rows);
        let focus_viewport_row = self.rendered_viewport_focus_row(visible_rows);
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        let mut focused = Vec::new();

        for (viewport_row, slot) in plan_diff_viewport_rows(self, visible_rows)
            .into_iter()
            .enumerate()
        {
            let ViewportSlotKind::DiffVisual {
                visual_scroll,
                model_row,
            } = slot.kind
            else {
                continue;
            };
            let Some(row) = self.document.model.row(model_row) else {
                continue;
            };
            for key in AnnotationKey::candidates_from_ui_row(&self.document.changeset, row) {
                if !seen.insert(key.clone()) {
                    continue;
                }

                focused.push(
                    row.typed_hunk_key()
                        .is_some_and(|hunk| Some(hunk) == focused_hunk),
                );
                targets.push(AnnotationTarget {
                    key,
                    model_row_index: model_row,
                    visual_scroll,
                    visual_height: self.annotation_visual_height_for_model_row(model_row),
                    viewport_row,
                    hint: String::new(),
                });
            }
        }

        // The viewport defines eligibility. Hunk focus only ranks targets so
        // the easiest, shortest hints stay near the reviewer's current work.
        let mut priority = (0..targets.len()).collect::<Vec<_>>();
        priority.sort_by_key(|index| {
            let target = &targets[*index];
            (
                !focused[*index],
                target.viewport_row.abs_diff(focus_viewport_row),
                target.viewport_row,
            )
        });
        let hint_keys = &self.config.syntax_settings.annotations.hint_keys;
        for (index, hint) in priority
            .into_iter()
            .zip(annotation_hint_codes(targets.len(), hint_keys))
        {
            targets[index].hint = hint;
        }

        (targets, 0)
    }

    pub(crate) fn close_annotation_target_mode(&mut self) -> bool {
        if self
            .annotations_state
            .annotation_target_mode
            .take()
            .is_none()
        {
            return false;
        }
        self.runtime.dirty = true;
        true
    }

    pub(crate) fn handle_annotation_target_key(&mut self, key: KeyEvent) -> bool {
        if self.annotations_state.annotation_target_mode.is_none() {
            return false;
        }
        self.handle_annotation_hint_key(key)
    }

    pub(crate) fn move_annotation_cursor(&mut self, delta: isize) -> bool {
        let previous = self
            .annotation_cursor_target()
            .map(|target| (target.key.clone(), target.model_row_index));
        self.move_annotation_cursor_inner(delta);
        self.annotation_cursor_target().is_some_and(|target| {
            previous.as_ref().is_none_or(|(key, model_row)| {
                key != &target.key || *model_row != target.model_row_index
            })
        })
    }

    fn move_annotation_cursor_inner(&mut self, delta: isize) {
        self.ensure_annotation_cursor();
        if delta == 0 {
            return;
        }
        if self.annotation_visual_mode_active() {
            self.move_visual_annotation_cursor(delta);
            return;
        }
        if self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.lazy)
        {
            self.move_lazy_annotation_cursor(delta);
            return;
        }

        let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
            return;
        };
        let previous = cursor.selected;
        let last = cursor.targets.len().saturating_sub(1);
        let selected = if delta == isize::MIN {
            0
        } else if delta == isize::MAX {
            last
        } else if delta < 0 {
            cursor.selected.saturating_sub(delta.unsigned_abs())
        } else {
            cursor.selected.saturating_add(delta as usize).min(last)
        };
        if selected == previous {
            self.refresh_annotation_cursor_target_layout();
            self.keep_annotation_cursor_inside_scroll_region(delta < 0);
            self.focus_hunk_at_annotation_cursor();
            return;
        }

        cursor.selected = selected;
        self.annotations_state.annotation_block_scroll = None;
        self.refresh_annotation_cursor_target_layout();
        self.keep_annotation_cursor_inside_scroll_region(selected < previous);
        self.focus_hunk_at_annotation_cursor();
        self.runtime.dirty = true;
    }

    fn move_visual_annotation_cursor(&mut self, delta: isize) {
        let Some(current) = self.annotation_cursor_target().cloned() else {
            return;
        };
        let Some((first, last)) = self.annotation_visual_model_bounds() else {
            return;
        };
        let moving_up = delta < 0;
        let desired = if delta == isize::MIN {
            first
        } else if delta == isize::MAX {
            last
        } else if moving_up {
            current
                .model_row_index
                .saturating_sub(delta.unsigned_abs())
                .max(first)
        } else {
            current
                .model_row_index
                .saturating_add(delta as usize)
                .min(last)
        };
        let next = if moving_up {
            self.annotation_visual_target_at_or_before(desired, first)
        } else {
            self.annotation_visual_target_at_or_after(desired, last)
        }
        .unwrap_or(current.clone());
        if next.key == current.key && next.model_row_index == current.model_row_index {
            self.refresh_annotation_cursor_target_layout();
            self.keep_annotation_cursor_inside_scroll_region(moving_up);
            return;
        }
        self.select_annotation_cursor_model_row_with_key(next.model_row_index, Some(&next.key));
        self.keep_annotation_cursor_inside_scroll_region(moving_up);
    }

    fn annotation_visual_target_at_or_after(
        &self,
        model_row: usize,
        last: usize,
    ) -> Option<AnnotationTarget> {
        let mut candidate = self.annotation_candidate_at_or_after(model_row)?;
        while candidate <= last {
            if let Some(target) = self.annotation_visual_target_for_model_row(candidate) {
                return Some(target);
            }
            candidate = self.annotation_candidate_at_or_after(candidate.checked_add(1)?)?;
        }
        None
    }

    fn annotation_visual_target_at_or_before(
        &self,
        model_row: usize,
        first: usize,
    ) -> Option<AnnotationTarget> {
        let mut candidate = self.annotation_candidate_at_or_before(model_row)?;
        while candidate >= first {
            if let Some(target) = self.annotation_visual_target_for_model_row(candidate) {
                return Some(target);
            }
            candidate = self.annotation_candidate_at_or_before(candidate.checked_sub(1)?)?;
        }
        None
    }

    fn annotation_visual_target_for_model_row(&self, model_row: usize) -> Option<AnnotationTarget> {
        let row = self.document.model.row(model_row)?;
        let (side, line) = self.annotation_visual_line_coordinate(row)?;
        let file = self
            .document
            .model
            .file_at_row(model_row)
            .and_then(|file| self.document.changeset.files.get(file))?;
        let key = AnnotationKey::for_file_line(file, side, line)?;
        Some(self.cursor_target_for_model_row_with_key(model_row, key))
    }

    pub(crate) fn move_annotation_cursor_by_visual_delta(&mut self, delta: isize) {
        self.ensure_annotation_cursor();
        if delta == 0 {
            return;
        }
        if self.annotation_visual_mode_active() {
            self.move_visual_annotation_cursor(delta);
            return;
        }
        let Some(cursor) = self.annotations_state.annotation_cursor.as_ref() else {
            return;
        };
        let Some(current) = cursor.targets.get(cursor.selected).cloned() else {
            return;
        };
        let lazy = cursor.lazy;
        let selected = cursor.selected;
        let targets = cursor.targets.clone();

        let mut delta = delta;
        let mut at_saved_block_end = false;
        let block_scroll = self
            .annotations_state
            .annotation_block_scroll
            .as_ref()
            .filter(|(key, _)| key == &current.key)
            .map(|(_, offset)| *offset)
            .unwrap_or_default();
        if ((delta < 0 && block_scroll > 0) || delta > 0)
            && let Some(remaining) = self.scroll_saved_annotation_block(&current, delta)
        {
            if remaining == 0 {
                self.focus_hunk_at_annotation_cursor();
                return;
            }
            at_saved_block_end = delta > 0;
            delta = remaining;
        }

        let annotation_height_prefixes = self.annotation_block_height_prefixes();
        let moving_up = delta < 0;
        let amount = delta.unsigned_abs();
        let current_target_visual_row =
            self.annotation_target_document_visual_row(&current, &annotation_height_prefixes);
        let current_target_height =
            self.annotation_visual_height_for_model_row(current.model_row_index);
        let current_visual_row = if at_saved_block_end {
            current_target_visual_row.saturating_add(current_target_height)
        } else {
            self.annotation_cursor_document_visual_row(&current, &annotation_height_prefixes)
        };
        let desired_visual_row = if moving_up {
            current_visual_row.saturating_sub(amount)
        } else {
            current_visual_row.saturating_add(amount)
        };
        if moving_up
            && self.select_preceding_saved_annotation_at_visual_row(&current, desired_visual_row)
        {
            return;
        }
        let destination_inside_current = desired_visual_row >= current_target_visual_row
            && desired_visual_row < current_target_visual_row.saturating_add(current_target_height);
        if destination_inside_current {
            // Advance through a tall wrapped logical row without changing the
            // annotation target represented by that row. At a clamped edge,
            // advance explicitly so a directional neighboring target can win.
            let previous_scroll = self.viewport.scroll;
            self.scroll_by(delta);
            if self.viewport.scroll == previous_scroll {
                self.move_annotation_cursor(if moving_up { -1 } else { 1 });
            } else {
                self.focus_hunk_at_annotation_cursor();
            }
            return;
        }

        let next = if lazy {
            let mut candidates = vec![current.clone()];
            let mut edge = current.clone();
            loop {
                let step = if moving_up { -1 } else { 1 };
                let candidate = match self.cursor_target_by_delta(&edge, step) {
                    CursorTargetDeltaResult::Target(candidate) => candidate,
                    CursorTargetDeltaResult::Unindexed => break,
                };
                if candidate.key == edge.key && candidate.model_row_index == edge.model_row_index {
                    break;
                }
                let candidate_visual_row = self
                    .annotation_target_document_visual_row(&candidate, &annotation_height_prefixes);
                edge = candidate.clone();
                candidates.push(candidate);
                if (moving_up && candidate_visual_row <= desired_visual_row)
                    || (!moving_up && candidate_visual_row >= desired_visual_row)
                {
                    break;
                }
            }
            candidates.into_iter().min_by_key(|target| {
                self.annotation_target_document_visual_row(target, &annotation_height_prefixes)
                    .abs_diff(desired_visual_row)
            })
        } else if moving_up {
            targets[..=selected]
                .iter()
                .rev()
                .min_by_key(|target| {
                    self.annotation_target_document_visual_row(target, &annotation_height_prefixes)
                        .abs_diff(desired_visual_row)
                })
                .cloned()
        } else {
            targets[selected..]
                .iter()
                .min_by_key(|target| {
                    self.annotation_target_document_visual_row(target, &annotation_height_prefixes)
                        .abs_diff(desired_visual_row)
                })
                .cloned()
        };
        let Some(next) = next else {
            return;
        };
        if next.key == current.key && next.model_row_index == current.model_row_index {
            self.scroll_by_preserving_annotation_block(delta);
            self.focus_hunk_at_annotation_cursor();
            return;
        }

        let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
            return;
        };
        if lazy {
            cursor.targets.clear();
            cursor.targets.push(next);
            cursor.selected = 0;
            cursor.previous_exhausted = false;
            cursor.next_exhausted = false;
        } else if let Some(next_selected) = cursor.targets.iter().position(|target| {
            target.key == next.key && target.model_row_index == next.model_row_index
        }) {
            cursor.selected = next_selected;
        } else {
            return;
        }
        self.annotations_state.annotation_block_scroll = None;
        self.refresh_annotation_cursor_target_layout();
        self.keep_annotation_cursor_inside_scroll_region(moving_up);
        self.focus_hunk_at_annotation_cursor();
        self.runtime.dirty = true;
    }

    fn scroll_saved_annotation_block(
        &mut self,
        current: &AnnotationTarget,
        delta: isize,
    ) -> Option<isize> {
        let text = self.annotations_state.annotations.get(&current.key)?;
        let height = self.annotation_saved_block_height(&current.key, text);
        if !self.saved_annotation_block_is_rendered(current) {
            if self.reveal_saved_annotation_block(current) {
                self.runtime.dirty = true;
                return Some(0);
            }
            return Some(delta);
        }
        // Keep one block row rendered. Reaching the block end must carry at
        // least one row of movement into the diff instead of hiding the whole
        // block while leaving model scroll unchanged.
        let max_offset = height.saturating_sub(1);
        let offset = self
            .annotations_state
            .annotation_block_scroll
            .as_ref()
            .filter(|(key, _)| key == &current.key)
            .map(|(_, offset)| *offset)
            .unwrap_or_default()
            .min(max_offset);
        let amount = delta.unsigned_abs();
        let consumed = if delta < 0 {
            amount.min(offset)
        } else {
            amount.min(max_offset.saturating_sub(offset))
        };
        let offset = if delta < 0 {
            offset.saturating_sub(consumed)
        } else {
            offset.saturating_add(consumed)
        };
        self.annotations_state.annotation_block_scroll = Some((current.key.clone(), offset));
        if consumed > 0 {
            self.clamp_scroll_for_annotation_block();
            self.refresh_annotation_cursor_target_layout();
            self.runtime.dirty = true;
        }
        let remaining = amount.saturating_sub(consumed).min(isize::MAX as usize) as isize;
        Some(if delta < 0 { -remaining } else { remaining })
    }

    fn saved_annotation_block_is_rendered(&self, current: &AnnotationTarget) -> bool {
        plan_diff_viewport_rows_at_scroll(
            self,
            self.viewport.scroll,
            self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX)),
        )
        .into_iter()
        .any(|slot| {
            matches!(
                slot.kind,
                ViewportSlotKind::AnnotationSaved {
                    model_row,
                    ref key,
                    ..
                } if model_row == current.model_row_index && key == &current.key
            )
        })
    }

    fn reveal_saved_annotation_block(&mut self, current: &AnnotationTarget) -> bool {
        let viewport_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        if viewport_rows < 2 {
            return false;
        }
        let target_height = self.annotation_visual_height_for_model_row(current.model_row_index);
        let scroll = self.scroll_for_model_row_offset_at_viewport_row(
            current.model_row_index,
            target_height.saturating_sub(1),
            viewport_rows.saturating_sub(2),
        );
        let block_scroll = self.annotations_state.annotation_block_scroll.clone();
        self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::Preserve);
        self.annotations_state.annotation_block_scroll = block_scroll;
        self.refresh_annotation_cursor_target_layout();
        self.saved_annotation_block_is_rendered(current)
    }

    fn scroll_by_preserving_annotation_block(&mut self, delta: isize) {
        self.close_annotation_target_mode();
        let block_scroll = self.annotations_state.annotation_block_scroll.clone();
        let next = if delta < 0 {
            self.viewport.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.viewport.scroll.saturating_add(delta as usize)
        };
        self.set_scroll_with_grep_sync(next, true, HunkFocusScrollBehavior::ClearOnScroll);
        self.annotations_state.annotation_block_scroll = block_scroll;
        self.clamp_scroll_for_annotation_block();
        self.sync_annotation_cursor_to_viewport();
    }

    fn clamp_scroll_for_annotation_block(&mut self) {
        let max_scroll = self.max_scroll();
        if self.viewport.scroll > max_scroll {
            let block_scroll = self.annotations_state.annotation_block_scroll.clone();
            self.set_scroll_with_grep_sync(max_scroll, true, HunkFocusScrollBehavior::Preserve);
            // The normal setter invalidates block scrolling when the raw scroll
            // changes. Restore the offset that defined this effective maximum.
            self.annotations_state.annotation_block_scroll = block_scroll;
        }
    }

    fn select_preceding_saved_annotation_at_visual_row(
        &mut self,
        current: &AnnotationTarget,
        desired_visual_row: usize,
    ) -> bool {
        let draft_key = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| &draft.key);
        let mut blocks: Vec<_> = self
            .annotations_state
            .annotations
            .iter()
            .filter(|(key, _)| draft_key != Some(*key))
            .filter_map(|(key, text)| {
                let model_row = self.annotation_model_row(key)?;
                let height = self.annotation_saved_block_height(key, text);
                let offset = self
                    .annotations_state
                    .annotation_block_scroll
                    .as_ref()
                    .filter(|(scroll_key, _)| scroll_key == key)
                    .map(|(_, offset)| *offset)
                    .unwrap_or_default()
                    .min(height.saturating_sub(1));
                Some((model_row, key.clone(), height, offset))
            })
            .collect();
        blocks.sort_unstable_by_key(|(model_row, _, _, _)| *model_row);

        let mut annotation_rows_before = 0usize;
        let mut destination = None;
        for (model_row, key, height, offset) in blocks {
            let rendered_height = height.saturating_sub(offset);
            let block_start = self
                .scroll_for_model_row(model_row)
                .saturating_add(annotation_rows_before)
                .saturating_add(self.annotation_visual_height_for_model_row(model_row));
            if model_row < current.model_row_index
                && desired_visual_row >= block_start
                && desired_visual_row < block_start.saturating_add(rendered_height)
            {
                destination = Some((
                    model_row,
                    key,
                    offset.saturating_add(desired_visual_row.saturating_sub(block_start)),
                ));
                break;
            }
            annotation_rows_before = annotation_rows_before.saturating_add(rendered_height);
        }
        let Some((model_row, key, offset)) = destination else {
            return false;
        };

        let scroll = self.scroll_for_model_row(model_row);
        self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::Preserve);
        self.select_annotation_cursor(&key);
        self.annotations_state.annotation_block_scroll = Some((key, offset));
        self.clamp_scroll_for_annotation_block();
        self.refresh_annotation_cursor_target_layout();
        self.focus_hunk_at_annotation_cursor();
        self.runtime.dirty = true;
        true
    }

    fn annotation_block_height_prefixes(&self) -> Vec<(usize, usize)> {
        let draft_key = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| &draft.key);
        let mut prefixes: Vec<_> = self
            .annotations_state
            .annotations
            .iter()
            .filter(|(key, _)| draft_key != Some(*key))
            .filter_map(|(key, text)| {
                let model_row = self.annotation_model_row(key)?;
                let height = self.annotation_saved_block_height(key, text);
                let block_scroll = self
                    .annotations_state
                    .annotation_block_scroll
                    .as_ref()
                    .filter(|(scroll_key, _)| scroll_key == key)
                    .map(|(_, offset)| *offset)
                    .unwrap_or_default()
                    .min(height.saturating_sub(1));
                Some((model_row, height.saturating_sub(block_scroll)))
            })
            .collect();
        prefixes.sort_unstable_by_key(|(model_row, _)| *model_row);
        let mut cumulative_height = 0usize;
        for (_, height) in &mut prefixes {
            cumulative_height = cumulative_height.saturating_add(*height);
            *height = cumulative_height;
        }
        prefixes
    }

    fn annotation_cursor_document_visual_row(
        &self,
        target: &AnnotationTarget,
        annotation_height_prefixes: &[(usize, usize)],
    ) -> usize {
        let target_is_rendered = plan_diff_viewport_rows(
            self,
            self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX)),
        )
        .into_iter()
        .any(|slot| {
            matches!(
                slot.kind,
                ViewportSlotKind::DiffVisual { model_row, .. }
                    if model_row == target.model_row_index
            )
        });
        // Planning above may materialize a sparse wrapped block. Keep the
        // wrapped continuation offset separate from annotation block prefixes.
        let wrapped_offset = if target_is_rendered {
            self.model_row_at_scroll(self.viewport.scroll)
                .filter(|(model_row, _)| *model_row == target.model_row_index)
                .map(|(_, row_offset)| row_offset)
                .unwrap_or_default()
        } else {
            0
        };
        self.annotation_target_document_visual_row(target, annotation_height_prefixes)
            .saturating_add(wrapped_offset)
    }

    fn annotation_target_document_visual_row(
        &self,
        target: &AnnotationTarget,
        annotation_height_prefixes: &[(usize, usize)],
    ) -> usize {
        let mut visual_row = self.scroll_for_model_row(target.model_row_index);
        let prefix_count = annotation_height_prefixes
            .partition_point(|(model_row, _)| *model_row < target.model_row_index);
        if let Some((_, height)) = prefix_count
            .checked_sub(1)
            .and_then(|index| annotation_height_prefixes.get(index))
        {
            visual_row = visual_row.saturating_add(*height);
        }
        if let Some(draft) = self.annotations_state.annotation_draft.as_ref()
            && draft.model_row_index < target.model_row_index
        {
            let body_width = annotation_block_body_width(
                self.viewport.layout,
                self.viewport.viewport_width,
                &draft.key,
            );
            visual_row =
                visual_row.saturating_add(annotation_compose_block_height(draft, body_width));
        }
        visual_row
    }

    fn move_lazy_annotation_cursor(&mut self, delta: isize) {
        let moving_up = delta < 0;
        let Some(previous) = self.annotation_cursor_target().cloned() else {
            let exhausted = self
                .annotations_state
                .annotation_cursor
                .as_ref()
                .is_some_and(|cursor| {
                    if moving_up {
                        cursor.previous_exhausted
                    } else {
                        cursor.next_exhausted
                    }
                });
            if !exhausted || matches!(delta, isize::MIN | isize::MAX) {
                self.initialize_lazy_annotation_cursor_for_move(delta);
            }
            return;
        };
        let exhausted = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .is_some_and(|cursor| {
                if moving_up {
                    cursor.previous_exhausted
                } else {
                    cursor.next_exhausted
                }
            });
        if exhausted {
            self.refresh_annotation_cursor_target_layout();
            self.keep_annotation_cursor_inside_scroll_region(moving_up);
            self.focus_hunk_at_annotation_cursor();
            return;
        }

        let next = if delta == isize::MIN {
            self.boundary_cursor_target(false)
                .map(CursorTargetDeltaResult::Target)
        } else if delta == isize::MAX {
            self.boundary_cursor_target(true)
                .map(CursorTargetDeltaResult::Target)
        } else {
            Some(self.cursor_target_by_delta(&previous, delta))
        };
        let Some(next) = next else {
            return;
        };
        let next = match next {
            CursorTargetDeltaResult::Target(next) => next,
            CursorTargetDeltaResult::Unindexed => {
                if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() {
                    if moving_up {
                        cursor.previous_exhausted = false;
                    } else {
                        cursor.next_exhausted = false;
                    }
                }
                return;
            }
        };
        if next.key == previous.key && next.model_row_index == previous.model_row_index {
            if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() {
                if moving_up {
                    cursor.previous_exhausted = true;
                } else {
                    cursor.next_exhausted = true;
                }
            }
            self.refresh_annotation_cursor_target_layout();
            self.keep_annotation_cursor_inside_scroll_region(moving_up);
            self.focus_hunk_at_annotation_cursor();
            return;
        }

        let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
            return;
        };
        cursor.targets.clear();
        cursor.targets.push(next);
        cursor.selected = 0;
        cursor.previous_exhausted = delta == isize::MIN;
        cursor.next_exhausted = delta == isize::MAX;
        self.annotations_state.annotation_block_scroll = None;
        self.refresh_annotation_cursor_target_layout();
        self.keep_annotation_cursor_inside_scroll_region(moving_up);
        self.focus_hunk_at_annotation_cursor();
        self.runtime.dirty = true;
    }

    fn initialize_lazy_annotation_cursor_for_move(&mut self, delta: isize) {
        let moving_up = delta < 0;
        let target = if delta == isize::MIN {
            self.boundary_cursor_target(false)
        } else if delta == isize::MAX {
            self.boundary_cursor_target(true)
        } else {
            self.cursor_target_for_model_row(
                self.viewport_focus_row()
                    .min(self.document.model.len().saturating_sub(1)),
            )
        };
        let Some(target) = target else {
            if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() {
                cursor.previous_exhausted |= moving_up;
                cursor.next_exhausted |= !moving_up;
            }
            return;
        };
        let Some(cursor) = self.annotations_state.annotation_cursor.as_mut() else {
            return;
        };
        cursor.targets.push(target);
        cursor.selected = 0;
        cursor.previous_exhausted = delta == isize::MIN;
        cursor.next_exhausted = delta == isize::MAX;
        self.refresh_annotation_cursor_target_layout();
        self.keep_annotation_cursor_inside_scroll_region(moving_up);
        self.focus_hunk_at_annotation_cursor();
        self.runtime.dirty = true;
    }

    fn boundary_cursor_target(&self, last: bool) -> Option<AnnotationTarget> {
        let model_row = if last {
            self.document.model.len().checked_sub(1)?
        } else {
            0
        };
        self.cursor_target_for_model_row(model_row)
    }

    fn cursor_target_by_delta(
        &self,
        current: &AnnotationTarget,
        delta: isize,
    ) -> CursorTargetDeltaResult {
        let last = self.document.model.len().saturating_sub(1);
        let model_row = if delta < 0 {
            current.model_row_index.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .model_row_index
                .saturating_add(delta as usize)
                .min(last)
        };
        self.cursor_target_for_model_row(model_row)
            .map(CursorTargetDeltaResult::Target)
            .unwrap_or(CursorTargetDeltaResult::Unindexed)
    }

    fn focus_hunk_at_annotation_cursor(&mut self) {
        let previous_hunk = self.viewport.manual_hunk_focus;
        let Some(model_row) = self
            .annotation_cursor_target()
            .map(|target| target.model_row_index)
        else {
            self.viewport.manual_hunk_focus = None;
            if previous_hunk.is_some() {
                self.runtime.dirty = true;
            }
            return;
        };
        self.viewport.manual_hunk_focus = self
            .document
            .model
            .row(model_row)
            .and_then(|row| row.typed_hunk_key());
        if self.viewport.manual_hunk_focus != previous_hunk {
            self.runtime.dirty = true;
        }
        if let Some(file) = self.document.model.file_at_row(model_row) {
            let file = crate::model::FileIndex::new(file);
            if self.sidebar.selected_file != file {
                self.sidebar.selected_file = file;
                self.runtime.dirty = true;
            }
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
        }
    }

    pub(in crate::app) fn keep_annotation_cursor_rendered(&mut self) -> bool {
        self.keep_annotation_cursor_rendered_with_anchor(None)
    }

    pub(in crate::app) fn keep_annotation_cursor_rendered_with_model_row(
        &mut self,
        anchor_model_row: usize,
    ) -> bool {
        self.keep_annotation_cursor_rendered_with_anchor(Some(anchor_model_row))
    }

    fn keep_annotation_cursor_rendered_with_anchor(
        &mut self,
        anchor_model_row: Option<usize>,
    ) -> bool {
        if self.annotation_cursor_viewport_row().is_some() {
            return true;
        }
        let Some(target) = self.annotation_cursor_target().cloned() else {
            return false;
        };
        let model_row = target.model_row_index;
        let viewport_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let target_scroll = self.scroll_for_model_row(model_row);
        let viewport_row = if target_scroll < self.viewport.scroll {
            0
        } else {
            viewport_rows.saturating_sub(1)
        };
        let scroll = self.scroll_for_model_row_offset_at_viewport_row(model_row, 0, viewport_row);
        if anchor_model_row
            .is_some_and(|anchor| !self.model_row_rendered_at_scroll(scroll, viewport_rows, anchor))
        {
            return false;
        }
        self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::Preserve);
        self.select_discovered_annotation_target(target);
        self.annotation_cursor_viewport_row().is_some()
            && anchor_model_row.is_none_or(|anchor| {
                self.model_row_rendered_at_scroll(self.viewport.scroll, viewport_rows, anchor)
            })
    }

    pub(in crate::app) fn keep_annotation_cursor_inside_scroll_region(&mut self, moving_up: bool) {
        let viewport_rows = self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX));
        let scroll_off = ANNOTATION_CURSOR_SCROLL_OFF.min(viewport_rows.saturating_sub(1) / 2);
        let top_row = scroll_off;
        let bottom_row = viewport_rows.saturating_sub(1).saturating_sub(scroll_off);
        let Some((target, previous_exhausted, next_exhausted)) = self
            .annotations_state
            .annotation_cursor
            .as_ref()
            .and_then(|cursor| {
                cursor.targets.get(cursor.selected).map(|target| {
                    (
                        target.clone(),
                        cursor.previous_exhausted,
                        cursor.next_exhausted,
                    )
                })
            })
        else {
            return;
        };
        let model_row = target.model_row_index;
        for _ in 0..2 {
            let desired_viewport_row = match self.annotation_cursor_viewport_row() {
                Some(row) if row < top_row => top_row,
                Some(row) if row > bottom_row => bottom_row,
                Some(_) => return,
                None if moving_up => top_row,
                None => bottom_row,
            };
            let scroll = self.scroll_for_model_row_offset_at_viewport_row(
                model_row,
                0,
                desired_viewport_row,
            );
            self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::Preserve);
            self.select_discovered_annotation_target(target.clone());
            if let Some(cursor) = self.annotations_state.annotation_cursor.as_mut()
                && cursor.targets.get(cursor.selected).is_some_and(|selected| {
                    selected.key == target.key && selected.model_row_index == model_row
                })
            {
                cursor.previous_exhausted = previous_exhausted;
                cursor.next_exhausted = next_exhausted;
            }
        }
    }

    pub(in crate::app) fn annotation_cursor_target_is_rendered(&self) -> bool {
        self.annotation_cursor_viewport_row().is_some()
    }

    pub(in crate::app) fn annotation_cursor_viewport_row(&self) -> Option<usize> {
        let target = self.annotation_cursor_target()?;
        plan_diff_viewport_rows_at_scroll(
            self,
            self.viewport.scroll,
            self.viewport.viewport_rows.clamp(1, usize::from(u16::MAX)),
        )
        .into_iter()
        .enumerate()
        .find_map(|(viewport_row, slot)| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. }
                if model_row == target.model_row_index =>
            {
                Some(viewport_row)
            }
            _ => None,
        })
    }

    pub(crate) fn annotation_cursor_target(&self) -> Option<&AnnotationTarget> {
        let cursor = self.annotations_state.annotation_cursor.as_ref()?;
        cursor.targets.get(cursor.selected)
    }

    pub(crate) fn annotation_cursor_is_visible(&self) -> bool {
        self.annotation_cursor_enabled()
            && !self.diff_modal_hides_annotation_cursor()
            && !self.overlays.annotation_menu_is_open()
    }

    fn handle_annotation_hint_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Esc {
            self.close_annotation_target_mode();
            return true;
        }
        if key.code == KeyCode::Backspace {
            if let Some(mode) = self.annotations_state.annotation_target_mode.as_mut()
                && mode.prefix.pop().is_some()
            {
                self.runtime.dirty = true;
            }
            return true;
        }
        if matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::PageUp
                | KeyCode::PageDown
        ) {
            self.close_annotation_target_mode();
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
            && key.code == KeyCode::Char('c')
        {
            return false;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return true;
        }

        let KeyCode::Char(character) = key.code else {
            return true;
        };
        let Some(character) = configured_hint_character(
            &self.config.syntax_settings.annotations.hint_keys,
            character,
        ) else {
            return true;
        };

        let selected = {
            let mode = self
                .annotations_state
                .annotation_target_mode
                .as_mut()
                .expect("annotation target mode should be open");
            let mut next_prefix = mode.prefix.clone();
            next_prefix.push(character);
            if !mode
                .targets
                .iter()
                .any(|target| target.hint.starts_with(&next_prefix))
            {
                return true;
            }

            mode.prefix = next_prefix;
            self.runtime.dirty = true;
            mode.targets
                .iter()
                .find(|target| target.hint == mode.prefix)
                .cloned()
        };

        if let Some(target) = selected {
            self.open_annotation_draft_for_key(target.key, target.model_row_index);
        }
        true
    }

    pub(crate) fn annotation_target_hints_at_visual_scroll(
        &self,
        visual_scroll: usize,
    ) -> Vec<(
        &str,
        crate::annotation::AnnotationScope,
        AnnotationSide,
        bool,
    )> {
        let Some(mode) = self.annotations_state.annotation_target_mode.as_ref() else {
            return Vec::new();
        };
        mode.targets_at_visual_scroll(visual_scroll)
            .filter_map(|target| {
                let remaining = target.hint.strip_prefix(&mode.prefix)?;
                let existing = self.annotations_state.annotations.contains_key(&target.key);
                Some((remaining, target.key.scope, target.key.side, existing))
            })
            .collect()
    }

    pub(crate) fn annotation_cursor_at_model_row(&self, model_row: usize) -> bool {
        if !self.annotation_cursor_is_visible() {
            return false;
        }
        self.annotation_visual_selection_range()
            .is_some_and(|selection| {
                selection.contains(&model_row)
                    && self
                        .document
                        .model
                        .row(model_row)
                        .and_then(|row| self.annotation_visual_line_coordinate(row))
                        .is_some()
            })
            || self
                .annotation_cursor_target()
                .is_some_and(|target| target.model_row_index == model_row)
    }

    pub(crate) fn annotation_cursor_at_visual_scroll(&self, visual_scroll: usize) -> bool {
        if !self.annotation_cursor_is_visible() {
            return false;
        }
        if let Some((model_row, _)) = self.model_row_at_scroll(visual_scroll)
            && self
                .annotation_visual_selection_range()
                .is_some_and(|selection| {
                    selection.contains(&model_row)
                        && self
                            .document
                            .model
                            .row(model_row)
                            .and_then(|row| self.annotation_visual_line_coordinate(row))
                            .is_some()
                })
        {
            return true;
        }
        self.annotation_cursor_target().is_some_and(|target| {
            visual_scroll >= target.visual_scroll
                && visual_scroll
                    < target
                        .visual_scroll
                        .saturating_add(target.visual_height.max(1))
        })
    }
}

fn source_range(minimum: usize, maximum: usize) -> (usize, usize) {
    if minimum == usize::MAX {
        (0, 0)
    } else {
        (minimum, maximum.saturating_sub(minimum).saturating_add(1))
    }
}

fn configured_hint_character(hint_keys: &str, input: char) -> Option<char> {
    hint_keys.chars().find(|candidate| {
        *candidate == input
            || (candidate.is_ascii_alphabetic()
                && input.is_ascii_alphabetic()
                && candidate.eq_ignore_ascii_case(&input))
    })
}
