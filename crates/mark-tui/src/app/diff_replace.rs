use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use mark_core::{MarkError, MarkResult};
use mark_diff::{Changeset, DiffOptions};

use super::{
    BranchMetadataPolicy, DiffApp, DiffCacheEntry, HunkFocusModelBehavior, HunkFocusScrollBehavior,
    MAX_LIVE_GREP_MATCHES, PostFilterNavigation, diff_file_matches_path_scope,
    show_rev_from_options, splice_diff_files_for_paths,
};
use crate::{
    controls::{
        DiffLayoutMode, branch_base_from_options, branch_head_from_options, comparison_branches,
        comparison_commits, current_head_label, default_branch_base,
    },
    model::{ContextKey, FileIndex, HunkIndex, UiModel, UiModelBuildOptions},
    review::{ReviewTransition, reset_review},
    search::DiffSearchIndex,
    syntax::invalidate_range_operand_revision_cache,
};

impl DiffApp {
    pub(crate) fn reload(&mut self) -> MarkResult<()> {
        self.invalidate_diff_cache();
        self.start_uncached_diff_load(self.document.options.clone(), "reload failed");
        Ok(())
    }

    pub(crate) fn replace_changeset(&mut self, changeset: Changeset) {
        self.invalidate_diff_cache();
        self.cache_loaded_diff(self.document.options.clone(), changeset.clone());
        self.replace_loaded_diff(self.document.options.clone(), changeset);
    }

    pub(crate) fn replace_path_changeset(
        &mut self,
        path: &Path,
        path_changeset: Changeset,
    ) -> MarkResult<()> {
        self.replace_paths_changeset(&[path.to_path_buf()], path_changeset)
    }

    pub(crate) fn replace_paths_changeset(
        &mut self,
        paths: &[PathBuf],
        path_changeset: Changeset,
    ) -> MarkResult<()> {
        let raw_patch_is_shared = Arc::ptr_eq(
            &self.document.changeset.raw_patch,
            &self.document.base_changeset.raw_patch,
        );
        let raw_patch =
            splice_raw_patch_for_paths(&self.document.changeset, paths, &path_changeset)?;
        let base_raw_patch = if raw_patch_is_shared {
            Arc::clone(&raw_patch)
        } else {
            splice_raw_patch_for_paths(&self.document.base_changeset, paths, &path_changeset)?
        };
        let review_transition = ReviewTransition::capture(self);
        self.close_annotation_target_mode();
        self.invalidate_diff_cache();
        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file.get())
            .map(|file| file.display_path().to_owned());
        let relative_scroll =
            self.relative_scroll_from_file_start(self.sidebar.selected_file.get());

