use super::super::{AsyncJob, ContextLoadWorker, DiffApp, HunkFocusScrollBehavior};
use crate::model::{ContextLines, ContextSourceEntry, ContextSourceKey, FileIndex};
use crate::render::text::display_width;
use crate::runtime;
use crate::syntax::{
    DiffSide, context_source_byte_limit, full_file_source, load_full_file_source_limited,
    load_full_file_source_limited_cancellable, split_context_source_lines,
};
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicBool},
};
use tokio::sync::oneshot;

impl DiffApp {
    fn clear_loading_context_entries(&mut self, keys: &[ContextSourceKey]) {
        for key in keys {
            if matches!(
                self.document.context_cache.get(key),
                Some(ContextSourceEntry::Loading)
            ) {
                self.document.context_cache.remove(key);
            }
        }
    }

    pub(super) fn cancel_context_load_worker(&mut self) -> bool {
        let Some(worker) = self.jobs.context_load_worker.take() else {
            return false;
        };
        self.clear_loading_context_entries(&worker.keys);
        true
    }

    pub(in crate::app) fn full_file_context_files_for_viewport(
        &self,
        visible_files: &[FileIndex],
        visible_rows: usize,
    ) -> Vec<FileIndex> {
        let visible_files = visible_files.iter().copied().collect::<HashSet<_>>();
        let mut files = Vec::new();
        for rendered in self.rendered_diff_rows_for_viewport(visible_rows.max(1)) {
            let Some(file) = self
                .document
                .model
                .file_at_row(rendered.model_row)
                .map(FileIndex::new)
            else {
                continue;
            };
            if visible_files.contains(&file) && !files.contains(&file) {
                files.push(file);
            }
        }

        if files.is_empty() {
            if visible_files.contains(&self.sidebar.selected_file) {
                files.push(self.sidebar.selected_file);
            } else if let Some(file) = visible_files.iter().copied().min() {
                files.push(file);
            }
        }
        files
    }

    pub(in crate::app) fn load_full_file_context_for_files_sync(
        &mut self,
        files: &[FileIndex],
    ) -> bool {
        self.cancel_context_load_worker();

        let expanded_files = self
            .document
            .context_expansions
            .keys()
            .map(|key| key.file)
            .collect::<HashSet<_>>();
        let requested_files = files
            .iter()
            .copied()
            .filter(|file| expanded_files.contains(file))
            .collect::<Vec<_>>();
        let mut changed = false;
        let mut unavailable_files = 0;
        for file in requested_files {
            let previous_entries = self.document.context_cache.len();
            let loaded = self.ensure_context_lines(file.get()).is_some();
            changed |= self.document.context_cache.len() != previous_entries;
            unavailable_files += usize::from(!loaded);
        }
        if unavailable_files > 0 {
            self.set_warning_notice(format!(
                "full-file context unavailable for {unavailable_files} file(s); the source may be missing or exceed configured full-file limits"
            ));
        }
        changed
    }

    pub(in crate::app) fn queue_full_file_context_for_files(
        &mut self,
        files: &[FileIndex],
    ) -> bool {
        if self.jobs.context_load_worker.is_some() {
            return false;
        }

        let expanded_files = self
            .document
            .context_expansions
            .keys()
            .map(|key| key.file)
            .collect::<HashSet<_>>();
        let mut request_groups = Vec::new();
        for file in files
            .iter()
            .copied()
            .filter(|file| expanded_files.contains(file))
        {
            let Some(file_diff) = self.document.changeset.files.get(file.get()) else {
                continue;
            };
            let mut requests = Vec::new();
            let mut has_lines = false;
            for side in self.context_side_order(file.get()) {
                let key = ContextSourceKey { file, side };
                match self.document.context_cache.get(&key) {
                    Some(ContextSourceEntry::Lines(_)) => {
                        has_lines = true;
                        break;
                    }
                    Some(ContextSourceEntry::Loading | ContextSourceEntry::Unavailable) => {
                        continue;
                    }
                    None => {}
                }
                if let Some(source) = full_file_source(
                    &self.document.changeset.repo,
                    &self.document.options,
                    file_diff,
                    side,
                ) {
                    requests.push((key, source));
                }
            }
            if !has_lines && !requests.is_empty() {
                request_groups.push(requests);
            }
        }
        if request_groups.is_empty() {
            return false;
        }

        let keys = request_groups
            .iter()
            .flatten()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in &keys {
            self.document
                .context_cache
                .insert(*key, ContextSourceEntry::Loading);
        }

        let generation = self.document.generation;
        let max_source_bytes = context_source_byte_limit();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (tx, rx) = oneshot::channel();
        drop(runtime::spawn_blocking(move || {
            let mut results = Vec::new();
            for requests in request_groups {
                if worker_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let mut loaded = false;
                for (key, source) in requests {
                    let lines = if loaded {
                        None
                    } else {
                        load_full_file_source_limited_cancellable(
                            &source,
                            max_source_bytes,
                            &worker_cancelled,
                        )
                        .ok()
                        .and_then(split_context_source_lines)
                        .map(Arc::new)
                    };
                    loaded |= lines.is_some();
                    results.push((key, lines));
                }
            }
            let _ = tx.send(results);
        }));
        self.jobs.context_load_worker = Some(ContextLoadWorker {
            generation,
            keys,
            cancelled,
            job: AsyncJob::new(rx),
        });
        true
    }

