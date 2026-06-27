use super::*;

impl DiffApp {
    pub(crate) fn open_filter_input(&mut self, kind: DiffFilterKind) {
        self.filter_input = Some(kind);
        self.clear_diff_mouse_hover();
        self.diff_menu_open = false;
        self.diff_menu.reset_input();
        self.rendered_diff_menu_area = None;
        self.options_menu_open = false;
        self.close_color_scheme_picker();
        self.close_review_input();
        self.close_branch_menu();

        let had_filter =
            !self.filter_query(kind).is_empty() || !self.filter_input_query(kind).is_empty();
        self.filter_query_mut(kind).clear();
        self.filter_input_query_mut(kind).clear();
        *self.filter_input_cursor_mut(kind) = 0;
        if had_filter {
            self.schedule_filter_change(kind, Duration::ZERO);
        } else {
            self.dirty = true;
        }
    }

    pub(crate) fn handle_filter_input_key(&mut self, key: KeyEvent) -> bool {
        let Some(kind) = self.filter_input else {
            return false;
        };

        match key.code {
            KeyCode::Esc => {
                self.clear_all_filters();
                self.filter_input = None;
            }
            KeyCode::Enter => {
                self.commit_filter_input(kind);
                self.filter_input = None;
            }
            _ => match self.apply_filter_input_key(kind, key) {
                TextInputKeyResult::Edited => self.sync_filter_input(kind),
                TextInputKeyResult::Moved => self.dirty = true,
                TextInputKeyResult::Ignored | TextInputKeyResult::Handled => {}
            },
        }

        true
    }

    pub(crate) fn filter_query(&self, kind: DiffFilterKind) -> &str {
        match kind {
            DiffFilterKind::File => &self.file_filter,
            DiffFilterKind::Grep => &self.grep_filter,
        }
    }

    pub(crate) fn filter_query_mut(&mut self, kind: DiffFilterKind) -> &mut String {
        match kind {
            DiffFilterKind::File => &mut self.file_filter,
            DiffFilterKind::Grep => &mut self.grep_filter,
        }
    }

    pub(crate) fn filter_input_query(&self, kind: DiffFilterKind) -> &str {
        match kind {
            DiffFilterKind::File => &self.file_filter_input,
            DiffFilterKind::Grep => &self.grep_filter_input,
        }
    }

    pub(crate) fn filter_input_query_mut(&mut self, kind: DiffFilterKind) -> &mut String {
        match kind {
            DiffFilterKind::File => &mut self.file_filter_input,
            DiffFilterKind::Grep => &mut self.grep_filter_input,
        }
    }

    pub(crate) fn filter_input_cursor(&self, kind: DiffFilterKind) -> usize {
        match kind {
            DiffFilterKind::File => self.file_filter_input_cursor,
            DiffFilterKind::Grep => self.grep_filter_input_cursor,
        }
    }

    pub(crate) fn filter_input_cursor_mut(&mut self, kind: DiffFilterKind) -> &mut usize {
        match kind {
            DiffFilterKind::File => &mut self.file_filter_input_cursor,
            DiffFilterKind::Grep => &mut self.grep_filter_input_cursor,
        }
    }

    pub(super) fn apply_filter_input_key(
        &mut self,
        kind: DiffFilterKind,
        key: KeyEvent,
    ) -> TextInputKeyResult {
        match kind {
            DiffFilterKind::File => handle_text_input_key(
                &mut self.file_filter_input,
                &mut self.file_filter_input_cursor,
                key,
            ),
            DiffFilterKind::Grep => handle_text_input_key(
                &mut self.grep_filter_input,
                &mut self.grep_filter_input_cursor,
                key,
            ),
        }
    }

    pub(crate) fn commit_filter_input(&mut self, kind: DiffFilterKind) {
        let next = self.filter_input_query(kind).to_owned();
        if self.filter_query(kind) == next {
            if self.pending_filter_apply.is_some() {
                self.schedule_filter_change(kind, Duration::ZERO);
            }
            self.dirty = true;
            return;
        }

        *self.filter_query_mut(kind) = next;
        self.schedule_filter_change(kind, Duration::ZERO);
    }

    pub(crate) fn sync_filter_input(&mut self, kind: DiffFilterKind) {
        let next = self.filter_input_query(kind).to_owned();
        if self.filter_query(kind) == next {
            self.dirty = true;
            return;
        }

        *self.filter_query_mut(kind) = next;
        self.schedule_filter_change(kind, FILTER_DEBOUNCE);
    }

