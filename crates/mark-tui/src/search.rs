use std::{collections::HashSet, io};

use fff_grep::{LineTerminator, Match, Matcher, NoError, Searcher, Sink, SinkMatch};
use mark_diff::{Changeset, DiffLine};
use memchr::{memchr, memchr2, memmem};

use crate::{
    controls::diff_line_grep_prefix,
    model::{DiffLineIndex, FileIndex, HunkIndex, ModelRow, UiModel, UiRow},
    render::{
        headers::normalized_hunk_header_text,
        text::{display_width, terminal_text},
    },
};

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
    searcher: Searcher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSearchIndex {
    filter_texts: Vec<String>,
    grep_text: Vec<u8>,
    grep_lines: Vec<SearchLineRef>,
    max_line_width: usize,
}

impl DiffSearchIndex {
    pub(crate) fn empty() -> Self {
        Self {
            files: Vec::new(),
            searcher: Searcher::new(),
        }
    }

    pub(crate) fn new(changeset: &Changeset) -> Self {
        let files = changeset
            .files
            .iter()
            .enumerate()
            .map(|(file_index, file)| {
                let file_index = FileIndex::new(file_index);
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

                let grep_line_count = file
                    .hunks()
                    .iter()
                    .map(|hunk| hunk.lines.len().saturating_add(1))
                    .sum::<usize>()
                    .saturating_add(usize::from(
                        file.is_binary() || file.has_no_textual_changes(),
                    ));
                let grep_text_bytes = file
                    .hunks()
                    .iter()
                    .map(|hunk| {
                        hunk.header.len().saturating_add(1).saturating_add(
                            hunk.lines
                                .iter()
                                .map(|line| line.text().len().saturating_add(2))
                                .sum::<usize>(),
                        )
                    })
                    .sum::<usize>();
                let mut grep_text = Vec::with_capacity(grep_text_bytes);
                let mut grep_lines = Vec::with_capacity(grep_line_count);
                let mut max_line_width = 0usize;

                for (hunk_index, hunk) in file.hunks().iter().enumerate() {
                    let hunk_index = HunkIndex::new(hunk_index);
                    push_search_line(
                        &mut grep_text,
                        &mut grep_lines,
                        SearchLineRef::hunk_header(file_index, hunk_index),
                        normalized_hunk_header_text(&hunk.header).as_bytes(),
                    );
                    for (line_index, line) in hunk.lines.iter().enumerate() {
                        let line_index = DiffLineIndex::new(line_index);
                        max_line_width = max_line_width.max(display_width(line.text()));
                        push_diff_line_search_line(
                            &mut grep_text,
                            &mut grep_lines,
                            SearchLineRef::diff_line(file_index, hunk_index, line_index),
                            line,
                        );
                    }
                }

                if file.is_binary() {
                    push_search_line(
                        &mut grep_text,
                        &mut grep_lines,
                        SearchLineRef::file_body(file_index),
                        b"binary file",
                    );
                } else if file.has_no_textual_changes() {
                    push_search_line(
                        &mut grep_text,
                        &mut grep_lines,
                        SearchLineRef::file_body(file_index),
                        b"no textual changes",
                    );
                }

                FileSearchIndex {
                    filter_texts,
                    grep_text,
                    grep_lines,
                    max_line_width,
                }
            })
            .collect();

        Self {
            files,
            searcher: Searcher::new(),
        }
    }

    pub(crate) fn search(&self, file_filter: &str, grep_filter: &str) -> DiffSearchResult {
        self.search_with_grep_match_limit(file_filter, grep_filter, usize::MAX)
    }

    pub(crate) fn search_with_grep_match_limit(
        &self,
        file_filter: &str,
        grep_filter: &str,
        max_grep_matches: usize,
    ) -> DiffSearchResult {
        let file_query = file_filter.trim().to_ascii_lowercase();
        let grep_matcher = GrepMatcher::new(grep_filter);
        let mut visible_files = Vec::new();
        let mut grep_matches = Vec::new();
        let mut grep_matches_truncated = false;

        for (file_index, file) in self.files.iter().enumerate() {
            let file_index = FileIndex::new(file_index);
            if !file.matches_file_filter(&file_query) {
                continue;
            }

            let Some(matcher) = grep_matcher.as_ref() else {
                visible_files.push(file_index);
                continue;
            };

            let remaining_matches = max_grep_matches.saturating_sub(grep_matches.len());
            if remaining_matches == 0 {
                let mut sink = FirstMatchSink { matched: false };
                self.searcher
                    .search_slice(matcher, &file.grep_text, &mut sink)
                    .expect("in-memory diff grep should not fail");
                if sink.matched {
                    visible_files.push(file_index);
                    grep_matches_truncated = true;
                }
                continue;
            }

            let mut sink = LineRefSink {
                line_refs: &file.grep_lines,
                matches: Vec::new(),
                match_limit: remaining_matches,
                truncated: false,
                last_match: None,
            };
            self.searcher
                .search_slice(matcher, &file.grep_text, &mut sink)
                .expect("in-memory diff grep should not fail");
            if !sink.matches.is_empty() {
                visible_files.push(file_index);
                grep_matches.extend(sink.matches);
                grep_matches_truncated |= sink.truncated;
            }
        }

        DiffSearchResult {
            visible_files,
            grep_matches,
            grep_matches_truncated,
        }
    }

