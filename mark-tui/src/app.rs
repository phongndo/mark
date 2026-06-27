use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    process,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use mark_core::{MarkError, MarkResult};
use mark_diff::{Changeset, DiffOptions, DiffScope, DiffSource, DiffStats};
use mark_syntax::{
    ColorOverrides, DiffContextExpansion, HighlightedLine, LayoutSetting, NotificationMode,
    NotificationSettings, SyntaxLimits, SyntaxSettings, SyntaxThemeConfig, SyntaxThemeSource,
    ToastCorner,
};
use ratatui::layout::Rect;
use tempfile::TempDir;
use tokio::sync::{mpsc::Receiver, oneshot};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    annotation::{
        AnnotationDraft, AnnotationKey, AnnotationSide, AnnotationStore,
        paired_old_line_for_addition,
    },
    controls::{
        BranchMenu, CrosstermTerminal, DiffChoice, DiffFilterKind, DiffLayoutMode, GitCommit,
        branch_base_from_options, branch_head_from_options, branch_match_score, commit_match_score,
        commit_menu_width, commit_short_sha, comparison_branches, comparison_commits,
        current_head_label, default_branch_base, default_layout_for_width, diff_stats_for_files,
        is_review_options, rev_display_label,
    },
    editor::{EditorTarget, configured_editor, open_editor, open_text_in_editor, repo_file_path},
    event_reader::TerminalEventReader,
    keymap::{GlobalAction, KeyPress, Keymap, MenuAction},
    live_diff::{LiveDiff, LiveDiffReload, live_diff_supported},
    model::{ContextKey, ContextSourceEntry, ContextSourceKey, UiModel, UiRow, context_expands_up},
    render::{
        annotations::{
            annotation_close_hit_at_column, annotation_compose_block_height,
            annotation_edit_hit_at_column, annotation_hit_at_column, annotation_saved_block_height,
            annotation_submit_hit_at_column,
        },
        draw,
        menus::{
            branch_menu_block, branch_menu_list_visible_rows, branch_menu_width,
            color_scheme_picker_block, color_scheme_picker_list_visible_rows, commit_menu_block,
            commit_menu_list_visible_rows, diff_menu_block, diff_selector_width,
            help_menu_list_visible_rows,
        },
        sidebar::max_file_sidebar_width,
        viewport_plan::{
            ViewportSlotKind, annotation_saved_key_at_bottom_border,
            annotation_saved_key_at_top_border, compose_block_bottom_viewport_row,
            compose_block_top_viewport_row, model_row_for_viewport_row,
            plan_diff_viewport_rows_at_scroll, visual_scroll_for_viewport_row,
        },
    },
    runtime,
    search::{DiffSearchIndex, DiffSearchResult, grep_match_rows},
    selector::SelectorState,
    syntax::{
        DiffSide, InlineHunkEmphasisCache, InlineHunkKey, InlineRange, LruCache, SyntaxPosition,
        SyntaxPriority, SyntaxRuntime, available_context_lines, full_file_source,
        load_full_file_source, split_context_source_lines, unified_syntax_side,
    },
    text_input::{TextInputKeyResult, handle_text_input_key},
    theme::{
        BASE_BRANCH_MARKER, BRANCH_COMPARISON_SEPARATOR, CURRENT_BRANCH_MARKER, DiffTheme,
        EVENT_POLL, FILE_SIDEBAR_MIN_WIDTH, GUTTER_WIDTH, HELP_MENU_ROWS, HORIZONTAL_SCROLL_STEP,
        HelpMenuKey, HelpMenuRow, MAX_BRANCH_MENU_ROWS, MAX_INLINE_DIFF_CACHE_ENTRIES,
        MAX_READY_EVENTS_PER_FRAME, MAX_SYNTAX_RESULTS_PER_FRAME, MOUSE_SCROLL_ACCEL_A,
        MOUSE_SCROLL_ACCEL_TAU, MOUSE_SCROLL_HISTORY_SIZE, MOUSE_SCROLL_MAX_MULTIPLIER,
        MOUSE_SCROLL_MIN_TICK_INTERVAL, MOUSE_SCROLL_REFERENCE_INTERVAL_MS,
        MOUSE_SCROLL_STREAK_TIMEOUT, STATUSLINE_SELECTOR_GAP, SyntaxBenchmarkReport,
        UNIFIED_GUTTER_WIDTH, diff_theme_from_config,
    },
    toast::{ToastLevel, Toasts},
};

