use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use mark_core::{MarkError, MarkResult};
use mark_syntax::{
    ColorOverrides, HighlightScopeTable, SyntaxThemeConfig, theme::BuiltinTextMateTheme,
};
use ratatui::prelude::Color;
use serde::Deserialize;

use super::DiffTheme;

pub(crate) fn config_color(value: &Option<String>, name: &str) -> MarkResult<Option<Color>> {
    value
        .as_deref()
        .map(|value| parse_config_color(value, name))
        .transpose()
}

pub(crate) fn parse_config_color(value: &str, name: &str) -> MarkResult<Color> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    if matches!(lower.as_str(), "default" | "reset" | "none") {
        return Ok(Color::Reset);
    }

    if let Some(color) = parse_config_hex_color(trimmed) {
        return Ok(color.color());
    }

    if let Some(index) = parse_ansi_index(&lower) {
        return Ok(Color::Indexed(index));
    }

    if let Some(color) = parse_named_color(&lower) {
        return Ok(color);
    }

    Err(MarkError::Usage(format!(
        "invalid color for {name}: {value}; expected #rrggbb, ansi-N, or a named color"
    )))
}

pub(crate) fn parse_config_hex_color(value: &str) -> Option<RgbColor> {
    let value = value
        .trim()
        .trim_matches(['\'', '"'])
        .strip_prefix('#')
        .or_else(|| value.trim().strip_prefix("0x"))
        .unwrap_or_else(|| value.trim().trim_matches(['\'', '"']));
    parse_hex_digits(value)
}

pub(crate) fn parse_ansi_index(value: &str) -> Option<u8> {
    let index = value
        .strip_prefix("ansi-")
        .or_else(|| value.strip_prefix("ansi:"))
        .or_else(|| value.strip_prefix("indexed-"))
        .or_else(|| value.strip_prefix("indexed:"))
        .unwrap_or(value);
    index.parse::<u8>().ok()
}