    pub(crate) fn max_line_width(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.max_line_width)
            .max()
            .unwrap_or_default()
    }

    pub(crate) fn max_line_width_for_files(&self, files: &[FileIndex]) -> usize {
        files
            .iter()
            .filter_map(|file| self.files.get(file.get()))
            .map(|file| file.max_line_width)
            .max()
            .unwrap_or_default()
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
}

fn push_search_line(
    grep_text: &mut Vec<u8>,
    grep_lines: &mut Vec<SearchLineRef>,
    line_ref: SearchLineRef,
    text: &[u8],
) {
    grep_lines.push(line_ref);
    grep_text.extend_from_slice(text);
    grep_text.push(b'\n');
}

fn push_diff_line_search_line(
    grep_text: &mut Vec<u8>,
    grep_lines: &mut Vec<SearchLineRef>,
    line_ref: SearchLineRef,
    line: &DiffLine,
) {
    let text = terminal_text(line.text());
    grep_lines.push(line_ref);
    grep_text.push(diff_line_grep_prefix(line.kind()) as u8);
    grep_text.extend_from_slice(text.as_bytes());
    grep_text.push(b'\n');
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

#[derive(Debug)]
struct LineRefSink<'a> {
    line_refs: &'a [SearchLineRef],
    matches: Vec<SearchLineRef>,
    match_limit: usize,
    truncated: bool,
    last_match: Option<SearchLineRef>,
}

impl Sink for LineRefSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if let Some(line_number) = mat.line_number()
            && let Some(line_ref) = line_number
                .checked_sub(1)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| self.line_refs.get(index))
        {
            if self.last_match == Some(*line_ref) {
                return Ok(true);
            }
            self.last_match = Some(*line_ref);
            self.matches.push(*line_ref);
            if self.matches.len() >= self.match_limit {
                self.truncated = true;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Debug)]
struct FirstMatchSink {
    matched: bool,
}

impl Sink for FirstMatchSink {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, _mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.matched = true;
        Ok(false)
    }
}

pub(crate) fn grep_match_rows(model: &UiModel, grep_matches: &[SearchLineRef]) -> Vec<ModelRow> {
    if grep_matches.is_empty() {
        return Vec::new();
    }

    let matches = grep_matches.iter().copied().collect::<HashSet<_>>();
    model
        .rows
        .iter()
        .enumerate()
        .filter_map(|(row_index, row)| {
            row_matches_grep_refs(*row, &matches).then_some(ModelRow::new(row_index))
        })
        .collect()
}

fn row_matches_grep_refs(row: UiRow, matches: &HashSet<SearchLineRef>) -> bool {
    match row {
        UiRow::FileSeparator
        | UiRow::FileHeader(_)
        | UiRow::Collapsed { .. }
        | UiRow::ContextLine { .. }
        | UiRow::ContextHide { .. } => false,
        UiRow::FileBodyNotice(file) => matches.contains(&SearchLineRef::file_body(file)),
        UiRow::HunkHeader { file, hunk } => {
            matches.contains(&SearchLineRef::hunk_header(file, hunk))
        }
        UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
            matches.contains(&SearchLineRef::diff_line(file, hunk, line))
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => {
            left.is_some_and(|line| matches.contains(&SearchLineRef::diff_line(file, hunk, line)))
                || right.is_some_and(|line| {
                    matches.contains(&SearchLineRef::diff_line(file, hunk, line))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use mark_diff::{
        Changeset, DiffFile, DiffFileBody, DiffHunk, DiffLine, FileChange, HunkLineRanges, RepoRoot,
    };

    use super::{DiffSearchIndex, SearchLineRef, find_ascii_case_insensitive};
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
        let index =
            DiffSearchIndex::new(&changeset_with_hunk_header("@@ -1 +1 @@     def\tneedle"));
        let result = index.search("", "@@ -1 +1 @@ def    needle");

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
        let index = DiffSearchIndex::new(&changeset_with_diff_line("before\tmiddle\rneedle"));

        let raw_result = index.search("", "before\tmiddle\rneedle");
        assert!(raw_result.visible_files.is_empty());
        assert!(raw_result.grep_matches.is_empty());

        let visible_result = index.search("", "before    middle\\rneedle");
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
            raw_patch: Vec::new(),
        }
    }
}
