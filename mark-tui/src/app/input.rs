use super::*;

impl DiffApp {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.handle_annotation_save_or_cancel_key(key) {
            return Ok(false);
        }
        if is_quit_key(key) {
            return Ok(true);
        }

        self.mouse_scroll.reset();

        if self.filter_input.is_some() && self.handle_filter_input_key(key) {
            return Ok(false);
        }

        if self.annotation_draft.is_some() && self.handle_annotation_input_key(key) {
            return Ok(false);
        }

        if self.help_menu_open {
            return self.handle_help_menu_key(key);
        }

        if self.branch_menu_open.is_some() {
            return self.handle_branch_menu_key(key);
        }

        if self.commit_menu_open {
            return self.handle_commit_menu_key(key);
        }

        if self.review_input_open {
            return self.handle_review_input_key(key);
        }

        if self.diff_menu_open {
            return self.handle_diff_menu_key(key);
        }

        if self.color_scheme_picker_open {
            return self.handle_color_scheme_picker_key(key);
        }

        if self.options_menu_open && !self.color_scheme_picker_open {
            return self.handle_options_menu_key(key);
        }

        if key.code == KeyCode::Esc && self.close_error_log() {
            return Ok(false);
        }

        if let Some(prefix) = self.key_prefix_pending.take() {
            return self.handle_prefix_key(prefix, key);
        }

        if let Some(should_quit) = self.handle_single_global_key(key)? {
            return Ok(should_quit);
        }

        if self.keymap.is_prefix(key) {
            self.key_prefix_pending = Some(KeyPress::from(key));
            self.dirty = true;
            return Ok(false);
        }

        if self.error_log.is_some() {
            match key.code {
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.resize_error_log(1);
                    return Ok(false);
                }
                KeyCode::Char('-') => {
                    self.resize_error_log(-1);
                    return Ok(false);
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc if self.filters_active() => self.clear_all_filters(),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_or_focus_hunk(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_or_focus_hunk(-1),
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
            KeyCode::PageDown => self.scroll_or_focus_hunk(20),
            KeyCode::Char('d') if is_plain_char_key(key, 'd') => self.scroll_or_focus_hunk(20),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_or_focus_hunk(20);
            }
            KeyCode::PageUp | KeyCode::Char('u') => self.scroll_or_focus_hunk(-20),
            KeyCode::Home => self.set_scroll(0),
            KeyCode::Char('g') if is_plain_char_key(key, 'g') => self.set_scroll(0),
            KeyCode::End | KeyCode::Char('G') => self.set_scroll(self.max_scroll()),
            KeyCode::Char('n') if !self.grep_filter.is_empty() => self.move_grep_match(1),
            KeyCode::Char('p') | KeyCode::Char('N') if !self.grep_filter.is_empty() => {
                self.move_grep_match(-1);
            }
            KeyCode::Char('n') | KeyCode::Char('p') | KeyCode::Char('N') => {}
            _ => {}
        }

        Ok(false)
    }

    pub(crate) fn handle_single_global_key(&mut self, key: KeyEvent) -> MarkResult<Option<bool>> {
        for action in NORMAL_GLOBAL_ACTIONS.iter().copied() {
            if self.keymap.matches_single(action, key) {
                return self.perform_global_action(action);
            }
        }

        Ok(None)
    }

    pub(crate) fn handle_prefix_key(
        &mut self,
        prefix: KeyPress,
        key: KeyEvent,
    ) -> MarkResult<bool> {
        self.key_prefix_pending = None;

        if key.code == KeyCode::Esc {
            self.dirty = true;
            return Ok(false);
        }

        for action in NORMAL_GLOBAL_ACTIONS.iter().copied() {
            if self.keymap.matches_prefix(action, prefix, key) {
                return Ok(self.perform_global_action(action)?.unwrap_or(false));
            }
        }

        self.dirty = true;
        Ok(false)
    }

    pub(super) fn perform_global_action(
        &mut self,
        action: GlobalAction,
    ) -> MarkResult<Option<bool>> {
        match action {
            GlobalAction::Quit => Ok(Some(true)),
            GlobalAction::Help => {
                self.toggle_help_menu();
                Ok(Some(false))
            }
            GlobalAction::Reload => {
                self.reload()?;
                Ok(Some(false))
            }
            GlobalAction::FileFilter => {
                self.open_filter_input(DiffFilterKind::File);
                Ok(Some(false))
            }
            GlobalAction::Grep => {
                self.open_filter_input(DiffFilterKind::Grep);
                Ok(Some(false))
            }
            GlobalAction::DiffMenu => {
                self.open_diff_menu();
                Ok(Some(false))
            }
            GlobalAction::HeadBranch => {
                self.toggle_branch_menu(BranchMenu::Head);
                Ok(Some(false))
            }
            GlobalAction::BaseBranch => {
                self.toggle_branch_menu(BranchMenu::Base);
                Ok(Some(false))
            }
            GlobalAction::CommitPicker => {
                self.toggle_commit_menu();
                Ok(Some(false))
            }
            GlobalAction::OptionsMenu => {
                self.open_options_menu();
                Ok(Some(false))
            }
            GlobalAction::FileBrowser => {
                self.toggle_file_sidebar();
                Ok(Some(false))
            }
            GlobalAction::PreviousFile => {
                self.move_file(-1);
                Ok(Some(false))
            }
            GlobalAction::NextFile => {
                self.move_file(1);
                Ok(Some(false))
            }
            GlobalAction::PreviousHunk => {
                self.previous_hunk();
                Ok(Some(false))
            }
            GlobalAction::NextHunk => {
                self.next_hunk();
                Ok(Some(false))
            }
            GlobalAction::ExpandContextUp => {
                self.expand_context_around_focused_hunk(-1);
                Ok(Some(false))
            }
            GlobalAction::ExpandContextDown => {
                self.expand_context_around_focused_hunk(1);
                Ok(Some(false))
            }
            GlobalAction::CollapseContextAll => {
                self.collapse_all_context();
                Ok(Some(false))
            }
            GlobalAction::Layout => {
                self.toggle_layout();
                Ok(Some(false))
            }
            GlobalAction::EditHunk => {
                self.open_focused_hunk_in_editor();
                Ok(Some(false))
            }
            GlobalAction::CopyMarks => {
                self.copy_marks_to_terminal_clipboard();
                Ok(Some(false))
            }
            GlobalAction::CopyErrorLog => {
                if self.error_log.is_none() {
                    return Ok(None);
                }
                self.copy_error_log_to_terminal_clipboard();
                Ok(Some(false))
            }
            GlobalAction::ClearFilters => {
                self.clear_all_filters();
                self.filter_input = None;
                Ok(Some(false))
            }
            GlobalAction::NextDiffType => {
                self.cycle_diff_choice(1);
                Ok(Some(false))
            }
            GlobalAction::PreviousDiffType => {
                self.cycle_diff_choice(-1);
                Ok(Some(false))
            }
            GlobalAction::NextAnnotation => {
                self.move_annotation(1);
                Ok(Some(false))
            }
            GlobalAction::PreviousAnnotation => {
                self.move_annotation(-1);
                Ok(Some(false))
            }
            GlobalAction::SaveMark | GlobalAction::CancelMark => Ok(None),
        }
    }

