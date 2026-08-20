use ratatui::prelude::Line;

use crate::{
    annotation::AnnotationKey,
    app::DiffApp,
    controls::DiffLayoutMode,
    model::UiRow,
    render::{
        annotation_hints::{AnnotationTargetHint, apply_annotation_target_hint},
        annotations::{render_annotation_compose_block, render_annotation_saved_block},
        grep::{
            highlight_saved_annotation_block, highlighted_cursor_diff_content_line,
            highlighted_cursor_full_line, highlighted_cursor_meta_line,
        },
    },
    theme::DiffTheme,
};

use super::{render_row_with_focus, render_row_wrapped_with_focus};

pub(crate) fn build_diff_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    app.prepare_full_file_context_for_viewport(visible_rows);
    let mut lines = if app.viewport.line_wrapping {
        build_wrapped_viewport_lines(app, width, visible_rows)
    } else {
        build_unwrapped_viewport_lines(app, width, visible_rows)
    };
    super::sticky::overlay_sticky_hunk_header(app, &mut lines, width, visible_rows);
    app.refresh_code_selection_render(&mut lines, width, visible_rows);
    lines
}

fn build_unwrapped_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let has_annotation_blocks = draft.is_some() || !app.annotations_state.annotations.is_empty();
    let focused_hunk = app.focused_hunk_for_viewport(visible_rows);
    let mut lines = Vec::with_capacity(visible_rows);

    for offset in 0..visible_rows {
        if lines.len() >= visible_rows {
            break;
        }
        let visual_row = app.viewport.scroll.saturating_add(offset);
        let Some(row) = app.document.model.row(visual_row) else {
            break;
        };
        let mut line = render_row_with_focus(app, visual_row, row, width, focused_hunk);
        if app.annotation_cursor_at_visual_scroll(visual_row) {
            line = highlighted_annotation_row(line, row, layout, width, theme);
        }
        for (hint, scope, side, existing) in
            app.annotation_target_hints_at_visual_scroll(visual_row)
        {
            line = apply_annotation_target_hint(
                line,
                layout,
                width,
                AnnotationTargetHint {
                    scope,
                    side,
                    hint,
                    existing_annotation: existing,
                    uppercase: app.config.syntax_settings.annotations.uppercase_hints,
                },
                theme,
            );
        }
        lines.push(line);

        if has_annotation_blocks {
            for key in app.annotation_keys_at_model_row(visual_row, row) {
                if let Some(draft) = draft
                    .as_ref()
                    .filter(|d| d.model_row_index == visual_row && d.key == key)
                {
                    let label = app.annotation_block_label(&draft.key, true);
                    push_annotation_block(
                        &mut lines,
                        render_annotation_compose_block(draft, width, theme, Some(&label)),
                        visible_rows,
                    );
                } else if let Some(text) = app.annotations_state.annotations.get(&key)
                    && draft.as_ref().is_none_or(|d| d.key != key)
                {
                    let label = app.annotation_block_label(&key, false);
                    let block_scroll = annotation_saved_block_scroll(app, &key);
                    let focused = app.annotation_cursor_focuses_saved_key(&key);
                    push_annotation_block(
                        &mut lines,
                        highlight_saved_annotation_block(
                            render_annotation_saved_block(
                                text,
                                width,
                                theme,
                                Some(&label),
                                app.annotations_state.annotations.is_human_only(&key),
                            ),
                            width,
                            theme,
                            focused,
                        )
                        .into_iter()
                        .skip(block_scroll),
                        visible_rows,
                    );
                }
            }
        }
    }

    lines.truncate(visible_rows);
    lines
}

