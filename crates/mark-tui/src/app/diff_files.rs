use super::EditorReloadRequest;
use crate::{
    controls::DiffLayoutMode,
    render::text::{
        DisplaySeek, DisplayWidthUnit, for_display_width_units, single_width_ascii_width,
    },
    theme::{GUTTER_WIDTH, UNIFIED_GUTTER_WIDTH},
};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

pub(crate) fn repo_relative_path(repo: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(repo).ok().map(Path::to_path_buf)
}

pub(crate) fn editor_reload_request_for_file(
    file: &mark_diff::DiffFile,
) -> Option<EditorReloadRequest> {
    let path = PathBuf::from(file.new_path()?);
    let mut pathspecs = Vec::new();
    push_unique_pathspec(&mut pathspecs, file.old_path());
    push_unique_pathspec(&mut pathspecs, file.new_path());

    Some(EditorReloadRequest {
        path,
        pathspecs,
        view_anchor: None,
    })
}

fn push_unique_pathspec(pathspecs: &mut Vec<PathBuf>, path: Option<&str>) {
    let Some(path) = path else {
        return;
    };

    let path = PathBuf::from(path);
    if !pathspecs.iter().any(|known| known == &path) {
        pathspecs.push(path);
    }
}

pub(crate) fn splice_diff_files_for_paths(
    files: &mut Vec<mark_diff::DiffFile>,
    paths: &[PathBuf],
    mut replacement: Vec<mark_diff::DiffFile>,
) {
    let mut next = Vec::with_capacity(files.len().saturating_add(replacement.len()));
    let mut inserted = false;

    for file in files.drain(..) {
        if paths
            .iter()
            .any(|path| diff_file_matches_path_scope(&file, path))
        {
            if !inserted {
                next.append(&mut replacement);
                inserted = true;
            }
            continue;
        }

        next.push(file);
    }

    if !inserted {
        next.append(&mut replacement);
    }

    *files = next;
}

pub(crate) fn diff_file_matches_path(file: &mark_diff::DiffFile, path: &Path) -> bool {
    let path = diff_path_string(path);
    file.old_path() == Some(path.as_str()) || file.new_path() == Some(path.as_str())
}

pub(crate) fn diff_file_matches_path_scope(file: &mark_diff::DiffFile, path: &Path) -> bool {
    let path = diff_path_string(path);
    let path = Path::new(&path);
    file.old_path()
        .is_some_and(|file_path| Path::new(file_path).starts_with(path))
        || file
            .new_path()
            .is_some_and(|file_path| Path::new(file_path).starts_with(path))
}

pub(crate) fn diff_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn diff_content_width(layout: DiffLayoutMode, width: usize) -> usize {
    match layout {
        DiffLayoutMode::Unified => unified_content_width(width),
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let right_width = width.saturating_sub(left_width);
            split_cell_content_width(left_width).min(split_cell_content_width(right_width))
        }
    }
}

pub(crate) fn unified_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = UNIFIED_GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}

pub(crate) fn split_cell_content_width(width: usize) -> usize {
    let indicator_width = 1.min(width);
    let gutter_width = GUTTER_WIDTH.min(width.saturating_sub(indicator_width));
    width.saturating_sub(indicator_width + gutter_width)
}

pub(crate) fn wrapped_line_count(text: &str, content_width: usize) -> usize {
    // Printable ASCII fills every row; this matches the unit walk below.
    if content_width > 0
        && let Some(width) = single_width_ascii_width(text.as_bytes())
    {
        return width.div_ceil(content_width).max(1);
    }
    let mut count = 1usize;
    for_wrapped_line_start_after_first(text, content_width, |_| {
        count = count.saturating_add(1);
    });
    count
}

pub(crate) fn wrapped_line_start_columns(text: &str, content_width: usize) -> Vec<usize> {
    let mut starts = vec![0];
    for_wrapped_line_start_after_first(text, content_width, |start| starts.push(start.column));
    starts
}