pub(crate) fn parse_named_color(value: &str) -> Option<Color> {
    match value.replace('_', "-").as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark-gray" | "dark-grey" | "bright-black" => Some(Color::DarkGray),
        "white" | "bright-white" => Some(Color::White),
        "bright-red" | "light-red" => Some(Color::LightRed),
        "bright-green" | "light-green" => Some(Color::LightGreen),
        "bright-yellow" | "light-yellow" => Some(Color::LightYellow),
        "bright-blue" | "light-blue" => Some(Color::LightBlue),
        "bright-magenta" | "light-magenta" | "bright-purple" | "light-purple" => {
            Some(Color::LightMagenta)
        }
        "bright-cyan" | "light-cyan" => Some(Color::LightCyan),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Base16Scheme {
    pub(crate) base00: RgbColor,
    pub(crate) base01: RgbColor,
    pub(crate) base03: RgbColor,
    pub(crate) base04: RgbColor,
    pub(crate) base05: RgbColor,
    pub(crate) base06: RgbColor,
    pub(crate) base08: RgbColor,
    pub(crate) base09: RgbColor,
    pub(crate) base0a: RgbColor,
    pub(crate) base0b: RgbColor,
    pub(crate) base0c: RgbColor,
    pub(crate) base0d: RgbColor,
    pub(crate) base0e: RgbColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RgbColor {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
}

impl RgbColor {
    pub(crate) const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub(crate) fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
    }

    pub(crate) fn blend(self, other: Self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| -> u8 {
            ((f32::from(a) * (1.0 - amount)) + (f32::from(b) * amount)).round() as u8
        };
        Self {
            red: mix(self.red, other.red),
            green: mix(self.green, other.green),
            blue: mix(self.blue, other.blue),
        }
    }

    pub(crate) fn surface(self) -> Self {
        let luminance =
            (u16::from(self.red) * 3 + u16::from(self.green) * 6 + u16::from(self.blue)) / 10;
        let contrast = if luminance < 160 {
            Self::new(0xff, 0xff, 0xff)
        } else {
            Self::new(0, 0, 0)
        };
        self.blend(contrast, 0.08)
    }
}

pub(crate) fn diff_theme_from_config(config: &SyntaxThemeConfig) -> MarkResult<DiffTheme> {
    match config {
        SyntaxThemeConfig::Builtin { name } => {
            let name = name.as_deref();
            match builtin_diff_theme(name) {
                Ok(theme) => Ok(theme),
                Err(error) => {
                    if let Some(name) = name
                        && let Some(theme) = load_named_colorscheme(name)?
                    {
                        return Ok(theme);
                    }
                    Err(error)
                }
            }
        }
        SyntaxThemeConfig::Ansi => Ok(DiffTheme::ansi()),
        SyntaxThemeConfig::Base16 { path } => Ok(DiffTheme::base16(load_base16_scheme(path)?)),
        SyntaxThemeConfig::Base16MissingPath => Err(MarkError::Usage(
            "base16 theme requires theme.path".to_owned(),
        )),
    }
}

pub(crate) fn load_named_colorscheme(name: &str) -> MarkResult<Option<DiffTheme>> {
    let name = name.trim();
    if name.is_empty() || Path::new(name).file_name().and_then(OsStr::to_str) != Some(name) {
        return Ok(None);
    }

    let theme_dir = mark_syntax::colorscheme_dir()?;
    let legacy_theme_dir = theme_dir.join("colorscheme");
    for path in [theme_dir, legacy_theme_dir]
        .into_iter()
        .flat_map(|dir| colorscheme_paths(&dir, name))
    {
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        // Base16 TOML commonly stores base00..base0F below `[colors]`.
        // Recognize it before native-theme validation sees that table.
        if let Some(scheme) = parse_base16_scheme(&contents) {
            return Ok(Some(DiffTheme::base16(scheme)));
        }
        if path.extension() == Some(OsStr::new("toml"))
            && let Some(theme) = parse_custom_colorscheme(&contents).map_err(|error| {
                MarkError::Usage(format!(
                    "failed to load custom theme {}: {error}",
                    path.display()
                ))
            })?
        {
            return Ok(Some(theme));
        }
        return Err(MarkError::Usage(format!(
            "failed to parse base16 theme at {}; expected base00 through base0F",
            path.display()
        )));
    }
    Ok(None)
}

#[derive(Debug, Default, Deserialize)]
struct CustomColorscheme {
    #[serde(default, alias = "base", alias = "inherits")]
    extends: Option<String>,
    #[serde(default)]
    colors: ColorOverrides,
    #[serde(default, alias = "background_transparent", alias = "transparent_bg")]
    transparent_background: Option<bool>,
}

const CUSTOM_COLOR_KEYS: &[&str] = &[
    "bg",
    "background",
    "fg",
    "foreground",
    "header",
    "file",
    "hunk",
    "notice",
    "cursor",
    "cursor_line",
    "cursor_line_bg",
    "muted",
    "gutter_bg",
    "empty_diff",
    "search_match_fg",
    "search_match_bg",
    "statusline_fg",
    "statusline_bg",
    "statusline_accent_fg",
    "statusline_accent_bg",
    "statusline_info_fg",
    "statusline_info_bg",
    "addition_fg",
    "addition_gutter_bg",
    "addition_bg",
    "addition_inline_bg",
    "deletion_fg",
    "deletion_gutter_bg",
    "deletion_bg",
    "deletion_inline_bg",
    "attribute",
    "comment",
    "constant",
    "constructor",
    "function",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "tag",
    "type",
    "variable",
];

pub(crate) fn parse_custom_colorscheme(contents: &str) -> MarkResult<Option<DiffTheme>> {
    let document = match contents.parse::<toml::Table>() {
        Ok(document) => document,
        // YAML Base16 files are tried by the legacy parser below. A file that
        // clearly declares the native format should instead get its TOML error.
        Err(error) if looks_like_custom_colorscheme(contents) => {
            return Err(MarkError::Usage(format!("invalid custom theme: {error}")));
        }
        Err(_) => return Ok(None),
    };
    let colors = document.get("colors");
    let is_base16_colors_table = matches!(colors, Some(toml::Value::Table(colors)) if
        !colors.is_empty() && colors.keys().all(|key| base16_index(key).is_some()));
    let is_native = [
        "extends",
        "base",
        "inherits",
        "transparent_background",
        "background_transparent",
        "transparent_bg",
    ]
    .into_iter()
    .any(|key| document.contains_key(key))
        || (colors.is_some() && !is_base16_colors_table);
    if !is_native {
        return Ok(None);
    }

    if let Some(toml::Value::Table(colors)) = colors {
        for key in colors.keys() {
            if !CUSTOM_COLOR_KEYS.contains(&key.as_str()) {
                return Err(MarkError::Usage(format!(
                    "unknown custom theme color '{key}'"
                )));
            }
        }
    }

    let custom: CustomColorscheme = toml::from_str(contents)
        .map_err(|error| MarkError::Usage(format!("invalid custom theme: {error}")))?;
    let base = custom.extends.as_deref().unwrap_or("system");
    let theme = builtin_diff_theme(Some(base)).map_err(|_| {
        MarkError::Usage(format!(
            "custom theme extends unknown built-in theme '{base}'"
        ))
    })?;
    theme
        .with_color_overrides(&custom.colors)
        .map(|theme| theme.with_transparent_background_override(custom.transparent_background))
        .map(Some)
}

fn looks_like_custom_colorscheme(contents: &str) -> bool {
    contents.lines().map(str::trim).any(|line| {
        line == "[colors]"
            || [
                "extends",
                "base",
                "inherits",
                "transparent_background",
                "background_transparent",
                "transparent_bg",
            ]
            .into_iter()
            .any(|key| line.starts_with(key) && line[key.len()..].trim_start().starts_with('='))
    })
}

pub(crate) fn colorscheme_paths(dir: &Path, name: &str) -> Vec<PathBuf> {
    let path = dir.join(name);
    if Path::new(name).extension().is_some() {
        return vec![path];
    }

    ["toml", "yaml", "yml"]
        .into_iter()
        .map(|extension| path.with_extension(extension))
        .collect()
}

pub(crate) fn builtin_diff_theme(name: Option<&str>) -> MarkResult<DiffTheme> {
    let name = name.unwrap_or("system").trim().to_ascii_lowercase();
    match name.as_str() {
        "system" | "default" | "" => Ok(DiffTheme::system()),
        "catppuccin-latte" | "latte" => Ok(DiffTheme::catppuccin_latte()),
        "catppuccin-frappe" | "frappe" => Ok(DiffTheme::catppuccin_frappe()),
        "catppuccin-macchiato" | "macchiato" => Ok(DiffTheme::catppuccin_macchiato()),
        "catppuccin" | "catppuccin-mocha" | "mocha" => Ok(DiffTheme::catppuccin_mocha()),
        "gruvbox" | "gruvbox-dark" => Ok(DiffTheme::gruvbox_dark()),
        "gruvbox-light" => Ok(DiffTheme::gruvbox_light()),
        "github" | "github-dark" => Ok(DiffTheme::github_dark()),
        "github-dark-high-contrast" | "github-high-contrast" => {
            Ok(DiffTheme::github_dark_high_contrast())
        }
        "github-light" => Ok(DiffTheme::github_light()),
        "github-light-high-contrast" => Ok(DiffTheme::github_light_high_contrast()),
        "tokyonight" | "tokyo-night" | "tokyonight-night" => Ok(DiffTheme::tokyonight()),
        name => BuiltinTextMateTheme::from_name(name)
            .and_then(builtin_base16_palette)
            .map(|(theme, palette)| diff_theme_from_textmate(theme, palette))
            .ok_or_else(|| MarkError::Usage(format!("unknown theme '{name}'"))),
    }
}

macro_rules! rgb {
    ($hex:tt) => {{
        const VALUE: u32 = $hex;
        RgbColor::new((VALUE >> 16) as u8, (VALUE >> 8) as u8, VALUE as u8)
    }};
}

macro_rules! scheme {
    ($base00:tt, $base01:tt, $base03:tt, $base05:tt, $base06:tt, $base08:tt, $base09:tt, $base0a:tt, $base0b:tt, $base0c:tt, $base0d:tt, $base0e:tt) => {
        Base16Scheme {
            base00: rgb!($base00),
            base01: rgb!($base01),
            base03: rgb!($base03),
            base04: rgb!($base03),
            base05: rgb!($base05),
            base06: rgb!($base06),
            base08: rgb!($base08),
            base09: rgb!($base09),
            base0a: rgb!($base0a),
            base0b: rgb!($base0b),
            base0c: rgb!($base0c),
            base0d: rgb!($base0d),
            base0e: rgb!($base0e),
        }
    };
}

fn builtin_base16_palette(
    theme: BuiltinTextMateTheme,
) -> Option<(BuiltinTextMateTheme, Base16Scheme)> {
    use BuiltinTextMateTheme as Theme;

    let palette = match theme {
        Theme::Nordic => scheme!(
            0x242933, 0x2E3440, 0x60728A, 0xBBC3D4, 0xD8DEE9, 0xBF616A, 0xD08770, 0xEBCB8B,
            0xA3BE8C, 0x8FBCBB, 0x81A1C1, 0xB48EAD
        ),
        Theme::Nord => scheme!(
            0x2E3440, 0x3B4252, 0x4C566A, 0xD8DEE9, 0xECEFF4, 0xBF616A, 0xD08770, 0xEBCB8B,
            0xA3BE8C, 0x8FBCBB, 0x81A1C1, 0xB48EAD
        ),
        Theme::AyuDark => scheme!(
            0x10141C, 0x171B24, 0x5A6673, 0xBFBDB6, 0xE6E1CF, 0xF07178, 0xFF8F40, 0xFFB454,
            0xAAD94C, 0x95E6CB, 0x59C2FF, 0xD2A6FF
        ),
        Theme::AyuLight => scheme!(
            0xFCFCFC, 0xF3F4F5, 0xADAEB1, 0x5C6166, 0x3D424D, 0xF07171, 0xFA8532, 0xEBA400,
            0x86B300, 0x4CBF99, 0x22A4E6, 0xA37ACC
        ),
        Theme::AyuMirage => scheme!(
            0x242936, 0x1F2430, 0x6E7C8F, 0xCCCAC2, 0xFFFFFF, 0xF28779, 0xFFA659, 0xFFCD66,
            0xD5FF80, 0x95E6CB, 0x73D0FF, 0xD4BFFF
        ),
        Theme::Molokai => scheme!(
            0x1B1D1E, 0x272822, 0x75715E, 0xF8F8F2, 0xFFFFFF, 0xF92672, 0xFD971F, 0xE6DB74,
            0xA6E22E, 0x66D9EF, 0x66D9EF, 0xAE81FF
        ),
        Theme::ZenbonesDark
        | Theme::ZenbonesLight
        | Theme::Duckbones
        | Theme::ForestbonesDark
        | Theme::ForestbonesLight
        | Theme::Kanagawabones
        | Theme::NeobonesDark
        | Theme::NeobonesLight
        | Theme::Nordbones
        | Theme::RosebonesDark
        | Theme::RosebonesLight
        | Theme::SeoulbonesDark
        | Theme::SeoulbonesLight
        | Theme::TokyobonesDark
        | Theme::TokyobonesLight
        | Theme::Vimbones
        | Theme::Zenburned
        | Theme::ZenwrittenDark
        | Theme::ZenwrittenLight => default_textmate_palette(theme),
        Theme::KanagawaWave => scheme!(
            0x1F1F28, 0x2A2A37, 0x727169, 0xDCD7BA, 0xC8C093, 0xE46876, 0xFFA066, 0xE6C384,
            0x98BB6C, 0x7AA89F, 0x7E9CD8, 0x957FB8
        ),
        Theme::KanagawaDragon => scheme!(
            0x181616, 0x282727, 0x737C73, 0xC5C9C5, 0xC8C093, 0xC4746E, 0xB98D7B, 0xC4B28A,
            0x87A987, 0x8EA4A2, 0x8BA4B0, 0xA292A3
        ),
        Theme::KanagawaLotus => scheme!(
            0xF2ECBC, 0xE7DBA0, 0x8A8980, 0x545464, 0x43436C, 0xC84053, 0xCC6D00, 0x77713F,
            0x6F894E, 0x597B75, 0x4D699B, 0x624C83
        ),
        Theme::EverforestDark => scheme!(
            0x2D353B, 0x343F44, 0x859289, 0xD3C6AA, 0xFFFBEF, 0xE67E80, 0xE69875, 0xDBBC7F,
            0xA7C080, 0x83C092, 0x7FBBB3, 0xD699B6
        ),
        Theme::EverforestLight => scheme!(
            0xFDF6E3, 0xF4F0D9, 0x939F91, 0x5C6A72, 0x3A515D, 0xF85552, 0xF57D26, 0xDFA000,
            0x8DA101, 0x35A77C, 0x3A94C5, 0xDF69BA
        ),
        Theme::TokenDark => scheme!(
            0x262624, 0x34332F, 0x938E87, 0xE8E4DC, 0xF8F5EF, 0xC67777, 0xD97757, 0xC4A855,
            0x7DA47A, 0x72A6A1, 0x7B9EBD, 0xA68BBF
        ),
        Theme::TokenLight => scheme!(
            0xFAF9F5, 0xECEAE4, 0x6C675F, 0x2A2920, 0x171610, 0xB05555, 0x9A4929, 0x6E5C20,
            0x3F643C, 0x39746F, 0x527594, 0x7C619A
        ),
        Theme::GruvboxMaterialDark => scheme!(
            0x292828, 0x32302F, 0x928374, 0xD4BE98, 0xFBEDC8, 0xEA6962, 0xE78A4E, 0xD8A657,
            0xA9B665, 0x89B482, 0x7DAEA3, 0xD3869B
        ),
        Theme::GruvboxMaterialLight => scheme!(
            0xFBF1C7, 0xEBDAB4, 0x928374, 0x654735, 0x3C3836, 0xC14A4A, 0xC35E0A, 0xB47109,
            0x6C782E, 0x4C7A5D, 0x45707A, 0x945E80
        ),
        Theme::Origin
        | Theme::Mfd
        | Theme::MfdDark
        | Theme::MfdStealth
        | Theme::MfdAmber
        | Theme::MfdMono
        | Theme::MfdScarlet
        | Theme::MfdPaper
        | Theme::MfdHud
        | Theme::MfdNvg
        | Theme::MfdBlackout
        | Theme::MfdFlir
        | Theme::MfdFlirBh
        | Theme::MfdFlirRh
        | Theme::MfdFlirFusion
        | Theme::MfdGblLight
        | Theme::MfdGblDark
        | Theme::MfdLumon
        | Theme::MfdNerv => default_textmate_palette(theme),
        _ => return None,
    };
    Some((theme, palette))
}

fn diff_theme_from_textmate(theme: BuiltinTextMateTheme, palette: Base16Scheme) -> DiffTheme {
    let exact = theme.get();
    let color = |name: &str| {
        exact
            .color(name)
            .map(|color| RgbColor::new(color.red, color.green, color.blue))
    };
    let mut result = DiffTheme::base16(palette).with_exact_syntax(theme);
    if let Some(cursor) = color("editorCursor.foreground") {
        result.cursor = cursor.color();
    }
    if let Some(cursor_line) = color("editor.lineHighlightBackground") {
        result.cursor_line_bg = cursor_line.color();
    }
    if let Some(search) = color("editor.findMatchBackground") {
        result.search_match_bg = search.color();
    }
    if let Some(addition) = color("gitDecoration.addedResourceForeground") {
        result.addition_fg = addition.color();
    }
    if let Some(deletion) = color("gitDecoration.deletedResourceForeground") {
        result.deletion_fg = deletion.color();
    }
    if let Some(addition_bg) = color("diffEditor.insertedLineBackground") {
        result.addition_bg = addition_bg.color();
        result.addition_gutter_bg = addition_bg.color();
    }
    if let Some(deletion_bg) = color("diffEditor.removedLineBackground") {
        result.deletion_bg = deletion_bg.color();
        result.deletion_gutter_bg = deletion_bg.color();
    }
    result
}

fn default_textmate_palette(theme: BuiltinTextMateTheme) -> Base16Scheme {
    let exact = theme.get();
    let style = exact.default_style();
    let to_rgb =
        |color: mark_syntax::theme::RgbColor| RgbColor::new(color.red, color.green, color.blue);
    let background = to_rgb(
        style
            .background
            .expect("built-in theme has an editor background"),
    );
    let foreground = to_rgb(
        style
            .foreground
            .expect("built-in theme has an editor foreground"),
    );
    let resolved = |scope: &str, fallback: RgbColor| {
        let (table, stack) = HighlightScopeTable::from_scope_names(&[scope]);
        let resolved = exact.resolve_style(&table, stack);
        if resolved.foreground_matched {
            resolved.style.foreground.map(to_rgb).unwrap_or(fallback)
        } else {
            fallback
        }
    };
    let dim = resolved("comment", background.blend(foreground, 0.34));
    let bright = background.blend(foreground, 0.84);
    Base16Scheme {
        base00: background,
        base01: background.blend(foreground, 0.10),
        base03: dim,
        base04: resolved("punctuation", background.blend(foreground, 0.60)),
        base05: foreground,
        base06: bright,
        base08: resolved("markup.deleted", bright),
        base09: resolved("constant.numeric", foreground),
        base0a: resolved("entity.name.type", bright),
        base0b: resolved("markup.inserted", foreground),
        base0c: resolved("constant.character.escape", bright),
        base0d: resolved(
            "markup.changed",
            resolved("entity.name.function", foreground),
        ),
        base0e: resolved("keyword", bright),
    }
}

pub(crate) fn load_base16_scheme(path: &Path) -> MarkResult<Base16Scheme> {
    let path = expand_user_path(path);
    let contents = fs::read_to_string(&path)?;
    parse_base16_scheme(&contents).ok_or_else(|| {
        MarkError::Usage(format!(
            "failed to parse base16 theme at {}; expected base00 through base0F",
            path.display()
        ))
    })
}

pub(crate) fn expand_user_path(path: &Path) -> PathBuf {
    let path_text = path.to_string_lossy();
    if path_text == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = path_text.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

pub(crate) fn parse_base16_scheme(contents: &str) -> Option<Base16Scheme> {
    let mut colors: [Option<RgbColor>; 16] = [None; 16];
    for line in contents.lines() {
        let Some((index, color)) = parse_base16_line(line) else {
            continue;
        };
        colors[index] = Some(color);
    }

    if colors.iter().any(Option::is_none) {
        return None;
    }

    Some(Base16Scheme {
        base00: colors[0]?,
        base01: colors[1]?,
        base03: colors[3]?,
        base04: colors[4]?,
        base05: colors[5]?,
        base06: colors[6]?,
        base08: colors[8]?,
        base09: colors[9]?,
        base0a: colors[10]?,
        base0b: colors[11]?,
        base0c: colors[12]?,
        base0d: colors[13]?,
        base0e: colors[14]?,
    })
}

pub(crate) fn parse_base16_line(line: &str) -> Option<(usize, RgbColor)> {
    let line = line.trim();
    let (key, value) = line.split_once(':').or_else(|| line.split_once('='))?;
    let key = key.trim().trim_matches(['\'', '"']).to_ascii_lowercase();
    let index = base16_index(&key)?;
    let color = parse_hex_color(value)?;
    Some((index, color))
}

pub(crate) fn base16_index(key: &str) -> Option<usize> {
    let suffix = key.strip_prefix("base")?;
    if suffix.len() != 2 || !suffix.starts_with('0') {
        return None;
    }
    usize::from_str_radix(suffix, 16)
        .ok()
        .filter(|index| *index < 16)
}

pub(crate) fn parse_hex_color(value: &str) -> Option<RgbColor> {
    let value = value.trim();
    if let Some(hash) = value.find('#') {
        return parse_hex_digits(value.get(hash + 1..hash + 7)?);
    }

    let token = value
        .trim_matches(['\'', '"', ',', ' '])
        .split_whitespace()
        .next()?;
    parse_hex_digits(token.trim_matches(['\'', '"', ',']))
}

pub(crate) fn parse_hex_digits(digits: &str) -> Option<RgbColor> {
    if digits.len() < 6
        || !digits.as_bytes()[..6]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(RgbColor {
        red: u8::from_str_radix(&digits[0..2], 16).ok()?,
        green: u8::from_str_radix(&digits[2..4], 16).ok()?,
        blue: u8::from_str_radix(&digits[4..6], 16).ok()?,
    })
}
