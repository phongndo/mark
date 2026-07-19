use std::{collections::HashSet, ops::Range};

use fff_grep::{LineTerminator, Match, Matcher, NoError};
use mark_diff::Changeset;
use memchr::{memchr, memchr2, memmem};
use rayon::prelude::*;

use crate::{
    controls::diff_line_grep_prefix,
    model::{DiffLineIndex, FileIndex, HunkIndex, ModelRow, UiModel},
    render::{
        headers::normalized_hunk_header_text,
        text::{display_width, terminal_text_cow},
    },
};

#[cfg(not(test))]
const MAX_EAGER_SEARCH_WIDTH_LINES: usize = 200_000;
#[cfg(test)]
const MAX_EAGER_SEARCH_WIDTH_LINES: usize = 16;
// A non-printable diff-line byte can expand to at most six terminal columns:
// tabs occupy four, and an ASCII control renders as a six-column `\u{xx}`
// escape. Multi-byte Unicode scalars have a lower columns-per-byte ratio.
const MAX_DISPLAY_COLUMNS_PER_LINE_BYTE: usize = 6;
#[cfg(not(test))]
const PARALLEL_SEARCH_MIN_LINES: usize = 200_000;
#[cfg(test)]
const PARALLEL_SEARCH_MIN_LINES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SearchLineRef {
    FileBody {
        file: FileIndex,
    },
    HunkHeader {
        file: FileIndex,
        hunk: HunkIndex,
    },
    DiffLine {
        file: FileIndex,
        hunk: HunkIndex,
        line: DiffLineIndex,
    },
}

impl SearchLineRef {
    fn file_body(file: FileIndex) -> Self {
        Self::FileBody { file }
    }

    fn hunk_header(file: FileIndex, hunk: HunkIndex) -> Self {
        Self::HunkHeader { file, hunk }
    }

