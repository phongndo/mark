use mark_diff::DiffFile;
use mark_session::{ProtocolError, RangeTarget, ReviewAnchor, ReviewAnchorScope, SourceRange};

use crate::{
    annotation::{AnnotationKey, AnnotationScope, AnnotationSide},
    app::DiffApp,
};

pub(crate) fn validate(
    app: &DiffApp,
    anchor: &ReviewAnchor,
) -> Result<AnnotationKey, ProtocolError> {
    if anchor.file.is_empty() || anchor.file.len() > mark_session::MAX_PATH_BYTES {
        return Err(ProtocolError::new(
            "invalid_path",
            "comment path is empty or exceeds the byte limit",
        ));
    }
    let file = find_file(app, &anchor.file)?;
    let target_count = usize::from(anchor.scope.is_some())
        + usize::from(anchor.hunk.is_some())
        + usize::from(anchor.old_line.is_some())
        + usize::from(anchor.new_line.is_some())
        + usize::from(anchor.range.is_some());
    if target_count != 1 {
        return Err(ProtocolError::new(
            "invalid_anchor",
            "exactly one file scope, hunk, old line, new line, or range target is required",
        ));
    }

    if anchor.scope == Some(ReviewAnchorScope::File) {
        return AnnotationKey::for_file(file)
            .ok_or_else(|| ProtocolError::new("anchor_not_found", "file has no source path"));
    }
    if let Some(hunk) = anchor.hunk {
        let index = hunk
            .checked_sub(1)
            .ok_or_else(|| ProtocolError::new("invalid_anchor", "hunk indexes are one-based"))?;
        let hunk = file.hunks().get(index).ok_or_else(|| {
            ProtocolError::new(
                "anchor_not_found",
                format!("hunk {} does not exist in {}", index + 1, anchor.file),
            )
        })?;
        return AnnotationKey::for_hunk(file, hunk)
            .ok_or_else(|| ProtocolError::new("anchor_not_found", "hunk has no source path"));
    }
    if let Some(line) = anchor.old_line {
        validate_line(file, AnnotationSide::Old, line, &anchor.file)?;
        return AnnotationKey::for_file_line(file, AnnotationSide::Old, line)
            .ok_or_else(|| ProtocolError::new("anchor_not_found", "old side has no source path"));
    }
    if let Some(line) = anchor.new_line {
        validate_line(file, AnnotationSide::New, line, &anchor.file)?;
        return AnnotationKey::for_file_line(file, AnnotationSide::New, line)
            .ok_or_else(|| ProtocolError::new("anchor_not_found", "new side has no source path"));
    }
    validate_range(file, anchor.range.as_ref().expect("range target"))
}

pub(crate) fn to_protocol(app: &DiffApp, key: &AnnotationKey) -> ReviewAnchor {
    let mut anchor = ReviewAnchor {
        file: key.path.clone(),
        scope: None,
        hunk: None,
        old_line: None,
        new_line: None,
        range: None,
    };
    match key.scope {
        AnnotationScope::File => anchor.scope = Some(ReviewAnchorScope::File),
        AnnotationScope::Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
        } => {
            anchor.hunk = app.document.changeset.files.iter().find_map(|file| {
                (file.old_path() == Some(key.path.as_str())
                    || file.new_path() == Some(key.path.as_str()))
                .then(|| {
                    file.hunks().iter().position(|hunk| {
                        hunk.old_start() == old_start
                            && hunk.old_count() == old_count
                            && hunk.new_start() == new_start
                            && hunk.new_count() == new_count
                    })
                })
                .flatten()
                .map(|index| index + 1)
            });
            if anchor.hunk.is_none() {
                anchor.range = Some(RangeTarget {
                    old: source_range(old_start, old_count),
                    new: source_range(new_start, new_count),
                });
            }
        }
        AnnotationScope::Range {
            old_start,
            old_count,
            new_start,
            new_count,
        } => {
            anchor.range = Some(RangeTarget {
                old: source_range(old_start, old_count),
                new: source_range(new_start, new_count),
            });
        }
        AnnotationScope::Line => match key.side {
            AnnotationSide::Old => anchor.old_line = Some(key.line),
            AnnotationSide::New => anchor.new_line = Some(key.line),
        },
    }
    anchor
}

