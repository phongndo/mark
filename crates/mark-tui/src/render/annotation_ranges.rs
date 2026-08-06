use ratatui::prelude::{Line, Modifier, Style};

use crate::{
    annotation::{AnnotationKey, AnnotationSide},
    controls::DiffLayoutMode,
    model::UiRow,
    theme::{DiffTheme, UNIFIED_GUTTER_WIDTH},
};

use super::{
    annotation_hints::overlay_line_cells,
    grep::{
        highlighted_cursor_diff_content_line, highlighted_cursor_full_line,
        highlighted_cursor_line_in_ranges, highlighted_cursor_meta_line,
        split_content_start_column, unified_content_start_column,
    },
};

const LINE_NUMBER_WIDTH: usize = 5;
const MIN_CONNECTED_CARD_WIDTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnnotationBlockGeometry {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) connected: bool,
}

impl AnnotationBlockGeometry {
    pub(crate) fn card_width(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub(crate) fn body_width(self) -> usize {
        // left/right borders plus one padding cell on each side
        self.card_width().saturating_sub(4)
    }
}

pub(crate) fn annotation_block_geometry(
    layout: DiffLayoutMode,
    width: usize,
    key: &AnnotationKey,
) -> AnnotationBlockGeometry {
    if width == 0 || !key.has_line_connector() {
        return AnnotationBlockGeometry {
            start: 0,
            end: width,
            connected: false,
        };
    }

    let side = key.block_side();
    let (cell_start, cell_end) = match layout {
        DiffLayoutMode::Unified => (0, width),
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            match side {
                AnnotationSide::Old => (0, left_width),
                AnnotationSide::New => (left_width, width),
            }
        }
    };
    let Some(start) = annotation_connector_column(layout, width, side) else {
        return AnnotationBlockGeometry {
            start: cell_start,
            end: cell_end,
            connected: false,
        };
    };
    let connected = annotation_connector_is_connected(start, cell_end);
    AnnotationBlockGeometry {
        start: if connected { start } else { cell_start },
        end: cell_end,
        connected,
    }
}

pub(crate) fn annotation_block_body_width(
    layout: DiffLayoutMode,
    width: usize,
    key: &AnnotationKey,
) -> usize {
    annotation_block_geometry(layout, width, key).body_width()
}

pub(crate) fn highlighted_annotation_row(
    line: Line<'static>,
    row: UiRow,
    layout: DiffLayoutMode,
    width: usize,
    side: Option<AnnotationSide>,
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
        | UiRow::MetaLine { .. } => match side {
            Some(side) => highlighted_diff_content_for_side(line, layout, width, side, theme),
            None => highlighted_cursor_diff_content_line(line, layout, width, theme),
        },
    }
}

fn highlighted_diff_content_for_side(
    line: Line<'static>,
    layout: DiffLayoutMode,
    width: usize,
    side: AnnotationSide,
    theme: DiffTheme,
) -> Line<'static> {
    let ranges = match layout {
        DiffLayoutMode::Unified => vec![(unified_content_start_column(width), width)],
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            match side {
                AnnotationSide::Old => {
                    vec![(split_content_start_column(left_width), left_width)]
                }
                AnnotationSide::New => {
                    let right_width = width.saturating_sub(left_width);
                    vec![(
                        left_width.saturating_add(split_content_start_column(right_width)),
                        width,
                    )]
                }
            }
        }
    };
    highlighted_cursor_line_in_ranges(line, ranges, theme)
}

pub(crate) fn apply_annotation_connector(
    line: Line<'static>,
    layout: DiffLayoutMode,
    width: usize,
    side: AnnotationSide,
    starts_range: bool,
    theme: DiffTheme,
) -> Line<'static> {
    let Some(column) = annotation_connector_column(layout, width, side) else {
        return line;
    };
    let cell_end = match (layout, side) {
        (DiffLayoutMode::Split, AnnotationSide::Old) => width / 2,
        (DiffLayoutMode::Unified | DiffLayoutMode::Split, _) => width,
    };
    if !annotation_connector_is_connected(column, cell_end) {
        return line;
    }
    let glyph = if theme.decorations.is_fancy() {
        if starts_range { "┌" } else { "│" }
    } else if starts_range {
        "+"
    } else {
        "|"
    };
    overlay_line_cells(
        line,
        column,
        1,
        glyph,
        Style::default().fg(theme.hunk).add_modifier(Modifier::BOLD),
    )
}

fn annotation_connector_is_connected(column: usize, cell_end: usize) -> bool {
    cell_end.saturating_sub(column) >= MIN_CONNECTED_CARD_WIDTH
}

fn annotation_connector_column(
    layout: DiffLayoutMode,
    width: usize,
    side: AnnotationSide,
) -> Option<usize> {
    let column = match layout {
        DiffLayoutMode::Unified => {
            // Unified: indicator, old number, separator, new number, rail, sign.
            // Old and new lines share the same sign column, so keeping the rail
            // immediately beside it avoids switching axes across replacements.
            let candidate = 1 + LINE_NUMBER_WIDTH + 1 + LINE_NUMBER_WIDTH;
            let content_start = 1usize.saturating_add(UNIFIED_GUTTER_WIDTH).min(width);
            (candidate < content_start).then_some(candidate)?
        }
        DiffLayoutMode::Split => {
            let left_width = width / 2;
            let cell_start = match side {
                AnnotationSide::Old => 0,
                AnnotationSide::New => left_width,
            };
            let cell_end = match side {
                AnnotationSide::Old => left_width,
                AnnotationSide::New => width,
            };
            let candidate = cell_start.saturating_add(1 + LINE_NUMBER_WIDTH);
            (candidate < cell_end).then_some(candidate)?
        }
    };
    (column < width).then_some(column)
}

#[cfg(test)]
mod tests {
    use crate::annotation::AnnotationScope;

    use super::*;

    fn line_key(side: AnnotationSide) -> AnnotationKey {
        AnnotationKey {
            path: "file.rs".to_owned(),
            side,
            line: 1,
            scope: AnnotationScope::Line,
        }
    }

    #[test]
    fn unified_cards_follow_the_sign_and_split_cards_follow_the_selected_pane() {
        let old = line_key(AnnotationSide::Old);
        let new = line_key(AnnotationSide::New);

        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Unified, 80, &old),
            AnnotationBlockGeometry {
                start: 12,
                end: 80,
                connected: true,
            }
        );
        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Unified, 80, &new),
            AnnotationBlockGeometry {
                start: 12,
                end: 80,
                connected: true,
            }
        );
        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Split, 80, &old),
            AnnotationBlockGeometry {
                start: 6,
                end: 40,
                connected: true,
            }
        );
        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Split, 80, &new),
            AnnotationBlockGeometry {
                start: 46,
                end: 80,
                connected: true,
            }
        );
    }

    #[test]
    fn narrow_split_cards_fall_back_to_their_selected_pane() {
        let old = line_key(AnnotationSide::Old);
        let new = line_key(AnnotationSide::New);

        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Split, 24, &old),
            AnnotationBlockGeometry {
                start: 0,
                end: 12,
                connected: false,
            }
        );
        assert_eq!(
            annotation_block_geometry(DiffLayoutMode::Split, 24, &new),
            AnnotationBlockGeometry {
                start: 12,
                end: 24,
                connected: false,
            }
        );

        let rendered = apply_annotation_connector(
            Line::from(" ".repeat(24)),
            DiffLayoutMode::Split,
            24,
            AnnotationSide::Old,
            true,
            DiffTheme::default(),
        );
        assert_eq!(
            rendered
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            " ".repeat(24)
        );
    }
}