fn build_wrapped_viewport_lines(
    app: &mut DiffApp,
    width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    let theme = app.config.theme;
    let layout = app.viewport.layout;
    let draft = app.annotations_state.annotation_draft.clone();
    let has_annotation_blocks = draft.is_some() || !app.annotations_state.annotations.is_empty();
    let focused_hunk = app.focused_hunk_for_viewport(visible_rows);
    let mut lines = Vec::with_capacity(visible_rows);
    let Some((mut row_index, mut row_offset)) = app.model_row_at_scroll(app.viewport.scroll) else {
        return lines;
    };
    let mut visual_row = app.viewport.scroll;
    while lines.len() < visible_rows {
        let Some(row) = app.document.model.row(row_index) else {
            break;
        };
        let remaining = visible_rows.saturating_sub(lines.len());
        let rendered = render_row_wrapped_with_focus(app, row_index, row, width, focused_hunk);
        let wrap_count = rendered.len().saturating_sub(row_offset);
        for (wrap_index, line) in rendered
            .into_iter()
            .skip(row_offset)
            .take(remaining)
            .enumerate()
        {
            let mut line = line;
            let is_last_wrap = wrap_index + 1 == wrap_count.min(remaining);
            if app.annotation_cursor_at_model_row(row_index) {
                line = highlighted_annotation_row(line, row, layout, width, theme);
            }
            for (hint, scope, side, existing) in
                app.annotation_target_hints_at_visual_scroll(visual_row)
            {
                line = apply_annotation_target_hint(
                    line,
                    layout,
                    width,
                    AnnotationTargetHint {
                        scope,
                        side,
                        hint,
                        existing_annotation: existing,
                        uppercase: app.config.syntax_settings.annotations.uppercase_hints,
                    },
                    theme,
                );
            }
            lines.push(line);
            visual_row = visual_row.saturating_add(1);
            if lines.len() >= visible_rows {
                break;
            }
            if is_last_wrap && has_annotation_blocks {
                for key in app.annotation_keys_at_model_row(row_index, row) {
                    if let Some(draft) = draft
                        .as_ref()
                        .filter(|d| d.model_row_index == row_index && d.key == key)
                    {
                        let label = app.annotation_block_label(&draft.key, true);
                        push_annotation_block(
                            &mut lines,
                            render_annotation_compose_block(draft, width, theme, Some(&label)),
                            visible_rows,
                        );
                    } else if let Some(text) = app.annotations_state.annotations.get(&key)
                        && draft.as_ref().is_none_or(|d| d.key != key)
                    {
                        let label = app.annotation_block_label(&key, false);
                        let block_scroll = annotation_saved_block_scroll(app, &key);
                        let focused = app.annotation_cursor_focuses_saved_key(&key);
                        push_annotation_block(
                            &mut lines,
                            highlight_saved_annotation_block(
                                render_annotation_saved_block(
                                    text,
                                    width,
                                    theme,
                                    Some(&label),
                                    app.annotations_state.annotations.is_human_only(&key),
                                ),
                                width,
                                theme,
                                focused,
                            )
                            .into_iter()
                            .skip(block_scroll),
                            visible_rows,
                        );
                    }
                }
            }
        }
        row_offset = 0;
        row_index = row_index.saturating_add(1);
    }
    lines.truncate(visible_rows);
    lines
}

fn highlighted_annotation_row(
    line: Line<'static>,
    row: UiRow,
    layout: DiffLayoutMode,
    width: usize,
    theme: DiffTheme,
) -> Line<'static> {
    match row {
        UiRow::FileHeader(_) | UiRow::FileBodyNotice(_) => {
            highlighted_cursor_full_line(line, width, theme)
        }
        UiRow::Collapsed { .. } | UiRow::ContextHide { .. } | UiRow::HunkHeader { .. } => {
            highlighted_cursor_meta_line(line, width, theme)
        }
        UiRow::ContextLine { .. }
        | UiRow::UnifiedLine { .. }
        | UiRow::SplitLine { .. }
        | UiRow::MetaLine { .. } => {
            highlighted_cursor_diff_content_line(line, layout, width, theme)
        }
    }
}

fn annotation_saved_block_scroll(app: &DiffApp, key: &AnnotationKey) -> usize {
    app.annotations_state
        .annotation_block_scroll
        .as_ref()
        .filter(|(scroll_key, _)| scroll_key == key)
        .map(|(_, offset)| *offset)
        .unwrap_or_default()
}

fn push_annotation_block(
    lines: &mut Vec<Line<'static>>,
    block: impl IntoIterator<Item = Line<'static>>,
    visible_rows: usize,
) {
    for line in block {
        if lines.len() >= visible_rows {
            break;
        }
        lines.push(line);
    }
}
