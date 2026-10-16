use mark_diff::{Changeset, DiffLineKind, DiffStats};

use crate::model::FileIndex;

pub(crate) fn branch_match_score(query: &str, branch: &str) -> Option<(usize, usize)> {
    let branch_lower = branch.to_ascii_lowercase();
    if branch_lower == query {
        return Some((0, 0));
    }
    if branch_lower.starts_with(query) {
        return Some((1, branch.len().saturating_sub(query.len())));
    }
    if let Some(index) = branch_lower.find(query) {
        return Some((2, index));
    }
    fuzzy_subsequence_score(query, &branch_lower).map(|score| (3, score))
}

pub(crate) fn fuzzy_subsequence_score(query: &str, branch: &str) -> Option<usize> {
    let mut last_match: Option<usize> = None;
    let mut score = 0usize;
    let mut search_start = 0usize;

    for character in query.chars() {
        let remaining = branch.get(search_start..)?;
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
pub(crate) struct TextMatcher {
    pub(crate) query: String,
    pub(crate) lowercase_query: String,
    pub(crate) case_sensitive: bool,
}

impl TextMatcher {
    pub(crate) fn new(query: &str) -> Option<Self> {
        if query.is_empty() {
            return None;
        }

        Some(Self {
            query: query.to_owned(),
            lowercase_query: query.to_ascii_lowercase(),
            case_sensitive: query
                .as_bytes()
                .iter()
                .any(|byte| byte.is_ascii_uppercase()),
        })
    }

    pub(crate) fn match_ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        if self.case_sensitive {
            text_match_ranges(text, &self.query)
        } else {
            text_match_ranges_ascii_case_insensitive(text, &self.lowercase_query)
        }
    }
}

pub(crate) fn text_match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(offset) = text[start..].find(query) {
        let byte_start = start + offset;
        let byte_end = byte_start + query.len();
        ranges.push(byte_start..byte_end);
        start = byte_end;
    }
    ranges
}

pub(crate) fn text_match_ranges_ascii_case_insensitive(
    text: &str,
    lowercase_query: &str,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(range) = text_match_range_ascii_case_insensitive(text, lowercase_query, start) {
        start = range.end;
        ranges.push(range);
    }
    ranges
}

pub(crate) fn text_match_range_ascii_case_insensitive(
    text: &str,
    lowercase_query: &str,
    start: usize,
) -> Option<std::ops::Range<usize>> {
    if lowercase_query.is_empty() || lowercase_query.len() > text.len() {
        return None;
    }

    let bytes = text.as_bytes();
    let query = lowercase_query.as_bytes();
    let mut index = start.min(text.len());
    while index + query.len() <= text.len() {
        if text.is_char_boundary(index) {
            let end = index + query.len();
            if text.is_char_boundary(end)
                && ascii_case_insensitive_bytes_equal(&bytes[index..end], query)
            {
                return Some(index..end);
            }
        }
        index += 1;
    }
    None
}

pub(crate) fn ascii_case_insensitive_bytes_equal(candidate: &[u8], lowercase_query: &[u8]) -> bool {
    candidate.len() == lowercase_query.len()
        && candidate
            .iter()
            .zip(lowercase_query)
            .all(|(candidate, query)| candidate.to_ascii_lowercase() == *query)
}

pub(crate) fn diff_line_grep_prefix(kind: DiffLineKind) -> char {
    match kind {
        DiffLineKind::Context => ' ',
        DiffLineKind::Addition => '+',
        DiffLineKind::Deletion => '-',
        DiffLineKind::Meta => '\\',
    }
}

pub(crate) fn diff_stats_for_files(changeset: &Changeset, files: &[FileIndex]) -> DiffStats {
    let mut stats = DiffStats {
        files: files.len(),
        ..DiffStats::default()
    };
    for file in files
        .iter()
        .filter_map(|file| changeset.files.get(file.get()))
    {
        stats.additions += file.additions;
        stats.deletions += file.deletions;
        if file.is_binary() {
            stats.binary_files += 1;
        }
    }
    stats
}
