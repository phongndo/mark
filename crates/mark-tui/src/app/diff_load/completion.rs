use mark_diff::Changeset;

use super::super::{DiffApp, PendingDiffLoad, cacheable_diff_options};
use super::cache::diff_cache_entry_with_annotation_candidates;

impl DiffApp {
    pub(super) fn apply_pending_diff_load(
        &mut self,
        pending: &PendingDiffLoad,
        changeset: Changeset,
    ) -> Result<u64, String> {
        if let Some(paths) = pending.scoped_paths.as_deref() {
            if pending.options != self.document.options {
                let message = format!(
                    "{}: review source changed during scoped reload",
                    pending.error_prefix
                );
                self.set_error_log(&message);
                return Err(message);
            }
            if let Err(error) = self.replace_paths_changeset(paths, changeset) {
                let message = format!("{}: {error}", pending.error_prefix);
                self.set_error_log(&message);
                return Err(message);
            }
        } else if pending.completion.is_some() {
            let generation_before_reload = self.document.generation;
            self.replace_loaded_diff(pending.options.clone(), changeset);
            if self.document.options != pending.options {
                return Err("session reload could not apply the requested source".to_owned());
            }
            if self.document.generation == generation_before_reload {
                self.document.generation = self.document.generation.wrapping_add(1);
                if let Some(syntax) = self.config.syntax.as_mut() {
                    syntax.clear(self.document.generation);
                }
            }
        } else if cacheable_diff_options(&pending.options) {
            let cached = diff_cache_entry_with_annotation_candidates(
                pending.options.clone(),
                changeset,
                self.annotation_cursor_enabled(),
            );
            self.replace_cached_diff(pending.options.clone(), cached, pending.branch_metadata);
        } else {
            self.replace_loaded_diff(pending.options.clone(), changeset);
        }
        Ok(self.document.generation)
    }
}