    pub(crate) fn clear_all_filters(&mut self) {
        self.grep_matches.clear();
        self.grep_matches_truncated = false;
        self.selected_grep_match = None;

        if self.file_filter.is_empty() && self.grep_filter.is_empty() {
            self.file_filter_input.clear();
            self.file_filter_input_cursor = 0;
            self.grep_filter_input.clear();
            self.grep_filter_input_cursor = 0;
            self.dirty = true;
            return;
        }

        self.file_filter.clear();
        self.file_filter_input.clear();
        self.file_filter_input_cursor = 0;
        self.grep_filter.clear();
        self.grep_filter_input.clear();
        self.grep_filter_input_cursor = 0;
        self.schedule_filter_apply(Duration::ZERO, false);
    }

    pub(crate) fn apply_filters(&mut self, jump_to_grep: bool) {
        self.pending_filter_apply = None;
        self.filter_worker = None;
        self.filter_searching = false;
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.selected_file);

        let search_result = self.search_index.search_with_grep_match_limit(
            &self.file_filter,
            &self.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            jump_to_grep,
            HunkFocusModelBehavior::PreserveIfValid,
        );
    }

    pub(crate) fn schedule_filter_change(&mut self, kind: DiffFilterKind, debounce: Duration) {
        self.schedule_filter_apply(
            debounce,
            kind == DiffFilterKind::Grep && !self.grep_filter.is_empty(),
        );
    }

    pub(crate) fn schedule_filter_apply(&mut self, debounce: Duration, jump_to_grep: bool) {
        #[cfg(test)]
        {
            let _ = debounce;
            self.apply_filters(jump_to_grep);
        }

        #[cfg(not(test))]
        {
            self.filter_generation = self.filter_generation.wrapping_add(1);
            self.pending_filter_apply = Some(PendingFilterApply {
                generation: self.filter_generation,
                due_at: Instant::now() + debounce,
                jump_to_grep,
            });
            self.filter_worker = None;
            self.filter_searching = true;
            self.dirty = true;
        }
    }

    pub(crate) fn start_due_filter_apply(&mut self) {
        let Some(pending) = self.pending_filter_apply else {
            return;
        };
        if Instant::now() < pending.due_at {
            return;
        }

        self.pending_filter_apply = None;
        let generation = pending.generation;
        let jump_to_grep = pending.jump_to_grep;
        let file_filter = self.file_filter.clone();
        let grep_filter = self.grep_filter.clone();
        let worker_file_filter = file_filter.clone();
        let worker_grep_filter = grep_filter.clone();
        let search_index = Arc::clone(&self.search_index);
        let (tx, rx) = oneshot::channel();
        runtime::spawn_detached_blocking(move || {
            let result = search_index.search_with_grep_match_limit(
                &worker_file_filter,
                &worker_grep_filter,
                MAX_LIVE_GREP_MATCHES,
            );
            let _ = tx.send(result);
        });

        self.filter_worker = Some(FilterWorker {
            generation,
            file_filter,
            grep_filter,
            jump_to_grep,
            rx,
        });
        self.filter_searching = true;
        self.dirty = true;
    }

    pub(crate) fn drain_filter_worker(&mut self) {
        let Some(outcome) =
            self.filter_worker
                .as_mut()
                .and_then(|worker| match worker.rx.try_recv() {
                    Ok(result) => Some(Some(result)),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(None),
                })
        else {
            return;
        };

        let Some(worker) = self.filter_worker.take() else {
            return;
        };

        if worker.generation != self.filter_generation
            || worker.file_filter != self.file_filter
            || worker.grep_filter != self.grep_filter
        {
            return;
        }

        self.filter_searching = false;
        match outcome {
            Some(result) => self.apply_filter_result(result, worker.jump_to_grep),
            None => self.set_error_log("filter worker stopped"),
        }
    }

    pub(crate) fn filter_busy(&self) -> bool {
        self.filter_searching || self.pending_filter_apply.is_some() || self.filter_worker.is_some()
    }

    pub(super) fn apply_filter_result(
        &mut self,
        search_result: DiffSearchResult,
        jump_to_grep: bool,
    ) {
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.selected_file);

        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            jump_to_grep,
            HunkFocusModelBehavior::PreserveIfValid,
        );
    }

    pub(super) fn replace_visible_files(
        &mut self,
        search_result: DiffSearchResult,
        selected_path: Option<String>,
        relative_scroll: usize,
        jump_to_grep: bool,
        hunk_focus_behavior: HunkFocusModelBehavior,
    ) {
        let DiffSearchResult {
            visible_files,
            grep_matches,
            grep_matches_truncated,
        } = search_result;

        let selected_file = selected_path
            .and_then(|path| {
                self.changeset
                    .files
                    .iter()
                    .position(|file| file.display_path() == path)
            })
            .filter(|file| visible_files.contains(file))
            .or_else(|| visible_files.first().copied())
            .unwrap_or(0);

        self.stats = diff_stats_for_files(&self.changeset, &visible_files);
        self.max_line_width = self.search_index.max_line_width_for_files(&visible_files);
        self.replace_model(&visible_files, hunk_focus_behavior);
        self.selected_file = selected_file;
        self.grep_matches = grep_match_rows(&self.model, &grep_matches);
        self.grep_matches_truncated = grep_matches_truncated;
        self.selected_grep_match = None;

        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|start| {
                self.scroll_for_model_row(start)
                    .saturating_add(relative_scroll)
            })
            .unwrap_or_default();
        let scroll_behavior = match hunk_focus_behavior {
            HunkFocusModelBehavior::PreserveIfValid => HunkFocusScrollBehavior::Preserve,
            HunkFocusModelBehavior::Clear => HunkFocusScrollBehavior::ClearOnScroll,
        };
        self.set_scroll_with_grep_sync(scroll, true, scroll_behavior);
        self.set_horizontal_scroll(self.horizontal_scroll);
        self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());

        if jump_to_grep && !self.grep_matches.is_empty() {
            self.selected_grep_match = Some(0);
            self.set_scroll_centered_on(self.grep_matches[0]);
        } else {
            self.sync_grep_match_selection_to_scroll();
        }

        self.ensure_annotation_draft_visible();
        self.dirty = true;
    }

    pub(crate) fn filters_active(&self) -> bool {
        !self.file_filter.is_empty() || !self.grep_filter.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn current_grep_match_row(&self) -> Option<usize> {
        self.selected_grep_match_row()
    }

    pub(super) fn selected_grep_match_row(&self) -> Option<usize> {
        if self.grep_filter.is_empty() {
            return None;
        }

        self.selected_grep_match
            .and_then(|index| self.grep_matches.get(index).copied())
    }

    pub(crate) fn sync_grep_match_selection_to_scroll(&mut self) {
        if self.grep_filter.is_empty() || self.grep_matches.is_empty() {
            self.selected_grep_match = None;
            return;
        }

        self.selected_grep_match = self
            .grep_matches
            .iter()
            .position(|row| self.grep_match_is_visible_or_below_scroll(*row))
            .or_else(|| self.grep_matches.len().checked_sub(1));
    }

    pub(crate) fn move_grep_match(&mut self, delta: isize) {
        if self.grep_filter.is_empty() {
            self.selected_grep_match = None;
            return;
        }

        if self.grep_matches.is_empty() {
            self.selected_grep_match = None;
            self.set_warning_notice("no grep matches");
            return;
        }

        let len = self.grep_matches.len();
        let current = self.selected_grep_match.unwrap_or_else(|| {
            self.grep_matches
                .iter()
                .position(|row| self.grep_match_is_visible_or_below_scroll(*row))
                .unwrap_or(0)
        });
        let next = if delta < 0 {
            current
                .saturating_add(len)
                .saturating_sub(delta.unsigned_abs() % len)
                % len
        } else {
            current.saturating_add(delta as usize) % len
        };

        self.selected_grep_match = Some(next);
        self.set_scroll_for_grep_navigation(self.grep_matches[next]);
        self.dirty = true;
    }

    pub(super) fn grep_match_is_visible_or_below_scroll(&self, row: usize) -> bool {
        let scroll = self.scroll_for_model_row(row);
        if !self.line_wrapping {
            return scroll >= self.scroll;
        }

        let height = self.wrapped_visual_height_for_model_row(row);
        scroll.saturating_add(height) > self.scroll
    }

    pub(crate) fn set_scroll_for_grep_navigation(&mut self, row: usize) {
        self.set_scroll_centered_on(row);
    }
}
