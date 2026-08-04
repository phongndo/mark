use super::super::DiffApp;

impl DiffApp {
    pub(crate) fn diff_modal_hides_annotation_cursor(&self) -> bool {
        self.overlays.help_menu_is_open()
            || self.overlays.color_scheme_picker_is_open()
            || self.overlays.options_menu_is_open()
            || self.overlays.diff_menu_is_open()
            || self.overlays.review_input_is_open()
            || self.refs.commit_menu_is_open()
            || self.refs.branch_menu_is_open()
            || self.filters.filter_input.is_some()
            || self.annotations_state.annotation_draft.is_some()
            || self.annotations_state.annotation_target_mode.is_some()
    }
}
