use super::super::{DiffApp, HunkFocusScrollBehavior, annotation_scroll_for_block};
use crate::annotation::{AnnotationDraft, AnnotationKey, AnnotationScope};
use crate::render::annotations::annotation_compose_block_height;
use crate::render::viewport_plan::{
    annotation_saved_key_at_bottom_border, annotation_saved_key_at_top_border,
    compose_block_bottom_viewport_row, compose_block_top_viewport_row,
};

impl DiffApp {
    pub(super) fn diff_viewport_position(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        let area = self.viewport.rendered_diff_area?;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || row < area.y
            || column >= area.x.saturating_add(area.width)
            || row >= area.y.saturating_add(area.height)
        {
            return None;
        }

        Some((column.saturating_sub(area.x), row.saturating_sub(area.y)))
    }

    pub(crate) fn annotation_anchor_visual_scroll(&self, model_row_index: usize) -> usize {
        if self.viewport.line_wrapping {
            let start = self.wrapped_visual_scroll_for_model_row(model_row_index);
            let height = self.wrapped_visual_height_for_model_row(model_row_index);
            start.saturating_add(height.saturating_sub(1))
        } else {
            model_row_index
        }
    }

    pub(crate) fn annotation_block_label(&self, key: &AnnotationKey, composing: bool) -> String {
        let label = if composing {
            "Add note"
        } else {
            self.annotations_state
                .annotations
                .label(key)
                .unwrap_or("Comment")
        };
        format!("{label} · {}", key.target_label())
    }

    pub(crate) fn annotation_label(&self, key: &AnnotationKey) -> Option<String> {
        let target = match key.scope {
            AnnotationScope::File => "file".to_owned(),
            AnnotationScope::Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
            } => format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
            AnnotationScope::Line => format!("{}{}", key.side.label(), key.line),
        };
        Some(format!("{} {target}", key.path))
    }

    pub(super) fn handle_annotation_submit_click(&mut self, viewport_row: u16) -> bool {
        let Some(draft) = self.annotations_state.annotation_draft.as_ref() else {
            return false;
        };
        if compose_block_bottom_viewport_row(self, draft.model_row_index) != Some(viewport_row) {
            return false;
        }
        let draft = self
            .annotations_state
            .annotation_draft
            .take()
            .expect("draft");
        self.commit_annotation_draft(draft);
        true
    }

    pub(super) fn handle_annotation_edit_click(&mut self, viewport_row: u16) -> bool {
        if self.annotations_state.annotation_draft.is_some() {
            return false;
        }
        let Some((model_row, key)) = annotation_saved_key_at_bottom_border(self, viewport_row)
        else {
            return false;
        };
        if self
            .annotations_state
            .annotations
            .editable_human_text(&key)
            .is_none()
        {
            return false;
        }
        self.open_annotation_draft_for_key(key, model_row)
    }

    pub(super) fn handle_annotation_close_click(&mut self, viewport_row: u16) -> bool {
        if let Some(draft) = self.annotations_state.annotation_draft.as_ref() {
            if compose_block_top_viewport_row(self, draft.model_row_index) == Some(viewport_row) {
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
            return false;
        }

        if self.filters.filter_input.is_some() {
            return false;
        }

        let Some((_model_row, key)) = annotation_saved_key_at_top_border(self, viewport_row) else {
            return false;
        };
        self.remove_mark_at_key(&key)
    }

    pub(in crate::app) fn open_annotation_draft_for_key(
        &mut self,
        key: AnnotationKey,
        model_row_index: usize,
    ) -> bool {
        if self.filters.filter_input.is_some() {
            return false;
        }
        self.select_annotation_cursor(&key);
        self.annotations_state.annotation_block_scroll = None;
        self.annotations_state.sticky_annotation_draft = self
            .annotations_state
            .annotation_target_mode
            .take()
            .is_some_and(|mode| mode.sticky);
        let existing = self
            .annotations_state
            .annotations
            .editable_human_text(&key)
            .map(str::to_owned)
            .unwrap_or_default();
        let cursor = existing.len();
        self.annotations_state.annotation_draft = Some(AnnotationDraft {
            key,
            model_row_index,
            input: existing,
            cursor,
        });
        self.ensure_annotation_draft_visible();
        self.runtime.dirty = true;
        true
    }

    pub(in crate::app) fn ensure_annotation_draft_visible(&mut self) {
        let Some((model_row, anchor, desired_scroll)) = self
            .annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| {
                let anchor = self.annotation_anchor_visual_scroll(draft.model_row_index);
                let height = annotation_compose_block_height(draft, self.viewport.viewport_width);
                (
                    draft.model_row_index,
                    anchor,
                    annotation_scroll_for_block(anchor, height, self.viewport.viewport_rows),
                )
            })
        else {
            return;
        };

        if compose_block_bottom_viewport_row(self, model_row).is_some() {
            return;
        }
        if desired_scroll != self.viewport.scroll {
            self.set_scroll_with_grep_sync(
                desired_scroll,
                false,
                HunkFocusScrollBehavior::Preserve,
            );
        }

        // The compose block is emitted only while the annotated row's anchor is still visible.
        // If the draft is too tall for the viewport, the footer can never be shown; do not
        // chase it past the anchor or the editor disappears entirely.
        let max_scroll = self.max_scroll().min(anchor);
        while compose_block_bottom_viewport_row(self, model_row).is_none()
            && self.viewport.scroll < max_scroll
        {
            let previous_scroll = self.viewport.scroll;
            self.set_scroll_with_grep_sync(
                self.viewport.scroll.saturating_add(1),
                false,
                HunkFocusScrollBehavior::Preserve,
            );
            if self.viewport.scroll == previous_scroll {
                break;
            }
        }
    }
}
