use std::ops::Range;

use ratatui::prelude::Line;

use crate::{
    app::DiffApp,
    model::{FileIndex, HunkIndex, UiRow},
    syntax::unified_syntax_side,
};

use super::{
    SplitLineRender, render_context_line_wrapped, render_row_with_focus,
    render_split_line_wrapped_with_focus,
    unified::{WrappedLineRender, render_unified_line_wrapped_with_focus},
};

/// Materialize only the requested continuation rows. Locating their terminal
/// columns still scans the source; invisible rows must not allocate styled text.
pub(crate) fn render_row_wrapped_with_focus(
    app: &mut DiffApp,
    row_index: usize,
    row: UiRow,
    width: usize,
    focused_hunk: Option<(FileIndex, HunkIndex)>,
    rows: Range<usize>,
) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let theme = app.config.theme;
    let hunk_focused = row
        .typed_hunk_key()
        .is_some_and(|hunk_key| Some(hunk_key) == focused_hunk);

    match row {
        UiRow::ContextLine {
            file,
            old_line,
            new_line,
        } => {
            render_context_line_wrapped(app, file.get(), old_line, new_line, row_index, width, rows)
        }
        UiRow::UnifiedLine { file, hunk, line } => {
            let kind = app.document.changeset.files[file].hunks()[hunk].lines[line].kind();
            let syntax = unified_syntax_side(kind)
                .and_then(|side| app.syntax_line(file.get(), hunk.get(), line.get(), side));
            let inline = app.inline_ranges(file.get(), hunk.get(), line.get());
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_wrapped_with_focus(
                diff_line,
                syntax.as_deref(),
                &inline,
                WrappedLineRender {
                    width,
                    theme,
                    focused: hunk_focused,
                    grep_filter: &app.filters.grep_filter,
                    rows,
                },
            )
        }
        UiRow::MetaLine { file, hunk, line } => {
            let diff_line = &app.document.changeset.files[file].hunks()[hunk].lines[line];
            render_unified_line_wrapped_with_focus(
                diff_line,
                None,
                &[],
                WrappedLineRender {
                    width,
                    theme,
                    focused: hunk_focused,
                    grep_filter: &app.filters.grep_filter,
                    rows,
                },
            )
        }
        UiRow::SplitLine {
            file,
            hunk,
            left,
            right,
        } => render_split_line_wrapped_with_focus(
            app,
            SplitLineRender {
                file: file.get(),
                hunk: hunk.get(),
                left: left.get().map(|line| line.get()),
                right: right.get().map(|line| line.get()),
                row_index,
                width,
                focused: hunk_focused,
            },
            rows,
        ),
        _ if rows.start == 0 => vec![render_row_with_focus(
            app,
            row_index,
            row,
            width,
            focused_hunk,
        )],
        _ => Vec::new(),
    }
}