    fn diff_line(file: FileIndex, hunk: HunkIndex, line: DiffLineIndex) -> Self {
        Self::DiffLine { file, hunk, line }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffSearchResult {
    pub(crate) visible_files: Vec<FileIndex>,
    pub(crate) grep_matches: Vec<SearchLineRef>,
    pub(crate) grep_matches_truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SearchMatchIndex(usize);

impl SearchMatchIndex {
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DiffSearchIndex {
    files: Vec<FileSearchIndex>,
    diff_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSearchIndex {
    filter_texts: Vec<String>,
    max_line_width: usize,
}

impl DiffSearchIndex {
    pub(crate) fn empty() -> Self {
        Self {
            files: Vec::new(),
            diff_lines: 0,
        }
    }

    pub(crate) fn new(changeset: &Changeset) -> Self {
        let diff_lines = changeset
            .files
            .iter()
            .flat_map(|file| file.hunks())
            .map(|hunk| hunk.lines.len())
            .sum::<usize>();
        let compute_widths = diff_lines <= MAX_EAGER_SEARCH_WIDTH_LINES;
        let files = changeset
            .files
            .iter()
            .map(|file| {
                let mut filter_texts = Vec::with_capacity(4);
                filter_texts.push(file.display_path().to_ascii_lowercase());
                if let Some(old_path) = file.old_path()
                    && old_path != file.display_path()
                {
                    filter_texts.push(old_path.to_ascii_lowercase());
                }
                if let Some(new_path) = file.new_path()
                    && new_path != file.display_path()
                {
                    filter_texts.push(new_path.to_ascii_lowercase());
                }
                filter_texts.push(file.status().label().to_ascii_lowercase());

                let mut max_line_width = 0usize;
                for hunk in file.hunks() {
                    for line in &hunk.lines {
                        if compute_widths {
                            max_line_width = max_line_width.max(display_width(&line.text_lossy()));
                        } else {
                            max_line_width = max_line_width
                                .max(unindexed_line_width_bound(line.text_bytes().len()));
                        }
                    }
                }

                FileSearchIndex {
                    filter_texts,
                    max_line_width,
                }
            })
            .collect();

        Self { files, diff_lines }
    }

    pub(crate) fn search(
        &self,
        changeset: &Changeset,
        file_filter: &str,
        grep_filter: &str,
    ) -> DiffSearchResult {
        self.search_with_grep_match_limit(changeset, file_filter, grep_filter, usize::MAX)
    }

    pub(crate) fn search_with_grep_match_limit(
        &self,
        changeset: &Changeset,
        file_filter: &str,
        grep_filter: &str,
        max_grep_matches: usize,
    ) -> DiffSearchResult {
        self.search_with_grep_match_limit_cancelable(
            changeset,
            file_filter,
            grep_filter,
            max_grep_matches,
            || false,
        )
    }

    pub(crate) fn search_with_grep_match_limit_cancelable(
        &self,
        changeset: &Changeset,
        file_filter: &str,
        grep_filter: &str,
        max_grep_matches: usize,
        cancelled: impl Fn() -> bool + Sync,
    ) -> DiffSearchResult {
        let file_query = file_filter.trim().to_ascii_lowercase();
        let grep_matcher = GrepMatcher::new(grep_filter);
        let Some(grep_matcher) = grep_matcher.as_ref() else {
            return self.search_range(
                changeset,
                &file_query,
                None,
                max_grep_matches,
                0..self.files.len(),
                &cancelled,
            );
        };

        if self.diff_lines < PARALLEL_SEARCH_MIN_LINES || self.files.len() < 2 {
            return self.search_range(
                changeset,
                &file_query,
                Some(grep_matcher),
                max_grep_matches,
                0..self.files.len(),
                &cancelled,
            );
        }

        let pool = mark_runtime::cpu_pool();
        let section_count = pool.current_num_threads().min(self.files.len());
        if section_count <= 1 {
            return self.search_range(
                changeset,
                &file_query,
                Some(grep_matcher),
                max_grep_matches,
                0..self.files.len(),
                &cancelled,
            );
        }

        let section_len = self.files.len().div_ceil(section_count);
        let sections = (0..self.files.len())
            .step_by(section_len)
            .map(|start| start..(start + section_len).min(self.files.len()))
            .collect::<Vec<_>>();
        let results = pool.install(|| {
            sections
                .par_iter()
                .map(|range| {
                    self.search_range(
                        changeset,
                        &file_query,
                        Some(grep_matcher),
                        max_grep_matches,
                        range.clone(),
                        &cancelled,
                    )
                })
                .collect::<Vec<_>>()
        });

        merge_search_sections(results, max_grep_matches)
    }

    fn search_range(
        &self,
        changeset: &Changeset,
        file_query: &str,
        grep_matcher: Option<&GrepMatcher>,
        max_grep_matches: usize,
        range: Range<usize>,
        cancelled: &(impl Fn() -> bool + Sync),
    ) -> DiffSearchResult {
        let mut visible_files = Vec::new();
        let mut grep_matches = Vec::new();
        let mut grep_matches_truncated = false;

        for file_index in range {
            if cancelled() {
                break;
            }
            let Some(file) = self.files.get(file_index) else {
                continue;
            };
            let file_index = FileIndex::new(file_index);
            if !file.matches_file_filter(file_query) {
                continue;
            }

            let Some(matcher) = grep_matcher.as_ref() else {
                visible_files.push(file_index);
                continue;
            };

            let remaining_matches = max_grep_matches.saturating_sub(grep_matches.len());
            let file_diff = changeset.files.get(file_index.get());
            let mut sink = FileSearchSink {
                matcher,
                matches: Vec::new(),
                match_limit: remaining_matches,
                truncated: remaining_matches == 0,
                matched_after_limit: false,
            };
            if let Some(file_diff) = file_diff {
                sink.search_file(file_index, file_diff);
            }
            if !sink.matches.is_empty() || sink.matched_after_limit {
                visible_files.push(file_index);
                grep_matches.extend(sink.matches);
                grep_matches_truncated |= sink.truncated || sink.matched_after_limit;
            }
        }

        DiffSearchResult {
            visible_files,
            grep_matches,
            grep_matches_truncated,
        }
    }

    pub(crate) fn max_line_width(&self) -> usize {
        self.max_line_width_from(self.files.iter())
    }

    pub(crate) fn max_line_width_for_files(&self, files: &[FileIndex]) -> usize {
        self.max_line_width_from(files.iter().filter_map(|file| self.files.get(file.get())))
    }

    fn max_line_width_from<'a>(
        &self,
        files: impl IntoIterator<Item = &'a FileSearchIndex>,
    ) -> usize {
        files
            .into_iter()
            .map(|file| file.max_line_width)
            .max()
            .unwrap_or_default()
    }

    pub(crate) fn estimated_memory_bytes(&self) -> usize {
        self.files
            .len()
            .saturating_mul(std::mem::size_of::<FileSearchIndex>())
            .saturating_add(
                self.files
                    .iter()
                    .map(FileSearchIndex::estimated_memory_bytes)
                    .sum::<usize>(),
            )
    }
}

fn unindexed_line_width_bound(text_bytes: usize) -> usize {
    // Do not inspect line payloads here: this fallback is specifically for
    // diffs too large for an eager width scan. Span/String lengths are O(1),
    // and six columns per byte conservatively covers tabs and escaped ASCII
    // controls as well as Unicode and lossy decoding.
    text_bytes.saturating_mul(MAX_DISPLAY_COLUMNS_PER_LINE_BYTE)
}

fn merge_search_sections(
    sections: Vec<DiffSearchResult>,
    max_grep_matches: usize,
) -> DiffSearchResult {
    let visible_capacity = sections
        .iter()
        .map(|section| section.visible_files.len())
        .sum();
    let match_capacity = sections
        .iter()
        .map(|section| section.grep_matches.len())
        .sum::<usize>()
        .min(max_grep_matches);
    let mut visible_files = Vec::with_capacity(visible_capacity);
    let mut grep_matches = Vec::with_capacity(match_capacity);
    let mut grep_matches_truncated = false;

    for section in sections {
        visible_files.extend(section.visible_files);
        let remaining = max_grep_matches.saturating_sub(grep_matches.len());
        grep_matches_truncated |=
            section.grep_matches_truncated || section.grep_matches.len() >= remaining;
        grep_matches.extend(section.grep_matches.into_iter().take(remaining));
    }

    DiffSearchResult {
        visible_files,
        grep_matches,
        grep_matches_truncated,
    }
}

impl FileSearchIndex {
    fn matches_file_filter(&self, query: &str) -> bool {
        query.is_empty()
            || self
                .filter_texts
                .iter()
                .any(|text| file_match_score(query, text).is_some())
    }

    fn estimated_memory_bytes(&self) -> usize {
        self.filter_texts
            .len()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.filter_texts.iter().map(String::len).sum::<usize>())
    }
}

fn file_match_score(query: &str, text: &str) -> Option<(usize, usize)> {
    if text == query {
        return Some((0, 0));
    }
    if text.starts_with(query) {
        return Some((1, text.len().saturating_sub(query.len())));
    }
    if let Some(index) = text.find(query) {
        return Some((2, index));
    }
    fuzzy_subsequence_score(query, text).map(|score| (3, score))
}

fn fuzzy_subsequence_score(query: &str, text: &str) -> Option<usize> {
    let mut last_match: Option<usize> = None;
    let mut score = 0usize;
    let mut search_start = 0usize;

    for character in query.chars() {
        let remaining = text.get(search_start..)?;
        let offset = remaining.find(character)?;
        let index = search_start + offset;
        if let Some(previous) = last_match {
            score = score.saturating_add(index.saturating_sub(previous + 1));
        } else {
            score = score.saturating_add(index);
        }
        last_match = Some(index);
        search_start = index + character.len_utf8();
    }

    Some(score)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepMatcher {
    query: Vec<u8>,
    lowercase_query: Vec<u8>,
    case_sensitive: bool,
}

impl GrepMatcher {
    fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }

        Some(Self {
            query: query.as_bytes().to_vec(),
            lowercase_query: query.to_ascii_lowercase().into_bytes(),
            case_sensitive: query
                .as_bytes()
                .iter()
                .any(|byte| byte.is_ascii_uppercase()),
        })
    }

    fn matches_prefixed(&self, prefix: u8, text: &[u8]) -> bool {
        self.matches_at_prefixed_start(prefix, text)
            || <&GrepMatcher as Matcher>::find_at(&self, text, 0)
                .ok()
                .flatten()
                .is_some()
    }

    fn matches_at_prefixed_start(&self, prefix: u8, text: &[u8]) -> bool {
        let query = if self.case_sensitive {
            self.query.as_slice()
        } else {
            self.lowercase_query.as_slice()
        };
        let Some((&first, rest)) = query.split_first() else {
            return false;
        };
        if !byte_equal_for_mode(prefix, first, self.case_sensitive) {
            return false;
        }
        rest.len() <= text.len()
            && bytes_equal_for_mode(&text[..rest.len()], rest, self.case_sensitive)
    }

    fn raw_prefilter_safe(&self) -> bool {
        self.query
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    }
}

impl Matcher for &GrepMatcher {
    type Error = NoError;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, Self::Error> {
        let start = at.min(haystack.len());
        if self.case_sensitive {
            return Ok(memmem::find(&haystack[start..], &self.query)
                .map(|offset| Match::new(start + offset, start + offset + self.query.len())));
        }

        Ok(
            find_ascii_case_insensitive(haystack, &self.lowercase_query, start)
                .map(|start| Match::new(start, start + self.lowercase_query.len())),
        )
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        (!self.query.contains(&b'\n')).then(|| LineTerminator::byte(b'\n'))
    }
}

fn find_ascii_case_insensitive(
    haystack: &[u8],
    lowercase_query: &[u8],
    start: usize,
) -> Option<usize> {
    if lowercase_query.is_empty() || lowercase_query.len() > haystack.len() {
        return None;
    }

    let first = *lowercase_query.first()?;
    let first_upper = first.to_ascii_uppercase();
    let search_end = haystack.len().saturating_sub(lowercase_query.len()) + 1;
    let mut search_start = start;
    while search_start + lowercase_query.len() <= haystack.len() {
        let offset = if first == first_upper {
            memchr(first, &haystack[search_start..search_end])?
        } else {
            memchr2(first, first_upper, &haystack[search_start..search_end])?
        };
        let candidate_start = search_start + offset;
        let candidate_end = candidate_start + lowercase_query.len();
        if ascii_case_insensitive_bytes_equal(
            &haystack[candidate_start..candidate_end],
            lowercase_query,
        ) {
            return Some(candidate_start);
        }
        search_start = candidate_start.saturating_add(1);
    }
    None
}

fn ascii_case_insensitive_bytes_equal(candidate: &[u8], lowercase_query: &[u8]) -> bool {
    candidate.len() == lowercase_query.len()
        && candidate
            .iter()
            .zip(lowercase_query)
            .all(|(candidate, query)| candidate.to_ascii_lowercase() == *query)
}

fn byte_equal_for_mode(candidate: u8, query: u8, case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate == query
    } else {
        candidate.to_ascii_lowercase() == query
    }
}

fn bytes_equal_for_mode(candidate: &[u8], query: &[u8], case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate == query
    } else {
        ascii_case_insensitive_bytes_equal(candidate, query)
    }
}

