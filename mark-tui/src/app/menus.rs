use super::*;

impl DiffApp {
    pub(crate) fn toggle_diff_menu(&mut self) {
        if self.overlays.diff_menu_open {
            self.close_diff_menu();
        } else {
            self.open_diff_menu();
        }
    }

    pub(crate) fn open_diff_menu(&mut self) {
        let choices = self.diff_menu_choices();
        if choices.is_empty() {
            return;
        }
        self.overlays.diff_menu.reset();
        self.overlays.diff_menu_open = true;
        self.overlays.options_menu_open = false;
        self.close_color_scheme_picker();
        self.refs.branch_menu_open = None;
        self.set_rendered_branch_menu_area(None);
        self.close_review_input();
        self.close_commit_menu();
        self.runtime.dirty = true;
    }

    pub(crate) fn close_diff_menu(&mut self) {
        if self.overlays.diff_menu_open
            || !self.overlays.diff_menu.input.is_empty()
            || self.overlays.rendered_diff_menu_area.is_some()
        {
            self.overlays.diff_menu_open = false;
            self.overlays.diff_menu.reset_input();
            self.set_rendered_diff_menu_area(None);
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn open_review_input(&mut self) {
        self.overlays.review_input.clear();
        self.overlays.review_input_cursor = 0;
        self.overlays.review_input_open = true;
        self.overlays.diff_menu_open = false;
        self.overlays.diff_menu.reset_input();
        self.set_rendered_diff_menu_area(None);
        self.overlays.options_menu_open = false;
        self.close_color_scheme_picker();
        self.refs.branch_menu_open = None;
        self.set_rendered_branch_menu_area(None);
        self.close_commit_menu();
        self.runtime.dirty = true;
    }

    pub(crate) fn close_review_input(&mut self) {
        if self.overlays.review_input_open
            || !self.overlays.review_input.is_empty()
            || self.overlays.rendered_review_input_area.is_some()
        {
            self.overlays.review_input_open = false;
            self.overlays.review_input.clear();
            self.overlays.review_input_cursor = 0;
            self.set_rendered_review_input_area(None);
            self.runtime.dirty = true;
        }
    }
}
