use super::EditorReloadRequest;
use crate::{
    controls::DiffLayoutMode,
    render::text::for_display_width_units,
    theme::{GUTTER_WIDTH, UNIFIED_GUTTER_WIDTH},
};
use std::path::{Path, PathBuf};

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

pub(crate) fn splice_diff_files_for_path(
    files: &mut Vec<mark_diff::DiffFile>,
    path: &Path,
    mut replacement: Vec<mark_diff::DiffFile>,
) {
    let mut next = Vec::with_capacity(files.len().saturating_add(replacement.len()));
    let mut inserted = false;

    for file in files.drain(..) {
        if diff_file_matches_path(&file, path) {
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
    let mut count = 1usize;
    for_wrapped_line_start_after_first(text, content_width, |_| {
        count = count.saturating_add(1);
    });
    count
}

pub(crate) fn wrapped_line_start_columns(text: &str, content_width: usize) -> Vec<usize> {
    let mut starts = vec![0];
    for_wrapped_line_start_after_first(text, content_width, |start| starts.push(start));
    starts
}

fn for_wrapped_line_start_after_first(
    text: &str,
    content_width: usize,
    mut visit: impl FnMut(usize),
) {
    if content_width == 0 {
        return;
    }

    let mut line_width = 0usize;
    let mut consumed_width = 0usize;
    for_display_width_units(text, |unit_width, supports_partial_render| {
        if unit_width == 0 {
            return;
        }

        if supports_partial_render {
            let mut remaining_width = unit_width;
            while remaining_width > 0 {
                if line_width >= content_width {
                    visit(consumed_width);
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

                if remaining_width > 0 {
                    visit(consumed_width);
                    line_width = 0;
                }
            }
            return;
        }

        if line_width == content_width
            || (line_width > 0 && line_width.saturating_add(unit_width) > content_width)
        {
            visit(consumed_width);
            line_width = 0;
        }

        line_width = line_width.saturating_add(unit_width);
        consumed_width = consumed_width.saturating_add(unit_width);
    });
}