/// Row seeks for one wrapped line: its row count, seeks for the rows inside a
/// requested window, and a seek to the end of the wrapped units (`None` when
/// the content width is zero). Rows outside the window are counted, not kept.
pub(crate) struct WrappedRowSeeks {
    pub(crate) rows: usize,
    first: usize,
    window: Vec<DisplaySeek>,
    pub(crate) end: Option<DisplaySeek>,
}

impl WrappedRowSeeks {
    pub(crate) fn get(&self, row: usize) -> Option<DisplaySeek> {
        row.checked_sub(self.first)
            .and_then(|index| self.window.get(index))
            .copied()
    }
}

pub(crate) fn wrapped_row_seeks(
    text: &str,
    content_width: usize,
    window: Range<usize>,
) -> WrappedRowSeeks {
    let mut seeks = Vec::new();
    if window.contains(&0) {
        seeks.push(DisplaySeek::default());
    }
    let mut rows = 1usize;
    let end = for_wrapped_line_start_after_first(text, content_width, |start| {
        if window.contains(&rows) {
            seeks.push(start);
        }
        rows += 1;
    });
    WrappedRowSeeks {
        rows,
        first: window.start,
        window: seeks,
        end,
    }
}

/// Visits each row start after the first and returns a seek to the end of the
/// consumed width, or `None` without scanning when `content_width` is zero.
fn for_wrapped_line_start_after_first(
    text: &str,
    content_width: usize,
    mut visit: impl FnMut(DisplaySeek),
) -> Option<DisplaySeek> {
    if content_width == 0 {
        return None;
    }

    let mut line_width = 0usize;
    let mut consumed_width = 0usize;
    // Start byte and column of the unit holding the last consumed column. Row
    // starts seek there, leaving at least one column to skip.
    let mut last_unit = (0usize, 0usize);
    let mut start_row = |consumed_width: usize, last_unit: (usize, usize)| {
        visit(DisplaySeek {
            column: consumed_width,
            byte: last_unit.0,
            byte_column: last_unit.1,
        });
    };
    for_display_width_units(text, |unit| {
        let (byte_start, unit_width, supports_partial_render) = match unit {
            DisplayWidthUnit::Unit {
                byte_start,
                width,
                supports_partial_render,
            } => (byte_start, width, supports_partial_render),
            DisplayWidthUnit::SingleWidthRun {
                byte_start,
                count: mut remaining,
            } => {
                // Equivalent to one-column units below: each starts a new row
                // exactly when the current row is full.
                let run_column = consumed_width;
                while remaining > 0 {
                    if line_width >= content_width {
                        start_row(consumed_width, last_unit);
                        line_width = 0;
                    }
                    let taken = remaining.min(content_width - line_width);
                    line_width += taken;
                    consumed_width = consumed_width.saturating_add(taken);
                    remaining -= taken;
                    let last_column = consumed_width - 1;
                    last_unit = (byte_start + (last_column - run_column), last_column);
                }
                return;
            }
        };
        if unit_width == 0 {
            return;
        }

        if supports_partial_render {
            let unit_column = consumed_width;
            let mut remaining_width = unit_width;
            while remaining_width > 0 {
                if line_width >= content_width {
                    start_row(consumed_width, last_unit);
                    line_width = 0;
                }

                let available = content_width.saturating_sub(line_width);
                if available == 0 {
                    break;
                }
                let taken = remaining_width.min(available);
                line_width = line_width.saturating_add(taken);
                consumed_width = consumed_width.saturating_add(taken);
                remaining_width -= taken;
                last_unit = (byte_start, unit_column);

                if remaining_width > 0 {
                    start_row(consumed_width, last_unit);
                    line_width = 0;
                }
            }
            return;
        }

        if line_width == content_width
            || (line_width > 0 && line_width.saturating_add(unit_width) > content_width)
        {
            start_row(consumed_width, last_unit);
            line_width = 0;
        }

        last_unit = (byte_start, consumed_width);
        line_width = line_width.saturating_add(unit_width);
        consumed_width = consumed_width.saturating_add(unit_width);
    });
    Some(DisplaySeek {
        column: consumed_width,
        byte: last_unit.0,
        byte_column: last_unit.1,
    })
}
