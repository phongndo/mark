mod cursor;
mod highlight;
mod ranges;
mod target;
pub(crate) mod types;

pub(crate) use cursor::{
    highlighted_cursor_diff_content_line, highlighted_cursor_full_line,
    highlighted_cursor_line_in_ranges, highlighted_cursor_meta_line,
};
pub(crate) use highlight::highlighted_grep_text_line;
pub(crate) use ranges::highlighted_line_in_ranges;
pub(crate) use target::{
    diff_line_grep_highlight_text, grep_highlight_target_for_columns,
    grep_highlight_targets_for_row, scrolled_text_byte_start, split_content_start_column,
    split_diff_line_grep_highlight_target, unified_content_start_column,
};
