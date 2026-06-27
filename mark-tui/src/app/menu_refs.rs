use super::*;

impl DiffApp {
    pub(crate) fn close_branch_menu(&mut self) {
        if self.refs.branch_menu_open.is_some()
            || !self.refs.branch_menu.input.is_empty()
            || self.refs.branch_menu.scroll != 0
            || self.overlays.rendered_branch_menu_area.is_some()
        {
            self.refs.branch_menu_open = None;
            self.refs.branch_menu.reset();
            self.set_rendered_branch_menu_area(None);
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn close_commit_menu(&mut self) {
        if self.refs.commit_menu_open
            || !self.refs.commit_menu.input.is_empty()
            || self.refs.commit_menu.scroll != 0
            || self.overlays.rendered_commit_menu_area.is_some()
        {
            self.refs.commit_menu_open = false;
            self.refs.commit_menu.reset();
            self.set_rendered_commit_menu_area(None);
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn toggle_commit_menu(&mut self) {
        if self.refs.comparison_commits.is_empty() {
            self.set_warning_notice("commit list unavailable");
            return;
        }
        if self.refs.commit_menu_open {
            self.close_commit_menu();
            return;
        }

        self.refs.commit_menu_open = true;
        self.overlays.diff_menu_open = false;
        self.overlays.diff_menu.reset_input();
        self.set_rendered_diff_menu_area(None);
        self.close_review_input();
        self.refs.branch_menu_open = None;
        self.refs.branch_menu.reset_input();
        self.set_rendered_branch_menu_area(None);
        self.overlays.options_menu_open = false;
        self.close_color_scheme_picker();
        self.refs.commit_menu.reset_input();
        self.refs.commit_menu.selected = self
            .selected_commit_menu_choice()
            .and_then(|commit| {
                self.filtered_commits()
                    .iter()
                    .position(|candidate| candidate.sha == commit.sha)
            })
            .unwrap_or_default()
            .min(self.max_commit_menu_selection());
        self.ensure_commit_selection_visible();
        self.runtime.dirty = true;
    }

    pub(crate) fn toggle_branch_menu(&mut self, menu: BranchMenu) {
        if self.refs.comparison_branches.is_empty() {
            return;
        }
        if self.refs.branch_menu_open == Some(menu) {
            self.close_branch_menu();
            return;
        }

        self.refs.branch_menu_open = Some(menu);
        self.overlays.diff_menu_open = false;
        self.overlays.diff_menu.reset_input();
        self.set_rendered_diff_menu_area(None);
        self.close_review_input();
        self.overlays.options_menu_open = false;
        self.close_color_scheme_picker();
        self.close_commit_menu();
        self.refs.branch_menu.reset_input();
        self.refs.branch_menu.selected = self
            .branch_ref(menu)
            .and_then(|branch| {
                self.filtered_branches()
                    .iter()
                    .position(|candidate| *candidate == branch)
            })
            .unwrap_or_default()
            .min(self.max_branch_menu_selection());
        self.ensure_branch_selection_visible();
        self.runtime.dirty = true;
    }

    pub(crate) fn branch_selector_at(&self, column: u16) -> Option<BranchMenu> {
        [BranchMenu::Head, BranchMenu::Base]
            .into_iter()
            .find(|menu| {
                let Some(start) = self.branch_selector_start(*menu) else {
                    return false;
                };
                let Some(width) = self.branch_selector_width(*menu) else {
                    return false;
                };
                column >= start && column < start.saturating_add(width)
            })
    }

    pub(crate) fn is_rendered_branch_menu_position(&self, column: u16, row: u16) -> bool {
        self.runtime
            .hit_map
            .branch_menu_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub(crate) fn branch_choice_at(
        &self,
        menu: BranchMenu,
        column: u16,
        row: u16,
    ) -> Option<String> {
        if self.refs.branch_menu_open != Some(menu) {
            return None;
        }

        let menu_area = self.runtime.hit_map.branch_menu_area?;
        let inner = branch_menu_block(self.config.theme, menu).inner(menu_area);
        if column < inner.x
            || column >= inner.x.saturating_add(inner.width)
            || row < inner.y.saturating_add(2)
            || row >= inner.y.saturating_add(inner.height)
        {
            return None;
        }

        let row_index = usize::from(row.saturating_sub(inner.y).saturating_sub(2));
        let pinned_rows = usize::from(self.selected_branch_menu_choice(menu).is_some());
        if row_index < pinned_rows {
            return None;
        }

        let branch_index = row_index.saturating_sub(pinned_rows);
        let rendered_choices = inner.height.saturating_sub(2 + pinned_rows as u16) as usize;
        if branch_index >= rendered_choices {
            return None;
        }

        self.filtered_branch(branch_index).map(str::to_owned)
    }

    pub(crate) fn filtered_branch(&self, row_index: usize) -> Option<&str> {
        self.filtered_branches()
            .get(self.refs.branch_menu.scroll.saturating_add(row_index))
            .copied()
    }

    pub(crate) fn move_branch_selection(&mut self, delta: isize) {
        let len = self.filtered_branches().len();
        let rows = self.branch_menu_rows();
        if SelectorController::new(&mut self.refs.branch_menu, len)
            .with_visible_rows(rows)
            .move_by(delta, SelectorMovement::Saturating)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_branch_selection(&mut self, selected: usize) {
        let len = self.filtered_branches().len();
        let rows = self.branch_menu_rows();
        if SelectorController::new(&mut self.refs.branch_menu, len)
            .with_visible_rows(rows)
            .set_selected(selected)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn cycle_branch_completion(&mut self, delta: isize) {
        let len = self.filtered_branches().len();
        if len == 0 {
            return;
        }

        let rows = self.branch_menu_rows();
        if SelectorController::new(&mut self.refs.branch_menu, len)
            .with_visible_rows(rows)
            .move_by(delta, SelectorMovement::Wrapping)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn ensure_branch_selection_visible(&mut self) {
        self.ensure_branch_selection_visible_for_rows(self.branch_menu_rows());
    }

    pub(crate) fn branch_menu_rows(&self) -> usize {
        branch_menu_list_visible_rows(self, self.viewport.terminal_area)
            .unwrap_or(MAX_BRANCH_MENU_ROWS)
            .max(1)
    }

    pub(crate) fn ensure_branch_selection_visible_for_rows(&mut self, visible_rows: usize) {
        let len = self.filtered_branches().len();
        self.refs
            .branch_menu
            .ensure_selected_visible(len, visible_rows);
    }

    pub(crate) fn max_branch_menu_selection(&self) -> usize {
        self.filtered_branches().len().saturating_sub(1)
    }

    pub(crate) fn max_branch_menu_scroll(&self) -> usize {
        self.max_branch_menu_scroll_for_rows(self.branch_menu_rows())
    }

    pub(crate) fn max_branch_menu_scroll_for_rows(&self, visible_rows: usize) -> usize {
        self.filtered_branches()
            .len()
            .saturating_sub(visible_rows.max(1))
    }

    pub(crate) fn is_show_diff(&self) -> bool {
        matches!(&self.document.options.source, DiffSource::Show(_))
    }

    pub(crate) fn show_rev_menu_detail(&self) -> String {
        let rev = self
            .refs
            .show_rev
            .as_deref()
            .or(match &self.document.options.source {
                DiffSource::Show(rev) => Some(rev.as_str()),
                _ => None,
            });
        match rev {
            None | Some("HEAD") => self
                .refs
                .current_head
                .clone()
                .or_else(|| current_head_label(&self.document.changeset.repo))
                .unwrap_or_else(|| "HEAD".to_owned()),
            Some(symbolic) => rev_display_label(symbolic).to_owned(),
        }
    }

    pub(crate) fn commit_menu_width(&self) -> u16 {
        let commit_width = commit_menu_width(&self.refs.comparison_commits) as usize;
        let input_width = self.refs.commit_menu.input.width().saturating_add(4);
        commit_width.max(input_width).max(36).saturating_add(4) as u16
    }

    pub(crate) fn max_commit_menu_selection(&self) -> usize {
        self.filtered_commits().len().saturating_sub(1)
    }

    pub(crate) fn max_commit_menu_scroll_for_rows(&self, visible_rows: usize) -> usize {
        self.filtered_commits()
            .len()
            .saturating_sub(visible_rows.max(1))
    }

    pub(crate) fn ensure_commit_selection_visible(&mut self) {
        self.ensure_commit_selection_visible_for_rows(self.commit_menu_rows());
    }

    pub(crate) fn commit_menu_rows(&self) -> usize {
        commit_menu_list_visible_rows(self, self.viewport.terminal_area)
            .unwrap_or(MAX_BRANCH_MENU_ROWS)
            .max(1)
    }

    pub(crate) fn ensure_commit_selection_visible_for_rows(&mut self, visible_rows: usize) {
        let len = self.filtered_commits().len();
        self.refs
            .commit_menu
            .ensure_selected_visible(len, visible_rows);
    }

    pub(crate) fn move_commit_selection(&mut self, delta: isize) {
        let len = self.filtered_commits().len();
        let rows = self.commit_menu_rows();
        if SelectorController::new(&mut self.refs.commit_menu, len)
            .with_visible_rows(rows)
            .move_by(delta, SelectorMovement::Saturating)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_commit_selection(&mut self, selected: usize) {
        let len = self.filtered_commits().len();
        let rows = self.commit_menu_rows();
        if SelectorController::new(&mut self.refs.commit_menu, len)
            .with_visible_rows(rows)
            .set_selected(selected)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn cycle_commit_completion(&mut self, delta: isize) {
        let len = self.filtered_commits().len();
        if len == 0 {
            return;
        }

        let rows = self.commit_menu_rows();
        if SelectorController::new(&mut self.refs.commit_menu, len)
            .with_visible_rows(rows)
            .move_by(delta, SelectorMovement::Wrapping)
        {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn apply_commit_input_key(&mut self, key: KeyEvent) -> bool {
        let len = self.filtered_commits().len();
        let rows = self.commit_menu_rows();
        let outcome = SelectorController::new(&mut self.refs.commit_menu, len)
            .with_visible_rows(rows)
            .apply_input_key(key);
        if outcome.changed {
            self.runtime.dirty = true;
        }
        outcome.handled
    }

    pub(crate) fn selected_commit_menu_choice(&self) -> Option<&GitCommit> {
        let rev = self.refs.show_rev.as_deref()?;
        self.refs.comparison_commits.iter().find(|commit| {
            commit.sha == rev
                || commit.sha.starts_with(rev)
                || rev.starts_with(&commit.sha[..commit.sha.len().min(7)])
        })
    }

    pub(crate) fn selectable_commit_count(&self) -> usize {
        let selected = self.selected_commit_menu_choice();
        self.refs
            .comparison_commits
            .iter()
            .filter(|commit| selected != Some(commit))
            .count()
    }

    pub(crate) fn filtered_commits(&self) -> Vec<&GitCommit> {
        let query = self.refs.commit_menu.input.trim().to_ascii_lowercase();
        let selected = self.selected_commit_menu_choice();
        if query.is_empty() {
            return self
                .refs
                .comparison_commits
                .iter()
                .filter(|commit| selected != Some(commit))
                .collect();
        }

        let mut matches: Vec<_> = self
            .refs
            .comparison_commits
            .iter()
            .enumerate()
            .filter(|(_, commit)| selected != Some(commit))
            .filter_map(|(index, commit)| {
                commit_match_score(&query, commit).map(|score| (score, index, commit))
            })
            .collect();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.sha.cmp(&right.2.sha))
        });
        matches.into_iter().map(|(_, _, commit)| commit).collect()
    }

    pub(crate) fn filtered_commit(&self, row_index: usize) -> Option<&GitCommit> {
        self.filtered_commits()
            .get(self.refs.commit_menu.scroll.saturating_add(row_index))
            .copied()
    }

    pub(crate) fn select_highlighted_commit_match(&mut self) {
        let Some(commit) = self
            .filtered_commits()
            .get(self.refs.commit_menu.selected)
            .map(|commit| (*commit).clone())
        else {
            self.set_warning_notice("no matching commit");
            return;
        };
        self.close_commit_menu();
        self.select_show_commit(commit.sha);
    }

    pub(crate) fn select_show_commit(&mut self, rev: String) {
        let mut options = self.document.options.clone();
        options.source = DiffSource::Show(rev.clone());
        options.scope = DiffScope::All;

        if options == self.document.options {
            self.refs.show_rev = Some(rev);
            self.runtime.dirty = true;
            return;
        }

        self.refs.show_rev = Some(rev);
        self.start_diff_load(options, "show unavailable");
    }

    pub(crate) fn commit_selector_text(&self) -> Option<String> {
        let rev = self.refs.show_rev.as_deref()?;
        let label = self
            .refs
            .comparison_commits
            .iter()
            .find(|commit| commit.sha == rev || commit.sha.starts_with(rev))
            .map(|commit| {
                let short = commit_short_sha(commit);
                if commit.subject.is_empty() {
                    short.to_owned()
                } else {
                    format!("{short} · {}", commit.subject)
                }
            })
            .unwrap_or_else(|| rev.to_owned());
        Some(format!("{label} ▾"))
    }

    pub(crate) fn commit_selector_width(&self) -> Option<u16> {
        self.commit_selector_text().map(|text| text.width() as u16)
    }

    pub(crate) fn commit_selector_start(&self) -> Option<u16> {
        if !self.is_show_diff() {
            return None;
        }
        let selector_gap = STATUSLINE_SELECTOR_GAP.width() as u16;
        Some(diff_selector_width(&self.document.options).saturating_add(selector_gap))
    }

    pub(crate) fn commit_selector_at(&self, column: u16) -> bool {
        let Some(start) = self.commit_selector_start() else {
            return false;
        };
        let Some(width) = self.commit_selector_width() else {
            return false;
        };
        column >= start && column < start.saturating_add(width)
    }

    pub(crate) fn is_rendered_commit_menu_position(&self, column: u16, row: u16) -> bool {
        self.runtime
            .hit_map
            .commit_menu_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub(crate) fn is_rendered_review_input_position(&self, column: u16, row: u16) -> bool {
        self.runtime
            .hit_map
            .review_input_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub(crate) fn commit_choice_at(&self, column: u16, row: u16) -> Option<String> {
        if !self.refs.commit_menu_open {
            return None;
        }

        let menu_area = self.runtime.hit_map.commit_menu_area?;
        let inner = commit_menu_block(self.config.theme).inner(menu_area);
        if column < inner.x
            || column >= inner.x.saturating_add(inner.width)
            || row < inner.y.saturating_add(2)
            || row >= inner.y.saturating_add(inner.height)
        {
            return None;
        }

        let row_index = usize::from(row.saturating_sub(inner.y).saturating_sub(2));
        let pinned_rows = usize::from(self.selected_commit_menu_choice().is_some());
        if row_index < pinned_rows {
            return None;
        }

        let commit_index = row_index.saturating_sub(pinned_rows);
        let rendered_choices = inner.height.saturating_sub(2 + pinned_rows as u16) as usize;
        if commit_index >= rendered_choices {
            return None;
        }

        self.filtered_commit(commit_index)
            .map(|commit| commit.sha.clone())
    }

    pub(crate) fn filtered_branches(&self) -> Vec<&str> {
        let menu = self.refs.branch_menu_open.unwrap_or(BranchMenu::Base);
        let query = self.refs.branch_menu.input.trim().to_ascii_lowercase();
        let selected = self.selected_branch_menu_choice(menu);
        if query.is_empty() {
            let mut matches: Vec<_> = self
                .refs
                .comparison_branches
                .iter()
                .enumerate()
                .filter(|(_, branch)| selected != Some(branch.as_str()))
                .map(|(index, branch)| (self.branch_pin_rank(menu, branch), index, branch.as_str()))
                .collect();
            matches.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            return matches.into_iter().map(|(_, _, branch)| branch).collect();
        }

        let mut matches: Vec<_> = self
            .refs
            .comparison_branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| selected != Some(branch.as_str()))
            .filter_map(|(index, branch)| {
                branch_match_score(&query, branch).map(|score| {
                    (
                        self.branch_pin_rank(menu, branch),
                        score,
                        branch.len(),
                        index,
                        branch.as_str(),
                    )
                })
            })
            .collect();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.4.cmp(right.4))
        });
        matches
            .into_iter()
            .map(|(_, _, _, _, branch)| branch)
            .collect()
    }

    pub(crate) fn selected_branch_menu_choice(&self, menu: BranchMenu) -> Option<&str> {
        self.branch_ref(menu)
    }

    pub(crate) fn selectable_branch_count(&self, menu: BranchMenu) -> usize {
        let selected = self.selected_branch_menu_choice(menu);
        self.refs
            .comparison_branches
            .iter()
            .filter(|branch| selected != Some(branch.as_str()))
            .count()
    }

    pub(crate) fn branch_pin_rank(&self, menu: BranchMenu, branch: &str) -> usize {
        let current = self.refs.current_head.as_deref();
        let base = self.refs.branch_base.as_deref();
        match menu {
            BranchMenu::Head => {
                if current == Some(branch) {
                    0
                } else if base == Some(branch) {
                    1
                } else {
                    2
                }
            }
            BranchMenu::Base => {
                if base == Some(branch) {
                    0
                } else if current == Some(branch) {
                    1
                } else {
                    2
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn push_branch_input(&mut self, character: char) {
        self.refs.branch_menu.push_input(character);
        self.runtime.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn clear_branch_input(&mut self) {
        if self.refs.branch_menu.clear_input_and_selection() {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn apply_branch_input_key(&mut self, key: KeyEvent) -> bool {
        let len = self.filtered_branches().len();
        let rows = self.branch_menu_rows();
        let outcome = SelectorController::new(&mut self.refs.branch_menu, len)
            .with_visible_rows(rows)
            .apply_input_key(key);
        if outcome.changed {
            self.runtime.dirty = true;
        }
        outcome.handled
    }

    pub(crate) fn select_highlighted_branch_match(&mut self) {
        let Some(menu) = self.refs.branch_menu_open else {
            return;
        };
        let Some(branch) = self
            .filtered_branches()
            .get(self.refs.branch_menu.selected)
            .map(|branch| (*branch).to_owned())
        else {
            self.set_warning_notice("no matching branch");
            return;
        };
        self.close_branch_menu();
        self.select_branch(menu, branch);
    }

    pub(crate) fn is_branch_diff(&self) -> bool {
        matches!(
            &self.document.options.source,
            DiffSource::Base(_) | DiffSource::Branch { .. }
        )
    }

    pub(crate) fn branch_ref(&self, menu: BranchMenu) -> Option<&str> {
        match menu {
            BranchMenu::Head => self.refs.branch_head.as_deref(),
            BranchMenu::Base => self.refs.branch_base.as_deref(),
        }
    }

    pub(crate) fn branch_selector_text(&self, menu: BranchMenu) -> Option<String> {
        let branch = self.branch_ref(menu)?;
        let label = self.branch_label(menu, branch);
        Some(format!("{label} ▾"))
    }

    pub(crate) fn branch_label(&self, menu: BranchMenu, branch: &str) -> String {
        match self.branch_marker(menu, branch) {
            Some(marker) => format!("{marker} {branch}"),
            None => branch.to_owned(),
        }
    }

    pub(crate) fn branch_marker(&self, menu: BranchMenu, branch: &str) -> Option<&'static str> {
        let current = self.refs.current_head.as_deref();
        let base = self.refs.branch_base.as_deref();
        match menu {
            BranchMenu::Head => {
                if current == Some(branch) {
                    Some(CURRENT_BRANCH_MARKER)
                } else if base == Some(branch) {
                    Some(BASE_BRANCH_MARKER)
                } else {
                    None
                }
            }
            BranchMenu::Base => {
                if base == Some(branch) {
                    Some(BASE_BRANCH_MARKER)
                } else if current == Some(branch) {
                    Some(CURRENT_BRANCH_MARKER)
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn branch_selector_width(&self, menu: BranchMenu) -> Option<u16> {
        self.branch_selector_text(menu)
            .map(|text| text.width() as u16)
    }

    pub(crate) fn branch_menu_width(&self) -> u16 {
        let branch_width = branch_menu_width(&self.refs.comparison_branches) as usize;
        let input_width = self.refs.branch_menu.input.width().saturating_add(4);
        branch_width.max(input_width).max(36).saturating_add(4) as u16
    }

    pub(crate) fn branch_selector_start(&self, menu: BranchMenu) -> Option<u16> {
        if !self.is_branch_diff() {
            return None;
        }

        let head_width = self.branch_selector_width(BranchMenu::Head)?;
        let selector_gap = STATUSLINE_SELECTOR_GAP.width() as u16;
        let head_start = diff_selector_width(&self.document.options).saturating_add(selector_gap);
        match menu {
            BranchMenu::Head => Some(head_start),
            BranchMenu::Base => Some(
                head_start
                    .saturating_add(head_width)
                    .saturating_add(BRANCH_COMPARISON_SEPARATOR.width() as u16),
            ),
        }
    }
}
