mod cache;
mod completion;
mod prefetch;
mod review;
mod session_reload;

#[cfg(test)]
pub(crate) use cache::diff_cache_entry;

use super::{AsyncJob, BranchMetadataPolicy, DiffApp, DiffLoadCachePolicy, PendingDiffLoad};
use crate::runtime;
use mark_diff::DiffOptions;
use tokio::sync::oneshot;

impl DiffApp {
    pub(crate) fn start_diff_load(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
    ) {
        self.start_diff_load_inner(options, error_prefix, DiffLoadCachePolicy::Use);
    }

    pub(crate) fn start_uncached_diff_load(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
    ) {
        self.start_diff_load_inner(options, error_prefix, DiffLoadCachePolicy::Bypass);
    }

    pub(super) fn start_diff_load_inner(
        &mut self,
        options: DiffOptions,
        error_prefix: impl Into<String>,
        cache_policy: DiffLoadCachePolicy,
    ) {
        let error_prefix = error_prefix.into();
        self.jobs.pending_review_load = None;

        let use_cache = matches!(cache_policy, DiffLoadCachePolicy::Use)
            && self.diff_cache_invalidator_active();

        if use_cache {
            self.store_current_diff_cache();

            if let Some(cached) = self.take_cached_diff(&options) {
                self.jobs.pending_diff_load = None;
                self.replace_cached_diff(options, cached, BranchMetadataPolicy::Preserve);
                return;
            }

            if self
                .jobs
                .pending_diff_load
                .as_ref()
                .is_some_and(|pending| pending.options == options)
            {
                self.runtime.dirty = true;
                return;
            }

            self.jobs
                .diff_prefetch_queue
                .retain(|prefetch_options| prefetch_options != &options);
            if let Some(prefetch) = self.take_pending_diff_prefetch(&options) {
                self.jobs.pending_diff_load = Some(PendingDiffLoad {
                    options,
                    error_prefix,
                    branch_metadata: BranchMetadataPolicy::Preserve,
                    scoped_paths: None,
                    completion: None,
                    job: prefetch.job,
                });
                self.set_success_notice("reloading");
                self.runtime.dirty = true;
                return;
            }
        } else {
            self.clear_cached_diff_choices();
        }

        let (tx, rx) = oneshot::channel();
        let load_options = options.clone();
        drop(runtime::spawn_blocking(move || {
            let _ = tx.send(mark_diff::load_review_ref_with_raw_patch(&load_options));
        }));

        self.jobs.pending_diff_load = Some(PendingDiffLoad {
            options,
            error_prefix,
            branch_metadata: if use_cache {
                BranchMetadataPolicy::Preserve
            } else {
                BranchMetadataPolicy::Refresh
            },
            scoped_paths: None,
            completion: None,
            job: AsyncJob::new(rx),
        });
        self.set_success_notice("reloading");
        self.runtime.dirty = true;
    }

    pub(crate) fn drain_pending_diff_load(&mut self) {
        self.drain_pending_review_load();

        let Some(outcome) = self
            .jobs
            .pending_diff_load
            .as_mut()
            .and_then(|pending| match pending.job.try_recv() {
                Ok(result) => Some(Some(result)),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(None),
            })
        else {
            return;
        };
        let Some(pending) = self.jobs.pending_diff_load.take() else {
            return;
        };

        let completion_result = match outcome {
            Some(Ok(changeset)) => self.apply_pending_diff_load(&pending, changeset),
            Some(Err(error)) => {
                let message = format!("{}: {error}", pending.error_prefix);
                self.set_error_log(&message);
                Err(message)
            }
            None => {
                let message = format!("{}: worker stopped", pending.error_prefix);
                self.set_error_log(&message);
                Err(message)
            }
        };
        if let Some(completion) = pending.completion {
            let _ = completion.send(completion_result);
        }
    }
}
