use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    process,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use mark_core::{MarkError, MarkResult};
use mark_diff::{Changeset, DiffOptions, DiffScope, DiffSource, DiffStats};
use mark_syntax::{
    ColorOverrides, DiffContextExpansion, HighlightedLine, LayoutSetting, NotificationMode,
    NotificationSettings, SyntaxLimits, SyntaxSettings, SyntaxThemeConfig, SyntaxThemeSource,
    ToastCorner,
};
use ratatui::layout::Rect;
use tempfile::TempDir;
use tokio::sync::{mpsc::Receiver, oneshot};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    annotation::{
        AnnotationDraft, AnnotationKey, AnnotationSide, AnnotationStore,
        paired_old_line_for_addition,
    },
    controls::{
        BranchMenu, CrosstermTerminal, DiffChoice, DiffFilterKind, DiffLayoutMode, GitCommit,
        branch_base_from_options, branch_head_from_options, branch_match_score, commit_match_score,
        commit_menu_width, commit_short_sha, comparison_branches, comparison_commits,
        current_head_label, default_branch_base, default_layout_for_width, diff_stats_for_files,
        is_review_options, rev_display_label,
    },
    editor::{EditorTarget, configured_editor, open_editor, open_text_in_editor, repo_file_path},
    event_reader::TerminalEventReader,
    keymap::{GlobalAction, KeyPress, Keymap, MenuAction},
    live_diff::{LiveDiff, LiveDiffReload, live_diff_supported},
    model::{ContextKey, ContextSourceEntry, ContextSourceKey, UiModel, UiRow, context_expands_up},
    render::{
        annotations::{
            annotation_close_hit_at_column, annotation_compose_block_height,
            annotation_edit_hit_at_column, annotation_hit_at_column, annotation_saved_block_height,
            annotation_submit_hit_at_column,
        },
        draw,
        menus::{
            branch_menu_block, branch_menu_list_visible_rows, branch_menu_width,
            color_scheme_picker_block, color_scheme_picker_list_visible_rows, commit_menu_block,
            commit_menu_list_visible_rows, diff_menu_block, diff_selector_width,
            help_menu_list_visible_rows,
        },
        sidebar::max_file_sidebar_width,
        snapshot::{HitMap, RenderPlan, RenderStatePlan},
        viewport_plan::{
            ViewportSlotKind, annotation_saved_key_at_bottom_border,
            annotation_saved_key_at_top_border, compose_block_bottom_viewport_row,
            compose_block_top_viewport_row, model_row_for_viewport_row,
            plan_diff_viewport_rows_at_scroll, visual_scroll_for_viewport_row,
        },
    },
    runtime,
    search::{DiffSearchIndex, DiffSearchResult, grep_match_rows},
    selector::{SelectorController, SelectorMovement, SelectorState},
    syntax::{
        DiffSide, InlineHunkEmphasisCache, InlineHunkKey, InlineRange, LruCache, SyntaxPosition,
        SyntaxPriority, SyntaxRuntime, available_context_lines, full_file_source,
        load_full_file_source, split_context_source_lines, unified_syntax_side,
    },
    text_input::{TextInputKeyResult, handle_text_input_key},
    theme::{
        BASE_BRANCH_MARKER, BRANCH_COMPARISON_SEPARATOR, CURRENT_BRANCH_MARKER, DiffTheme,
        EVENT_POLL, FILE_SIDEBAR_MIN_WIDTH, GUTTER_WIDTH, HELP_MENU_ROWS, HORIZONTAL_SCROLL_STEP,
        HelpMenuKey, HelpMenuRow, MAX_BRANCH_MENU_ROWS, MAX_INLINE_DIFF_CACHE_ENTRIES,
        MAX_READY_EVENTS_PER_FRAME, MAX_SYNTAX_RESULTS_PER_FRAME, MOUSE_SCROLL_ACCEL_A,
        MOUSE_SCROLL_ACCEL_TAU, MOUSE_SCROLL_HISTORY_SIZE, MOUSE_SCROLL_MAX_MULTIPLIER,
        MOUSE_SCROLL_MIN_TICK_INTERVAL, MOUSE_SCROLL_REFERENCE_INTERVAL_MS,
        MOUSE_SCROLL_STREAK_TIMEOUT, STATUSLINE_SELECTOR_GAP, SyntaxBenchmarkReport,
        UNIFIED_GUTTER_WIDTH, diff_theme_from_config,
    },
    toast::{ToastLevel, Toasts},
};

