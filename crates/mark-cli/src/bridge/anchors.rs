//! Findings refer to the saved snapshot, rather than arbitrary worktree lines.
//! Validation leaves the supplied anchor intact, including complete ranges.

use std::{collections::BTreeSet, io};

use mark_diff::{DiffFile, DiffLine};
use mark_session::{RangeTarget, ReviewAnchor, ReviewAnchorScope, SourceRange};
use serde_json::{Map, Value};

pub(super) fn validate(patch: &str, value: &Value) -> io::Result<()> {
    fields(
        value,
        &["file", "scope", "hunk", "old_line", "new_line", "range"],
    )?;
    if let Some(range) = value.get("range") {
        let sides = fields(range, &["old", "new"])?;
        for side in sides.values() {
            fields(side, &["start", "end"])?;
        }
    }
    let anchor: ReviewAnchor = serde_json::from_value(value.clone())
        .map_err(|error| invalid("invalid_anchor", &error.to_string()))?;
    let targets = usize::from(anchor.scope.is_some())
        + usize::from(anchor.hunk.is_some())
        + usize::from(anchor.old_line.is_some())
        + usize::from(anchor.new_line.is_some())
        + usize::from(anchor.range.is_some());
    if targets > 1 {
        return Err(invalid(
            "invalid_anchor",
            "choose one file scope, hunk, old line, new line, or range target",
        ));
    }
    let files = mark_diff::parse_patch(patch);
    let mut matches = files.iter().filter(|file| {
        file.old_path() == Some(anchor.file.as_str())
            || file.new_path() == Some(anchor.file.as_str())
            || file.display_path() == anchor.file
    });
    let file = matches
        .next()
        .ok_or_else(|| invalid("anchor_not_found", "file is not in this review snapshot"))?;
    if matches.next().is_some() {
        return Err(invalid(
            "ambiguous_anchor",
            "path matches multiple diff files; use a unique old or new path",
        ));
    }
    if targets == 0 || anchor.scope == Some(ReviewAnchorScope::File) {
        return Ok(());
    }
    if let Some(hunk) = anchor.hunk {
        let index = hunk
            .checked_sub(1)
            .ok_or_else(|| invalid("invalid_anchor", "hunk numbers are one-based"))?;
        if file.hunks().get(index).is_none() {
            return Err(invalid(
                "anchor_not_found",
                "hunk is not in this review snapshot",
            ));
        }
    }
    if let Some(line) = anchor.old_line {
        validate_line(file, true, line)?;
    }
    if let Some(line) = anchor.new_line {
        validate_line(file, false, line)?;
    }
    if let Some(range) = anchor.range {
        validate_range(file, &range)?;
    }
    Ok(())
}

fn fields<'a>(value: &'a Value, allowed: &[&str]) -> io::Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("invalid_anchor", "anchor targets must be objects"))?;
    if object
        .iter()
        .any(|(key, value)| !allowed.contains(&key.as_str()) || value.is_null())
    {
        return Err(invalid("invalid_anchor", "unknown or null anchor field"));
    }
    Ok(object)
}

fn lines(file: &DiffFile, old: bool) -> BTreeSet<usize> {
    let side = if old {
        DiffLine::old_line
    } else {
        DiffLine::new_line
    };
    file.hunks()
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter_map(side)
        .collect()
}

fn validate_line(file: &DiffFile, old: bool, line: usize) -> io::Result<()> {
    if line == 0 {
        return Err(invalid("invalid_anchor", "line numbers are one-based"));
    }
    if !lines(file, old).contains(&line) {
        let side = if old { "old" } else { "new" };
        return Err(invalid(
            "anchor_not_found",
            &format!("{side}-side line is not in this review snapshot"),
        ));
    }
    Ok(())
}

fn validate_range(file: &DiffFile, range: &RangeTarget) -> io::Result<()> {
    if range.old.is_none() && range.new.is_none() {
        return Err(invalid(
            "invalid_anchor",
            "range requires an old or new side",
        ));
    }
    for (side, old) in [(range.old, true), (range.new, false)] {
        if let Some(range) = side {
            validate_source_range(file, old, range)?;
        }
    }
    Ok(())
}

fn validate_source_range(file: &DiffFile, old: bool, range: SourceRange) -> io::Result<()> {
    let count = range
        .end
        .checked_sub(range.start)
        .and_then(|count| count.checked_add(1))
        .filter(|_| range.start > 0)
        .ok_or_else(|| invalid("invalid_anchor", "ranges must be positive and ordered"))?;
    if lines(file, old).range(range.start..=range.end).count() != count {
        let side = if old { "old" } else { "new" };
        return Err(invalid(
            "anchor_not_found",
            &format!("every {side}-side line in a range must exist in this review snapshot"),
        ));
    }
    Ok(())
}

fn invalid(code: &str, message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{code}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn duplicated_old_path_is_ambiguous_but_each_new_path_is_valid() {
        let patch = "diff --git a/source b/first\nsimilarity index 100%\nrename from source\nrename to first\ndiff --git a/source b/second\nsimilarity index 100%\nrename from source\nrename to second\n";
        let error = validate(patch, &json!({"file": "source"})).unwrap_err();
        assert!(error.to_string().starts_with("ambiguous_anchor:"));
        validate(patch, &json!({"file": "first", "scope": "file"})).unwrap();
        validate(patch, &json!({"file": "second"})).unwrap();
    }

    #[test]
    fn complete_range_validation_rejects_a_gap_even_when_endpoints_exist() {
        let patch = "diff --git a/file b/file\n--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n@@ -5 +5 @@\n-old\n+new\n";
        let error = validate(
            patch,
            &json!({"file": "file", "range": {"new": {"start": 1, "end": 5}}}),
        )
        .unwrap_err();
        assert!(error.to_string().starts_with("anchor_not_found:"));
        validate(
            patch,
            &json!({"file": "file", "range": {"new": {"start": 5, "end": 5}}}),
        )
        .unwrap();
    }
}