struct FileSearchSink<'a> {
    matcher: &'a GrepMatcher,
    matches: Vec<SearchLineRef>,
    match_limit: usize,
    truncated: bool,
    matched_after_limit: bool,
}

impl FileSearchSink<'_> {
    fn search_file(&mut self, file_index: FileIndex, file: &mark_diff::DiffFile) {
        for (hunk_index, hunk) in file.hunks().iter().enumerate() {
            let hunk_index = HunkIndex::new(hunk_index);
            let header = normalized_hunk_header_text(&hunk.header);
            self.search_line(
                SearchLineRef::hunk_header(file_index, hunk_index),
                header.as_bytes(),
            );
            if self.done_without_need_for_visibility() {
                return;
            }
            if !self.hunk_may_match_raw(&hunk.lines) {
                continue;
            }
            for (line_index, line) in hunk.lines.iter().enumerate() {
                let line_ref = SearchLineRef::diff_line(
                    file_index,
                    hunk_index,
                    DiffLineIndex::new(line_index),
                );
                self.search_prefixed_line(
                    line_ref,
                    diff_line_grep_prefix(line.kind()) as u8,
                    line.text_bytes(),
                );
                if self.done_without_need_for_visibility() {
                    return;
                }
            }
        }

        if file.is_binary() {
            self.search_line(SearchLineRef::file_body(file_index), b"binary file");
        } else if file.has_no_textual_changes() {
            self.search_line(SearchLineRef::file_body(file_index), b"no textual changes");
        }
    }

    fn search_line(&mut self, line_ref: SearchLineRef, text: &[u8]) {
        if self.matcher.find_at(text, 0).ok().flatten().is_none() {
            return;
        }
        if self.match_limit == 0 || self.matches.len() >= self.match_limit {
            self.truncated = true;
            self.matched_after_limit = true;
            return;
        }
        self.matches.push(line_ref);
        if self.matches.len() >= self.match_limit {
            self.truncated = true;
        }
    }

    fn search_prefixed_line(&mut self, line_ref: SearchLineRef, prefix: u8, text: &[u8]) {
        let matched = if needs_terminal_text_for_search(text) {
            self.matches_terminal_text(prefix, text)
        } else {
            self.matcher.matches_prefixed(prefix, text)
        };
        if !matched {
            return;
        }
        if self.match_limit == 0 || self.matches.len() >= self.match_limit {
            self.truncated = true;
            self.matched_after_limit = true;
            return;
        }
        self.matches.push(line_ref);
        if self.matches.len() >= self.match_limit {
            self.truncated = true;
        }
    }

    fn matches_terminal_text(&self, prefix: u8, text: &[u8]) -> bool {
        let text = String::from_utf8_lossy(text);
        let text = terminal_text_cow(&text);
        self.matcher.matches_prefixed(prefix, text.as_bytes())
    }

    fn hunk_may_match_raw(&self, lines: &[mark_diff::DiffLine]) -> bool {
        if !self.matcher.raw_prefilter_safe() {
            return true;
        }
        let Some((raw, range)) = hunk_raw_range(lines) else {
            return true;
        };
        self.matcher
            .find_at(&raw[range], 0)
            .ok()
            .flatten()
            .is_some()
    }

    fn done_without_need_for_visibility(&self) -> bool {
        self.matched_after_limit
    }
}