mod action;
mod annotation_editor;
mod annotations;
mod choices;
mod clipboard;
mod context;
mod core;
mod diff_files;
mod diff_load;
mod editor_reload;
mod effect;
mod error_log;
mod file_sidebar;
mod filters;
mod help;
mod init;
mod input;
mod marks;
mod menu_options;
mod menu_refs;
mod menus;
mod mouse;
mod navigation;
mod options;
mod runner;
mod state;
mod syntax;
mod viewport;

pub(crate) use action::AppAction;
pub(crate) use annotation_editor::{
    create_annotation_scratch_file, normalize_annotation_editor_contents,
};
#[cfg(test)]
pub(crate) use clipboard::osc52_clipboard_sequence;
pub(crate) use clipboard::{json_string, write_osc52_clipboard};
pub(crate) use core::*;
pub(crate) use diff_files::*;
#[cfg(test)]
pub(crate) use diff_load::diff_cache_entry;
#[cfg(test)]
pub(crate) use editor_reload::{FileFingerprint, file_changed_since};
pub(crate) use effect::{ActionOutcome, AppEffect};
#[cfg(test)]
pub(crate) use init::{
    layout_override_from_settings, syntax_runtime_for_diff, syntax_settings_for_diff,
};
pub(crate) use mouse::{MouseScroll, MouseScrollDirection};
pub(crate) use options::*;
#[cfg(test)]
pub(crate) use runner::{drain_live_reloads, handle_event};
pub(crate) use runner::{is_quit_key, run_loop, sync_live_diff};
pub(crate) use state::*;
pub(crate) use viewport::*;

impl DiffApp {
    pub(crate) fn event_poll(&self) -> Duration {
        let now = Instant::now();
        let mut poll = EVENT_POLL;
        if self.jobs.editor_reload.is_some() || self.jobs.pending_editor_reload.is_some() {
            poll = poll.min(EDITOR_RELOAD_POLL);
        }
        if self.jobs.filter_worker.is_some() {
            poll = poll.min(FILTER_WORKER_POLL);
        }
        if let Some(pending) = self.jobs.pending_filter_apply {
            poll = poll.min(pending.due_at.saturating_duration_since(now));
        }
        if self.jobs.pending_diff_prefetch.is_some() {
            poll = poll.min(DIFF_PREFETCH_POLL);
        }
        poll
    }

    pub(crate) fn ignore_post_editor_quit_key(&mut self, key: KeyEvent, now: Instant) -> bool {
        let Some(ignore_until) = self.jobs.post_editor_quit_key_ignore_until else {
            return false;
        };
        if now >= ignore_until {
            self.jobs.post_editor_quit_key_ignore_until = None;
            return false;
        }

        is_quit_key(key) || self.config.keymap.matches_single(GlobalAction::Quit, key)
    }

    pub(crate) fn set_terminal_area(&mut self, area: Rect) {
        if self.viewport.terminal_area != area {
            self.viewport.terminal_area = area;
            self.sync_help_menu_visible_rows();
        }
    }

