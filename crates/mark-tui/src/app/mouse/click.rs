use super::super::DiffApp;
use crate::annotation::{AnnotationKey, AnnotationSide};
use crate::controls::DiffLayoutMode;
use crate::render::annotations::{
    annotation_close_hit_at_column, annotation_edit_hit_at_column, annotation_submit_hit_at_column,
};
use crate::render::menus::diff_selector_width;
use crate::render::viewport_plan::{model_row_for_viewport_row, visual_scroll_for_viewport_row};

impl DiffApp {
    pub(crate) fn handle_click(&mut self, column: u16, row: u16) {
        self.close_annotation_target_mode();
        let clicked_selector = row == 0 && column < diff_selector_width(&self.document.options);
        let clicked_branch_selector = (row == 0)
            .then(|| self.branch_selector_at(column))
            .flatten();
        let clicked_commit_selector = row == 0 && self.commit_selector_at(column);

        if self.overlays.review_input_is_open() {
            if self.is_rendered_review_input_position(column, row) {
                self.runtime.dirty = true;
                return;
            }

            self.close_review_input();
            if clicked_selector {
                self.toggle_diff_menu();
            }
            return;
        }

        if self.refs.commit_menu_is_open() {
            if let Some(rev) = self.commit_choice_at(column, row) {
                self.close_commit_menu();
                self.select_show_commit(rev);
                return;
            }

            if self.is_rendered_commit_menu_position(column, row) {
                return;
            }

            if clicked_commit_selector {
                self.toggle_commit_menu();
                return;
            }

            self.close_commit_menu();
            if clicked_selector {
                self.toggle_diff_menu();
            }
            return;
        }

        if let Some(menu) = self.refs.branch_menu_open() {
            if let Some(branch) = self.branch_choice_at(menu, column, row) {
                self.close_branch_menu();
                self.select_branch(menu, branch);
                return;
            }

            if self.is_rendered_branch_menu_position(column, row) {
                return;
            }

            if let Some(clicked_menu) = clicked_branch_selector {
                self.toggle_branch_menu(clicked_menu);
                return;
            }

            self.close_branch_menu();
            if clicked_selector {
                self.toggle_diff_menu();
            }
            return;
        }

        if self.overlays.diff_menu_is_open() {
            if let Some(choice) = self.diff_choice_at(column, row) {
                self.close_diff_menu();
                self.select_diff_choice(choice);
                return;
            }

            if self.is_rendered_diff_menu_position(column, row) {
                return;
            }

            if let Some(menu) = clicked_branch_selector {
                self.close_diff_menu();
                self.toggle_branch_menu(menu);
                return;
            }

            if clicked_selector {
                self.toggle_diff_menu();
                return;
            }

            self.close_diff_menu();
            return;
        }

        if self.overlays.color_scheme_picker_is_open() {
            self.close_color_scheme_picker();
            return;
        }

        if self.overlays.options_menu_is_open() {
            self.close_options_menu();
            return;
        }

        if clicked_selector {
            self.toggle_diff_menu();
        } else if clicked_commit_selector {
            self.toggle_commit_menu();
        } else if let Some(menu) = clicked_branch_selector {
            self.toggle_branch_menu(menu);
        } else if !self.handle_file_sidebar_click(column, row) {
            self.handle_diff_click(column, row);
        }
    }

    pub(crate) fn handle_file_sidebar_click(&mut self, column: u16, row: u16) -> bool {
        if !self.is_file_sidebar_position(column, row) {
            return false;
        }

        let position = self
            .sidebar
            .file_sidebar_scroll
            .saturating_add(usize::from(row - 1));
        let file = crate::render::sidebar::file_sidebar_file_at_row(self, position);

        if let Some(file) = file {
            self.select_file(file.get());
        }
        true
    }

    pub(crate) fn handle_diff_click(&mut self, column: u16, row: u16) -> bool {
        let Some((diff_column, viewport_row)) = self.diff_viewport_position(column, row) else {
            return false;
        };
        let width = self.viewport.viewport_width;
        if annotation_submit_hit_at_column(diff_column, width)
            && self.handle_annotation_submit_click(viewport_row)
        {
            return true;
        }
        if annotation_edit_hit_at_column(diff_column, width)
            && self.handle_annotation_edit_click(viewport_row)
        {
            return true;
        }
        if annotation_close_hit_at_column(diff_column, width)
            && self.handle_annotation_close_click(viewport_row)
        {
            return true;
        }
        let selected_cursor =
            self.select_annotation_cursor_at_viewport_row(diff_column, viewport_row);
        let Some(model_row) = model_row_for_viewport_row(self, viewport_row) else {
            return selected_cursor;
        };
        self.handle_context_at_row(model_row) || selected_cursor
    }

    fn select_annotation_cursor_at_viewport_row(
        &mut self,
        diff_column: u16,
        viewport_row: u16,
    ) -> bool {
        if !self.annotation_cursor_enabled()
            || self.filters.filter_input.is_some()
            || visual_scroll_for_viewport_row(self, viewport_row).is_none()
        {
            return false;
        }
        let Some(model_row) = model_row_for_viewport_row(self, viewport_row) else {
            return false;
        };
        self.select_annotation_cursor_model_row(model_row);
        self.select_split_annotation_side(model_row, diff_column);
        true
    }

    fn select_split_annotation_side(&mut self, model_row: usize, diff_column: u16) {
        if self.viewport.layout != DiffLayoutMode::Split {
            return;
        }
        let side = if usize::from(diff_column) < self.viewport.viewport_width / 2 {
            AnnotationSide::Old
        } else {
            AnnotationSide::New
        };
        let Some(row) = self.document.model.row(model_row) else {
            return;
        };
        let Some(key) = AnnotationKey::candidates_from_ui_row(&self.document.changeset, row)
            .into_iter()
            .find(|key| key.side == side)
        else {
            return;
        };
        self.select_annotation_cursor(&key);
    }
}
