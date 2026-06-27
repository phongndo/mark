use super::*;

impl DiffApp {
    pub(crate) fn start_diff_load(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
    ) {
        self.start_diff_load_inner(options, error_prefix, true);
    }

    pub(crate) fn start_uncached_diff_load(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
    ) {
        self.start_diff_load_inner(options, error_prefix, false);
    }

    pub(super) fn start_diff_load_inner(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
        use_cache: bool,
    ) {
        let error_prefix = error_prefix.into();
        self.pending_review_load = None;

        let use_cache = use_cache && self.diff_cache_invalidator_active();

        if use_cache {
            self.store_current_diff_cache();

            if let Some(cached) = self.take_cached_diff(&options) {
                self.pending_diff_load = None;
                self.replace_cached_diff(options, cached, false);
                return;
            }

            if self
                .pending_diff_load
                .as_ref()
                .is_some_and(|pending| pending.options == options)
            {
                self.dirty = true;
                return;
            }

            self.diff_prefetch_queue
                .retain(|prefetch_options| prefetch_options != &options);
            if let Some(prefetch) = self.take_pending_diff_prefetch(&options) {
                self.pending_diff_load = Some(PendingDiffLoad {
                    options,
                    error_prefix,
                    refresh_branch_metadata: false,
                    rx: prefetch.rx,
                });
                self.dirty = true;
                return;
            }
        } else {
            self.clear_cached_diff_choices();
        }

        let (tx, rx) = oneshot::channel();
        let load_options = options.clone();
        runtime::spawn_detached_blocking(move || {
            let _ = tx.send(mark_diff::load_review_ref(&load_options));
        });

        self.pending_diff_load = Some(PendingDiffLoad {
            options,
            error_prefix,
            refresh_branch_metadata: !use_cache,
            rx,
        });
        self.dirty = true;
    }

    pub(crate) fn drain_pending_diff_load(&mut self) {
        self.drain_pending_review_load();

        let Some(outcome) =
            self.pending_diff_load
                .as_mut()
                .and_then(|pending| match pending.rx.try_recv() {
                    Ok(result) => Some(Some(result)),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(None),
                })
        else {
            return;
        };
        let Some(pending) = self.pending_diff_load.take() else {
            return;
        };

        match outcome {
            Some(Ok(changeset)) => {
                if cacheable_diff_options(&pending.options) {
                    let cached = diff_cache_entry(pending.options.clone(), changeset);
                    self.replace_cached_diff(
                        pending.options,
                        cached,
                        pending.refresh_branch_metadata,
                    );
                } else {
                    self.replace_loaded_diff(pending.options, changeset);
                }
            }
            Some(Err(error)) => self.set_error_log(format!("{}: {error}", pending.error_prefix)),
            None => self.set_error_log(format!("{}: worker stopped", pending.error_prefix)),
        }
    }

    pub(crate) fn start_review_load(&mut self, target: String) {
        let (tx, rx) = oneshot::channel();
        let target = target.trim().to_owned();
        let repo = Self::review_load_repo_for_target(&self.changeset.repo, &target);
        let worker_target = target;
        runtime::spawn_detached_blocking(move || {
            let result = mark_command::review_diff_options(repo, &worker_target, false).and_then(
                |options| {
                    mark_diff::load_review_ref(&options).map(|changeset| (options, changeset))
                },
            );
            let _ = tx.send(result);
        });

        self.pending_review_load = Some(PendingReviewLoad {
            error_prefix: "review unavailable".to_owned(),
            rx,
        });
        self.pending_diff_load = None;
        self.dirty = true;
    }

    pub(crate) fn review_load_repo_for_target(repo: &Path, _target: &str) -> Option<PathBuf> {
        // Numeric review IDs are resolved against this repository, while URLs
        // carry their own pull request identity. In both cases, preserve the
        // active repository so the loaded patch can resolve follow-up local
        // actions relative to the same repo the TUI was reviewing.
        if repo.as_os_str().is_empty() {
            return None;
        }

        Some(repo.to_path_buf())
    }

    pub(crate) fn drain_pending_review_load(&mut self) {
        let Some(outcome) =
            self.pending_review_load
                .as_mut()
                .and_then(|pending| match pending.rx.try_recv() {
                    Ok(result) => Some(Some(result)),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(None),
                })
        else {
            return;
        };
        let Some(pending) = self.pending_review_load.take() else {
            return;
        };

        match outcome {
            Some(Ok((mut options, changeset))) => {
                options.include_untracked = self.options.include_untracked;
                self.replace_loaded_diff(options, changeset);
            }
            Some(Err(error)) => self.set_error_log(format!("{}: {error}", pending.error_prefix)),
            None => self.set_error_log(format!("{}: worker stopped", pending.error_prefix)),
        }
    }

    pub(crate) fn start_diff_prefetches(&mut self) {
        if !self.diff_cache_invalidator_active() {
            self.clear_cached_diff_choices();
            return;
        }

        if self.diff_prefetch_started {
            self.start_next_diff_prefetch();
            return;
        }

        self.diff_prefetch_started = true;
        self.queue_diff_prefetches();
        self.start_next_diff_prefetch();
    }