fn hunk_raw_range(
    lines: &[mark_diff::DiffLine],
) -> Option<(std::sync::Arc<[u8]>, std::ops::Range<usize>)> {
    let (raw, first) = lines.first()?.text_span_range()?;
    let (last_raw, last) = lines.last()?.text_span_range()?;
    if !std::sync::Arc::ptr_eq(&raw, &last_raw) || first.start > last.end {
        return None;
    }
    Some((raw, first.start..last.end))
}

fn needs_terminal_text_for_search(text: &[u8]) -> bool {
    text.iter()
        .any(|byte| matches!(*byte, b'\t' | b'\r' | 0x00..=0x08 | 0x0b..=0x1f | 0x7f))
}

pub(crate) fn grep_match_rows(model: &UiModel, grep_matches: &[SearchLineRef]) -> Vec<ModelRow> {
    if grep_matches.is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::with_capacity(grep_matches.len());
    let mut seen = HashSet::new();
    for line_ref in grep_matches.iter().copied() {
        let row = match line_ref {
            SearchLineRef::FileBody { file } => model.file_body_notice_row(file),
            SearchLineRef::HunkHeader { file, hunk } => model.hunk_header_row(file, hunk),
            SearchLineRef::DiffLine { file, hunk, line } => model.diff_line_row(file, hunk, line),
        };
        if let Some(row) = row
            && seen.insert(row)
        {
            rows.push(row);
        }
    }
    rows.sort_unstable();
    rows
}