    pub(crate) fn handle_diff_menu_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_diff_menu();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Down, key) {
            self.move_diff_menu_selection(1);
        } else if self.keymap.matches_menu(MenuAction::Up, key) {
            self.move_diff_menu_selection(-1);
        } else if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.select_highlighted_diff_choice();
        } else if !self.apply_diff_menu_input_key(key) {
            match key.code {
                KeyCode::Home => self.set_diff_menu_selection(0),
                KeyCode::End => self.set_diff_menu_selection(usize::MAX),
                _ => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn handle_review_input_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_review_input();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.submit_review_input();
        } else {
            match handle_text_input_key(&mut self.review_input, &mut self.review_input_cursor, key)
            {
                TextInputKeyResult::Edited | TextInputKeyResult::Moved => self.dirty = true,
                TextInputKeyResult::Handled | TextInputKeyResult::Ignored => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn handle_branch_menu_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_branch_menu();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Down, key) {
            self.cycle_branch_completion(1);
        } else if self.keymap.matches_menu(MenuAction::Up, key) {
            self.cycle_branch_completion(-1);
        } else if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.select_highlighted_branch_match();
        } else if !self.apply_branch_input_key(key) {
            match key.code {
                KeyCode::PageDown => self.move_branch_selection(self.branch_menu_rows() as isize),
                KeyCode::PageUp => self.move_branch_selection(-(self.branch_menu_rows() as isize)),
                KeyCode::Home => self.set_branch_selection(0),
                KeyCode::End => self.set_branch_selection(usize::MAX),
                _ => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn handle_commit_menu_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_commit_menu();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Down, key) {
            self.cycle_commit_completion(1);
        } else if self.keymap.matches_menu(MenuAction::Up, key) {
            self.cycle_commit_completion(-1);
        } else if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.select_highlighted_commit_match();
        } else if !self.apply_commit_input_key(key) {
            match key.code {
                KeyCode::PageDown => self.move_commit_selection(self.commit_menu_rows() as isize),
                KeyCode::PageUp => self.move_commit_selection(-(self.commit_menu_rows() as isize)),
                KeyCode::Home => self.set_commit_selection(0),
                KeyCode::End => self.set_commit_selection(usize::MAX),
                _ => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn handle_options_menu_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_options_menu();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Down, key) {
            self.move_options_menu_selection(1);
        } else if self.keymap.matches_menu(MenuAction::Up, key) {
            self.move_options_menu_selection(-1);
        } else if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.activate_selected_option();
        } else if !self.apply_options_menu_input_key(key) {
            match key.code {
                KeyCode::Left => self.cycle_selected_option(-1),
                KeyCode::Right => self.cycle_selected_option(1),
                KeyCode::Home => self.set_options_menu_selection(0),
                KeyCode::End => self.set_options_menu_selection(usize::MAX),
                _ => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn handle_color_scheme_picker_key(&mut self, key: KeyEvent) -> MarkResult<bool> {
        if self.keymap.matches_menu(MenuAction::Close, key) {
            self.close_color_scheme_picker();
            return Ok(false);
        }

        if self.keymap.matches_menu(MenuAction::Down, key) {
            self.move_color_scheme_selection(1);
        } else if self.keymap.matches_menu(MenuAction::Up, key) {
            self.move_color_scheme_selection(-1);
        } else if self.keymap.matches_menu(MenuAction::Select, key)
            || self.keymap.matches_menu(MenuAction::Confirm, key)
        {
            self.select_highlighted_color_scheme();
        } else if !self.apply_color_scheme_input_key(key) {
            match key.code {
                KeyCode::Home => self.set_color_scheme_selection(0),
                KeyCode::End => self.set_color_scheme_selection(usize::MAX),
                _ => {}
            }
        }

        Ok(false)
    }

    pub(crate) fn editor_shortcut_available(&self) -> bool {
        self.filter_input.is_none()
            && !self.help_menu_open
            && self.branch_menu_open.is_none()
            && !self.diff_menu_open
            && !self.review_input_open
            && !self.options_menu_open
            && self.key_prefix_pending.is_none()
            && !self.color_scheme_picker_open
            && !self.commit_menu_open
    }
}
