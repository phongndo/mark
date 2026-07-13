use crate::controls::INPUT_CURSOR;
use mark_core::MarkResult;
use mark_diff::DiffLineKind;
use mark_syntax::{
    ColorOverrides, DecorationSettings, DiffGutterBackground, DiffSettings, SyntaxRuleOverride,
    theme::{BuiltinTextMateTheme, TextMateTheme},
};
use ratatui::prelude::Color;
use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{OnceLock, RwLock},
};

mod benchmark;
mod colorscheme;
mod constants;
mod help;
mod palettes;

pub use benchmark::{
    DiffBenchmarkOptions, DiffBenchmarkReport, SyntaxBenchmarkReport, SyntaxLatencyBucket,
};
pub(crate) use colorscheme::{Base16Scheme, RgbColor, config_color, diff_theme_from_config};
#[cfg(test)]
pub(crate) use colorscheme::{builtin_diff_theme, parse_base16_scheme};
pub(crate) use constants::*;
pub(crate) use help::{HELP_MENU_ROWS, HelpMenuKey, HelpMenuRow};
pub(crate) use palettes::SyntaxPalette;

pub(crate) fn line_gutter_fg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match kind {
        DiffLineKind::Addition => theme.addition_fg,
        DiffLineKind::Deletion => theme.deletion_fg,
        DiffLineKind::Context | DiffLineKind::Meta => theme.foreground,
    }
}

pub(crate) fn line_gutter_bg(kind: DiffLineKind, theme: DiffTheme) -> Color {
    match (theme.diff.gutter_background, kind) {
        (DiffGutterBackground::Delta, DiffLineKind::Addition) => theme.addition_gutter_bg,
        (DiffGutterBackground::Delta, DiffLineKind::Deletion) => theme.deletion_gutter_bg,
        _ => theme.gutter_bg,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DecorationPreference {
    #[default]
    Auto,
    Fancy,
    Minimal,
}

impl From<mark_syntax::DecorationSetting> for DecorationPreference {
    fn from(setting: mark_syntax::DecorationSetting) -> Self {
        match setting {
            mark_syntax::DecorationSetting::Auto => Self::Auto,
            mark_syntax::DecorationSetting::Fancy => Self::Fancy,
            mark_syntax::DecorationSetting::Minimal => Self::Minimal,
        }
    }
}

impl From<DecorationSettings> for DecorationPreference {
    fn from(settings: DecorationSettings) -> Self {
        Self::from(settings.mode)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DecorationMode {
    #[default]
    Fancy,
    Minimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecorationStyle {
    pub(crate) mode: DecorationMode,
    pub(crate) empty_fill: bool,
    pub(crate) no_borders: bool,
}

impl Default for DecorationStyle {
    fn default() -> Self {
        Self {
            mode: DecorationMode::Fancy,
            empty_fill: DecorationSettings::default().empty_fill,
            no_borders: DecorationSettings::default().no_borders,
        }
    }
}

impl DecorationStyle {
    pub(crate) fn is_fancy(self) -> bool {
        self.mode == DecorationMode::Fancy
    }

    pub(crate) fn show_borders(self) -> bool {
        self.is_fancy() && !self.no_borders
    }

    pub(crate) fn show_empty_fill(self) -> bool {
        self.is_fancy() && self.empty_fill
    }

    pub(crate) fn with_mode(mut self, mode: DecorationMode) -> Self {
        self.mode = mode;
        self
    }

    pub(crate) fn diff_indicator(self) -> &'static str {
        if self.is_fancy() { DIFF_INDICATOR } else { " " }
    }

    pub(crate) fn horizontal_rule(self) -> &'static str {
        if self.is_fancy() { "─" } else { " " }
    }

    pub(crate) fn scrollbar_track(self) -> Option<&'static str> {
        self.is_fancy().then_some("│")
    }

    pub(crate) fn scrollbar_thumb(self) -> &'static str {
        if self.is_fancy() { "┃" } else { " " }
    }

    pub(crate) fn submenu_indicator(self) -> &'static str {
        if self.is_fancy() { "›" } else { "" }
    }

    pub(crate) fn dropdown_indicator(self) -> &'static str {
        if self.is_fancy() { "▾" } else { "" }
    }

    pub(crate) fn comparison_separator(self) -> &'static str {
        if self.is_fancy() { " → " } else { " to " }
    }

    pub(crate) fn current_branch_marker(self) -> &'static str {
        if self.is_fancy() { "●" } else { "*" }
    }

    pub(crate) fn base_branch_marker(self) -> &'static str {
        if self.is_fancy() { "⌂" } else { "base" }
    }

    pub(crate) fn commit_subject_separator(self) -> &'static str {
        if self.is_fancy() { " · " } else { " - " }
    }

    pub(crate) fn ellipsis(self) -> &'static str {
        if self.is_fancy() { "…" } else { "..." }
    }

    pub(crate) fn input_cursor(self) -> &'static str {
        if self.is_fancy() { INPUT_CURSOR } else { "_" }
    }
}