    pub(crate) fn drain_context_load_worker(&mut self) -> bool {
        let Some(outcome) = self
            .jobs
            .context_load_worker
            .as_mut()
            .and_then(|worker| match worker.job.try_recv() {
                Ok(results) => Some(Some(results)),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(None),
            })
        else {
            return false;
        };
        let Some(worker) = self.jobs.context_load_worker.take() else {
            return false;
        };

        if worker.generation != self.document.generation || !self.full_file_mode_active() {
            self.clear_loading_context_entries(&worker.keys);
            return false;
        }

        let anchor = self
            .viewport
            .line_wrapping
            .then(|| self.full_file_viewport_anchor())
            .flatten();
        let requested_files = worker
            .keys
            .iter()
            .map(|key| key.file)
            .collect::<HashSet<_>>();
        let mut loaded = false;
        match outcome {
            Some(results) => {
                for (key, lines) in results {
                    loaded |= lines.is_some();
                    self.document.context_cache.insert(
                        key,
                        lines.map_or(ContextSourceEntry::Unavailable, ContextSourceEntry::Lines),
                    );
                }
            }
            None => {
                for key in &worker.keys {
                    if matches!(
                        self.document.context_cache.get(key),
                        Some(ContextSourceEntry::Loading)
                    ) {
                        self.document
                            .context_cache
                            .insert(*key, ContextSourceEntry::Unavailable);
                    }
                }
            }
        }

        let unavailable_files = requested_files
            .into_iter()
            .filter(|file| {
                ![DiffSide::New, DiffSide::Old].into_iter().any(|side| {
                    matches!(
                        self.document
                            .context_cache
                            .get(&ContextSourceKey { file: *file, side }),
                        Some(ContextSourceEntry::Lines(_))
                    )
                })
            })
            .count();
        if unavailable_files > 0 {
            self.set_warning_notice(format!(
                "full-file context unavailable for {unavailable_files} file(s); the source may be missing or exceed configured full-file limits"
            ));
        }

        self.invalidate_wrapped_visual_layout();
        let visible_files = self.document.model.visible_files().to_vec();
        if loaded {
            self.update_max_line_width_from_cached_context(&visible_files);
        }
        if let Some((anchor, model_row)) = anchor.and_then(|anchor| {
            self.model_row_for_full_file_anchor(anchor)
                .map(|row| (anchor, row))
        }) {
            let scroll = self.scroll_for_model_row_offset_at_viewport_row(
                model_row,
                anchor.row_visual_offset,
                anchor.viewport_row,
            );
            self.set_scroll_with_grep_sync(scroll, false, HunkFocusScrollBehavior::Preserve);
            self.sync_grep_match_selection_to_scroll();
        }
        self.runtime.dirty = true;
        true
    }

    pub(in crate::app) fn prepare_full_file_context_layout(&mut self, visible_files: &[FileIndex]) {
        let context_needed_before_layout = self.viewport.line_wrapping
            || self.viewport.horizontal_scroll > self.max_horizontal_scroll();
        if context_needed_before_layout {
            let layout_files = self
                .full_file_context_files_for_viewport(visible_files, self.viewport.viewport_rows);
            self.load_full_file_context_for_files_sync(&layout_files);
        }
        self.update_max_line_width_from_cached_context(visible_files);
    }

    pub(crate) fn prepare_full_file_context_for_viewport(&mut self, visible_rows: usize) -> bool {
        if !self.full_file_mode_active() {
            return false;
        }

        let visible_files = self.document.model.visible_files().to_vec();
        let files = self.full_file_context_files_for_viewport(&visible_files, visible_rows);
        !files.is_empty() && self.queue_full_file_context_for_files(&files)
    }

