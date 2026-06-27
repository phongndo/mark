use super::*;

impl DiffApp {
    pub(super) fn handle_open_menu_mouse_scroll(&mut self, kind: MouseEventKind) -> bool {
        let delta = match kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            _ => return false,
        };

        if self.help_menu_open {
            self.scroll_help_menu(delta);
        } else if self.color_scheme_picker_open {
            self.move_color_scheme_selection(delta);
        } else if self.branch_menu_open.is_some() {
            self.move_branch_selection(delta);
        } else if self.commit_menu_open {
            self.move_commit_selection(delta);
        } else if self.review_input_open {
            // Review input has no scrollable content, but the open modal should
            // still consume wheel events instead of scrolling the diff behind it.
        } else if self.diff_menu_open {
            self.move_diff_menu_selection(delta);
        } else if self.options_menu_open {
            self.move_options_menu_selection(delta);
        } else {
            return false;
        }

        self.mouse_scroll.reset();
        true
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> MarkResult<()> {
        if self.handle_open_menu_mouse_scroll(mouse.kind) {
            return Ok(());
        }

        if self.help_menu_open {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.close_help_menu();
            }
            self.mouse_scroll.reset();
            return Ok(());
        }

        if self.file_sidebar_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    self.resize_file_sidebar_to_column(mouse.column);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.file_sidebar_resizing = false;
                    self.resize_file_sidebar_to_column(mouse.column);
                    return Ok(());
                }
                _ => {}
            }
        }

        if self.color_scheme_picker_open {
            match mouse.kind {
                MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(index) = self.color_scheme_index_at(mouse.column, mouse.row) {
                        self.set_color_scheme_selection(index);
                    }
                    return Ok(());
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = self.color_scheme_index_at(mouse.column, mouse.row) {
                        self.set_color_scheme_selection(index);
                        self.select_highlighted_color_scheme();
                    } else if self.is_rendered_color_scheme_picker_position(mouse.column, mouse.row)
                    {
                        self.dirty = true;
                    } else {
                        self.close_color_scheme_picker();
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        if self.options_menu_open {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.close_options_menu();
                return Ok(());
            }
        }

        if self.error_log_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    self.resize_error_log_to_separator_row(mouse.row);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.resize_error_log_to_separator_row(mouse.row);
                    self.error_log_resizing = false;
                    self.dirty = true;
                    return Ok(());
                }
                _ => {}
            }
        }

        self.update_diff_mouse_hover(mouse.column, mouse.row);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.start_error_log_resize(mouse.row) {
                    return Ok(());
                }
                if self.start_file_sidebar_resize(mouse.column, mouse.row) {
                    return Ok(());
                }
                self.handle_click(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollDown => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    self.scroll_file_sidebar_by(1);
                    return Ok(());
                }
                self.mouse_scroll_or_focus_hunk(MouseScrollDirection::Down);
                self.update_diff_mouse_hover(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollUp => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    self.scroll_file_sidebar_by(-1);
                    return Ok(());
                }
                self.mouse_scroll_or_focus_hunk(MouseScrollDirection::Up);
                self.update_diff_mouse_hover(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollLeft => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    return Ok(());
                }
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
                self.update_diff_mouse_hover(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollRight => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.mouse_scroll.reset();
                    return Ok(());
                }
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
                self.update_diff_mouse_hover(mouse.column, mouse.row);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn is_file_sidebar_position(&self, column: u16, row: u16) -> bool {
        self.file_sidebar_open
            && self.file_sidebar_render_width > 0
            && column < self.file_sidebar_render_width
            && row > 0
            && usize::from(row - 1) < self.visible_file_sidebar_rows()
    }

    pub(crate) fn is_file_sidebar_resize_handle(&self, column: u16, row: u16) -> bool {
        self.is_file_sidebar_position(column, row)
            && column.saturating_add(1) == self.file_sidebar_render_width
    }

    pub(crate) fn start_file_sidebar_resize(&mut self, column: u16, row: u16) -> bool {
        if !self.is_file_sidebar_resize_handle(column, row) {
            return false;
        }

        self.file_sidebar_resizing = true;
        self.resize_file_sidebar_to_column(column);
        true
    }

    pub(crate) fn resize_file_sidebar_to_column(&mut self, column: u16) {
        let width = column.saturating_add(1);
        self.set_file_sidebar_width(width);
    }

    pub(crate) fn handle_click(&mut self, column: u16, row: u16) {
        let clicked_selector = row == 0 && column < diff_selector_width(&self.options);
        let clicked_branch_selector = (row == 0)
            .then(|| self.branch_selector_at(column))
            .flatten();
        let clicked_commit_selector = row == 0 && self.commit_selector_at(column);

        if self.review_input_open {
            if self.is_rendered_review_input_position(column, row) {
                self.dirty = true;
                return;
            }

            self.close_review_input();
            if clicked_selector {
                self.toggle_diff_menu();
            }
            return;
        }

        if self.commit_menu_open {
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

        if let Some(menu) = self.branch_menu_open {
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

        if self.diff_menu_open {
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

        if self.color_scheme_picker_open {
            self.close_color_scheme_picker();
            return;
        }

        if self.options_menu_open {
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
            .file_sidebar_scroll
            .saturating_add(usize::from(row - 1));
        let Some(file) = self.model.visible_files().get(position).copied() else {
            return false;
        };

        self.select_file(file);
        true
    }

    pub(crate) fn handle_diff_click(&mut self, column: u16, row: u16) -> bool {
        let Some((diff_column, viewport_row)) = self.diff_viewport_position(column, row) else {
            return false;
        };
        let width = self.viewport_width;
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
        if self
            .mouse_hover
            .is_some_and(|(_, hover_row)| hover_row == viewport_row)
            && self.try_open_annotation_draft_at_viewport_row(viewport_row, diff_column)
        {
            return true;
        }

        let Some(model_row) = model_row_for_viewport_row(self, viewport_row) else {
            return false;
        };
        self.handle_context_at_row(model_row)
    }

    pub(super) fn diff_viewport_position(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        let area = self.rendered_diff_area?;
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
        if self.line_wrapping {
            let start = self.wrapped_visual_scroll_for_model_row(model_row_index);
            let height = self.wrapped_visual_height_for_model_row(model_row_index);
            start.saturating_add(height.saturating_sub(1))
        } else {
            model_row_index
        }
    }

    pub(crate) fn annotation_label(&self, key: &AnnotationKey) -> Option<String> {
        Some(format!("{} {}{}", key.path, key.side.label(), key.line))
    }

    pub(super) fn handle_annotation_submit_click(&mut self, viewport_row: u16) -> bool {
        let Some(draft) = self.annotation_draft.as_ref() else {
            return false;
        };
        if compose_block_bottom_viewport_row(self, draft.model_row_index) != Some(viewport_row) {
            return false;
        }
        let draft = self.annotation_draft.take().expect("draft");
        self.commit_annotation_draft(draft);
        true
    }

    pub(super) fn handle_annotation_edit_click(&mut self, viewport_row: u16) -> bool {
        if self.annotation_draft.is_some() {
            return false;
        }
        let Some((model_row, key)) = annotation_saved_key_at_bottom_border(self, viewport_row)
        else {
            return false;
        };
        self.open_annotation_draft_for_key(key, model_row)
    }

    pub(super) fn handle_annotation_close_click(&mut self, viewport_row: u16) -> bool {
        if let Some(draft) = self.annotation_draft.as_ref() {
            if compose_block_top_viewport_row(self, draft.model_row_index) == Some(viewport_row) {
                self.annotation_draft = None;
                self.set_scroll_with_grep_sync(
                    self.scroll,
                    false,
                    HunkFocusScrollBehavior::Preserve,
                );
                self.dirty = true;
                return true;
            }
            return false;
        }

        if self.filter_input.is_some() {
            return false;
        }

        let Some((_model_row, key)) = annotation_saved_key_at_top_border(self, viewport_row) else {
            return false;
        };
        if self.annotations.remove(&key).is_some() {
            self.set_scroll_with_grep_sync(self.scroll, false, HunkFocusScrollBehavior::Preserve);
            self.dirty = true;
            return true;
        }
        false
    }

    pub(super) fn try_open_annotation_draft_at_viewport_row(
        &mut self,
        viewport_row: u16,
        column: u16,
    ) -> bool {
        if self.filter_input.is_some() {
            return false;
        }
        if self.annotation_draft.is_some() {
            return false;
        }
        let Some(visual_row) = visual_scroll_for_viewport_row(self, viewport_row) else {
            return false;
        };
        let row_index = if self.line_wrapping {
            let Some((row_index, _)) = self.model_row_at_scroll(visual_row) else {
                return false;
            };
            row_index
        } else {
            visual_row
        };
        let Some(row) = self.model.row(row_index) else {
            return false;
        };
        if !crate::render::viewport_plan::row_has_diff_code_content(row) {
            return false;
        }
        if self.annotation_anchor_visual_scroll(row_index) != visual_row {
            return false;
        }
        let Some(key) = self.annotation_key_for_add_click(row, column) else {
            return false;
        };
        self.open_annotation_draft_for_key(key, row_index)
    }

    pub(super) fn annotation_key_for_add_click(
        &self,
        row: UiRow,
        column: u16,
    ) -> Option<AnnotationKey> {
        if !annotation_hit_at_column(column, self.viewport_width) {
            return None;
        }
        AnnotationKey::from_ui_row(&self.changeset, row)
    }

    pub(super) fn open_annotation_draft_for_key(
        &mut self,
        key: AnnotationKey,
        model_row_index: usize,
    ) -> bool {
        if self.filter_input.is_some() {
            return false;
        }
        let existing = self.annotations.get(&key).cloned().unwrap_or_default();
        let cursor = existing.len();
        self.annotation_draft = Some(AnnotationDraft {
            key,
            model_row_index,
            input: existing,
            cursor,
        });
        self.ensure_annotation_draft_visible();
        self.dirty = true;
        true
    }

    pub(super) fn ensure_annotation_draft_visible(&mut self) {
        let Some((model_row, anchor, desired_scroll)) =
            self.annotation_draft.as_ref().map(|draft| {
                let anchor = self.annotation_anchor_visual_scroll(draft.model_row_index);
                let height = annotation_compose_block_height(draft, self.viewport_width);
                (
                    draft.model_row_index,
                    anchor,
                    annotation_scroll_for_block(anchor, height, self.viewport_rows),
                )
            })
        else {
            return;
        };

        if compose_block_bottom_viewport_row(self, model_row).is_some() {
            return;
        }
        if desired_scroll != self.scroll {
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
            && self.scroll < max_scroll
        {
            let previous_scroll = self.scroll;
            self.set_scroll_with_grep_sync(
                self.scroll.saturating_add(1),
                false,
                HunkFocusScrollBehavior::Preserve,
            );
            if self.scroll == previous_scroll {
                break;
            }
        }
    }
    pub(crate) fn update_diff_mouse_hover(&mut self, column: u16, row: u16) {
        let next = self.diff_mouse_hover_in_diff_area(column, row);
        if self.mouse_hover != next {
            self.mouse_hover = next;
            self.dirty = true;
        }
    }

    pub(crate) fn clear_diff_mouse_hover(&mut self) {
        if self.mouse_hover.take().is_some() {
            self.dirty = true;
        }
    }

    fn diff_mouse_hover_in_diff_area(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        if self.diff_modal_blocks_mouse_hover() {
            return None;
        }
        let area = self.rendered_diff_area?;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if column < area.x
            || row < area.y
            || column >= area.x.saturating_add(area.width)
            || row >= area.y.saturating_add(area.height)
        {
            return None;
        }
        Some((column.saturating_sub(area.x), row.saturating_sub(area.y)))
    }

    pub(crate) fn diff_modal_blocks_mouse_hover(&self) -> bool {
        self.help_menu_open
            || self.color_scheme_picker_open
            || self.options_menu_open
            || self.diff_menu_open
            || self.review_input_open
            || self.commit_menu_open
            || self.branch_menu_open.is_some()
            || self.filter_input.is_some()
            || self.annotation_draft.is_some()
    }

    pub(crate) fn diff_mouse_highlight_visual_row(&self) -> Option<usize> {
        let (_, viewport_row) = self.mouse_hover?;
        visual_scroll_for_viewport_row(self, viewport_row)
    }
}