#[cfg(test)]
mod tests {
    use mark_diff::{
        Changeset, DiffFile, DiffFileBody, DiffHunk, DiffLine, FileChange, HunkLineRanges, RepoRoot,
    };

    use super::{
        DiffSearchIndex, MAX_DISPLAY_COLUMNS_PER_LINE_BYTE, SearchLineRef,
        find_ascii_case_insensitive,
    };
    use crate::model::{DiffLineIndex, FileIndex, HunkIndex};

    #[test]
    fn case_insensitive_search_ignores_partial_tail_candidate() {
        assert_eq!(find_ascii_case_insensitive(b"abc s", b"search", 0), None);
        assert_eq!(
            find_ascii_case_insensitive(b"abc Search", b"search", 0),
            Some(4)
        );
    }

    #[test]
    fn grep_search_uses_normalized_hunk_header_text() {
        let changeset = changeset_with_hunk_header("@@ -1 +1 @@     def\tneedle");
        let index = DiffSearchIndex::new(&changeset);
        let result = index.search(&changeset, "", "@@ -1 +1 @@ def    needle");

        assert_eq!(result.visible_files, vec![FileIndex::new(0)]);
        assert_eq!(
            result.grep_matches,
            vec![SearchLineRef::hunk_header(
                FileIndex::new(0),
                HunkIndex::new(0)
            )]
        );
    }