fn find_file<'a>(app: &'a DiffApp, path: &str) -> Result<&'a DiffFile, ProtocolError> {
    let mut matches = app
        .document
        .changeset
        .files
        .iter()
        .filter(|file| file.old_path() == Some(path) || file.new_path() == Some(path));
    let file = matches.next().ok_or_else(|| {
        ProtocolError::new(
            "path_not_found",
            format!("file is not in the loaded changeset: {path}"),
        )
    })?;
    if matches.next().is_some() {
        return Err(ProtocolError::new(
            "ambiguous_path",
            format!("path matches multiple files in the changeset: {path}"),
        ));
    }
    Ok(file)
}

fn validate_line(
    file: &DiffFile,
    side: AnnotationSide,
    line: usize,
    path: &str,
) -> Result<(), ProtocolError> {
    if line == 0
        || !file
            .hunks()
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .any(|candidate| match side {
                AnnotationSide::Old => candidate.old_line() == Some(line),
                AnnotationSide::New => candidate.new_line() == Some(line),
            })
    {
        let side = match side {
            AnnotationSide::Old => "old",
            AnnotationSide::New => "new",
        };
        return Err(ProtocolError::new(
            "anchor_not_found",
            format!("no {side}-side line {line} exists in {path}"),
        ));
    }
    Ok(())
}

fn validate_range(file: &DiffFile, range: &RangeTarget) -> Result<AnnotationKey, ProtocolError> {
    if range.old.is_none() && range.new.is_none() {
        return Err(ProtocolError::new(
            "invalid_anchor",
            "range target has no old or new side",
        ));
    }
    let old = range
        .old
        .map(|range| validate_source_range(file, AnnotationSide::Old, range))
        .transpose()?;
    let new = range
        .new
        .map(|range| validate_source_range(file, AnnotationSide::New, range))
        .transpose()?;
    let (anchor_side, anchor_line) = new
        .map(|(_, end, _)| (AnnotationSide::New, end))
        .or_else(|| old.map(|(_, end, _)| (AnnotationSide::Old, end)))
        .expect("range has one side");
    let (old_start, old_count) = old.map_or((0, 0), |(start, _, count)| (start, count));
    let (new_start, new_count) = new.map_or((0, 0), |(start, _, count)| (start, count));
    AnnotationKey::for_range(
        file,
        anchor_side,
        anchor_line,
        old_start,
        old_count,
        new_start,
        new_count,
    )
    .ok_or_else(|| ProtocolError::new("anchor_not_found", "range has no source path"))
}

fn validate_source_range(
    file: &DiffFile,
    side: AnnotationSide,
    range: SourceRange,
) -> Result<(usize, usize, usize), ProtocolError> {
    let count = range
        .end
        .checked_sub(range.start)
        .and_then(|count| count.checked_add(1))
        .filter(|_| range.start > 0)
        .ok_or_else(|| {
            ProtocolError::new(
                "invalid_anchor",
                "range must be positive and end at or after start",
            )
        })?;
    let matched = file
        .hunks()
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter_map(|line| match side {
            AnnotationSide::Old => line.old_line(),
            AnnotationSide::New => line.new_line(),
        })
        .filter(|line| *line >= range.start && *line <= range.end)
        .count();
    if matched != count {
        return Err(ProtocolError::new(
            "anchor_not_found",
            "every line in a comment range must exist in the loaded diff",
        ));
    }
    Ok((range.start, range.end, count))
}

fn source_range(start: usize, count: usize) -> Option<SourceRange> {
    (count > 0).then(|| SourceRange {
        start,
        end: start.saturating_add(count.saturating_sub(1)),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mark_diff::{Changeset, DiffOptions, RepoRoot};

    use crate::controls::DiffLayoutMode;

    use super::*;

    #[test]
    fn file_anchor_round_trips_through_the_protocol() {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let app = DiffApp::new(
            DiffOptions::default(),
            Changeset {
                repo: RepoRoot::new("/repo"),
                title: "test".to_owned(),
                files: mark_diff::parse_patch(patch),
                raw_patch: Arc::from(patch.as_bytes()),
            },
            DiffLayoutMode::Unified,
        );
        let key = AnnotationKey::for_file(&app.document.changeset.files[0]).unwrap();

        let protocol = to_protocol(&app, &key);

        assert_eq!(protocol.scope, Some(ReviewAnchorScope::File));
        assert_eq!(validate(&app, &protocol).unwrap(), key);
    }
}