const MOUSE_HUNK_FOCUS_SCROLL_TICKS: isize = 3;
const EDITOR_RELOAD_POLL: Duration = Duration::from_millis(8);
const FILTER_DEBOUNCE: Duration = Duration::from_millis(120);
const DIFF_PREFETCH_POLL: Duration = Duration::from_millis(8);
const FILTER_WORKER_POLL: Duration = Duration::from_millis(8);
const MAX_LIVE_GREP_MATCHES: usize = 10_000;
const MAX_DIFF_CACHE_ENTRIES: usize = 4;
const MAX_COLOR_SCHEME_MENU_ROWS: usize = 9;
pub(crate) const ERROR_LOG_DEFAULT_HEIGHT: u16 = 6;
pub(crate) const ERROR_LOG_MIN_HEIGHT: u16 = 3;
pub(crate) const ERROR_LOG_MAX_HEIGHT: u16 = 40;
const POST_EDITOR_QUIT_KEY_IGNORE: Duration = Duration::from_millis(250);
const NORMAL_GLOBAL_ACTIONS: &[GlobalAction] = &[
    GlobalAction::Quit,
    GlobalAction::Help,
    GlobalAction::Reload,
    GlobalAction::FileFilter,
    GlobalAction::Grep,
    GlobalAction::DiffMenu,
    GlobalAction::HeadBranch,
    GlobalAction::BaseBranch,
    GlobalAction::CommitPicker,
    GlobalAction::OptionsMenu,
    GlobalAction::FileBrowser,
    GlobalAction::PreviousFile,
    GlobalAction::NextFile,
    GlobalAction::PreviousHunk,
    GlobalAction::NextHunk,
    GlobalAction::ExpandContextUp,
    GlobalAction::ExpandContextDown,
    GlobalAction::CollapseContextAll,
    GlobalAction::Layout,
    GlobalAction::EditHunk,
    GlobalAction::CopyMarks,
    GlobalAction::CopyErrorLog,
    GlobalAction::ClearFilters,
    GlobalAction::NextDiffType,
    GlobalAction::PreviousDiffType,
    GlobalAction::NextAnnotation,
    GlobalAction::PreviousAnnotation,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusScrollBehavior {
    Preserve,
    ClearOnScroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusModelBehavior {
    PreserveIfValid,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorReloadBehavior {
    None,
    ScopedAsync,
    Sync,
}

struct FocusedEditorLaunch {
    target: EditorTarget,
    editor: String,
}

pub(crate) struct EditorReloadWorker {
    generation: u64,
    rx: oneshot::Receiver<EditorScopedReload>,
}

impl std::fmt::Debug for EditorReloadWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("EditorReloadWorker").finish()
    }
}

pub(crate) struct EditorScopedReload {
    path: PathBuf,
    changeset: MarkResult<Changeset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingFilterApply {
    generation: u64,
    due_at: Instant,
    jump_to_grep: bool,
}

pub(crate) struct FilterWorker {
    generation: u64,
    file_filter: String,
    grep_filter: String,
    jump_to_grep: bool,
    rx: oneshot::Receiver<DiffSearchResult>,
}

impl std::fmt::Debug for FilterWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilterWorker")
            .field("generation", &self.generation)
            .field("file_filter", &self.file_filter)
            .field("grep_filter", &self.grep_filter)
            .field("jump_to_grep", &self.jump_to_grep)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkExport {
    path: String,
    old_line: Option<usize>,
    new_line: Option<usize>,
    body: String,
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn osc52_clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

pub(crate) fn write_osc52_clipboard<W: Write>(writer: &mut W, text: &str) -> io::Result<()> {
    writer.write_all(osc52_clipboard_sequence(text).as_bytes())?;
    writer.flush()
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(BASE64[(first >> 2) as usize] as char);
        encoded.push(BASE64[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(BASE64[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(BASE64[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if character <= '\u{1f}' => {
                out.push_str("\\u00");
                let value = character as u8;
                out.push(hex_digit(value >> 4));
                out.push(hex_digit(value & 0x0f));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => '0',
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorReloadRequest {
    pub(crate) path: PathBuf,
    pub(crate) pathspecs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64, u64),
}

impl FileFingerprint {
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec(), metadata.ino()),
        })
    }
}

pub(crate) fn file_changed_since(path: &Path, before: Option<FileFingerprint>) -> bool {
    let after = FileFingerprint::read(path);
    match (before, after) {
        (Some(before), Some(after)) => before != after,
        (None, None) => false,
        _ => true,
    }
}

pub(crate) fn is_plain_char_key(key: KeyEvent, character: char) -> bool {
    key.code == KeyCode::Char(character)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(crate) fn show_rev_from_options(options: &DiffOptions) -> Option<String> {
    match &options.source {
        DiffSource::Show(rev) if !rev.is_empty() => Some(rev.clone()),
        _ => None,
    }
}

pub(crate) fn diff_choice_for_options(options: &DiffOptions) -> Option<DiffChoice> {
    if is_review_options(options) {
        return Some(DiffChoice::Review);
    }

    match (&options.source, options.scope) {
        (DiffSource::Worktree, DiffScope::All) => Some(DiffChoice::All),
        (DiffSource::Worktree, DiffScope::Unstaged) => Some(DiffChoice::Unstaged),
        (DiffSource::Worktree, DiffScope::Staged) => Some(DiffChoice::Staged),
        (DiffSource::Base(_) | DiffSource::Branch { .. }, DiffScope::All) => {
            Some(DiffChoice::Branch)
        }
        (DiffSource::Show(_), DiffScope::All) => Some(DiffChoice::Show),
        _ => None,
    }
}

pub(crate) fn cacheable_diff_options(options: &DiffOptions) -> bool {
    !options.stat
        && !matches!(
            options.source,
            DiffSource::Patch(_) | DiffSource::Difftool { .. }
        )
}

pub(crate) fn next_context_expansion(expansion: DiffContextExpansion) -> DiffContextExpansion {
    match expansion {
        DiffContextExpansion::Lines(lines) if lines < 20 => DiffContextExpansion::Lines(20),
        DiffContextExpansion::Lines(lines) if lines < 50 => DiffContextExpansion::Lines(50),
        DiffContextExpansion::Lines(_) => DiffContextExpansion::Full,
        DiffContextExpansion::Full => DiffContextExpansion::Lines(5),
    }
}

pub(crate) fn previous_context_expansion(expansion: DiffContextExpansion) -> DiffContextExpansion {
    match expansion {
        DiffContextExpansion::Lines(lines) if lines <= 5 => DiffContextExpansion::Full,
        DiffContextExpansion::Lines(lines) if lines <= 20 => DiffContextExpansion::Lines(5),
        DiffContextExpansion::Lines(lines) if lines <= 50 => DiffContextExpansion::Lines(20),
        DiffContextExpansion::Lines(_) => DiffContextExpansion::Lines(50),
        DiffContextExpansion::Full => DiffContextExpansion::Lines(50),
    }
}

fn cycle_choice<T: Copy + PartialEq>(choices: &[T], current: T, delta: isize) -> T {
    let current = choices
        .iter()
        .position(|candidate| *candidate == current)
        .unwrap_or_default();
    let next = choice_index_with_delta(current, choices.len(), delta);
    choices[next]
}

fn choice_index_with_delta(current: usize, len: usize, delta: isize) -> usize {
    let len = len as isize;
    (current as isize + delta.rem_euclid(len)).rem_euclid(len) as usize
}

fn cycle_ordered_choice<T: Copy + Ord>(choices: &[T], current: T, delta: isize) -> T {
    if choices.is_empty() || delta == 0 {
        return current;
    }

    if let Some(current) = choices.iter().position(|candidate| *candidate == current) {
        let next = choice_index_with_delta(current, choices.len(), delta);
        return choices[next];
    }

    // Numeric settings can have valid custom values that are not listed in the
    // menu. Snap those to the next choice in the requested direction, or to the
    // nearest boundary when the custom value is outside the choice range.
    let first_step = if delta > 0 {
        choices
            .iter()
            .position(|candidate| *candidate > current)
            .unwrap_or(choices.len() - 1)
    } else {
        choices
            .iter()
            .rposition(|candidate| *candidate < current)
            .unwrap_or_default()
    };
    let remaining_delta = delta - delta.signum();
    let next = choice_index_with_delta(first_step, choices.len(), remaining_delta);
    choices[next]
}

fn next_notification_mode(mode: NotificationMode) -> NotificationMode {
    match mode {
        NotificationMode::Default => NotificationMode::Debug,
        NotificationMode::Debug => NotificationMode::Default,
    }
}

fn next_toast_corner(corner: ToastCorner, delta: isize) -> ToastCorner {
    cycle_choice(TOAST_CORNER_CHOICES, corner, delta)
}

fn next_toast_timeout_ms(timeout_ms: u64, delta: isize) -> u64 {
    cycle_ordered_choice(TOAST_TIMEOUT_CHOICES_MS, timeout_ms, delta)
}

fn next_toast_max_visible(max_visible: usize, delta: isize) -> usize {
    cycle_ordered_choice(TOAST_MAX_VISIBLE_CHOICES, max_visible, delta)
}

pub(crate) fn context_expansion_label(expansion: DiffContextExpansion) -> String {
    match expansion {
        DiffContextExpansion::Lines(lines) => lines.to_string(),
        DiffContextExpansion::Full => "full".to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorSchemeChoice {
    Custom,
    System,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
    GruvboxDark,
    GruvboxLight,
    GithubDark,
    GithubDarkHighContrast,
    GithubLight,
    GithubLightHighContrast,
    Tokyonight,
}

pub(crate) const COLOR_SCHEME_CHOICES: &[ColorSchemeChoice] = &[
    ColorSchemeChoice::System,
    ColorSchemeChoice::CatppuccinLatte,
    ColorSchemeChoice::CatppuccinFrappe,
    ColorSchemeChoice::CatppuccinMacchiato,
    ColorSchemeChoice::CatppuccinMocha,
    ColorSchemeChoice::GruvboxDark,
    ColorSchemeChoice::GruvboxLight,
    ColorSchemeChoice::GithubDark,
    ColorSchemeChoice::GithubDarkHighContrast,
    ColorSchemeChoice::GithubLight,
    ColorSchemeChoice::GithubLightHighContrast,
    ColorSchemeChoice::Tokyonight,
];

pub(crate) fn color_scheme_label(choice: ColorSchemeChoice) -> &'static str {
    match choice {
        ColorSchemeChoice::Custom => "custom",
        ColorSchemeChoice::System => "system",
        ColorSchemeChoice::CatppuccinLatte => "catppuccin-latte",
        ColorSchemeChoice::CatppuccinFrappe => "catppuccin-frappe",
        ColorSchemeChoice::CatppuccinMacchiato => "catppuccin-macchiato",
        ColorSchemeChoice::CatppuccinMocha => "catppuccin-mocha",
        ColorSchemeChoice::GruvboxDark => "gruvbox-dark",
        ColorSchemeChoice::GruvboxLight => "gruvbox-light",
        ColorSchemeChoice::GithubDark => "github-dark",
        ColorSchemeChoice::GithubDarkHighContrast => "github-dark-high-contrast",
        ColorSchemeChoice::GithubLight => "github-light",
        ColorSchemeChoice::GithubLightHighContrast => "github-light-high-contrast",
        ColorSchemeChoice::Tokyonight => "tokyonight",
    }
}

pub(crate) fn color_scheme_from_config(config: &SyntaxThemeConfig) -> ColorSchemeChoice {
    match config.source {
        SyntaxThemeSource::Ansi | SyntaxThemeSource::Base16 => ColorSchemeChoice::Custom,
        SyntaxThemeSource::Builtin => color_scheme_from_name(config.name.as_deref()),
    }
}

pub(crate) fn color_scheme_from_name(name: Option<&str>) -> ColorSchemeChoice {
    match name
        .unwrap_or("system")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "system" | "default" | "" => ColorSchemeChoice::System,
        "catppuccin-latte" | "latte" => ColorSchemeChoice::CatppuccinLatte,
        "catppuccin-frappe" | "frappe" => ColorSchemeChoice::CatppuccinFrappe,
        "catppuccin-macchiato" | "macchiato" => ColorSchemeChoice::CatppuccinMacchiato,
        "catppuccin" | "catppuccin-mocha" | "mocha" => ColorSchemeChoice::CatppuccinMocha,
        "gruvbox" | "gruvbox-dark" => ColorSchemeChoice::GruvboxDark,
        "gruvbox-light" => ColorSchemeChoice::GruvboxLight,
        "github" | "github-dark" => ColorSchemeChoice::GithubDark,
        "github-dark-high-contrast" | "github-high-contrast" => {
            ColorSchemeChoice::GithubDarkHighContrast
        }
        "github-light" => ColorSchemeChoice::GithubLight,
        "github-light-high-contrast" => ColorSchemeChoice::GithubLightHighContrast,
        "tokyonight" | "tokyo-night" | "tokyonight-night" => ColorSchemeChoice::Tokyonight,
        _ => ColorSchemeChoice::Custom,
    }
}

pub(crate) fn color_scheme_config(choice: ColorSchemeChoice) -> Option<SyntaxThemeConfig> {
    match choice {
        ColorSchemeChoice::Custom => None,
        choice => Some(SyntaxThemeConfig {
            source: SyntaxThemeSource::Builtin,
            name: Some(color_scheme_label(choice).to_owned()),
            path: None,
        }),
    }
}

pub(crate) fn layout_override_from_setting(setting: LayoutSetting) -> Option<DiffLayoutMode> {
    match setting {
        LayoutSetting::Dynamic => None,
        LayoutSetting::Split => Some(DiffLayoutMode::Split),
        LayoutSetting::Unified => Some(DiffLayoutMode::Unified),
    }
}

pub(crate) fn layout_setting_from_override(
    layout_override: Option<DiffLayoutMode>,
) -> LayoutSetting {
    match layout_override {
        Some(DiffLayoutMode::Split) => LayoutSetting::Split,
        Some(DiffLayoutMode::Unified) => LayoutSetting::Unified,
        None => LayoutSetting::Dynamic,
    }
}

pub(crate) fn layout_setting_label(layout: LayoutSetting) -> &'static str {
    match layout {
        LayoutSetting::Dynamic => "dynamic",
        LayoutSetting::Split => "split",
        LayoutSetting::Unified => "unified",
    }
}

pub(crate) fn next_layout_setting(setting: LayoutSetting, delta: isize) -> LayoutSetting {
    let settings = [
        LayoutSetting::Dynamic,
        LayoutSetting::Split,
        LayoutSetting::Unified,
    ];
    let current = settings
        .iter()
        .position(|candidate| *candidate == setting)
        .unwrap_or_default();
    let next = (current as isize + delta).rem_euclid(settings.len() as isize) as usize;
    settings[next]
}

pub(crate) fn diff_cache_entry(options: DiffOptions, changeset: Changeset) -> DiffCacheEntry {
    let search_index = Arc::new(DiffSearchIndex::new(&changeset));
    let max_line_width = search_index.max_line_width();
    let total_stats = changeset.stats();
    let context_expansions = HashMap::new();
    let unified_model = UiModel::new(&changeset, DiffLayoutMode::Unified, &context_expansions);
    let split_model = UiModel::new(&changeset, DiffLayoutMode::Split, &context_expansions);
    DiffCacheEntry {
        options,
        changeset,
        search_index,
        total_stats,
        max_line_width,
        unified_model,
        split_model,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Default)]
pub(crate) struct MouseScroll {
    pub(crate) last_tick: Option<Instant>,
    pub(crate) direction: Option<MouseScrollDirection>,
    pub(crate) intervals: Vec<Duration>,
    pub(crate) pending_lines: f64,
    pub(crate) pending_hunk_focus_ticks: isize,
}

impl MouseScroll {
    pub(crate) fn scroll_delta(&mut self, direction: MouseScrollDirection, now: Instant) -> isize {
        let multiplier = self.multiplier(direction, now);
        self.pending_lines += multiplier;
        let lines = self.pending_lines.trunc() as isize;
        self.pending_lines -= lines as f64;

        match direction {
            MouseScrollDirection::Down => lines,
            MouseScrollDirection::Up => -lines,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.last_tick = None;
        self.direction = None;
        self.intervals.clear();
        self.pending_lines = 0.0;
        self.pending_hunk_focus_ticks = 0;
    }

    pub(crate) fn reset_hunk_focus_ticks(&mut self) {
        self.pending_hunk_focus_ticks = 0;
    }

    pub(crate) fn hunk_focus_delta(&mut self, direction: MouseScrollDirection) -> isize {
        match direction {
            MouseScrollDirection::Down => self.pending_hunk_focus_ticks += 1,
            MouseScrollDirection::Up => self.pending_hunk_focus_ticks -= 1,
        }

        if self.pending_hunk_focus_ticks >= MOUSE_HUNK_FOCUS_SCROLL_TICKS {
            self.pending_hunk_focus_ticks -= MOUSE_HUNK_FOCUS_SCROLL_TICKS;
            1
        } else if self.pending_hunk_focus_ticks <= -MOUSE_HUNK_FOCUS_SCROLL_TICKS {
            self.pending_hunk_focus_ticks += MOUSE_HUNK_FOCUS_SCROLL_TICKS;
            -1
        } else {
            0
        }
    }

    pub(crate) fn multiplier(&mut self, direction: MouseScrollDirection, now: Instant) -> f64 {
        let Some(last_tick) = self.last_tick else {
            self.start_streak(direction, now);
            return 1.0;
        };

        let elapsed = now.saturating_duration_since(last_tick);
        if self.direction != Some(direction) || elapsed > MOUSE_SCROLL_STREAK_TIMEOUT {
            self.start_streak(direction, now);
            return 1.0;
        }

        if elapsed < MOUSE_SCROLL_MIN_TICK_INTERVAL {
            return 1.0;
        }

        self.last_tick = Some(now);
        self.intervals.push(elapsed);
        if self.intervals.len() > MOUSE_SCROLL_HISTORY_SIZE {
            self.intervals.remove(0);
        }

        let average_interval_ms = self
            .intervals
            .iter()
            .map(|interval| interval.as_secs_f64() * 1000.0)
            .sum::<f64>()
            / self.intervals.len() as f64;
        let velocity = MOUSE_SCROLL_REFERENCE_INTERVAL_MS / average_interval_ms;
        let multiplier =
            1.0 + MOUSE_SCROLL_ACCEL_A * ((velocity / MOUSE_SCROLL_ACCEL_TAU).exp() - 1.0);

        multiplier.min(MOUSE_SCROLL_MAX_MULTIPLIER)
    }

    pub(crate) fn start_streak(&mut self, direction: MouseScrollDirection, now: Instant) {
        self.last_tick = Some(now);
        self.direction = Some(direction);
        self.intervals.clear();
        self.pending_lines = 0.0;
        self.pending_hunk_focus_ticks = 0;
    }
}

#[derive(Debug)]
pub(crate) struct PendingDiffLoad {
    pub(crate) options: DiffOptions,
    pub(crate) error_prefix: String,
    pub(crate) refresh_branch_metadata: bool,
    pub(crate) rx: oneshot::Receiver<MarkResult<Changeset>>,
}

#[derive(Debug)]
pub(crate) struct PendingReviewLoad {
    pub(crate) error_prefix: String,
    pub(crate) rx: oneshot::Receiver<MarkResult<(DiffOptions, Changeset)>>,
}

#[derive(Debug)]
pub(crate) struct DiffCacheEntry {
    pub(crate) options: DiffOptions,
    pub(crate) changeset: Changeset,
    pub(crate) search_index: Arc<DiffSearchIndex>,
    pub(crate) total_stats: DiffStats,
    pub(crate) max_line_width: usize,
    pub(crate) unified_model: UiModel,
    pub(crate) split_model: UiModel,
}

#[derive(Debug)]
pub(crate) struct PendingDiffPrefetch {
    pub(crate) options: DiffOptions,
    pub(crate) rx: oneshot::Receiver<MarkResult<Changeset>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyntaxStartupMode {
    Config,
    Disabled,
    Languages(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkFocusSearch {
    FirstVisible,
    NearestTo(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderedDiffRow {
    viewport_row: usize,
    model_row: usize,
}

#[derive(Debug)]
struct AnnotationScratchFile {
    _dir: TempDir,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OptionsMenuItem {
    Layout,
    LiveReload,
    #[allow(dead_code)] // Legacy settings persistence path; hidden from the options menu.
    ContextExpansion,
    SyntaxHighlighting,
    LineWrapping,
    ColorScheme,
    NotificationMode,
    ToastCorner,
    ToastTimeout,
    ToastMaxVisible,
}

pub(crate) const COMMON_OPTIONS_MENU_ITEMS: &[OptionsMenuItem] = &[
    OptionsMenuItem::Layout,
    OptionsMenuItem::LiveReload,
    OptionsMenuItem::SyntaxHighlighting,
    OptionsMenuItem::LineWrapping,
    OptionsMenuItem::ColorScheme,
    OptionsMenuItem::NotificationMode,
    OptionsMenuItem::ToastCorner,
    OptionsMenuItem::ToastTimeout,
    OptionsMenuItem::ToastMaxVisible,
];

pub(crate) fn option_label(item: OptionsMenuItem) -> &'static str {
    match item {
        OptionsMenuItem::Layout => "Layout",
        OptionsMenuItem::LiveReload => "Live reload",
        OptionsMenuItem::ContextExpansion => "Context expand",
        OptionsMenuItem::SyntaxHighlighting => "Syntax highlighting",
        OptionsMenuItem::LineWrapping => "Line wrapping",
        OptionsMenuItem::ColorScheme => "Colorscheme",
        OptionsMenuItem::NotificationMode => "Notification mode",
        OptionsMenuItem::ToastCorner => "Toast corner",
        OptionsMenuItem::ToastTimeout => "Toast timeout",
        OptionsMenuItem::ToastMaxVisible => "Toast max visible",
    }
}

fn checkbox(enabled: bool) -> String {
    if enabled { "[x]" } else { "[ ]" }.to_owned()
}

fn on_off_search(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_owned()
}

pub(crate) fn notification_mode_label(mode: NotificationMode) -> &'static str {
    match mode {
        NotificationMode::Default => "default",
        NotificationMode::Debug => "debug",
    }
}

fn notification_mode_config_value(mode: NotificationMode) -> toml::Value {
    toml::Value::String(notification_mode_label(mode).to_owned())
}

pub(crate) fn toast_corner_label(corner: ToastCorner) -> &'static str {
    match corner {
        ToastCorner::TopLeft => "top-left",
        ToastCorner::TopRight => "top-right",
        ToastCorner::BottomLeft => "bottom-left",
        ToastCorner::BottomRight => "bottom-right",
    }
}

fn toast_corner_config_value(corner: ToastCorner) -> toml::Value {
    toml::Value::String(toast_corner_label(corner).to_owned())
}

fn toast_timeout_label(timeout_ms: u64) -> String {
    if timeout_ms % 1_000 == 0 {
        format!("{}s", timeout_ms / 1_000)
    } else {
        format!("{timeout_ms}ms")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OptionsDraft {
    pub(crate) layout: LayoutSetting,
    pub(crate) live_updates_enabled: bool,
    pub(crate) context_expansion: DiffContextExpansion,
    pub(crate) syntax_enabled: bool,
    pub(crate) line_wrapping: bool,
    pub(crate) color_scheme: ColorSchemeChoice,
    pub(crate) notification_mode: NotificationMode,
    pub(crate) toast_corner: ToastCorner,
    pub(crate) toast_timeout_ms: u64,
    pub(crate) toast_max_visible: usize,
}

const TOAST_TIMEOUT_CHOICES_MS: &[u64] = &[500, 1_000, 1_500, 2_500, 5_000, 10_000];
const TOAST_MAX_VISIBLE_CHOICES: &[usize] = &[1, 2, 3, 4, 5];
const TOAST_CORNER_CHOICES: &[ToastCorner] = &[
    ToastCorner::TopRight,
    ToastCorner::BottomRight,
    ToastCorner::BottomLeft,
    ToastCorner::TopLeft,
];

pub(crate) fn persist_options_menu_draft_to_path(
    path: &Path,
    draft: OptionsDraft,
    changed_item: OptionsMenuItem,
) -> MarkResult<()> {
    let mut table = if path.exists() {
        let contents = fs::read_to_string(path)?;
        if contents.trim().is_empty() {
            toml::Table::new()
        } else {
            contents.parse::<toml::Table>().map_err(|error| {
                MarkError::Usage(format!("failed to parse {}: {error}", path.display()))
            })?
        }
    } else {
        toml::Table::new()
    };

    match changed_item {
        OptionsMenuItem::Layout => {
            table.insert(
                "layout".to_owned(),
                toml::Value::String(layout_setting_label(draft.layout).to_owned()),
            );
        }
        OptionsMenuItem::LiveReload => {
            table.insert(
                "live_reload".to_owned(),
                toml::Value::Boolean(draft.live_updates_enabled),
            );
        }
        OptionsMenuItem::ContextExpansion => {
            let mut diff = match table.remove("diff") {
                Some(toml::Value::Table(diff)) => diff,
                Some(_) => {
                    return Err(MarkError::Usage(format!(
                        "failed to update {}: diff must be a table",
                        path.display()
                    )));
                }
                None => toml::Table::new(),
            };
            diff.remove("context_expansion");
            diff.remove("context_lines");
            diff.remove("expand_context");
            diff.insert(
                "context_expand".to_owned(),
                context_expansion_config_value(draft.context_expansion),
            );
            table.insert("diff".to_owned(), toml::Value::Table(diff));
        }
        OptionsMenuItem::SyntaxHighlighting => {
            table.insert(
                "syntax_highlighting".to_owned(),
                toml::Value::Boolean(draft.syntax_enabled),
            );
        }
        OptionsMenuItem::LineWrapping => {
            table.remove("word_wrap");
            table.remove("wrap_lines");
            table.insert(
                "line_wrapping".to_owned(),
                toml::Value::Boolean(draft.line_wrapping),
            );
        }
        OptionsMenuItem::ColorScheme => {
            if let Some(config) = color_scheme_config(draft.color_scheme)
                && config.source == SyntaxThemeSource::Builtin
                && let Some(name) = config.name
            {
                table.insert("colorscheme".to_owned(), toml::Value::String(name));
            }
        }
        OptionsMenuItem::NotificationMode
        | OptionsMenuItem::ToastCorner
        | OptionsMenuItem::ToastTimeout
        | OptionsMenuItem::ToastMaxVisible => {
            let mut notifications = match table.remove("notifications") {
                Some(toml::Value::Table(notifications)) => notifications,
                Some(_) => {
                    return Err(MarkError::Usage(format!(
                        "failed to update {}: notifications must be a table",
                        path.display()
                    )));
                }
                None => toml::Table::new(),
            };
            match changed_item {
                OptionsMenuItem::NotificationMode => {
                    notifications.insert(
                        "mode".to_owned(),
                        notification_mode_config_value(draft.notification_mode),
                    );
                }
                OptionsMenuItem::ToastCorner => {
                    notifications.insert(
                        "corner".to_owned(),
                        toast_corner_config_value(draft.toast_corner),
                    );
                }
                OptionsMenuItem::ToastTimeout => {
                    notifications.insert(
                        "timeout_ms".to_owned(),
                        toml::Value::Integer(draft.toast_timeout_ms as i64),
                    );
                }
                OptionsMenuItem::ToastMaxVisible => {
                    notifications.insert(
                        "max_visible".to_owned(),
                        toml::Value::Integer(draft.toast_max_visible as i64),
                    );
                }
                _ => {}
            }
            table.insert(
                "notifications".to_owned(),
                toml::Value::Table(notifications),
            );
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(&table)
        .map_err(|error| MarkError::Usage(format!("failed to serialize settings: {error}")))?;
    fs::write(path, contents)?;
    Ok(())
}

fn context_expansion_config_value(expansion: DiffContextExpansion) -> toml::Value {
    match expansion {
        DiffContextExpansion::Lines(lines) => toml::Value::Integer(lines as i64),
        DiffContextExpansion::Full => toml::Value::String("full".to_owned()),
    }
}

#[derive(Debug)]
struct WrappedVisualLayout {
    layout: DiffLayoutMode,
    viewport_width: usize,
    model_rows: usize,
    model_rows_ptr: usize,
    row_starts: Vec<usize>,
    total_rows: usize,
}

impl WrappedVisualLayout {
    fn matches(&self, app: &DiffApp) -> bool {
        self.layout == app.layout
            && self.viewport_width == app.viewport_width
            && self.model_rows == app.model.len()
            && self.model_rows_ptr == app.model.rows.as_ptr() as usize
    }
}

#[derive(Debug)]
pub(crate) struct DiffApp {
    pub(crate) options: DiffOptions,
    pub(crate) base_changeset: Changeset,
    pub(crate) changeset: Changeset,
    pub(crate) search_index: Arc<DiffSearchIndex>,
    pub(crate) total_stats: DiffStats,
    pub(crate) stats: DiffStats,
    pub(crate) model: UiModel,
    pub(crate) layout: DiffLayoutMode,
    pub(crate) layout_override: Option<DiffLayoutMode>,
    pub(crate) scroll: usize,
    pub(crate) horizontal_scroll: usize,
    pub(crate) line_wrapping: bool,
    pub(crate) viewport_rows: usize,
    pub(crate) viewport_width: usize,
    pub(crate) max_line_width: usize,
    wrapped_visual_layout: RefCell<Option<WrappedVisualLayout>>,
    pub(crate) manual_hunk_focus: Option<(usize, usize)>,
    pub(crate) selected_file: usize,
    pub(crate) file_sidebar_open: bool,
    pub(crate) file_sidebar_scroll: usize,
    pub(crate) file_sidebar_width: Option<u16>,
    pub(crate) file_sidebar_render_width: u16,
    pub(crate) file_sidebar_resizing: bool,
    pub(crate) rendered_diff_menu_area: Option<Rect>,
    pub(crate) rendered_branch_menu_area: Option<Rect>,
    pub(crate) rendered_commit_menu_area: Option<Rect>,
    pub(crate) rendered_review_input_area: Option<Rect>,
    pub(crate) rendered_color_scheme_picker_area: Option<Rect>,
    pub(crate) rendered_diff_area: Option<Rect>,
    pub(crate) mouse_hover: Option<(u16, u16)>,
    pub(crate) annotations: AnnotationStore,
    pub(crate) annotation_draft: Option<AnnotationDraft>,
    pub(crate) key_prefix_pending: Option<KeyPress>,
    pub(crate) help_menu_open: bool,
    pub(crate) help_menu_input: String,
    pub(crate) help_menu_input_cursor: usize,
    pub(crate) help_menu_scroll: usize,
    pub(crate) help_menu_visible_rows: usize,
    terminal_area: Rect,
    pub(crate) diff_menu_open: bool,
    pub(crate) diff_menu: SelectorState,
    pub(crate) review_input_open: bool,
    pub(crate) review_input: String,
    pub(crate) review_input_cursor: usize,
    pub(crate) options_menu_open: bool,
    pub(crate) options_menu: SelectorState,
    pub(crate) options_menu_draft: OptionsDraft,
    pub(crate) color_scheme_picker_open: bool,
    pub(crate) color_scheme_picker: SelectorState,
    pub(crate) color_scheme_preview_original: Option<(ColorSchemeChoice, DiffTheme)>,
    pub(crate) filter_input: Option<DiffFilterKind>,
    pub(crate) file_filter: String,
    pub(crate) file_filter_input: String,
    pub(crate) file_filter_input_cursor: usize,
    pub(crate) grep_filter: String,
    pub(crate) grep_filter_input: String,
    pub(crate) grep_filter_input_cursor: usize,
    pub(crate) grep_matches: Vec<usize>,
    pub(crate) grep_matches_truncated: bool,
    pub(crate) selected_grep_match: Option<usize>,
    pub(crate) branch_menu_open: Option<BranchMenu>,
    pub(crate) branch_menu: SelectorState,
    pub(crate) branch_base: Option<String>,
    pub(crate) branch_head: Option<String>,
    pub(crate) current_head: Option<String>,
    pub(crate) comparison_branches: Vec<String>,
    pub(crate) commit_menu_open: bool,
    pub(crate) commit_menu: SelectorState,
    pub(crate) show_rev: Option<String>,
    pub(crate) comparison_commits: Vec<GitCommit>,
    pub(crate) live_diff_failed_options: Option<DiffOptions>,
    pub(crate) editor_reload: Option<EditorReloadWorker>,
    pub(crate) pending_editor_reload: Option<EditorReloadRequest>,
    pub(crate) post_editor_quit_key_ignore_until: Option<Instant>,
    pub(crate) live_updates_allowed: bool,
    pub(crate) live_updates_enabled: bool,
    pub(crate) live_reload_invalidated: bool,
    pub(crate) live_reload_pending: bool,
    pub(crate) pending_diff_load: Option<PendingDiffLoad>,
    pub(crate) pending_review_load: Option<PendingReviewLoad>,
    pub(crate) diff_cache: Vec<DiffCacheEntry>,
    pub(crate) pending_diff_prefetch: Option<PendingDiffPrefetch>,
    pub(crate) diff_prefetch_queue: VecDeque<DiffOptions>,
    pub(crate) diff_prefetch_started: bool,
    pub(crate) filter_generation: u64,
    pub(crate) pending_filter_apply: Option<PendingFilterApply>,
    pub(crate) filter_worker: Option<FilterWorker>,
    pub(crate) filter_searching: bool,
    pub(crate) error_log: Option<String>,
    pub(crate) error_log_height: u16,
    pub(crate) error_log_resizing: bool,
    pub(crate) rendered_error_log_separator_row: Option<u16>,
    pub(crate) toasts: Toasts,
    pub(crate) mouse_scroll: MouseScroll,
    pub(crate) keymap: Keymap,
    pub(crate) theme: DiffTheme,
    pub(crate) color_scheme: ColorSchemeChoice,
    pub(crate) theme_color_overrides: ColorOverrides,
    pub(crate) theme_transparent_background: bool,
    pub(crate) settings_persistence_enabled: bool,
    #[cfg(test)]
    pub(crate) last_persisted_options_menu_draft: Option<(OptionsDraft, OptionsMenuItem)>,
    pub(crate) context_expansions: HashMap<ContextKey, usize>,
    pub(crate) context_cache: HashMap<ContextSourceKey, ContextSourceEntry>,
    pub(crate) syntax_settings: SyntaxSettings,
    pub(crate) syntax_startup_mode: SyntaxStartupMode,
    pub(crate) syntax_limits: SyntaxLimits,
    pub(crate) syntax: Option<SyntaxRuntime>,
    pub(crate) inline_cache: LruCache<InlineHunkKey, InlineHunkEmphasisCache>,
    pub(crate) generation: u64,
    pub(crate) terminal_clear_requested: bool,
    pub(crate) dirty: bool,
}

pub(crate) fn load_syntax_settings_for_diff(
    load_user_settings: bool,
) -> (SyntaxSettings, Option<String>) {
    if !load_user_settings {
        return (SyntaxSettings::default(), None);
    }

    syntax_settings_for_diff(mark_syntax::load_settings())
}

pub(crate) fn syntax_settings_for_diff(
    result: MarkResult<SyntaxSettings>,
) -> (SyntaxSettings, Option<String>) {
    match result {
        Ok(settings) => (settings, None),
        Err(error) => (
            SyntaxSettings::default(),
            Some(format!("syntax settings ignored: {error}")),
        ),
    }
}

fn push_startup_error_log(error_log: &mut Option<String>, message: impl Into<String>) {
    match error_log {
        Some(error_log) => {
            error_log.push('\n');
            error_log.push_str(&message.into());
        }
        None => *error_log = Some(message.into()),
    }
}

pub(crate) fn syntax_runtime_for_diff(
    result: MarkResult<Option<SyntaxRuntime>>,
    error_log: &mut Option<String>,
) -> Option<SyntaxRuntime> {
    match result {
        Ok(syntax) => syntax,
        Err(error) => {
            push_startup_error_log(error_log, format!("syntax disabled: {error}"));
            None
        }
    }
}

pub(crate) fn load_keymap_for_diff(load_user_settings: bool) -> (Keymap, Option<String>) {
    if !load_user_settings {
        return (Keymap::default(), None);
    }

    match Keymap::load() {
        Ok(keymap) => (keymap, None),
        Err(error) => (Keymap::default(), Some(format!("keymap ignored: {error}"))),
    }
}

pub(crate) fn layout_override_from_settings(
    settings: &SyntaxSettings,
    honor_settings_layout: bool,
) -> Option<DiffLayoutMode> {
    honor_settings_layout
        .then_some(settings.layout)
        .flatten()
        .and_then(layout_override_from_setting)
}

mod annotations;
mod choices;
mod context;
mod diff_load;
mod editor_reload;
mod error_log;
mod file_sidebar;
mod filters;
mod help;
mod input;
mod marks;
mod menus;
mod mouse;
mod navigation;
mod runner;
mod syntax;

#[cfg(test)]
pub(crate) use runner::{drain_live_reloads, handle_event};
pub(crate) use runner::{is_quit_key, run_loop, sync_live_diff};

impl DiffApp {
    #[cfg(test)]
    pub(crate) fn new(options: DiffOptions, changeset: Changeset, layout: DiffLayoutMode) -> Self {
        Self::new_with_syntax(options, changeset, layout, SyntaxStartupMode::Config)
    }

    pub(crate) fn new_with_syntax(
        options: DiffOptions,
        changeset: Changeset,
        layout: DiffLayoutMode,
        syntax_mode: SyntaxStartupMode,
    ) -> Self {
        Self::new_with_syntax_and_layout_settings(options, changeset, layout, syntax_mode, true)
    }

    pub(crate) fn new_with_explicit_layout(
        options: DiffOptions,
        changeset: Changeset,
        layout: DiffLayoutMode,
        syntax_mode: SyntaxStartupMode,
    ) -> Self {
        let mut app = Self::new_with_syntax_and_layout_settings(
            options,
            changeset,
            layout,
            syntax_mode,
            false,
        );
        app.layout_override = Some(layout);
        app.options_menu_draft.layout = layout_setting_from_override(app.layout_override);
        app
    }

    fn new_with_syntax_and_layout_settings(
        options: DiffOptions,
        changeset: Changeset,
        mut layout: DiffLayoutMode,
        syntax_mode: SyntaxStartupMode,
        honor_settings_layout: bool,
    ) -> Self {
        let context_expansions = HashMap::new();
        let context_cache = HashMap::new();
        let load_user_settings = matches!(
            syntax_mode,
            SyntaxStartupMode::Config | SyntaxStartupMode::Disabled
        ) && !cfg!(test);
        let (settings, mut startup_error_log) = load_syntax_settings_for_diff(load_user_settings);
        let layout_override = layout_override_from_settings(&settings, honor_settings_layout);
        if let Some(setting_layout) = layout_override {
            layout = setting_layout;
        }
        let model = UiModel::new(&changeset, layout, &context_expansions);
        let search_index = Arc::new(DiffSearchIndex::new(&changeset));
        let manual_hunk_focus = model
            .hunk_start_rows
            .first()
            .and_then(|row| model.row(*row).and_then(UiRow::hunk_key));
        let stats = changeset.stats();
        let total_stats = stats.clone();
        let branch_base = default_branch_base(&options, &changeset.repo);
        let current_head = current_head_label(&changeset.repo);
        let branch_head = branch_head_from_options(&options, current_head.as_deref());
        let comparison_branches = comparison_branches(
            &changeset.repo,
            &[
                current_head.as_deref(),
                branch_head.as_deref(),
                branch_base.as_deref(),
            ],
        );
        let show_rev = show_rev_from_options(&options);
        let comparison_commits = comparison_commits(&changeset.repo, show_rev.as_deref());
        let (keymap, keymap_notice) = load_keymap_for_diff(load_user_settings);
        if let Some(message) = keymap_notice {
            push_startup_error_log(&mut startup_error_log, message);
        }
        let mut color_scheme = color_scheme_from_config(&settings.theme);
        let theme = match diff_theme_from_config(&settings.theme).and_then(|theme| {
            theme
                .with_color_overrides(&settings.colors)
                .map(|theme| theme.with_transparent_background(settings.transparent_background))
        }) {
            Ok(theme) => theme.with_diff_settings(settings.diff),
            Err(error) => {
                push_startup_error_log(
                    &mut startup_error_log,
                    format!("syntax theme ignored: {error}"),
                );
                color_scheme = ColorSchemeChoice::System;
                DiffTheme::default()
                    .with_color_overrides(&settings.colors)
                    .unwrap_or_else(|_| DiffTheme::default())
                    .with_transparent_background(settings.transparent_background)
                    .with_diff_settings(settings.diff)
            }
        };
        let syntax_limits = settings.limits;
        let context_expansion = theme.diff.context_expansion;
        let theme_color_overrides = settings.colors.clone();
        let theme_transparent_background = settings.transparent_background;
        let syntax = match &syntax_mode {
            SyntaxStartupMode::Config if settings.syntax_highlighting => {
                syntax_runtime_for_diff(SyntaxRuntime::start(&settings), &mut startup_error_log)
            }
            SyntaxStartupMode::Config => None,
            SyntaxStartupMode::Disabled => None,
            SyntaxStartupMode::Languages(languages) => {
                SyntaxRuntime::start_with_languages(languages.clone(), syntax_limits)
            }
        };
        let max_line_width = search_index.max_line_width();
        Self {
            options,
            base_changeset: changeset.clone(),
            changeset,
            search_index,
            total_stats,
            stats,
            model,
            layout,
            layout_override,
            scroll: 0,
            horizontal_scroll: 0,
            line_wrapping: settings.line_wrapping,
            viewport_rows: 1,
            viewport_width: 1,
            max_line_width,
            wrapped_visual_layout: RefCell::new(None),
            manual_hunk_focus,
            selected_file: 0,
            file_sidebar_open: false,
            file_sidebar_scroll: 0,
            file_sidebar_width: None,
            file_sidebar_render_width: 0,
            file_sidebar_resizing: false,
            rendered_diff_menu_area: None,
            rendered_branch_menu_area: None,
            rendered_commit_menu_area: None,
            rendered_review_input_area: None,
            rendered_color_scheme_picker_area: None,
            rendered_diff_area: None,
            mouse_hover: None,
            annotations: AnnotationStore::default(),
            annotation_draft: None,
            key_prefix_pending: None,
            help_menu_open: false,
            help_menu_input: String::new(),
            help_menu_input_cursor: 0,
            help_menu_scroll: 0,
            help_menu_visible_rows: 1,
            terminal_area: Rect::default(),
            diff_menu_open: false,
            diff_menu: SelectorState::default(),
            review_input_open: false,
            review_input: String::new(),
            review_input_cursor: 0,
            options_menu_open: false,
            options_menu: SelectorState::default(),
            options_menu_draft: OptionsDraft {
                layout: layout_setting_from_override(layout_override),
                live_updates_enabled: settings.live_reload,
                context_expansion,
                syntax_enabled: syntax.is_some(),
                line_wrapping: settings.line_wrapping,
                color_scheme,
                notification_mode: settings.notifications.mode,
                toast_corner: settings.notifications.corner,
                toast_timeout_ms: settings.notifications.timeout_ms,
                toast_max_visible: settings.notifications.max_visible,
            },
            color_scheme_picker_open: false,
            color_scheme_picker: SelectorState::default(),
            color_scheme_preview_original: None,
            filter_input: None,
            file_filter: String::new(),
            file_filter_input: String::new(),
            file_filter_input_cursor: 0,
            grep_filter: String::new(),
            grep_filter_input: String::new(),
            grep_filter_input_cursor: 0,
            grep_matches: Vec::new(),
            grep_matches_truncated: false,
            selected_grep_match: None,
            branch_menu_open: None,
            branch_menu: SelectorState::default(),
            branch_base,
            branch_head,
            current_head,
            comparison_branches,
            commit_menu_open: false,
            commit_menu: SelectorState::default(),
            show_rev,
            comparison_commits,
            live_diff_failed_options: None,
            editor_reload: None,
            pending_editor_reload: None,
            post_editor_quit_key_ignore_until: None,
            live_updates_allowed: true,
            live_updates_enabled: settings.live_reload,
            live_reload_invalidated: false,
            live_reload_pending: false,
            pending_diff_load: None,
            pending_review_load: None,
            diff_cache: Vec::new(),
            pending_diff_prefetch: None,
            diff_prefetch_queue: VecDeque::new(),
            diff_prefetch_started: false,
            filter_generation: 0,
            pending_filter_apply: None,
            filter_worker: None,
            filter_searching: false,
            error_log: startup_error_log,
            error_log_height: ERROR_LOG_DEFAULT_HEIGHT,
            error_log_resizing: false,
            rendered_error_log_separator_row: None,
            toasts: Toasts::new(settings.notifications),
            mouse_scroll: MouseScroll::default(),
            keymap,
            theme,
            color_scheme,
            theme_color_overrides,
            theme_transparent_background,
            settings_persistence_enabled: !cfg!(test),
            #[cfg(test)]
            last_persisted_options_menu_draft: None,
            context_expansions,
            context_cache,
            syntax_settings: settings,
            syntax_startup_mode: syntax_mode,
            syntax_limits,
            syntax,
            inline_cache: LruCache::new(MAX_INLINE_DIFF_CACHE_ENTRIES),
            generation: 0,
            terminal_clear_requested: false,
            dirty: true,
        }
    }

    pub(crate) fn event_poll(&self) -> Duration {
        let now = Instant::now();
        let mut poll = EVENT_POLL;
        if self.editor_reload.is_some() || self.pending_editor_reload.is_some() {
            poll = poll.min(EDITOR_RELOAD_POLL);
        }
        if self.filter_worker.is_some() {
            poll = poll.min(FILTER_WORKER_POLL);
        }
        if let Some(pending) = self.pending_filter_apply {
            poll = poll.min(pending.due_at.saturating_duration_since(now));
        }
        if self.pending_diff_prefetch.is_some() {
            poll = poll.min(DIFF_PREFETCH_POLL);
        }
        poll
    }

    pub(crate) fn ignore_post_editor_quit_key(&mut self, key: KeyEvent, now: Instant) -> bool {
        let Some(ignore_until) = self.post_editor_quit_key_ignore_until else {
            return false;
        };
        if now >= ignore_until {
            self.post_editor_quit_key_ignore_until = None;
            return false;
        }

        is_quit_key(key) || self.keymap.matches_single(GlobalAction::Quit, key)
    }

    pub(crate) fn set_terminal_area(&mut self, area: Rect) {
        if self.terminal_area != area {
            self.terminal_area = area;
            self.sync_help_menu_visible_rows();
        }
    }

    pub(crate) fn set_notice(&mut self, text: impl Into<String>) {
        if self.toasts.push(ToastLevel::Info, text) {
            self.dirty = true;
        }
    }

    pub(crate) fn set_success_notice(&mut self, text: impl Into<String>) {
        if self.toasts.push(ToastLevel::Success, text) {
            self.dirty = true;
        }
    }

    pub(crate) fn set_warning_notice(&mut self, text: impl Into<String>) {
        if self.toasts.push(ToastLevel::Warning, text) {
            self.dirty = true;
        }
    }

    pub(crate) fn set_blocked_notice(&mut self, text: impl Into<String>) {
        if self.toasts.push(ToastLevel::Error, text) {
            self.dirty = true;
        }
    }

    pub(crate) fn set_debug_notice(&mut self, text: impl Into<String>) {
        if self.toasts.push(ToastLevel::Debug, text) {
            self.dirty = true;
        }
    }

    pub(crate) fn expire_toasts(&mut self, now: Instant) {
        if self.toasts.expire(now) {
            self.dirty = true;
        }
    }

    pub(crate) fn debug_notifications_enabled(&self) -> bool {
        self.toasts.debug_enabled()
    }

    pub(crate) fn mark_live_reload_invalidated(&mut self) {
        self.invalidate_diff_cache();
        self.live_reload_invalidated = true;
    }

    pub(crate) fn mark_live_reload_pending(&mut self) {
        self.mark_live_reload_invalidated();
        self.live_reload_pending = true;
        self.dirty = true;
    }

    pub(crate) fn set_rendered_diff_area(&mut self, area: Rect) {
        if self.rendered_diff_area != Some(area) {
            self.clear_diff_mouse_hover();
        }
        self.rendered_diff_area = Some(area);
    }

    pub(crate) fn set_rendered_diff_menu_area(&mut self, area: Option<Rect>) {
        self.rendered_diff_menu_area = area.filter(|_| self.diff_menu_open);
    }

    pub(crate) fn set_rendered_branch_menu_area(&mut self, area: Option<Rect>) {
        self.rendered_branch_menu_area = area.filter(|_| self.branch_menu_open.is_some());
    }

    pub(crate) fn set_rendered_commit_menu_area(&mut self, area: Option<Rect>) {
        self.rendered_commit_menu_area = area.filter(|_| self.commit_menu_open);
    }

    pub(crate) fn set_rendered_review_input_area(&mut self, area: Option<Rect>) {
        self.rendered_review_input_area = area.filter(|_| self.review_input_open);
    }

    pub(crate) fn viewport_focus_row(&self) -> usize {
        if self.line_wrapping {
            let row_count = self.wrapped_visual_row_count();
            let focus_scroll = self.scroll.saturating_add(viewport_focus_offset(
                self.scroll,
                row_count,
                self.viewport_rows,
            ));
            return self
                .model_row_at_scroll(focus_scroll)
                .map(|(row, _)| row)
                .unwrap_or_else(|| self.model.len().saturating_sub(1));
        }

        self.scroll
            .saturating_add(viewport_focus_offset(
                self.scroll,
                self.model.len(),
                self.viewport_rows,
            ))
            .min(self.model.len().saturating_sub(1))
    }

    pub(crate) fn set_viewport_rows(&mut self, rows: usize) {
        let rows = rows.max(1);
        let previous_rows = self.viewport_rows;
        if previous_rows == rows {
            return;
        }

        let centered_grep_match_row = self.selected_grep_match_row().filter(|row| {
            let previous_centered_scroll = row
                .saturating_sub(viewport_center_offset(previous_rows))
                .min(max_scroll_for_viewport(self.model.len(), previous_rows));
            self.scroll == previous_centered_scroll
        });

        self.viewport_rows = rows;
        if let Some(row) = centered_grep_match_row {
            self.set_scroll_centered_on(row);
        } else {
            self.set_scroll(self.scroll);
        }
        self.clamp_file_sidebar_scroll(self.visible_file_sidebar_rows());
        self.ensure_annotation_draft_visible();
    }

    pub(crate) fn set_viewport_width(&mut self, width: usize) {
        let width = width.max(1);
        if self.viewport_width == width {
            return;
        }

        let wrapped_position = self
            .line_wrapping
            .then(|| self.model_row_at_scroll(self.scroll))
            .flatten();
        self.viewport_width = width;
        self.invalidate_wrapped_visual_layout();
        self.set_horizontal_scroll(self.horizontal_scroll);
        if let Some((row, row_offset)) = wrapped_position {
            let row_scroll = self.wrapped_visual_scroll_for_model_row(row);
            let row_height = self.wrapped_visual_height_for_model_row(row);
            self.set_scroll(
                row_scroll.saturating_add(row_offset.min(row_height.saturating_sub(1))),
            );
        } else {
            self.set_scroll(self.scroll);
        }
        self.ensure_annotation_draft_visible();
    }

    pub(crate) fn inline_ranges(
        &mut self,
        file: usize,
        hunk: usize,
        line: usize,
    ) -> Vec<InlineRange> {
        let key = InlineHunkKey {
            generation: self.generation,
            file,
            hunk,
        };
        if !self.inline_cache.contains_key(&key) {
            let cache = self
                .changeset
                .files
                .get(file)
                .and_then(|file_diff| file_diff.hunks.get(hunk))
                .map(|hunk_diff| InlineHunkEmphasisCache::new(&hunk_diff.lines))
                .unwrap_or_else(|| InlineHunkEmphasisCache::new(&[]));
            self.inline_cache.insert(key, cache);
        }

        let Some(lines) = self
            .changeset
            .files
            .get(file)
            .and_then(|file_diff| file_diff.hunks.get(hunk))
            .map(|hunk_diff| hunk_diff.lines.as_slice())
        else {
            return Vec::new();
        };

        self.inline_cache
            .get_mut(&key)
            .map(|hunk_emphasis| hunk_emphasis.ranges_for_line(lines, line))
            .unwrap_or_default()
    }

    pub(crate) fn next_hunk(&mut self) {
        if let Some(row) = self.model.next_hunk_row(self.hunk_navigation_anchor_row()) {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn previous_hunk(&mut self) {
        if let Some(row) = self
            .model
            .previous_hunk_row(self.hunk_navigation_anchor_row())
        {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn move_focused_hunk(&mut self, delta: isize) {
        let anchor = self.hunk_navigation_anchor_row();
        let next = if delta < 0 {
            self.model.previous_hunk_row(anchor)
        } else {
            self.model.next_hunk_row(anchor)
        };
        if let Some(row) = next {
            self.focus_hunk_row(row);
        }
    }

    pub(crate) fn hunk_navigation_anchor_row(&self) -> usize {
        if let Some((file, hunk)) = self.focused_hunk_for_viewport(self.viewport_rows)
            && let Some(row) = self.model.hunk_start_row(file, hunk)
        {
            return row;
        }

        self.viewport_focus_row()
    }

    pub(crate) fn focus_hunk_row(&mut self, row: usize) {
        let target_hunk = self.model.row(row).and_then(|row| row.hunk_key());
        let previous_hunk = self.manual_hunk_focus;
        self.clear_manual_hunk_focus();

        let Some((file, hunk)) = target_hunk else {
            self.set_scroll_centered_on(row);
            return;
        };

        self.set_scroll_focused_on_hunk(file, hunk);

        if let Some(row) = self.model.hunk_start_row(file, hunk)
            && self.model_row_rendered_at_scroll(self.scroll, self.viewport_rows, row)
        {
            let previous_file = self.selected_file;
            self.manual_hunk_focus = Some((file, hunk));
            self.selected_file = file;
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            if self.manual_hunk_focus != previous_hunk || self.selected_file != previous_file {
                self.dirty = true;
            }
        }
    }

    pub(crate) fn toggle_layout(&mut self) {
        let layout = self.layout.toggled();
        self.set_manual_layout(layout);
    }

    pub(crate) fn set_manual_layout(&mut self, layout: DiffLayoutMode) {
        self.layout_override = Some(layout);
        self.set_layout(layout);
    }

    pub(crate) fn set_layout_setting(&mut self, setting: LayoutSetting) {
        match layout_override_from_setting(setting) {
            Some(layout) => self.set_manual_layout(layout),
            None => {
                self.layout_override = None;
                self.set_layout(default_layout_for_width(
                    self.viewport_width.min(u16::MAX as usize) as u16,
                ));
            }
        }
    }

    pub(crate) fn apply_responsive_layout(&mut self, width: u16) {
        let horizontal_scroll = self.horizontal_scroll;
        self.set_viewport_width(width as usize);
        let responsive_layout = default_layout_for_width(width);
        let layout = self.layout_override.unwrap_or(responsive_layout);
        self.set_layout(layout);
        self.set_horizontal_scroll(horizontal_scroll);
        self.dirty = true;
    }

    pub(crate) fn set_layout(&mut self, layout: DiffLayoutMode) {
        if self.layout == layout {
            return;
        }

        self.layout = layout;
        let search_result = self.search_index.search_with_grep_match_limit(
            &self.file_filter,
            &self.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_model(&search_result.visible_files, HunkFocusModelBehavior::Clear);
        self.grep_matches = grep_match_rows(&self.model, &search_result.grep_matches);
        self.grep_matches_truncated = search_result.grep_matches_truncated;
        self.selected_grep_match = None;
        self.set_horizontal_scroll(self.horizontal_scroll);
        let scroll = self
            .model
            .file_start_row(self.selected_file)
            .map(|row| self.scroll_for_model_row(row))
            .unwrap_or_default();
        self.set_scroll(scroll);
        self.sync_grep_match_selection_to_scroll();
        self.ensure_annotation_draft_visible();
        self.dirty = true;
    }

    pub(crate) fn reload(&mut self) -> MarkResult<()> {
        self.invalidate_diff_cache();
        self.start_uncached_diff_load(self.options.clone(), "reload failed");
        Ok(())
    }

    pub(crate) fn replace_changeset(&mut self, changeset: Changeset) {
        self.invalidate_diff_cache();
        self.cache_loaded_diff(self.options.clone(), changeset.clone());
        self.replace_loaded_diff(self.options.clone(), changeset);
    }

    pub(crate) fn replace_path_changeset(&mut self, path: &Path, path_changeset: Changeset) {
        self.invalidate_diff_cache();
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.selected_file);

        splice_diff_files_for_path(
            &mut self.changeset.files,
            path,
            path_changeset.files.clone(),
        );
        splice_diff_files_for_path(&mut self.base_changeset.files, path, path_changeset.files);
        self.total_stats = self.changeset.stats();
        self.context_expansions.clear();
        self.context_cache.clear();
        self.generation = self.generation.wrapping_add(1);
        self.inline_cache.clear();
        self.search_index = Arc::new(DiffSearchIndex::new(&self.changeset));
        self.pending_filter_apply = None;
        self.filter_worker = None;
        self.filter_searching = false;
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.clear(self.generation);
        }
        let search_result = self.search_index.search_with_grep_match_limit(
            &self.file_filter,
            &self.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            false,
            HunkFocusModelBehavior::Clear,
        );
        self.store_current_diff_cache();
        self.dirty = true;
    }

    pub(crate) fn replace_cached_diff(
        &mut self,
        options: DiffOptions,
        cached: DiffCacheEntry,
        refresh_branch_metadata: bool,
    ) {
        let DiffCacheEntry {
            changeset,
            search_index,
            total_stats,
            max_line_width,
            unified_model,
            split_model,
            ..
        } = cached;
        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.selected_file);

        let previous_branch_base = self.branch_base.clone();
        let previous_branch_head = self.branch_head.clone();
        let previous_repo = self.changeset.repo.clone();
        self.options = options;
        self.live_reload_invalidated = false;
        self.live_reload_pending = false;
        if !refresh_branch_metadata && previous_repo == changeset.repo {
            self.branch_base = branch_base_from_options(&self.options).or(previous_branch_base);
            self.branch_head =
                branch_head_from_options(&self.options, self.current_head.as_deref())
                    .or(previous_branch_head)
                    .or_else(|| self.current_head.clone());
            for branch in [
                self.current_head.clone(),
                self.branch_head.clone(),
                self.branch_base.clone(),
            ]
            .into_iter()
            .flatten()
            {
                if !self
                    .comparison_branches
                    .iter()
                    .any(|candidate| candidate == &branch)
                {
                    self.comparison_branches.push(branch);
                }
            }
        } else {
            self.current_head = current_head_label(&changeset.repo);
            self.branch_base = branch_base_from_options(&self.options)
                .or(previous_branch_base)
                .or_else(|| default_branch_base(&self.options, &changeset.repo));
            self.branch_head =
                branch_head_from_options(&self.options, self.current_head.as_deref())
                    .or(previous_branch_head)
                    .or_else(|| self.current_head.clone());
            self.comparison_branches = comparison_branches(
                &changeset.repo,
                &[
                    self.current_head.as_deref(),
                    self.branch_head.as_deref(),
                    self.branch_base.as_deref(),
                ],
            );
        }
        self.branch_menu.scroll = self.branch_menu.scroll.min(self.max_branch_menu_scroll());
        self.show_rev = show_rev_from_options(&self.options);
        self.comparison_commits =
            comparison_commits(&self.changeset.repo, self.show_rev.as_deref());
        self.commit_menu.scroll = self
            .commit_menu
            .scroll
            .min(self.max_commit_menu_scroll_for_rows(self.commit_menu_rows()));
        self.total_stats = total_stats;
        self.base_changeset = changeset.clone();
        self.changeset = changeset;
        self.search_index = search_index;
        self.context_expansions.clear();
        self.context_cache.clear();
        self.generation = self.generation.wrapping_add(1);
        self.inline_cache.clear();
        self.pending_filter_apply = None;
        self.filter_worker = None;
        self.filter_searching = false;
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.clear(self.generation);
        }

        if self.filters_active() {
            let search_result = self.search_index.search_with_grep_match_limit(
                &self.file_filter,
                &self.grep_filter,
                MAX_LIVE_GREP_MATCHES,
            );
            self.replace_visible_files(
                search_result,
                selected_path,
                relative_scroll,
                false,
                HunkFocusModelBehavior::Clear,
            );
        } else {
            self.stats = self.total_stats.clone();
            self.max_line_width = max_line_width;
            self.model = match self.layout {
                DiffLayoutMode::Split => split_model,
                DiffLayoutMode::Unified => unified_model,
            };
            self.invalidate_wrapped_visual_layout();
            self.reanchor_annotation_draft();
            self.manual_hunk_focus = None;
            self.selected_file = selected_path
                .and_then(|path| {
                    self.changeset
                        .files
                        .iter()
                        .position(|file| file.display_path() == path)
                })
                .unwrap_or(0);
            self.grep_matches.clear();
            self.grep_matches_truncated = false;
            self.selected_grep_match = None;

            let scroll = self
                .model
                .file_start_row(self.selected_file)
                .map(|start| {
                    self.scroll_for_model_row(start)
                        .saturating_add(relative_scroll)
                })
                .unwrap_or_default();
            self.set_scroll_with_grep_sync(scroll, true, HunkFocusScrollBehavior::ClearOnScroll);
            self.set_horizontal_scroll(self.horizontal_scroll);
            self.ensure_file_sidebar_selection_visible(self.visible_file_sidebar_rows());
            self.ensure_annotation_draft_visible();
            self.dirty = true;
        }
    }

    pub(crate) fn replace_loaded_diff(&mut self, options: DiffOptions, changeset: Changeset) {
        let options_changed = self.options != options;
        if !options_changed && self.base_changeset == changeset {
            if self.live_reload_invalidated || self.live_reload_pending {
                self.live_reload_invalidated = false;
                self.live_reload_pending = false;
            }
            self.dirty = true;
            return;
        }

        let selected_path = self
            .changeset
            .files
            .get(self.selected_file)
            .map(|file| file.display_path().to_owned());
        let relative_scroll = self.relative_scroll_from_file_start(self.selected_file);

        let previous_branch_base = self.branch_base.clone();
        let previous_branch_head = self.branch_head.clone();
        self.options = options;
        self.live_reload_invalidated = false;
        self.live_reload_pending = false;
        self.current_head = current_head_label(&changeset.repo);
        self.branch_base = branch_base_from_options(&self.options)
            .or(previous_branch_base)
            .or_else(|| default_branch_base(&self.options, &changeset.repo));
        self.branch_head = branch_head_from_options(&self.options, self.current_head.as_deref())
            .or(previous_branch_head)
            .or_else(|| self.current_head.clone());
        self.comparison_branches = comparison_branches(
            &changeset.repo,
            &[
                self.current_head.as_deref(),
                self.branch_head.as_deref(),
                self.branch_base.as_deref(),
            ],
        );
        self.branch_menu.scroll = self.branch_menu.scroll.min(self.max_branch_menu_scroll());
        self.show_rev = show_rev_from_options(&self.options);
        self.comparison_commits = comparison_commits(&changeset.repo, self.show_rev.as_deref());
        self.commit_menu.scroll = self
            .commit_menu
            .scroll
            .min(self.max_commit_menu_scroll_for_rows(self.commit_menu_rows()));
        self.total_stats = changeset.stats();
        self.base_changeset = changeset.clone();
        self.changeset = changeset;
        self.search_index = Arc::new(DiffSearchIndex::new(&self.changeset));
        self.context_expansions.clear();
        self.context_cache.clear();
        self.generation = self.generation.wrapping_add(1);
        self.inline_cache.clear();
        self.pending_filter_apply = None;
        self.filter_worker = None;
        self.filter_searching = false;
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.clear(self.generation);
        }
        let search_result = self.search_index.search_with_grep_match_limit(
            &self.file_filter,
            &self.grep_filter,
            MAX_LIVE_GREP_MATCHES,
        );
        self.replace_visible_files(
            search_result,
            selected_path,
            relative_scroll,
            false,
            HunkFocusModelBehavior::Clear,
        );
        self.dirty = true;
    }
}

pub(crate) fn max_scroll_for_viewport(row_count: usize, viewport_rows: usize) -> usize {
    row_count.saturating_sub(viewport_rows.max(1))
}

fn max_scroll_for_annotated_viewport(
    row_count: usize,
    viewport_rows: usize,
    mut annotation_blocks: Vec<(usize, usize)>,
) -> usize {
    if row_count == 0 {
        return 0;
    }

    annotation_blocks.retain(|(anchor, height)| *anchor < row_count && *height > 0);
    if annotation_blocks.is_empty() {
        return max_scroll_for_viewport(row_count, viewport_rows);
    }

    annotation_blocks.sort_unstable_by_key(|(anchor, _)| *anchor);
    let mut merged_blocks: Vec<(usize, usize)> = Vec::with_capacity(annotation_blocks.len());
    for (anchor, height) in annotation_blocks {
        if let Some((last_anchor, last_height)) = merged_blocks.last_mut()
            && *last_anchor == anchor
        {
            *last_height = last_height.saturating_add(height);
            continue;
        }
        merged_blocks.push((anchor, height));
    }

    let annotation_rows = merged_blocks
        .iter()
        .fold(0usize, |total, (_, height)| total.saturating_add(*height));
    let target_rendered_scroll = row_count
        .saturating_add(annotation_rows)
        .saturating_sub(viewport_rows.max(1));
    if target_rendered_scroll == 0 {
        return 0;
    }

    // `scroll` is expressed in diff visual rows, while annotations add rendered
    // rows after their anchors. Project the last rendered viewport start back to
    // the first diff visual row at or after that rendered position; if that
    // position lands inside an annotation, scrolling to the next diff row reveals
    // rows hidden by the annotation block. If there is no next diff row, fall back
    // to the final anchor so an oversized trailing annotation remains reachable.
    let mut annotation_rows_before = 0usize;
    let mut first_row_in_range = 0usize;
    for (anchor, height) in merged_blocks {
        let candidate = target_rendered_scroll.saturating_sub(annotation_rows_before);
        if candidate <= anchor {
            let projected_scroll = candidate.max(first_row_in_range).min(row_count - 1);
            return projected_scroll;
        }

        annotation_rows_before = annotation_rows_before.saturating_add(height);
        first_row_in_range = anchor.saturating_add(1).min(row_count);
    }

    if first_row_in_range < row_count {
        let projected_scroll = target_rendered_scroll
            .saturating_sub(annotation_rows_before)
            .max(first_row_in_range)
            .min(row_count - 1);
        return projected_scroll;
    }

    row_count - 1
}

fn annotation_scroll_for_block(
    anchor_visual_scroll: usize,
    block_height: usize,
    viewport_rows: usize,
) -> usize {
    anchor_visual_scroll
        .saturating_add(1)
        .saturating_add(block_height)
        .saturating_sub(viewport_rows.max(1))
        .min(anchor_visual_scroll)
}

pub(crate) fn viewport_center_offset(viewport_rows: usize) -> usize {
    viewport_rows.saturating_sub(1) / 2
}

pub(crate) fn viewport_focus_offset(
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if row_count == 0 {
        return 0;
    }

    let viewport_rows = viewport_rows.max(1);
    let visible_rows = viewport_rows.min(row_count);
    let center = viewport_center_offset(visible_rows);
    if row_count <= viewport_rows {
        return center;
    }

    let bottom = visible_rows.saturating_sub(1);
    let max_scroll = max_scroll_for_viewport(row_count, viewport_rows);
    let scroll = scroll.min(max_scroll);
    let distance_to_end = max_scroll.saturating_sub(scroll);
    let top_ramp = scroll.min(center);
    let bottom_ramp = bottom.saturating_sub(distance_to_end);

    top_ramp.max(bottom_ramp).min(bottom)
}

fn hunk_focus_row_range(
    model: &UiModel,
    file: usize,
    hunk: usize,
) -> Option<(Range<usize>, usize)> {
    let mut range = model.hunk_row_range(file, hunk)?;
    let hunk_start = range.start;

    while range.start > 0
        && model
            .row(range.start - 1)
            .is_some_and(row_extends_hunk_focus_before)
    {
        range.start -= 1;
    }

    while range.end < model.len()
        && model
            .row(range.end)
            .is_some_and(row_extends_hunk_focus_after)
    {
        range.end += 1;
    }

    Some((range, hunk_start))
}

fn row_extends_hunk_focus_before(row: UiRow) -> bool {
    matches!(
        row,
        UiRow::FileHeader(_)
            | UiRow::Collapsed { .. }
            | UiRow::ContextLine { .. }
            | UiRow::ContextHide { .. }
    )
}

fn row_extends_hunk_focus_after(row: UiRow) -> bool {
    matches!(
        row,
        UiRow::Collapsed { .. } | UiRow::ContextLine { .. } | UiRow::ContextHide { .. }
    )
}

fn find_rendered_diff_row_outward<T>(
    rendered_rows: &[RenderedDiffRow],
    focus_viewport_row: usize,
    mut find: impl FnMut(RenderedDiffRow) -> Option<T>,
) -> Option<T> {
    let max_viewport_row = rendered_rows.iter().map(|row| row.viewport_row).max()?;
    let max_distance = focus_viewport_row.max(max_viewport_row.saturating_sub(focus_viewport_row));

    for distance in 0..=max_distance {
        if let Some(viewport_row) = focus_viewport_row.checked_add(distance)
            && viewport_row <= max_viewport_row
            && let Some(rendered_row) = rendered_rows
                .iter()
                .find(|row| row.viewport_row == viewport_row)
            && let Some(found) = find(*rendered_row)
        {
            return Some(found);
        }
        if distance > 0
            && let Some(viewport_row) = focus_viewport_row.checked_sub(distance)
            && let Some(rendered_row) = rendered_rows
                .iter()
                .find(|row| row.viewport_row == viewport_row)
            && let Some(found) = find(*rendered_row)
        {
            return Some(found);
        }
    }

    None
}

pub(crate) fn repo_relative_path(repo: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(repo).ok().map(Path::to_path_buf)
}

pub(crate) fn editor_reload_request_for_file(
    file: &mark_diff::DiffFile,
) -> Option<EditorReloadRequest> {
    let path = PathBuf::from(file.new_path.as_deref()?);
    let mut pathspecs = Vec::new();
    push_unique_pathspec(&mut pathspecs, file.old_path.as_deref());
    push_unique_pathspec(&mut pathspecs, file.new_path.as_deref());

    Some(EditorReloadRequest { path, pathspecs })
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
    file.old_path.as_deref() == Some(path.as_str())
        || file.new_path.as_deref() == Some(path.as_str())
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
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch_width == 0 {
            continue;
        }

        if line_width == content_width
            || (line_width > 0 && line_width.saturating_add(ch_width) > content_width)
        {
            visit(consumed_width);
            line_width = 0;
        }

        line_width = line_width.saturating_add(ch_width);
        consumed_width = consumed_width.saturating_add(ch_width);
    }
}

fn normalize_annotation_editor_contents(contents: &str) -> String {
    contents
        .replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_owned()
}

fn create_annotation_scratch_file(contents: &str) -> MarkResult<AnnotationScratchFile> {
    let prefix = format!("mark-annotations-{}-", process::id());
    let dir = tempfile::Builder::new().prefix(&prefix).tempdir()?;
    #[cfg(unix)]
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;

    let path = dir.path().join("annotation.md");
    write_annotation_scratch_file(&path, contents)?;

    Ok(AnnotationScratchFile { _dir: dir, path })
}

#[cfg(unix)]
fn write_annotation_scratch_file(path: &Path, contents: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_annotation_scratch_file(path: &Path, contents: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(test)]
mod annotation_editor_tests {
    use super::*;

    #[test]
    fn annotation_editor_contents_normalize_crlf_line_endings() {
        assert_eq!(
            normalize_annotation_editor_contents("first\r\nsecond\r\n"),
            "first\nsecond"
        );
        assert_eq!(
            normalize_annotation_editor_contents("first\r\nsecond"),
            "first\nsecond"
        );
        assert_eq!(
            normalize_annotation_editor_contents("first\r\n\r\nsecond\r\n"),
            "first\n\nsecond"
        );
        assert_eq!(
            normalize_annotation_editor_contents("trailing spaces  \r\n"),
            "trailing spaces  "
        );
    }
}

#[cfg(all(test, unix))]
mod annotation_scratch_tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn annotation_scratch_file_is_private_and_removed_on_drop() {
        let scratch = create_annotation_scratch_file("secret").expect("scratch file");
        let dir = scratch.path.parent().expect("scratch dir").to_path_buf();

        assert_eq!(
            fs::metadata(&dir)
                .expect("scratch dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&scratch.path)
                .expect("scratch file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(&scratch.path).expect("scratch contents"),
            "secret"
        );

        drop(scratch);

        assert!(!dir.exists());
    }
}