    #[test]
    fn grep_search_uses_terminal_text_for_diff_lines() {
        let changeset = changeset_with_diff_line("before\tmiddle\rneedle");
        let index = DiffSearchIndex::new(&changeset);

        let raw_result = index.search(&changeset, "", "before\tmiddle\rneedle");
        assert!(raw_result.visible_files.is_empty());
        assert!(raw_result.grep_matches.is_empty());

        let visible_result = index.search(&changeset, "", "before    middle\\rneedle");
        assert_eq!(visible_result.visible_files, vec![FileIndex::new(0)]);
        assert_eq!(
            visible_result.grep_matches,
            vec![SearchLineRef::diff_line(
                FileIndex::new(0),
                HunkIndex::new(0),
                DiffLineIndex::new(0)
            )]
        );
    }

    #[test]
    fn parallel_grep_matches_serial_order_and_limit() {
        let mut changeset = changeset_with_hunk(
            "@@ -1,8 +1,8 @@",
            (0..8)
                .map(|line| DiffLine::context(line + 1, line + 1, "needle"))
                .collect(),
        );
        let template = changeset.files[0].clone();
        changeset.files = (0..4)
            .map(|file| {
                let mut diff = template.clone();
                diff.change = FileChange::modified(format!("src/{file}.rs"));
                diff
            })
            .collect();
        let index = DiffSearchIndex::new(&changeset);
        let matcher = super::GrepMatcher::new("needle").expect("query should create matcher");
        let serial = index.search_range(
            &changeset,
            "",
            Some(&matcher),
            5,
            0..changeset.files.len(),
            &|| false,
        );

        assert_eq!(
            index.search_with_grep_match_limit(&changeset, "", "needle", 5),
            serial
        );
    }

