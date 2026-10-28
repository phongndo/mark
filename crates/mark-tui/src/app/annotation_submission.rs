use super::{AppEffect, DiffApp};
use crate::{annotation::AnnotationKey, keymap::GlobalAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

impl DiffApp {
    pub(crate) fn all_annotations_submission(&self) -> AppEffect {
        if !crate::integration::has_feedback(self) {
            return AppEffect::Quit;
        }
        let keys = self
            .annotations_state
            .annotations
            .keys()
            .filter(|key| self.annotations_state.annotations.has_human(key))
            .cloned()
            .collect();
        self.annotations_submission(&keys)
            .unwrap_or(AppEffect::Quit)
    }

    fn annotations_submission(&self, keys: &HashSet<AnnotationKey>) -> Option<AppEffect> {
        let markdown = self.selected_annotations_markdown(keys)?;
        Some(AppEffect::SubmitMarks(
            markdown,
            keys.iter().cloned().collect(),
        ))
    }

    pub(crate) fn annotation_submission_key(&mut self, key: KeyEvent) -> Option<bool> {
        if self
            .config
            .keymap
            .matches_single(GlobalAction::SubmitMarks, key)
        {
            self.queue_effect(self.all_annotations_submission());
            return Some(true);
        }
        if key.code == KeyCode::Char(' ') && key.modifiers == KeyModifiers::NONE {
            if let Some(item) = self
                .filtered_annotation_menu_items()
                .get(self.overlays.annotation_menu.selected)
                && self.annotations_state.annotations.has_human(&item.key)
            {
                let selected = &mut self.overlays.annotation_selection;
                if !selected.remove(&item.key) {
                    selected.insert(item.key.clone());
                }
                self.move_annotation_menu_selection(1);
                self.runtime.dirty = true;
            }
            return Some(false);
        }
        if self
            .config
            .keymap
            .matches_single(GlobalAction::SaveMark, key)
        {
            // With nothing checked, send the highlighted mark.
            let mut keys = self.overlays.annotation_selection.clone();
            if keys.is_empty()
                && let Some(item) = self
                    .filtered_annotation_menu_items()
                    .get(self.overlays.annotation_menu.selected)
                    .filter(|item| self.annotations_state.annotations.has_human(&item.key))
            {
                keys.insert(item.key.clone());
            }
            let Some(effect) = self.annotations_submission(&keys) else {
                self.set_warning_notice("No marks to send.");
                return Some(false);
            };
            self.close_annotation_menu();
            self.queue_effect(effect);
            return Some(true);
        }
        None
    }
}