pub(crate) fn decoration_preference_from_env() -> Option<DecorationPreference> {
    if env::var_os("MARK_ASCII").is_some() {
        return Some(DecorationPreference::Minimal);
    }

    let value = env::var_os("MARK_DECORATIONS")?;
    let value = value.to_string_lossy().trim().to_ascii_lowercase();
    match value.as_str() {
        "auto" | "" => Some(DecorationPreference::Auto),
        "fancy" | "rich" => Some(DecorationPreference::Fancy),
        "minimal" | "plain" | "ascii" => Some(DecorationPreference::Minimal),
        _ => None,
    }
}

pub(crate) fn resolve_decoration_mode(preference: DecorationPreference) -> DecorationMode {
    match preference {
        DecorationPreference::Fancy => DecorationMode::Fancy,
        DecorationPreference::Minimal => DecorationMode::Minimal,
        DecorationPreference::Auto => auto_decoration_mode(),
    }
}

pub(crate) fn resolve_decoration_style(settings: DecorationSettings) -> DecorationStyle {
    DecorationStyle {
        mode: resolve_decoration_mode(DecorationPreference::from(settings.mode)),
        empty_fill: settings.empty_fill,
        no_borders: settings.no_borders,
    }
}

fn auto_decoration_mode() -> DecorationMode {
    if cfg!(test) {
        return DecorationMode::Fancy;
    }
    if env_value_eq("TERM", "dumb") || !locale_is_utf8() {
        DecorationMode::Minimal
    } else {
        DecorationMode::Fancy
    }
}

fn env_value_eq(name: &str, expected: &str) -> bool {
    env::var_os(name).is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case(expected))
}

fn locale_is_utf8() -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| env::var_os(name).filter(|value| !value.is_empty()));
    locale_env_is_utf8(locale.as_deref())
}

fn locale_env_is_utf8(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    })
}

#[cfg(test)]
mod tests {
    use super::{TextMateEngineMode, locale_env_is_utf8};
    use std::ffi::OsStr;

    #[test]
    fn locale_env_requires_present_utf8_locale() {
        assert!(!locale_env_is_utf8(None));
        assert!(!locale_env_is_utf8(Some(OsStr::new("C"))));
        assert!(locale_env_is_utf8(Some(OsStr::new("en_US.UTF-8"))));
        assert!(locale_env_is_utf8(Some(OsStr::new("C.UTF8"))));
    }