    #[test]
    fn grep_search_can_cancel_between_files() {
        let changeset = changeset_with_diff_line("needle");
        let index = DiffSearchIndex::new(&changeset);
        let result = index.search_with_grep_match_limit_cancelable(
            &changeset,
            "",
            "needle",
            usize::MAX,
            || true,
        );

        assert!(result.visible_files.is_empty());
        assert!(result.grep_matches.is_empty());
    }

    #[test]
    fn large_diff_bounds_width_by_the_widest_line_not_the_whole_patch() {
        let patch = format!(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,17 +1,17 @@\n{}",
            " short source line\n".repeat(17)
        );
        let patch: std::sync::Arc<[u8]> = patch.into_bytes().into();
        let patch_bytes = patch.len();
        let line_bytes = "short source line".len();
        let expected_bound = line_bytes * MAX_DISPLAY_COLUMNS_PER_LINE_BYTE;
        let changeset = Changeset {
            repo: RepoRoot::new("."),
            title: String::new(),
            files: mark_diff::parse_patch_bytes(patch),
            raw_patch: Changeset::empty_raw_patch(),
        };

        let index = DiffSearchIndex::new(&changeset);

        assert!(changeset.raw_patch.is_empty());
        assert!(patch_bytes > expected_bound);
        assert_eq!(index.files[0].max_line_width, expected_bound);
        assert_eq!(index.max_line_width(), expected_bound);
        assert_eq!(
            index.max_line_width_for_files(&[FileIndex::new(0)]),
            expected_bound
        );
    }

    #[test]
    fn large_diff_considers_lines_from_every_patch_backing() {
        let first_patch = format!(
            "diff --git a/first b/first\n--- a/first\n+++ b/first\n@@ -1,9 +1,9 @@\n{}",
            " short\n".repeat(9)
        );
        let first_patch: std::sync::Arc<[u8]> = first_patch.into_bytes().into();
        let first_patch_bytes = first_patch.len();
        let mut files = mark_diff::parse_patch_bytes(first_patch);

        let wide_line = "x".repeat(first_patch_bytes.saturating_mul(6).saturating_add(1));
        let second_patch = format!(
            "diff --git a/second b/second\n--- a/second\n+++ b/second\n@@ -1,8 +1,8 @@\n {}\n{}",
            wide_line,
            " short\n".repeat(7)
        );
        let second_patch: std::sync::Arc<[u8]> = second_patch.into_bytes().into();
        files.extend(mark_diff::parse_patch_bytes(second_patch));
        let changeset = Changeset {
            repo: RepoRoot::new("."),
            title: String::new(),
            files,
            raw_patch: Changeset::empty_raw_patch(),
        };

        let index = DiffSearchIndex::new(&changeset);
        let expected_bound = wide_line.len() * MAX_DISPLAY_COLUMNS_PER_LINE_BYTE;

        assert_eq!(index.max_line_width(), expected_bound);
        assert_eq!(
            index.max_line_width_for_files(&[FileIndex::new(1)]),
            expected_bound
        );
        assert!(index.max_line_width() > first_patch_bytes * 6);
    }

    fn changeset_with_hunk_header(header: &str) -> Changeset {
        changeset_with_hunk(header, vec![DiffLine::context(1, 1, "unchanged")])
    }

    fn changeset_with_diff_line(text: &str) -> Changeset {
        changeset_with_hunk("@@ -1 +1 @@", vec![DiffLine::context(1, 1, text)])
    }

    fn changeset_with_hunk(header: &str, lines: Vec<DiffLine>) -> Changeset {
        Changeset {
            repo: RepoRoot::new("."),
            title: String::new(),
            files: vec![DiffFile {
                change: FileChange::modified("src/lib.rs"),
                additions: 0,
                deletions: 0,
                body: DiffFileBody::Text {
                    hunks: vec![DiffHunk {
                        header: header.to_owned(),
                        ranges: HunkLineRanges::new(1, 1, 1, 1),
                        lines,
                    }],
                },
            }],
            raw_patch: mark_diff::Changeset::empty_raw_patch(),
        }
    }
}
