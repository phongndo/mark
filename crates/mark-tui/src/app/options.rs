use crate::controls::DiffLayoutMode;
use crate::theme::DecorationPreference;
use mark_core::{MarkError, MarkResult};
use mark_syntax::{
    DiffContextExpansion, LayoutSetting, NotificationMode, SyntaxThemeConfig, ToastCorner,
};
use std::fs;
use std::path::Path;

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

pub(crate) fn next_notification_mode(mode: NotificationMode) -> NotificationMode {
    match mode {
        NotificationMode::Default => NotificationMode::Debug,
        NotificationMode::Debug => NotificationMode::Default,
    }
}

pub(crate) fn next_toast_corner(corner: ToastCorner, delta: isize) -> ToastCorner {
    cycle_choice(TOAST_CORNER_CHOICES, corner, delta)
}

pub(crate) fn next_toast_timeout_ms(timeout_ms: u64, delta: isize) -> u64 {
    cycle_ordered_choice(TOAST_TIMEOUT_CHOICES_MS, timeout_ms, delta)
}

pub(crate) fn next_toast_max_visible(max_visible: usize, delta: isize) -> usize {
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
    match config {
        SyntaxThemeConfig::Builtin { name } => color_scheme_from_name(name.as_deref()),
        SyntaxThemeConfig::Ansi
        | SyntaxThemeConfig::Base16 { .. }
        | SyntaxThemeConfig::Base16MissingPath => ColorSchemeChoice::Custom,
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
        choice => Some(SyntaxThemeConfig::Builtin {
            name: Some(color_scheme_label(choice).to_owned()),
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

pub(crate) fn decoration_preference_label(preference: DecorationPreference) -> &'static str {
    match preference {
        DecorationPreference::Auto => "auto",
        DecorationPreference::Fancy => "fancy",
        DecorationPreference::Minimal => "minimal",
    }
}

pub(crate) fn next_decoration_preference(
    preference: DecorationPreference,
    delta: isize,
) -> DecorationPreference {
    let choices = [
        DecorationPreference::Auto,
        DecorationPreference::Fancy,
        DecorationPreference::Minimal,
    ];
    cycle_choice(&choices, preference, delta)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OptionsMenuItem {
    Layout,
    LiveReload,
    #[allow(dead_code)]
    ContextExpansion,
    SyntaxHighlighting,
    LineWrapping,
    Decorations,
    ColorScheme,
    NotificationMode,
    ToastCorner,
    ToastTimeout,
    ToastMaxVisible,
}

// Construct the legacy-only variant so unused-option linting stays meaningful.
// It remains session-only like every non-colorscheme TUI option.
const _: OptionsMenuItem = OptionsMenuItem::ContextExpansion;

pub(crate) const COMMON_OPTIONS_MENU_ITEMS: &[OptionsMenuItem] = &[
    // Review-view controls: most likely to vary per session.
    OptionsMenuItem::Layout,
    OptionsMenuItem::LineWrapping,
    OptionsMenuItem::SyntaxHighlighting,
    // Presentation controls: visual preferences and terminal fit.
    OptionsMenuItem::Decorations,
    OptionsMenuItem::ColorScheme,
    // Review workflow controls.
    OptionsMenuItem::LiveReload,
    // Feedback controls: least commonly changed during review.
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
        OptionsMenuItem::Decorations => "Decorations",
        OptionsMenuItem::ColorScheme => "Colorscheme",
        OptionsMenuItem::NotificationMode => "Notification mode",
        OptionsMenuItem::ToastCorner => "Toast corner",
        OptionsMenuItem::ToastTimeout => "Toast timeout",
        OptionsMenuItem::ToastMaxVisible => "Toast max visible",
    }
}

pub(crate) fn checkbox(enabled: bool) -> String {
    if enabled { "[x]" } else { "[ ]" }.to_owned()
}

pub(crate) fn on_off_search(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_owned()
}

pub(crate) fn notification_mode_label(mode: NotificationMode) -> &'static str {
    match mode {
        NotificationMode::Default => "default",
        NotificationMode::Debug => "debug",
    }
}

pub(crate) fn toast_corner_label(corner: ToastCorner) -> &'static str {
    match corner {
        ToastCorner::TopLeft => "top-left",
        ToastCorner::TopRight => "top-right",
        ToastCorner::BottomLeft => "bottom-left",
        ToastCorner::BottomRight => "bottom-right",
    }
}

pub(crate) fn toast_timeout_label(timeout_ms: u64) -> String {
    if timeout_ms.is_multiple_of(1_000) {
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
    pub(crate) decorations: DecorationPreference,
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
    if changed_item != OptionsMenuItem::ColorScheme {
        return Ok(());
    }

    let Some(SyntaxThemeConfig::Builtin { name: Some(name) }) =
        color_scheme_config(draft.color_scheme)
    else {
        return Ok(());
    };

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

    table.insert("colorscheme".to_owned(), toml::Value::String(name));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(&table)
        .map_err(|error| MarkError::Usage(format!("failed to serialize settings: {error}")))?;
    fs::write(path, contents)?;
    Ok(())
}
