//! The file and grep filter that predates `SearchIndex`.
//!
//! The diff benchmark still times it as a baseline for the indexed search,
//! so it is kept unchanged here beside the benchmark. Nothing else uses it.

use mark_diff::{Changeset, DiffLine};

use crate::controls::{
    TextMatcher, branch_match_score, diff_line_grep_prefix, text_match_range_ascii_case_insensitive,
};

pub(crate) fn filtered_file_indices(
    base: &Changeset,
    file_filter: &str,
    grep_filter: &str,
) -> Vec<usize> {
    let grep_matcher = TextMatcher::new(grep_filter);
    base.files
        .iter()
        .enumerate()
        .filter(|file| file_matches_file_filter(file, file_filter))
        .filter(|file| file_matches_grep_filter(file, grep_matcher.as_ref()))
        .map(|(index, _)| index)
        .collect()
}

fn file_matches_file_filter((_, file): &(usize, &mark_diff::DiffFile), query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }

    file_filter_texts(file)
        .into_iter()
        .any(|text| branch_match_score(&query, &text).is_some())
}

fn file_filter_texts(file: &mark_diff::DiffFile) -> Vec<String> {
    let mut texts = Vec::with_capacity(4);
    texts.push(file.display_path().to_owned());
    if let Some(old_path) = file.old_path()
        && old_path != file.display_path()
    {
        texts.push(old_path.to_owned());
    }
    if let Some(new_path) = file.new_path()
        && new_path != file.display_path()
    {
        texts.push(new_path.to_owned());
    }
    texts.push(file.status().label().to_owned());
    texts
}

fn file_matches_grep_filter(
    (_, file): &(usize, &mark_diff::DiffFile),
    matcher: Option<&TextMatcher>,
) -> bool {
    let Some(matcher) = matcher else {
        return true;
    };

    file_grep_text_matches(file, matcher)
}

fn file_grep_text_matches(file: &mark_diff::DiffFile, matcher: &TextMatcher) -> bool {
    matches(matcher, file.display_path())
        || file.old_path().is_some_and(|path| matches(matcher, path))
        || file.new_path().is_some_and(|path| matches(matcher, path))
        || matches(matcher, file.status().label())
        || file
            .hunks()
            .iter()
            .any(|hunk| hunk_grep_text_matches(hunk, matcher))
        || (file.is_binary() && matches(matcher, "binary file"))
        || (file.has_no_textual_changes() && matches(matcher, "no textual changes"))
}

fn hunk_grep_text_matches(hunk: &mark_diff::DiffHunk, matcher: &TextMatcher) -> bool {
    matches(matcher, &hunk.header)
        || hunk
            .lines
            .iter()
            .any(|line| diff_line_grep_text_matches(line, matcher))
}

fn diff_line_grep_text_matches(line: &DiffLine, matcher: &TextMatcher) -> bool {
    matches_prefixed(
        matcher,
        diff_line_grep_prefix(line.kind()),
        &line.text_lossy(),
    )
}

fn matches(matcher: &TextMatcher, text: &str) -> bool {
    if matcher.case_sensitive {
        text.contains(&matcher.query)
    } else {
        text_match_range_ascii_case_insensitive(text, &matcher.lowercase_query, 0).is_some()
    }
}

fn matches_prefixed(matcher: &TextMatcher, prefix: char, text: &str) -> bool {
    if matches(matcher, text) {
        return true;
    }

    let query = if matcher.case_sensitive {
        &matcher.query
    } else {
        &matcher.lowercase_query
    };
    let mut prefix_buffer = [0; 4];
    let prefix = prefix.encode_utf8(&mut prefix_buffer);
    query.len() <= prefix.len() + text.len()
        && prefixed_text_bytes_equal(
            prefix.as_bytes(),
            text.as_bytes(),
            query.as_bytes(),
            !matcher.case_sensitive,
        )
}

fn prefixed_text_bytes_equal(
    prefix: &[u8],
    text: &[u8],
    query: &[u8],
    ignore_ascii_case: bool,
) -> bool {
    query.iter().enumerate().all(|(index, query_byte)| {
        let candidate = if index < prefix.len() {
            prefix[index]
        } else {
            text[index - prefix.len()]
        };
        if ignore_ascii_case {
            candidate.to_ascii_lowercase() == *query_byte
        } else {
            candidate == *query_byte
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_matches_diff_line_prefix_and_text() {
        let addition = DiffLine::addition(1, "changed".to_owned());

        for query in ["+changed", "+chan", "changed"] {
            let matcher = TextMatcher::new(query).expect("matcher should be created");
            assert!(diff_line_grep_text_matches(&addition, &matcher), "{query}");
        }
        for query in ["-changed", "+CHANGED", "+changed!"] {
            let matcher = TextMatcher::new(query).expect("matcher should be created");
            assert!(!diff_line_grep_text_matches(&addition, &matcher), "{query}");
        }
    }
}
