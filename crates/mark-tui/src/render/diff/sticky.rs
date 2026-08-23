use ratatui::prelude::Line;

use crate::{
    app::DiffApp,
    model::{FileIndex, HunkIndex, UiRow},
};

use super::render_row_with_focus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StickyHunkHeader {
    file: FileIndex,
    hunk: HunkIndex,
    model_row: usize,
}

fn sticky_hunk_header(
    app: &DiffApp,
    scroll: usize,
    visible_rows: usize,
) -> Option<StickyHunkHeader> {
    let (top_model_row, _) = app.model_row_at_scroll(scroll)?;
    let top_row = app.document.model.row(top_model_row)?;
    if matches!(
        top_row,
        UiRow::FileHeader(_) | UiRow::FileBodyNotice(_) | UiRow::HunkHeader { .. }
    ) {
        return None;
    }
    if app.annotation_cursor_at_visual_scroll(scroll) {
        return None;
    }

    let model_row = app.document.model.previous_hunk_row(top_model_row)?;
    let UiRow::HunkHeader { file, hunk } = app.document.model.row(model_row)? else {
        return None;
    };
    if row_file(top_row) != file {
        return None;
    }

    let header_end = visual_start(app, model_row).saturating_add(visual_height(app, model_row));
    if header_end > scroll {
        return None;
    }

    // Only pin while the hunk continues below the viewport. This avoids
    // covering the last page of a hunk or pinning a hunk that already fits.
    let range = app.document.model.hunk_row_range(file.get(), hunk.get())?;
    let last_row = range.end.checked_sub(1)?;
    let hunk_end = visual_start(app, last_row).saturating_add(visual_height(app, last_row));
    let viewport_end = scroll.saturating_add(visible_rows.max(1));
    (hunk_end > viewport_end).then_some(StickyHunkHeader {
        file,
        hunk,
        model_row,
    })
}

pub(crate) fn overlay_sticky_hunk_header(
    app: &mut DiffApp,
    lines: &mut [Line<'static>],
    width: usize,
    visible_rows: usize,
    focused_hunk: Option<(FileIndex, HunkIndex)>,
) {
    if lines.is_empty() {
        return;
    }
    let Some(sticky) = sticky_hunk_header(app, app.viewport.scroll, visible_rows) else {
        return;
    };
    lines[0] = render_row_with_focus(
        app,
        sticky.model_row,
        UiRow::HunkHeader {
            file: sticky.file,
            hunk: sticky.hunk,
        },
        width,
        focused_hunk,
    );
}

fn visual_start(app: &DiffApp, model_row: usize) -> usize {
    if app.viewport.line_wrapping {
        app.wrapped_visual_scroll_for_model_row(model_row)
    } else {
        model_row
    }
}

fn visual_height(app: &DiffApp, model_row: usize) -> usize {
    if app.viewport.line_wrapping {
        app.wrapped_visual_height_for_model_row(model_row).max(1)
    } else {
        1
    }
}

fn row_file(row: UiRow) -> FileIndex {
    match row {
        UiRow::FileHeader(file) | UiRow::FileBodyNotice(file) => file,
        UiRow::Collapsed { file, .. }
        | UiRow::ContextLine { file, .. }
        | UiRow::ContextHide { file, .. }
        | UiRow::HunkHeader { file, .. }
        | UiRow::UnifiedLine { file, .. }
        | UiRow::SplitLine { file, .. }
        | UiRow::MetaLine { file, .. } => file,
    }
}