    pub(crate) fn set_notice(&mut self, text: impl Into<String>) {
        if self.notifications.toasts.push(ToastLevel::Info, text) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_success_notice(&mut self, text: impl Into<String>) {
        if self.notifications.toasts.push(ToastLevel::Success, text) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_warning_notice(&mut self, text: impl Into<String>) {
        if self.notifications.toasts.push(ToastLevel::Warning, text) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_blocked_notice(&mut self, text: impl Into<String>) {
        if self.notifications.toasts.push(ToastLevel::Error, text) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn set_debug_notice(&mut self, text: impl Into<String>) {
        if self.notifications.toasts.push(ToastLevel::Debug, text) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn expire_toasts(&mut self, now: Instant) {
        if self.notifications.toasts.expire(now) {
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn debug_notifications_enabled(&self) -> bool {
        self.notifications.toasts.debug_enabled()
    }

    pub(crate) fn mark_live_reload_invalidated(&mut self) {
        self.invalidate_diff_cache();
        self.jobs.live_reload_invalidated = true;
    }

    pub(crate) fn mark_live_reload_pending(&mut self) {
        self.mark_live_reload_invalidated();
        self.jobs.live_reload_pending = true;
        self.runtime.dirty = true;
    }

    #[allow(dead_code)]
    pub(crate) fn set_rendered_diff_area(&mut self, area: Rect) {
        if self.viewport.rendered_diff_area != Some(area) {
            self.clear_diff_mouse_hover();
        }
        self.viewport.rendered_diff_area = Some(area);
        self.runtime.hit_map.diff_area = Some(area);
    }

    pub(crate) fn set_rendered_diff_menu_area(&mut self, area: Option<Rect>) {
        self.overlays.rendered_diff_menu_area = area.filter(|_| self.overlays.diff_menu_open);
        self.runtime.hit_map.diff_menu_area = self.overlays.rendered_diff_menu_area;
    }

    pub(crate) fn set_rendered_branch_menu_area(&mut self, area: Option<Rect>) {
        self.overlays.rendered_branch_menu_area =
            area.filter(|_| self.refs.branch_menu_open.is_some());
        self.runtime.hit_map.branch_menu_area = self.overlays.rendered_branch_menu_area;
    }

    pub(crate) fn set_rendered_commit_menu_area(&mut self, area: Option<Rect>) {
        self.overlays.rendered_commit_menu_area = area.filter(|_| self.refs.commit_menu_open);
        self.runtime.hit_map.commit_menu_area = self.overlays.rendered_commit_menu_area;
    }

    pub(crate) fn set_rendered_review_input_area(&mut self, area: Option<Rect>) {
        self.overlays.rendered_review_input_area = area.filter(|_| self.overlays.review_input_open);
        self.runtime.hit_map.review_input_area = self.overlays.rendered_review_input_area;
    }

    pub(crate) fn set_rendered_color_scheme_picker_area(&mut self, area: Option<Rect>) {
        self.overlays.rendered_color_scheme_picker_area =
            area.filter(|_| self.overlays.color_scheme_picker_open);
        self.runtime.hit_map.color_scheme_picker_area =
            self.overlays.rendered_color_scheme_picker_area;
    }

    pub(crate) fn apply_render_hit_map(&mut self, mut hit_map: HitMap) {
        hit_map.diff_menu_area = hit_map
            .diff_menu_area
            .filter(|_| self.overlays.diff_menu_open);
        hit_map.branch_menu_area = hit_map
            .branch_menu_area
            .filter(|_| self.refs.branch_menu_open.is_some());
        hit_map.commit_menu_area = hit_map
            .commit_menu_area
            .filter(|_| self.refs.commit_menu_open);
        hit_map.options_menu_area = hit_map
            .options_menu_area
            .filter(|_| self.overlays.options_menu_open);
        hit_map.review_input_area = hit_map
            .review_input_area
            .filter(|_| self.overlays.review_input_open);
        hit_map.color_scheme_picker_area = hit_map
            .color_scheme_picker_area
            .filter(|_| self.overlays.color_scheme_picker_open);
        hit_map.error_log_separator_row = hit_map
            .error_log_separator_row
            .filter(|_| self.notifications.error_log.is_some());

        if self.viewport.rendered_diff_area != hit_map.diff_area {
            self.clear_diff_mouse_hover();
        }
        self.viewport.rendered_diff_area = hit_map.diff_area;
        self.overlays.rendered_diff_menu_area = hit_map.diff_menu_area;
        self.overlays.rendered_branch_menu_area = hit_map.branch_menu_area;
        self.overlays.rendered_commit_menu_area = hit_map.commit_menu_area;
        self.overlays.rendered_review_input_area = hit_map.review_input_area;
        self.overlays.rendered_color_scheme_picker_area = hit_map.color_scheme_picker_area;
        self.notifications.rendered_error_log_separator_row = hit_map.error_log_separator_row;
        self.runtime.hit_map = hit_map;
    }

    pub(crate) fn apply_render_plan(&mut self, plan: RenderPlan) {
        self.apply_render_state_plan(plan.state);
        self.apply_render_hit_map(plan.hit_map);
    }

    fn apply_render_state_plan(&mut self, state: RenderStatePlan) {
        self.set_terminal_area(state.terminal_area);
        self.sidebar.file_sidebar_render_width = state.file_sidebar_render_width;
        self.set_viewport_rows(state.viewport_rows);
        self.set_viewport_width(state.viewport_width);

        if let Some(rows) = state.options_menu_visible_rows {
            self.ensure_options_menu_selection_visible(rows);
        }
        if let Some(rows) = state.branch_menu_visible_rows {
            self.ensure_branch_selection_visible_for_rows(rows);
        }
        if let Some(rows) = state.commit_menu_visible_rows {
            self.ensure_commit_selection_visible_for_rows(rows);
        }
        if let Some(rows) = state.help_menu_visible_rows
            && self.overlays.help_menu_open
        {
            self.overlays.help_menu_visible_rows = rows;
            self.clamp_help_menu_scroll(rows);
        }
    }

    pub(crate) fn viewport_focus_row(&self) -> usize {
        if self.viewport.line_wrapping {
            let row_count = self.wrapped_visual_row_count();
            let focus_scroll = self.viewport.scroll.saturating_add(viewport_focus_offset(
                self.viewport.scroll,
                row_count,
                self.viewport.viewport_rows,
            ));
            return self
                .model_row_at_scroll(focus_scroll)
                .map(|(row, _)| row)
                .unwrap_or_else(|| self.document.model.len().saturating_sub(1));
        }

        self.viewport
            .scroll
            .saturating_add(viewport_focus_offset(
                self.viewport.scroll,
                self.document.model.len(),
                self.viewport.viewport_rows,
            ))
            .min(self.document.model.len().saturating_sub(1))
    }

    pub(crate) fn set_viewport_rows(&mut self, rows: usize) {
        let rows = rows.max(1);
        let previous_rows = self.viewport.viewport_rows;
        if previous_rows == rows {
            return;
        }

        let centered_grep_match_row = self.selected_grep_match_row().filter(|row| {
            let previous_centered_scroll = row
                .saturating_sub(viewport_center_offset(previous_rows))
                .min(max_scroll_for_viewport(
                    self.document.model.len(),
                    previous_rows,
                ));
            self.viewport.scroll == previous_centered_scroll
        });

        self.viewport.viewport_rows = rows;
        if let Some(row) = centered_grep_match_row {
            self.set_scroll_centered_on(row);
        } else {
            self.set_scroll(self.viewport.scroll);
        }
        self.clamp_file_sidebar_scroll(self.visible_file_sidebar_rows());
        self.ensure_annotation_draft_visible();
    }

    pub(crate) fn set_viewport_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.viewport.viewport_width == width {
            return;
        }

        let wrapped_position = self
            .viewport
            .line_wrapping
            .then(|| self.model_row_at_scroll(self.viewport.scroll))
            .flatten();
        self.viewport.viewport_width = width;
        self.invalidate_wrapped_visual_layout();
        self.set_horizontal_scroll(self.viewport.horizontal_scroll);
        if let Some((row, row_offset)) = wrapped_position {
            let row_scroll = self.wrapped_visual_scroll_for_model_row(row);
            let row_height = self.wrapped_visual_height_for_model_row(row);
            self.set_scroll(
                row_scroll.saturating_add(row_offset.min(row_height.saturating_sub(1))),
            );
        } else {
            self.set_scroll(self.viewport.scroll);
        }
        self.ensure_annotation_draft_visible();
    }

    pub(crate) fn inline_ranges(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
    ) -> Vec<InlineRange> {
        let key = InlineHunkKey {
            generation: self.document.generation,
            file,
            hunk,
        };
        if !self.document.inline_cache.contains_key(&key) {
            let cache = self
                .document
                .changeset
                .files
                .get(file)
                .and_then(|file_diff| file_diff.hunks.get(hunk))
                .map(|hunk_diff| InlineHunkEmphasisCache::new(&hunk_diff.lines))
                .unwrap_or_else(|| InlineHunkEmphasisCache::new(&[]));
            self.document.inline_cache.insert(key, cache);
        }

        let Some(lines) = self
            .document
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| file_diff.hunks.get(hunk))
            .map(|hunk_diff| hunk_diff.lines.as_slice())
        else {
            return Vec::new();
        };

        self.document
            .inline_cache
            .get_mut(&key)
            .map(|hunk_emphasis| hunk_emphasis.ranges_for_line(lines, line))
            .unwrap_or_default()
    }

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
        let anchor = self.hunk_navigation_anchor_row();
        let next = if delta < 0 {
            self.document.model.previous_hunk_row(anchor)
        } else {
            self.document.model.next_hunk_row(anchor)
        };
        if let Some(row) = next {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn hunk_navigation_anchor_row(&self) -> usize {
        if let Some((file, hunk)) = self.focused_hunk_for_viewport(self.viewport.viewport_rows)
            && let Some(row) = self.document.model.hunk_start_row(file, hunk)
        {
            return row;
        }

        self.viewport_focus_row()
    }

    pub(crate) fn focus_hunk_row(&mut self, row: usize) {
        let target_hunk = self.document.model.row(row).and_then(|row| row.hunk_key());
        let previous_hunk = self.viewport.manual_hunk_focus;
        self.clear_manual_hunk_focus();

        let Some((file, hunk)) = target_hunk else {
            self.set_scroll_centered_on(row);
            return;
        };

        self.set_scroll_focused_on_hunk(file, hunk);

        if let Some(row) = self.document.model.hunk_start_row(file, hunk)
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

    pub(crate) fn toggle_layout(&mut self) {
        let layout = self.viewport.layout.toggled();
        self.set_manual_layout(layout);
    }

    pub(crate) fn set_manual_layout(&mut self, layout: DiffLayoutMode) {
        self.viewport.layout_override = Some(layout);
        self.set_layout(layout);
    }

    pub(crate) fn set_layout_setting(&mut self, setting: LayoutSetting) {
        match layout_override_from_setting(setting) {
            Some(layout) => self.set_manual_layout(layout),
            None => {
                self.viewport.layout_override = None;
                self.set_layout(default_layout_for_width(
                    self.viewport.viewport_width.min(u16::MAX as usize) as u16,
                ));
            }
        }
    }

    pub(crate) fn apply_responsive_layout(&mut self, width: u16) {
        let horizontal_scroll = self.viewport.horizontal_scroll;
        self.set_viewport_width(width as usize);
        let responsive_layout = default_layout_for_width(width);
        let layout = self.viewport.layout_override.unwrap_or(responsive_layout);
        self.set_layout(layout);
        self.set_horizontal_scroll(horizontal_scroll);
        self.runtime.dirty = true;
    }

    pub(crate) fn set_layout(&mut self, layout: DiffLayoutMode) {
        if self.viewport.layout == layout {
            return;
        }

        self.viewport.layout = layout;
        let search_result = self.document.search_index.search_with_grep_match_limit(
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_model(&search_result.visible_files, HunkFocusModelBehavior::Clear);
        self.filters.grep_matches =
            grep_match_rows(&self.document.model, &search_result.grep_matches);
        self.filters.grep_matches_truncated = search_result.grep_matches_truncated;
        self.filters.selected_grep_match = None;
        self.set_horizontal_scroll(self.viewport.horizontal_scroll);
        let scroll = self
            .document
            .model
            .file_start_row(self.sidebar.selected_file)
            .map(|row| self.scroll_for_model_row(row))
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.sync_grep_match_selection_to_scroll();
        self.ensure_annotation_draft_visible();
        self.runtime.dirty = true;
    }

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

    pub(crate) fn replace_path_changeset(&mut self, path: &Path, path_changeset: Changeset) {
        self.invalidate_diff_cache();
        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.sidebar.selected_file);

        splice_diff_files_for_path(
            &mut self.document.changeset.files,
            path,
            path_changeset.files.clone(),
        );
        splice_diff_files_for_path(
            &mut self.document.base_changeset.files,
            path,
            path_changeset.files,
        );
        self.document.total_stats = self.document.changeset.stats();
        self.document.context_expansions.clear();
        self.document.context_cache.clear();
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
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            false,
            HunkFocusModelBehavior::Clear,
        );
        self.store_current_diff_cache();
        self.runtime.dirty = true;
    }

    pub(crate) fn replace_cached_diff(
        &mut self,
        options: DiffOptions,
        cached: DiffCacheEntry,
        refresh_branch_metadata: bool,
    ) {
        let DiffCacheEntry {
            changeset,
            search_index,
            total_stats,
            max_line_width,
            unified_model,
            split_model,
            ..
        } = cached;
        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.sidebar.selected_file);

        let previous_branch_base = self.refs.branch_base.clone();
        let previous_branch_head = self.refs.branch_head.clone();
        let previous_repo = self.document.changeset.repo.clone();
        self.document.options = options;
        self.jobs.live_reload_invalidated = false;
        self.jobs.live_reload_pending = false;
        if !refresh_branch_metadata && previous_repo == changeset.repo {
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
                    .any(|candidate| candidate == &branch)
                {
                    self.refs.comparison_branches.push(branch);
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
        self.document.context_cache.clear();
        self.document.generation = self.document.generation.wrapping_add(1);
        self.document.inline_cache.clear();
        self.jobs.pending_filter_apply = None;
        self.jobs.filter_worker = None;
        self.jobs.filter_searching = false;
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.clear(self.document.generation);
        }

        if self.filters_active() {
            let search_result = self.document.search_index.search_with_grep_match_limit(
                &self.filters.file_filter,
                &self.filters.grep_filter,
                MAX_LIVE_GREP_MATCHES,
            );
            self.replace_visible_files(
                search_result,
                selected_path,
                relative_scroll,
                false,
                HunkFocusModelBehavior::Clear,
            );
        } else {
            self.document.stats = self.document.total_stats.clone();
            self.document.max_line_width = max_line_width;
            self.document.model = match self.viewport.layout {
                DiffLayoutMode::Split => split_model,
                DiffLayoutMode::Unified => unified_model,
            };
            self.invalidate_wrapped_visual_layout();
            self.reanchor_annotation_draft();
            self.viewport.manual_hunk_focus = None;
            self.sidebar.selected_file = selected_path
                .and_then(|path| {
                    self.document
                        .changeset
                        .files
                        .iter()
                        .position(|file| file.display_path() == path)
                })
                .unwrap_or(0);
            self.filters.grep_matches.clear();
            self.filters.grep_matches_truncated = false;
            self.filters.selected_grep_match = None;

            let scroll = self
                .document
                .model
                .file_start_row(self.sidebar.selected_file)
                .map(|start| {
                    self.scroll_for_model_row(start)
                        .saturating_add(relative_scroll)
                })
                .unwrap_or_default();
            self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::ClearOnScroll);
            self.set_horizontal_scroll(self.viewport.horizontal_scroll);
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            self.ensure_annotation_draft_visible();
            self.runtime.dirty = true;
        }
    }

    pub(crate) fn replace_loaded_diff(&mut self, options: DiffOptions, changeset: Changeset) {
        let options_changed = self.document.options != options;
        if !options_changed && self.document.base_changeset == changeset {
            if self.jobs.live_reload_invalidated || self.jobs.live_reload_pending {
                self.jobs.live_reload_invalidated = false;
                self.jobs.live_reload_pending = false;
            }
            self.runtime.dirty = true;
            return;
        }

        let selected_path = self
            .document
            .changeset
            .files
            .get(self.sidebar.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.sidebar.selected_file);

        let previous_branch_base = self.refs.branch_base.clone();
        let previous_branch_head = self.refs.branch_head.clone();
        self.document.options = options;
        self.jobs.live_reload_invalidated = false;
        self.jobs.live_reload_pending = false;
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
        self.document.context_cache.clear();
        self.document.generation = self.document.generation.wrapping_add(1);
        self.document.inline_cache.clear();
        self.jobs.pending_filter_apply = None;
        self.jobs.filter_worker = None;
        self.jobs.filter_searching = false;
        if let Some(syntax) = self.config.syntax.as_mut() {
            syntax.clear(self.document.generation);
        }
        let search_result = self.document.search_index.search_with_grep_match_limit(
            &self.filters.file_filter,
            &self.filters.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            false,
            HunkFocusModelBehavior::Clear,
        );
        self.runtime.dirty = true;
    }
}
