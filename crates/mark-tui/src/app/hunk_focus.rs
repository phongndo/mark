use super::DiffApp;

impl DiffApp {
    pub(crate) fn next_hunk(&mut self) {
        if let Some(row) = self
            .document
            .model
            .next_hunk_row(self.hunk_navigation_anchor_row())
        {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn previous_hunk(&mut self) {
        if let Some(row) = self
            .document
            .model
            .previous_hunk_row(self.hunk_navigation_anchor_row())
        {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn move_focused_hunk(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }

        // This path is used only when ordinary scrolling is already clamped at
        // the top or bottom. Moving focus must not recenter the viewport: doing
        // so moves it away from the edge, and the next wheel/key event scrolls
        // it back again, producing a focus/scroll loop.
        let rendered_rows = self.rendered_diff_rows_for_viewport(self.viewport.viewport_rows);
        let mut visible_hunks = Vec::new();
        for rendered_row in rendered_rows {
            let Some(hunk) = self
                .document
                .model
                .row(rendered_row.model_row)
                .and_then(|row| row.typed_hunk_key())
            else {
                continue;
            };
            if visible_hunks.last() != Some(&hunk) {
                visible_hunks.push(hunk);
            }
        }
        let Some(current) = self.focused_hunk_for_viewport(self.viewport.viewport_rows) else {
            return;
        };
        let Some(current_index) = visible_hunks.iter().position(|hunk| *hunk == current) else {
            return;
        };
        let target_index = if delta < 0 {
            current_index.checked_sub(1)
        } else {
            current_index
                .checked_add(1)
                .filter(|index| *index < visible_hunks.len())
        };
        let Some((file, hunk)) = target_index.and_then(|index| visible_hunks.get(index).copied())
        else {
            return;
        };

        let previous_hunk = self.viewport.manual_hunk_focus;
        let previous_file = self.sidebar.selected_file;
        self.viewport.manual_hunk_focus = Some((file, hunk));
        self.sidebar.selected_file = file;
        if let Some(row) = self.document.model.hunk_start_row(file.get(), hunk.get()) {
            self.select_annotation_cursor_near_model_row_in_rendered_hunk(row, (file, hunk));
        }
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
        if self.viewport.manual_hunk_focus != previous_hunk
            || self.sidebar.selected_file != previous_file
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn hunk_navigation_anchor_row(&self) -> usize {
        if let Some((file, hunk)) = self.focused_hunk_for_viewport(self.viewport.viewport_rows)
            && let Some(row) = self.document.model.hunk_start_row(file.get(), hunk.get())
        {
            return row;
        }

        self.viewport_focus_row()
    }

    pub(crate) fn focus_hunk_row(&mut self, row: usize) {
        let target_hunk = self
            .document
            .model
            .row(row)
            .and_then(|row| row.typed_hunk_key());
        let previous_hunk = self.viewport.manual_hunk_focus;
        self.clear_manual_hunk_focus();

        let Some((file, hunk)) = target_hunk else {
            self.set_scroll_centered_on(row);
            return;
        };

        self.set_scroll_focused_on_hunk(file.get(), hunk.get());
        let hunk_start_row = self
            .document
            .model
            .hunk_start_row(file.get(), hunk.get())
            .unwrap_or(row);
        self.select_annotation_cursor_near_model_row_in_hunk(hunk_start_row, (file, hunk));
        let cursor_targets_hunk = self
            .annotation_cursor_target()
            .and_then(|target| self.document.model.row(target.model_row_index))
            .and_then(|row| row.typed_hunk_key())
            == Some((file, hunk));
        let cursor_is_rendered = self
            .keep_annotation_cursor_rendered_with_model_row(hunk_start_row)
            || (cursor_targets_hunk && self.keep_annotation_cursor_rendered());
        if !cursor_is_rendered {
            self.clear_annotation_cursor_target();
        }

        if let Some(row) = self.document.model.hunk_start_row(file.get(), hunk.get())
            && self.model_row_rendered_at_scroll(
                self.viewport.scroll,
                self.viewport.viewport_rows,
                row,
            )
        {
            let previous_file = self.sidebar.selected_file;
            self.viewport.manual_hunk_focus = Some((file, hunk));
            self.sidebar.selected_file = file;
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            if self.viewport.manual_hunk_focus != previous_hunk
                || self.sidebar.selected_file != previous_file
            {
                self.runtime.dirty = true;
            }
        }
    }
}
