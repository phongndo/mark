mod diff_choice;
mod git_queries;
mod layout;
mod matcher;
mod refs;

pub(crate) use diff_choice::{DiffChoice, DiffFilterKind, is_review_options};
pub(crate) use layout::{
    CrosstermTerminal, DiffLayoutMode, INPUT_CURSOR, default_layout_for_width,
};
pub(crate) use matcher::{
    TextMatcher, ascii_case_insensitive_bytes_equal, branch_match_score, diff_line_grep_prefix,
    diff_stats_for_files, fuzzy_subsequence_score, text_match_range_ascii_case_insensitive,
};
pub(crate) use refs::{
    BranchMenu, GitCommit, branch_base_from_options, branch_head_from_options, commit_match_score,
    commit_menu_width, commit_short_sha, comparison_branches, comparison_commits,
    current_head_label, default_branch_base, rev_display_label,
};
