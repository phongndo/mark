use super::DiffApp;
use crate::syntax::{InlineHunkEmphasisCache, InlineHunkKey, InlineRanges};

impl DiffApp {
    pub(crate) fn inline_ranges(&mut self, file: usize, hunk: usize, line: usize) -> InlineRanges {
        let key = InlineHunkKey {
            generation: self.document.generation,
            file,
            hunk,
        };
        let Some(lines) = self
            .document
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| file_diff.hunks().get(hunk))
            .map(|hunk_diff| hunk_diff.lines.as_slice())
        else {
            return InlineRanges::default();
        };

        if let Some(hunk_emphasis) = self.document.inline_cache.get_mut(&key) {
            return hunk_emphasis.ranges_for_line(lines, line);
        }

        self.document
            .inline_cache
            .insert(key, InlineHunkEmphasisCache::new(lines));
        self.document
            .inline_cache
            .get_mut(&key)
            .map(|hunk_emphasis| hunk_emphasis.ranges_for_line(lines, line))
            .unwrap_or_default()
    }
}