    pub(in crate::app) fn update_max_line_width_from_cached_context(
        &mut self,
        visible_files: &[FileIndex],
    ) {
        let visible_file_set = visible_files.iter().copied().collect::<HashSet<_>>();
        let width = self
            .document
            .context_cache
            .iter()
            .filter(|(key, _)| visible_file_set.contains(&key.file))
            .filter_map(|(_, entry)| match entry {
                ContextSourceEntry::Lines(lines) => lines.iter().map(display_width).max(),
                ContextSourceEntry::Loading | ContextSourceEntry::Unavailable => None,
            })
            .max();
        if let Some(width) = width {
            self.document.max_line_width = self.document.max_line_width.max(width);
        }
    }

    pub(crate) fn ensure_context_lines(
        &mut self,
        file: usize,
    ) -> Option<(DiffSide, Arc<ContextLines>)> {
        for side in self.context_side_order(file) {
            let key = ContextSourceKey {
                file: FileIndex::new(file),
                side,
            };
            if matches!(
                self.document.context_cache.get(&key),
                Some(ContextSourceEntry::Loading)
            ) {
                return None;
            }
            if !self.has_context_source(file, side) {
                continue;
            }
            if let Some(lines) = self.context_lines(file, side) {
                return Some((side, lines));
            }
        }
        None
    }

    pub(crate) fn has_context_source(&self, file: usize, side: DiffSide) -> bool {
        self.document
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| {
                full_file_source(
                    &self.document.changeset.repo,
                    &self.document.options,
                    file_diff,
                    side,
                )
            })
            .is_some()
    }

    pub(crate) fn context_source_side(&self, file: usize) -> Option<DiffSide> {
        for side in self.context_side_order(file) {
            match self.document.context_cache.get(&ContextSourceKey {
                file: FileIndex::new(file),
                side,
            }) {
                Some(ContextSourceEntry::Lines(_)) => return Some(side),
                Some(ContextSourceEntry::Loading) => return None,
                Some(ContextSourceEntry::Unavailable) => continue,
                None if self.has_context_source(file, side) => return Some(side),
                None => {}
            }
        }
        None
    }

    pub(crate) fn context_lines(
        &mut self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<ContextLines>> {
        let key = ContextSourceKey {
            file: FileIndex::new(file),
            side,
        };
        if !self.document.context_cache.contains_key(&key) {
            let entry = self
                .load_context_lines(file, side)
                .map(ContextSourceEntry::Lines)
                .unwrap_or(ContextSourceEntry::Unavailable);
            if self.full_file_mode_active()
                && let ContextSourceEntry::Lines(lines) = &entry
                && let Some(width) = lines.iter().map(display_width).max()
            {
                self.document.max_line_width = self.document.max_line_width.max(width);
            }
            self.document.context_cache.insert(key, entry);
            self.invalidate_wrapped_visual_layout();
        }

        match self.document.context_cache.get(&key) {
            Some(ContextSourceEntry::Lines(lines)) => Some(Arc::clone(lines)),
            Some(ContextSourceEntry::Loading | ContextSourceEntry::Unavailable) | None => None,
        }
    }

    pub(crate) fn load_context_lines(
        &self,
        file: usize,
        side: DiffSide,
    ) -> Option<Arc<ContextLines>> {
        let file_diff = self.document.changeset.files.get(file)?;
        let source = full_file_source(
            &self.document.changeset.repo,
            &self.document.options,
            file_diff,
            side,
        )?;
        let text = load_full_file_source_limited(&source, context_source_byte_limit()).ok()?;
        split_context_source_lines(text).map(Arc::new)
    }

    pub(crate) fn context_line_text(
        &mut self,
        file: usize,
        old_line: usize,
        new_line: usize,
    ) -> String {
        let Some((side, source_lines)) = self.ensure_context_lines(file) else {
            let loading = self.context_side_order(file).into_iter().any(|side| {
                matches!(
                    self.document.context_cache.get(&ContextSourceKey {
                        file: FileIndex::new(file),
                        side,
                    }),
                    Some(ContextSourceEntry::Loading)
                )
            });
            return if loading {
                "loading context…".to_owned()
            } else {
                "context unavailable".to_owned()
            };
        };
        let line_number = match side {
            DiffSide::Old => old_line,
            DiffSide::New => new_line,
        };
        let Some(line_index) = line_number.checked_sub(1) else {
            return String::new();
        };
        source_lines.get(line_index).unwrap_or_default().to_owned()
    }

    pub(crate) fn update_max_line_width_for_expanded_context(
        &mut self,
        source_lines: &ContextLines,
        source_start: usize,
        offsets: std::ops::Range<usize>,
    ) {
        let Some(source_index_start) = source_start.checked_sub(1) else {
            return;
        };
        for offset in offsets {
            let Some(text) = source_lines.get(source_index_start + offset) else {
                continue;
            };
            self.document.max_line_width = self.document.max_line_width.max(display_width(text));
        }
    }
}
