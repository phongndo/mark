use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::DiffApp;
use crate::{
    annotation::{
        AnnotationKey, AnnotationSide, AnnotationTarget, AnnotationTargetMode,
        annotation_hint_codes,
    },
    render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows},
};

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

        let visible_rows = self.viewport.viewport_rows.max(1);
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
            let Some(key) = AnnotationKey::from_ui_row(&self.document.changeset, row) else {
                continue;
            };
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
                viewport_row,
                hint: String::new(),
            });
        }

        if targets.is_empty() {
            self.set_notice("no annotatable lines in viewport");
            return;
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

        self.clear_diff_mouse_hover();
        self.input.reset_mouse_scroll();
        self.annotations_state.annotation_target_mode = Some(AnnotationTargetMode {
            targets,
            prefix: String::new(),
            sticky,
        });
        self.runtime.dirty = true;
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

    pub(crate) fn annotation_target_hint_at_visual_scroll(
        &self,
        visual_scroll: usize,
    ) -> Option<(&str, AnnotationSide, bool)> {
        let mode = self.annotations_state.annotation_target_mode.as_ref()?;
        let target = mode.target_at_visual_scroll(visual_scroll)?;
        let remaining = target.hint.strip_prefix(&mode.prefix)?;
        let existing = self.annotations_state.annotations.contains_key(&target.key);
        Some((remaining, target.key.side, existing))
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