    pub(super) fn queue_diff_prefetches(&mut self) {
        for choice in self.diff_menu_choices() {
            let Some(options) = self.options_for_choice(choice) else {
                continue;
            };
            if options == self.options
                || !cacheable_diff_options(&options)
                || self.diff_cache_contains(&options)
                || self
                    .pending_diff_load
                    .as_ref()
                    .is_some_and(|pending| pending.options == options)
                || self
                    .pending_diff_prefetch
                    .as_ref()
                    .is_some_and(|pending| pending.options == options)
                || self
                    .diff_prefetch_queue
                    .iter()
                    .any(|queued| queued == &options)
            {
                continue;
            }

            self.diff_prefetch_queue.push_back(options);
        }
    }

    pub(super) fn start_next_diff_prefetch(&mut self) {
        if !self.diff_cache_invalidator_active() {
            self.clear_cached_diff_choices();
            return;
        }

        if self.pending_diff_prefetch.is_some() {
            return;
        }

        while let Some(options) = self.diff_prefetch_queue.pop_front() {
            if options == self.options
                || !cacheable_diff_options(&options)
                || self.diff_cache_contains(&options)
                || self
                    .pending_diff_load
                    .as_ref()
                    .is_some_and(|pending| pending.options == options)
            {
                continue;
            }

            let (tx, rx) = oneshot::channel();
            let load_options = options.clone();
            runtime::spawn_detached_blocking(move || {
                let _ = tx.send(mark_diff::load_review_ref(&load_options));
            });
            self.pending_diff_prefetch = Some(PendingDiffPrefetch { options, rx });
            return;
        }
    }

    pub(crate) fn drain_diff_prefetch(&mut self) {
        let Some(outcome) =
            self.pending_diff_prefetch
                .as_mut()
                .and_then(|pending| match pending.rx.try_recv() {
                    Ok(result) => Some(Some(result)),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(None),
                })
        else {
            return;
        };
        let Some(pending) = self.pending_diff_prefetch.take() else {
            return;
        };

        if let Some(Ok(changeset)) = outcome {
            self.cache_loaded_diff(pending.options, changeset);
        }
        self.start_next_diff_prefetch();
    }

    pub(super) fn take_pending_diff_prefetch(
        &mut self,
        options: &DiffOptions,
    ) -> Option<PendingDiffPrefetch> {
        if self
            .pending_diff_prefetch
            .as_ref()
            .is_some_and(|pending| pending.options == *options)
        {
            self.pending_diff_prefetch.take()
        } else {
            None
        }
    }

    pub(crate) fn invalidate_diff_cache(&mut self) {
        self.clear_cached_diff_choices();
    }

    pub(crate) fn clear_cached_diff_choices(&mut self) {
        self.diff_cache.clear();
        self.pending_diff_prefetch = None;
        self.diff_prefetch_queue.clear();
        self.diff_prefetch_started = false;
    }

    pub(super) fn diff_cache_invalidator_active(&self) -> bool {
        self.live_updates_allowed
            && self.live_updates_enabled
            && !self.live_reload_invalidated
            && !self.live_reload_pending
            && live_diff_supported(&self.options)
            && self.live_diff_failed_options.as_ref() != Some(&self.options)
    }

    pub(super) fn store_current_diff_cache(&mut self) {
        if !self.diff_cache_invalidator_active() || !cacheable_diff_options(&self.options) {
            return;
        }

        let options = self.options.clone();
        let changeset = self.base_changeset.clone();
        self.diff_cache.retain(|entry| entry.options != options);
        let search_index = Arc::clone(&self.search_index);
        let total_stats = self.total_stats.clone();
        let max_line_width = search_index.max_line_width();
        let can_reuse_current_model =
            !self.filters_active() && !self.filter_busy() && self.context_expansions.is_empty();
        let context_expansions = HashMap::new();
        let unified_model = if can_reuse_current_model && self.layout == DiffLayoutMode::Unified {
            self.model.clone()
        } else {
            UiModel::new(&changeset, DiffLayoutMode::Unified, &context_expansions)
        };
        let split_model = if can_reuse_current_model && self.layout == DiffLayoutMode::Split {
            self.model.clone()
        } else {
            UiModel::new(&changeset, DiffLayoutMode::Split, &context_expansions)
        };
        self.diff_cache.insert(
            0,
            DiffCacheEntry {
                options,
                changeset,
                search_index,
                total_stats,
                max_line_width,
                unified_model,
                split_model,
            },
        );
        self.diff_cache.truncate(MAX_DIFF_CACHE_ENTRIES);
    }

    pub(crate) fn cache_loaded_diff(&mut self, options: DiffOptions, changeset: Changeset) {
        if !self.diff_cache_invalidator_active() || !cacheable_diff_options(&options) {
            return;
        }

        self.diff_cache.retain(|entry| entry.options != options);
        self.diff_cache
            .insert(0, diff_cache_entry(options, changeset));
        self.diff_cache.truncate(MAX_DIFF_CACHE_ENTRIES);
    }

    pub(super) fn take_cached_diff(&mut self, options: &DiffOptions) -> Option<DiffCacheEntry> {
        let index = self
            .diff_cache
            .iter()
            .position(|entry| &entry.options == options)?;
        Some(self.diff_cache.remove(index))
    }

    pub(super) fn diff_cache_contains(&self, options: &DiffOptions) -> bool {
        self.diff_cache
            .iter()
            .any(|entry| &entry.options == options)
    }
}