    #[test]
    fn textmate_engine_mode_parses_rollout_values() {
        assert_eq!(
            TextMateEngineMode::parse(Some(OsStr::new("coarse"))),
            TextMateEngineMode::Coarse
        );
        assert_eq!(
            TextMateEngineMode::parse(Some(OsStr::new("compare"))),
            TextMateEngineMode::Compare
        );
        assert_eq!(
            TextMateEngineMode::parse(Some(OsStr::new("exact"))),
            TextMateEngineMode::Exact
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffTheme {
    pub(crate) foreground: Color,
    pub(crate) foreground_overridden: bool,
    pub(crate) background: Color,
    pub(crate) header: Color,
    pub(crate) file: Color,
    pub(crate) hunk: Color,
    pub(crate) notice: Color,
    pub(crate) cursor: Color,
    pub(crate) cursor_line_bg: Color,
    pub(crate) muted: Color,
    pub(crate) gutter_bg: Color,
    pub(crate) empty_diff: Color,
    pub(crate) search_match_fg: Color,
    pub(crate) search_match_bg: Color,
    pub(crate) statusline_fg: Color,
    pub(crate) statusline_bg: Color,
    pub(crate) statusline_accent_fg: Color,
    pub(crate) statusline_accent_bg: Color,
    pub(crate) statusline_info_fg: Color,
    pub(crate) statusline_info_bg: Color,
    pub(crate) addition_fg: Color,
    pub(crate) addition_gutter_bg: Color,
    pub(crate) addition_bg: Color,
    pub(crate) addition_inline_bg: Color,
    pub(crate) deletion_fg: Color,
    pub(crate) deletion_gutter_bg: Color,
    pub(crate) deletion_bg: Color,
    pub(crate) deletion_inline_bg: Color,
    pub(crate) transparent_background: bool,
    pub(crate) decorations: DecorationStyle,
    pub(crate) diff: DiffSettings,
    pub(crate) syntax: SyntaxPalette,
    pub(crate) syntax_overrides: SyntaxPalette,
    pub(crate) exact_syntax: Option<&'static TextMateTheme>,
    pub(crate) scope_overrides: Option<u64>,
}

impl Default for DiffTheme {
    fn default() -> Self {
        Self::system()
    }
}

impl DiffTheme {
    pub(crate) fn system() -> Self {
        let base = RgbColor::new(0x11, 0x13, 0x15);
        let green = RgbColor::new(0x88, 0xd3, 0x9b);
        let red = RgbColor::new(0xf0, 0xa0, 0xa0);
        Self {
            foreground: Color::Reset,
            foreground_overridden: false,
            background: Color::Reset,
            header: Color::Reset,
            file: Color::Reset,
            hunk: Color::Indexed(13),
            notice: green.color(),
            cursor: Color::Reset,
            cursor_line_bg: Color::Indexed(237),
            muted: Color::Rgb(0x7d, 0x87, 0x94),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Rgb(0x3d, 0x42, 0x49),
            search_match_fg: Color::Indexed(0),
            search_match_bg: Color::Indexed(3),
            statusline_fg: Color::Reset,
            statusline_bg: Color::Reset,
            statusline_accent_fg: Color::Indexed(0),
            statusline_accent_bg: Color::Indexed(13),
            statusline_info_fg: Color::Reset,
            statusline_info_bg: Color::Indexed(0),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.12).color(),
            addition_bg: Color::Rgb(0x1f, 0x30, 0x25),
            addition_inline_bg: base.blend(green, 0.28).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.12).color(),
            deletion_bg: Color::Rgb(0x37, 0x25, 0x26),
            deletion_inline_bg: base.blend(red, 0.28).color(),
            transparent_background: false,
            decorations: DecorationStyle::default(),
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::ansi(),
            syntax_overrides: SyntaxPalette::empty(),
            exact_syntax: None,
            scope_overrides: None,
        }
    }

    pub(crate) fn ansi() -> Self {
        Self {
            foreground: Color::Reset,
            foreground_overridden: false,
            background: Color::Reset,
            header: Color::Indexed(15),
            file: Color::Indexed(15),
            hunk: Color::Indexed(13),
            notice: Color::Indexed(2),
            cursor: Color::Indexed(15),
            cursor_line_bg: Color::Indexed(237),
            muted: Color::Indexed(8),
            gutter_bg: Color::Indexed(0),
            empty_diff: Color::Indexed(8),
            search_match_fg: Color::Indexed(0),
            search_match_bg: Color::Indexed(3),
            statusline_fg: Color::Indexed(15),
            statusline_bg: Color::Indexed(0),
            statusline_accent_fg: Color::Indexed(0),
            statusline_accent_bg: Color::Indexed(13),
            statusline_info_fg: Color::Indexed(15),
            statusline_info_bg: Color::Indexed(8),
            addition_fg: Color::Indexed(2),
            addition_gutter_bg: Color::Indexed(0),
            addition_bg: Color::Reset,
            addition_inline_bg: Color::Indexed(22),
            deletion_fg: Color::Indexed(1),
            deletion_gutter_bg: Color::Indexed(0),
            deletion_bg: Color::Reset,
            deletion_inline_bg: Color::Indexed(52),
            transparent_background: false,
            decorations: DecorationStyle::default(),
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::ansi(),
            syntax_overrides: SyntaxPalette::empty(),
            exact_syntax: None,
            scope_overrides: None,
        }
    }

    pub(crate) fn tokyonight() -> Self {
        let base = RgbColor::new(0x1a, 0x1b, 0x26);
        let green = RgbColor::new(0x9e, 0xce, 0x6a);
        let red = RgbColor::new(0xf7, 0x76, 0x8e);
        Self {
            foreground: Color::Rgb(0xc0, 0xca, 0xf5),
            foreground_overridden: false,
            background: base.color(),
            header: Color::Rgb(0xc0, 0xca, 0xf5),
            file: Color::Rgb(0xc0, 0xca, 0xf5),
            hunk: Color::Rgb(0xbb, 0x9a, 0xf7),
            notice: green.color(),
            cursor: Color::Rgb(0xc0, 0xca, 0xf5),
            cursor_line_bg: base.blend(RgbColor::new(0xbb, 0x9a, 0xf7), 0.12).color(),
            muted: Color::Rgb(0x56, 0x5f, 0x89),
            gutter_bg: base.blend(RgbColor::new(0, 0, 0), 0.22).color(),
            empty_diff: Color::Rgb(0x24, 0x28, 0x3b),
            search_match_fg: base.color(),
            search_match_bg: Color::Rgb(0xe0, 0xaf, 0x68),
            statusline_fg: Color::Rgb(0xc0, 0xca, 0xf5),
            statusline_bg: base.blend(RgbColor::new(0, 0, 0), 0.18).color(),
            statusline_accent_fg: base.color(),
            statusline_accent_bg: Color::Rgb(0xbb, 0x9a, 0xf7),
            statusline_info_fg: Color::Rgb(0xc0, 0xca, 0xf5),
            statusline_info_bg: Color::Rgb(0x24, 0x28, 0x3b),
            addition_fg: green.color(),
            addition_gutter_bg: base.blend(green, 0.035).color(),
            addition_bg: base.blend(green, 0.045).color(),
            addition_inline_bg: base.blend(green, 0.14).color(),
            deletion_fg: red.color(),
            deletion_gutter_bg: base.blend(red, 0.035).color(),
            deletion_bg: base.blend(red, 0.045).color(),
            deletion_inline_bg: base.blend(red, 0.14).color(),
            transparent_background: false,
            decorations: DecorationStyle::default(),
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::tokyonight(),
            syntax_overrides: SyntaxPalette::empty(),
            exact_syntax: Some(BuiltinTextMateTheme::Tokyonight.get()),
            scope_overrides: None,
        }
    }

    pub(crate) fn base16(scheme: Base16Scheme) -> Self {
        Self {
            foreground: scheme.base05.color(),
            foreground_overridden: false,
            background: scheme.base00.color(),
            header: scheme.base06.color(),
            file: scheme.base05.color(),
            hunk: scheme.base0e.color(),
            notice: scheme.base0b.color(),
            cursor: scheme.base05.color(),
            cursor_line_bg: scheme.base00.blend(scheme.base0d, 0.12).color(),
            muted: scheme.base03.color(),
            gutter_bg: scheme.base00.blend(RgbColor::new(0, 0, 0), 0.18).color(),
            empty_diff: scheme.base01.color(),
            search_match_fg: scheme.base00.color(),
            search_match_bg: scheme.base0a.color(),
            statusline_fg: scheme.base05.color(),
            statusline_bg: scheme.base00.blend(RgbColor::new(0, 0, 0), 0.18).color(),
            statusline_accent_fg: scheme.base00.color(),
            statusline_accent_bg: scheme.base0e.color(),
            statusline_info_fg: scheme.base05.color(),
            statusline_info_bg: scheme.base01.color(),
            addition_fg: scheme.base0b.color(),
            addition_gutter_bg: scheme.base00.blend(scheme.base0b, 0.035).color(),
            addition_bg: scheme.base00.blend(scheme.base0b, 0.045).color(),
            addition_inline_bg: scheme.base00.blend(scheme.base0b, 0.14).color(),
            deletion_fg: scheme.base08.color(),
            deletion_gutter_bg: scheme.base00.blend(scheme.base08, 0.035).color(),
            deletion_bg: scheme.base00.blend(scheme.base08, 0.045).color(),
            deletion_inline_bg: scheme.base00.blend(scheme.base08, 0.14).color(),
            transparent_background: false,
            decorations: DecorationStyle::default(),
            diff: DiffSettings::default(),
            syntax: SyntaxPalette::base16(scheme),
            syntax_overrides: SyntaxPalette::empty(),
            exact_syntax: None,
            scope_overrides: None,
        }
    }

    pub(crate) fn with_transparent_background(mut self, transparent: bool) -> Self {
        self.transparent_background = transparent;
        self
    }

    pub(crate) fn with_diff_settings(mut self, diff: DiffSettings) -> Self {
        self.diff = diff;
        self
    }

    pub(crate) fn with_decorations(mut self, decorations: DecorationStyle) -> Self {
        self.decorations = decorations;
        self
    }

    pub(crate) fn with_color_overrides(mut self, colors: &ColorOverrides) -> MarkResult<Self> {
        if let Some(color) = config_color(&colors.bg, "bg")? {
            self.background = color;
        }
        if let Some(color) = config_color(&colors.fg, "fg")? {
            self.foreground = color;
            self.foreground_overridden = true;
        }
        if let Some(color) = config_color(&colors.header, "header")? {
            self.header = color;
        }
        if let Some(color) = config_color(&colors.file, "file")? {
            self.file = color;
        }
        if let Some(color) = config_color(&colors.hunk, "hunk")? {
            self.hunk = color;
        }
        if let Some(color) = config_color(&colors.notice, "notice")? {
            self.notice = color;
        }
        if let Some(color) = config_color(&colors.cursor, "cursor")? {
            self.cursor = color;
        }
        if let Some(color) = config_color(&colors.cursor_line_bg, "cursor_line_bg")? {
            self.cursor_line_bg = color;
        }
        if let Some(color) = config_color(&colors.muted, "muted")? {
            self.muted = color;
        }
        if let Some(color) = config_color(&colors.gutter_bg, "gutter_bg")? {
            self.gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.empty_diff, "empty_diff")? {
            self.empty_diff = color;
        }
        if let Some(color) = config_color(&colors.search_match_fg, "search_match_fg")? {
            self.search_match_fg = color;
        }
        if let Some(color) = config_color(&colors.search_match_bg, "search_match_bg")? {
            self.search_match_bg = color;
        }
        if let Some(color) = config_color(&colors.statusline_fg, "statusline_fg")? {
            self.statusline_fg = color;
        }
        if let Some(color) = config_color(&colors.statusline_bg, "statusline_bg")? {
            self.statusline_bg = color;
        }
        if let Some(color) = config_color(&colors.statusline_accent_fg, "statusline_accent_fg")? {
            self.statusline_accent_fg = color;
        }
        if let Some(color) = config_color(&colors.statusline_accent_bg, "statusline_accent_bg")? {
            self.statusline_accent_bg = color;
        }
        if let Some(color) = config_color(&colors.statusline_info_fg, "statusline_info_fg")? {
            self.statusline_info_fg = color;
        }
        if let Some(color) = config_color(&colors.statusline_info_bg, "statusline_info_bg")? {
            self.statusline_info_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_fg, "addition_fg")? {
            self.addition_fg = color;
        }
        if let Some(color) = config_color(&colors.addition_gutter_bg, "addition_gutter_bg")? {
            self.addition_gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_bg, "addition_bg")? {
            self.addition_bg = color;
        }
        if let Some(color) = config_color(&colors.addition_inline_bg, "addition_inline_bg")? {
            self.addition_inline_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_fg, "deletion_fg")? {
            self.deletion_fg = color;
        }
        if let Some(color) = config_color(&colors.deletion_gutter_bg, "deletion_gutter_bg")? {
            self.deletion_gutter_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_bg, "deletion_bg")? {
            self.deletion_bg = color;
        }
        if let Some(color) = config_color(&colors.deletion_inline_bg, "deletion_inline_bg")? {
            self.deletion_inline_bg = color;
        }
        if let Some(color) = config_color(&colors.attribute, "attribute")? {
            self.syntax.attribute = Some(color);
            self.syntax_overrides.attribute = Some(color);
        }
        if let Some(color) = config_color(&colors.comment, "comment")? {
            self.syntax.comment = Some(color);
            self.syntax_overrides.comment = Some(color);
        }
        if let Some(color) = config_color(&colors.constant, "constant")? {
            self.syntax.constant = Some(color);
            self.syntax_overrides.constant = Some(color);
        }
        if let Some(color) = config_color(&colors.constructor, "constructor")? {
            self.syntax.constructor = Some(color);
            self.syntax_overrides.constructor = Some(color);
        }
        if let Some(color) = config_color(&colors.function, "function")? {
            self.syntax.function = Some(color);
            self.syntax_overrides.function = Some(color);
        }
        if let Some(color) = config_color(&colors.keyword, "keyword")? {
            self.syntax.keyword = Some(color);
            self.syntax_overrides.keyword = Some(color);
        }
        if let Some(color) = config_color(&colors.label, "label")? {
            self.syntax.label = Some(color);
            self.syntax_overrides.label = Some(color);
        }
        if let Some(color) = config_color(&colors.module, "module")? {
            self.syntax.module = Some(color);
            self.syntax_overrides.module = Some(color);
        }
        if let Some(color) = config_color(&colors.number, "number")? {
            self.syntax.number = Some(color);
            self.syntax_overrides.number = Some(color);
        }
        if let Some(color) = config_color(&colors.operator, "operator")? {
            self.syntax.operator = Some(color);
            self.syntax_overrides.operator = Some(color);
        }
        if let Some(color) = config_color(&colors.property, "property")? {
            self.syntax.property = Some(color);
            self.syntax_overrides.property = Some(color);
        }
        if let Some(color) = config_color(&colors.punctuation, "punctuation")? {
            self.syntax.punctuation = Some(color);
            self.syntax_overrides.punctuation = Some(color);
        }
        if let Some(color) = config_color(&colors.string, "string")? {
            self.syntax.string = Some(color);
            self.syntax_overrides.string = Some(color);
        }
        if let Some(color) = config_color(&colors.tag, "tag")? {
            self.syntax.tag = Some(color);
            self.syntax_overrides.tag = Some(color);
        }
        if let Some(color) = config_color(&colors.r#type, "type")? {
            self.syntax.r#type = Some(color);
            self.syntax_overrides.r#type = Some(color);
        }
        if let Some(color) = config_color(&colors.variable, "variable")? {
            self.syntax.variable = Some(color);
            self.syntax_overrides.variable = Some(color);
        }
        Ok(self)
    }

    pub(crate) fn with_syntax_rules(mut self, rules: &[SyntaxRuleOverride]) -> MarkResult<Self> {
        if rules.is_empty() {
            self.scope_overrides = None;
            return Ok(self);
        }
        let compiled = TextMateTheme::from_syntax_rules(rules).map_err(|error| {
            mark_core::MarkError::Usage(format!("invalid syntax_rules: {error}"))
        })?;
        let mut hasher = DefaultHasher::new();
        rules.hash(&mut hasher);
        let id = hasher.finish().max(1);
        let registry = scope_override_registry();
        let mut registry = registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if registry.len() >= 64 && !registry.contains_key(&id) {
            registry.clear();
        }
        registry.insert(id, compiled);
        self.scope_overrides = Some(id);
        Ok(self)
    }
}

fn scope_override_registry() -> &'static RwLock<HashMap<u64, TextMateTheme>> {
    static REGISTRY: OnceLock<RwLock<HashMap<u64, TextMateTheme>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(crate) fn with_scope_override_theme<T>(
    id: u64,
    resolve: impl FnOnce(&TextMateTheme) -> T,
) -> Option<T> {
    let registry = scope_override_registry()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.get(&id).map(resolve)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextMateEngineMode {
    Coarse,
    Exact,
    Compare,
}

impl TextMateEngineMode {
    fn parse(value: Option<&OsStr>) -> Self {
        match value
            .map(|value| value.to_string_lossy().trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("coarse" | "legacy") => Self::Coarse,
            Some("compare") => Self::Compare,
            _ => Self::Exact,
        }
    }
}

pub(crate) fn textmate_engine_mode() -> TextMateEngineMode {
    static MODE: OnceLock<TextMateEngineMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        TextMateEngineMode::parse(env::var_os("MARK_TEXTMATE_THEME_ENGINE").as_deref())
    })
}