        splice_diff_files_for_paths(
            &mut self.document.changeset.files,
            paths,
            path_changeset.files.clone(),
        );
        self.document.changeset.raw_patch = raw_patch;
        splice_diff_files_for_paths(
            &mut self.document.base_changeset.files,
            paths,
            path_changeset.files,
        );
        self.document.base_changeset.raw_patch = base_raw_patch;
        self.document.total_stats = self.document.changeset.stats();
        self.document.context_expansions.clear();
        self.document.trailing_context_lines.clear();
        self.document.trailing_context_sides.clear();
        self.document.context_cache.clear();
        self.jobs.context_load_worker = None;
        self.jobs.trailing_context_worker = None;
        self.document.generation = self.document.generation.wrapping_add(1);
        self.document.inline_cache.clear();
        self.document.search_index = Arc::new(DiffSearchIndex::new(&self.document.changeset));
        self.jobs.pending_filter_apply = None;
        self.jobs.filter_worker = None;
        self.jobs.filter_searching = false;
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.clear(self.document.generation);
        }
        let search_result = self.document.search_index.search_with_grep_match_limit(
            &self.document.changeset,
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            PostFilterNavigation::Preserve,
            HunkFocusModelBehavior::Clear,
        );
        self.store_current_diff_cache();
        review_transition.apply(self);
        self.runtime.dirty = true;
        Ok(())
    }

    pub(crate) fn replace_cached_diff(
        &mut self,
        options: DiffOptions,
        cached: DiffCacheEntry,
        branch_metadata: BranchMetadataPolicy,
    ) {
        let options_changed = self.document.options != options;
        let review_transition = (!options_changed).then(|| ReviewTransition::capture(self));
        self.close_annotation_target_mode();
        if options_changed {
            reset_review(self);
        }
        let DiffCacheEntry {
            changeset,
            search_index,
            total_stats,
            max_line_width,
            trailing_context_lines,
            trailing_context_sides,
            unified_model,
            split_model,
            ..
        } = cached;
        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file.get())
            .map(|file| file.display_path().to_owned());
        let relative_scroll =
            self.relative_scroll_from_file_start(self.sidebar.selected_file.get());

        let previous_branch_base = self.refs.branch_base.clone();
        let previous_branch_head = self.refs.branch_head.clone();
        let previous_repo = self.document.changeset.repo.clone();
        self.document.options = options;
        self.jobs.live_updates.reset_reload();
        if branch_metadata == BranchMetadataPolicy::Preserve && previous_repo == changeset.repo {
            self.refs.branch_base =
                branch_base_from_options(&self.document.options).or(previous_branch_base);
            self.refs.branch_head =
                branch_head_from_options(&self.document.options, self.refs.current_head.as_deref())
                    .or(previous_branch_head)
                    .or_else(|| self.refs.current_head.clone());
            for branch in [
                self.refs.current_head.clone(),
                self.refs.branch_head.clone(),
                self.refs.branch_base.clone(),
            ]
            .into_iter()
            .flatten()
            {
                if !self
                    .refs
                    .comparison_branches
                    .iter()
                    .any(|candidate| candidate.as_str() == branch)
                {
                    self.refs.comparison_branches.push(branch.into());
                }
            }
        } else {
            self.refs.current_head = current_head_label(&changeset.repo);
            self.refs.branch_base = branch_base_from_options(&self.document.options)
                .or(previous_branch_base)
                .or_else(|| default_branch_base(&self.document.options, &changeset.repo));
            self.refs.branch_head =
                branch_head_from_options(&self.document.options, self.refs.current_head.as_deref())
                    .or(previous_branch_head)
                    .or_else(|| self.refs.current_head.clone());
            self.refs.comparison_branches = comparison_branches(
                &changeset.repo,
                &[
                    self.refs.current_head.as_deref(),
                    self.refs.branch_head.as_deref(),
                    self.refs.branch_base.as_deref(),
                ],
            );
        }
        self.refs.branch_menu.scroll = self
            .refs
            .branch_menu
            .scroll
            .min(self.max_branch_menu_scroll());
        self.refs.show_rev = show_rev_from_options(&self.document.options);
        self.refs.comparison_commits =
            comparison_commits(&self.document.changeset.repo, self.refs.show_rev.as_deref());
        self.refs.commit_menu.scroll = self
            .refs
            .commit_menu
            .scroll
            .min(self.max_commit_menu_scroll_for_rows(self.commit_menu_rows()));
        self.document.total_stats = total_stats;
        self.document.base_changeset = changeset.clone();
        self.document.changeset = changeset;
        self.document.search_index = search_index;
        self.document.context_expansions.clear();
        self.document.trailing_context_lines = trailing_context_lines;
        self.document.trailing_context_sides = trailing_context_sides;
        self.document.context_cache.clear();
        self.jobs.context_load_worker = None;
        self.jobs.trailing_context_worker = None;
        self.document.generation = self.document.generation.wrapping_add(1);
        self.jobs.source_changed = false;
        self.document.inline_cache.clear();
        self.jobs.pending_filter_apply = None;
        self.jobs.filter_worker = None;
        self.jobs.filter_searching = false;
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.clear(self.document.generation);
        }
        let full_file_mode = self.full_file_mode_active();
        if full_file_mode {
            self.retry_unresolved_trailing_context();
            self.sync_full_file_context_expansions();
        }

        if self.filters.active() {
            let search_result = self.document.search_index.search_with_grep_match_limit(
                &self.document.changeset,
                &self.filters.file_filter,
                &self.filters.grep_filter,
                MAX_LIVE_GREP_MATCHES,
            );
            self.replace_visible_files(
                search_result,
                selected_path,
                relative_scroll,
                PostFilterNavigation::Preserve,
                HunkFocusModelBehavior::Clear,
            );
        } else {
            self.document.stats = self.document.total_stats.clone();
            self.document.max_line_width = max_line_width;
            let build_annotation_candidates = self.annotation_cursor_enabled();
            self.document.model = if full_file_mode {
                UiModel::new_with_trailing_context_controls_and_annotation_candidates(
                    &self.document.changeset,
                    self.viewport.layout,
                    &self.document.context_expansions,
                    &self.document.trailing_context_lines,
                    UiModelBuildOptions::new(false, false, build_annotation_candidates),
                )
            } else {
                match self.viewport.layout {
                    DiffLayoutMode::Split => split_model,
                    DiffLayoutMode::Unified => unified_model,
                }
            };
            if full_file_mode {
                let visible_files = self.document.model.visible_files().to_vec();
                self.prepare_full_file_context_layout(&visible_files);
            }
            self.annotations_state.annotation_rows.borrow_mut().clear();
            *self.annotations_state.annotation_keys_by_row.borrow_mut() = None;
            self.invalidate_wrapped_visual_layout();
            self.reanchor_annotation_draft();
            self.viewport.manual_hunk_focus = None;
            self.rebuild_annotation_cursor();
            self.sidebar.selected_file = FileIndex::new(
                selected_path
                    .and_then(|path| {
                        self.document
                            .changeset
                            .files
                            .iter()
                            .position(|file| file.display_path() == path)
                    })
                    .unwrap_or(0),
            );
            self.filters.grep_matches.clear();
            self.filters.grep_matches_truncated = false;
            self.filters.selected_grep_match = None;

            let scroll = self
                .document
                .model
                .file_start_row(self.sidebar.selected_file.get())
                .map(|start| {
                    self.scroll_for_model_row(start)
                        .saturating_add(relative_scroll)
                })
                .unwrap_or_default();
            self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::ClearOnScroll);
            self.set_horizontal_scroll(self.viewport.horizontal_scroll);
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            self.ensure_annotation_draft_visible();
            self.sync_annotation_cursor_to_viewport();
            self.runtime.dirty = true;
        }
        if let Some(review_transition) = review_transition {
            review_transition.apply(self);
        }
    }

    pub(crate) fn replace_loaded_diff(&mut self, options: DiffOptions, changeset: Changeset) {
        // Range operands such as `:/message` can resolve to a different commit
        // after refs move even when their spelling is unchanged.
        invalidate_range_operand_revision_cache(&changeset.repo, &options);
        let options_changed = self.document.options != options;
        if !options_changed
            && self.document.base_changeset == changeset
            && !self.full_file_mode_active()
        {
            self.jobs.live_updates.reset_reload();
            self.jobs.source_changed = false;
            self.runtime.dirty = true;
            return;
        }
        let review_transition = (!options_changed).then(|| ReviewTransition::capture(self));
        self.close_annotation_target_mode();
        if options_changed {
            reset_review(self);
        }

        // Keep only the current full-file anchor hot across a reload, and
        // recompute its source line count below. DiffFile equality says nothing
        // about unchanged source lines after the final hunk.
        let refresh_trailing_context_file = (!options_changed
            && self.document.changeset.repo == changeset.repo
            && self.full_file_mode_active())
        .then(|| {
            let old_file_index = self.sidebar.selected_file.get();
            let old_file = self.document.changeset.files.get(old_file_index)?;
            let old_key = ContextKey {
                file: FileIndex::new(old_file_index),
                hunk: HunkIndex::new(old_file.hunks().len()),
            };
            if !self.document.trailing_context_lines.contains_key(&old_key)
                || !self.document.trailing_context_sides.contains_key(&old_key)
            {
                return None;
            }
            changeset
                .files
                .iter()
                .position(|new_file| new_file.display_path() == old_file.display_path())
        })
        .flatten();
        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file.get())
            .map(|file| file.display_path().to_owned());
        let relative_scroll =
            self.relative_scroll_from_file_start(self.sidebar.selected_file.get());

        let previous_branch_base = self.refs.branch_base.clone();
        let previous_branch_head = self.refs.branch_head.clone();
        self.document.options = options;
        self.jobs.live_updates.reset_reload();
        self.refs.current_head = current_head_label(&changeset.repo);
        self.refs.branch_base = branch_base_from_options(&self.document.options)
            .or(previous_branch_base)
            .or_else(|| default_branch_base(&self.document.options, &changeset.repo));
        self.refs.branch_head =
            branch_head_from_options(&self.document.options, self.refs.current_head.as_deref())
                .or(previous_branch_head)
                .or_else(|| self.refs.current_head.clone());
        self.refs.comparison_branches = comparison_branches(
            &changeset.repo,
            &[
                self.refs.current_head.as_deref(),
                self.refs.branch_head.as_deref(),
                self.refs.branch_base.as_deref(),
            ],
        );
        self.refs.branch_menu.scroll = self
            .refs
            .branch_menu
            .scroll
            .min(self.max_branch_menu_scroll());
        self.refs.show_rev = show_rev_from_options(&self.document.options);
        self.refs.comparison_commits =
            comparison_commits(&changeset.repo, self.refs.show_rev.as_deref());
        self.refs.commit_menu.scroll = self
            .refs
            .commit_menu
            .scroll
            .min(self.max_commit_menu_scroll_for_rows(self.commit_menu_rows()));
        self.document.total_stats = changeset.stats();
        self.document.base_changeset = changeset.clone();
        self.document.changeset = changeset;
        self.document.search_index = Arc::new(DiffSearchIndex::new(&self.document.changeset));
        self.document.context_expansions.clear();
        self.document.trailing_context_lines.clear();
        self.document.trailing_context_sides.clear();
        self.document.context_cache.clear();
        self.jobs.context_load_worker = None;
        self.jobs.trailing_context_worker = None;
        self.document.generation = self.document.generation.wrapping_add(1);
        self.jobs.source_changed = false;
        self.document.inline_cache.clear();
        self.jobs.pending_filter_apply = None;
        self.jobs.filter_worker = None;
        self.jobs.filter_searching = false;
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.clear(self.document.generation);
        }
        if self.full_file_mode_active()
            && let Some(file) = refresh_trailing_context_file
        {
            self.refresh_trailing_context_for_file(file);
        }
        let search_result = self.document.search_index.search_with_grep_match_limit(
            &self.document.changeset,
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            PostFilterNavigation::Preserve,
            HunkFocusModelBehavior::Clear,
        );
        if let Some(review_transition) = review_transition {
            review_transition.apply(self);
        }
        self.runtime.dirty = true;
    }
}

fn splice_raw_patch_for_paths(
    changeset: &Changeset,
    paths: &[PathBuf],
    replacement: &Changeset,
) -> MarkResult<Arc<[u8]>> {
    if changeset.raw_patch.is_empty() {
        return if changeset.files.is_empty() {
            Ok(Arc::clone(&replacement.raw_patch))
        } else {
            Ok(Changeset::empty_raw_patch())
        };
    }

    let segments = aligned_raw_file_segments(changeset)?;
    let replacement_segments = aligned_raw_file_segments(replacement)?;
    let mut raw_patch = Vec::with_capacity(
        changeset
            .raw_patch
            .len()
            .saturating_add(replacement.raw_patch.len()),
    );
    let mut inserted = false;
    for (file, segment) in changeset.files.iter().zip(segments) {
        if paths
            .iter()
            .any(|path| diff_file_matches_path_scope(file, path))
        {
            if !inserted {
                for replacement_segment in &replacement_segments {
                    raw_patch.extend_from_slice(replacement_segment);
                }
                inserted = true;
            }
        } else {
            raw_patch.extend_from_slice(segment);
        }
    }
    if !inserted {
        for replacement_segment in replacement_segments {
            raw_patch.extend_from_slice(replacement_segment);
        }
    }
    Ok(Arc::from(raw_patch.into_boxed_slice()))
}

fn aligned_raw_file_segments(changeset: &Changeset) -> MarkResult<Vec<&[u8]>> {
    let segments = raw_file_segments(&changeset.raw_patch);
    if segments.len() != changeset.files.len() {
        return Err(MarkError::Usage(
            "scoped reload patch did not align with its parsed files".to_owned(),
        ));
    }
    Ok(segments)
}

fn raw_file_segments(raw_patch: &[u8]) -> Vec<&[u8]> {
    const HEADER: &[u8] = b"diff --git ";
    let mut starts = raw_patch
        .windows(HEADER.len())
        .enumerate()
        .filter_map(|(index, window)| {
            (window == HEADER && (index == 0 || raw_patch[index - 1] == b'\n')).then_some(index)
        })
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Vec::new();
    }
    starts.push(raw_patch.len());
    starts
        .windows(2)
        .map(|range| &raw_patch[range[0]..range[1]])
        .collect()
}
