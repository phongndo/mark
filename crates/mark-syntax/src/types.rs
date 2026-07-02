use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    detect_custom_language_from_path, detect_language_name, enabled_language_set_for_mode,
    has_highlights, installed_language_set, language_vec_to_set, load_config, load_settings,
    normalize_language_name,
};
use mark_core::{MarkError, MarkResult};
pub use mark_textmate::{
    HighlightedLine, HighlightedText, LineTextFingerprint, SyntaxClass, SyntaxSegment,
};
use serde::{Deserialize, Serialize};

pub(crate) const CONFIG_DIR: &str = "mark";
pub(crate) const CONFIG_FILE: &str = "syntax.json";
pub(crate) const SETTINGS_FILE: &str = "config.toml";
pub(crate) const LEGACY_SETTINGS_FILE: &str = "syntax.toml";
pub(crate) const COLORSCHEME_DIR: &str = "colorscheme";
pub(crate) const TEXTMATE_BUNDLE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;
pub const DEFAULT_HIGHLIGHT_CACHE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_QUEUE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 1;
pub const MAX_NOTIFICATION_TIMEOUT_MS: u64 = 10_000;

pub(crate) const CORE_LANGUAGES: &[&str] = &[
    "asciidoc-asciidoctor",
    "agda",
    "angular-html",
    "angular-ts",
    "apache-conf",
    "apl",
    "arm-assembly",
    "asm",
    "astro",
    "bash",
    "ballerina",
    "beancount",
    "bibtex",
    "bicep",
    "blade",
    "c",
    "c3",
    "cadence",
    "cairo",
    "chapel",
    "codeowners",
    "codeql",
    "clojure",
    "cmake",
    "common-lisp",
    "coq",
    "cpp",
    "csharp",
    "css",
    "crystal",
    "cuda",
    "cue",
    "cypher",
    "dart",
    "dax",
    "dhall",
    "dockerfile",
    "dockerfile-with-bash",
    "dotenv",
    "elixir",
    "elm",
    "erlang",
    "emacs-lisp",
    "edge",
    "ejs",
    "fennel",
    "fish",
    "fortran-fixed-form",
    "fortran-modern",
    "forth",
    "f#",
    "gn",
    "glsl",
    "go",
    "gomod",
    "gosum",
    "graphql",
    "haskell",
    "handlebars",
    "hack",
    "hlsl",
    "hy",
    "idris",
    "html",
    "html-jinja2",
    "html-rails",
    "html-twig",
    "ini",
    "java",
    "java-properties",
    "javascript",
    "json",
    "json-terraform",
    "jsonnet",
    "julia",
    "just",
    "kotlin",
    "kusto",
    "latex",
    "lean-4",
    "liquid",
    "lisp",
    "llvm",
    "lua",
    "make",
    "markdown",
    "matlab",
    "marko",
    "mdc",
    "mdx",
    "mermaid",
    "meson",
    "metal",
    "mipsasm",
    "mlir",
    "mojo",
    "moonbit",
    "move",
    "nginx",
    "nim",
    "ninja",
    "nix",
    "nushell",
    "objective-c",
    "objective-c++",
    "ocaml",
    "ocamllex",
    "ocamlyacc",
    "opencl",
    "odin",
    "orgmode",
    "perl",
    "php",
    "pkl",
    "pony",
    "powershell",
    "powerquery",
    "prisma",
    "prolog",
    "pug",
    "protocol-buffer",
    "protocol-buffer-text",
    "python",
    "qml",
    "r",
    "racket",
    "raku",
    "razor",
    "rego",
    "restructuredtext",
    "ruby",
    "ruby-haml",
    "rust",
    "sas",
    "scala",
    "scheme",
    "scss",
    "shell-unix-generic",
    "smalltalk",
    "sml",
    "solidity",
    "sparql",
    "spirv",
    "sql",
    "starlark",
    "stata",
    "surrealql",
    "svelte",
    "swift",
    "systemd",
    "systemverilog",
    "tablegen",
    "templ",
    "terraform",
    "tex",
    "toml",
    "tsx",
    "twig",
    "typescript",
    "typespec",
    "v",
    "vala",
    "verilog",
    "vue-component",
    "vyper",
    "vhdl",
    "wasm",
    "wgsl",
    "wolfram",
    "x86-64-assembly",
    "yaml",
    "zig",
];

pub(crate) const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("bazel", "starlark"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("c#", "csharp"),
    ("coq", "coq"),
    ("cxx", "cpp"),
    ("docker", "dockerfile"),
    ("gradle", "groovy"),
    ("hcl", "terraform"),
    ("ignorefile", "gitignore"),
    ("ipynb", "json"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("justfile", "just"),
    ("commonlisp", "common-lisp"),
    ("node", "javascript"),
    ("objc", "objective-c"),
    ("proto", "protocol-buffer"),
    ("protobuf", "protocol-buffer"),
    ("prolog", "prolog"),
    ("ps1", "powershell"),
    ("pwsh", "powershell"),
    ("python3", "python"),
    ("scm", "scheme"),
    ("service", "systemd"),
    ("makefile", "make"),
    ("shell", "bash"),
    ("sh", "bash"),
    ("tf", "terraform"),
    ("timer", "systemd"),
    ("ts", "typescript"),
    ("wast", "wasm"),
    ("wat", "wasm"),
    ("vue", "vue-component"),
];

pub(crate) const BASENAME_LANGUAGES: &[(&str, &str)] = &[
    ("build", "starlark"),
    ("build.bazel", "starlark"),
    ("workspace", "starlark"),
    ("workspace.bazel", "starlark"),
    ("module.bazel", "starlark"),
    ("codeowners", "codeowners"),
    ("dockerfile", "dockerfile"),
    ("justfile", "just"),
    ("makefile", "make"),
    ("gnumakefile", "make"),
    ("bsdmakefile", "make"),
    ("cmakelists.txt", "cmake"),
    (".bazelrc", "starlark"),
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".dockerignore", "gitignore"),
    (".gitignore", "gitignore"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredSyntaxConfig {
    #[serde(default)]
    pub(crate) languages: Vec<String>,
    #[serde(default)]
    pub(crate) extensions: Vec<StoredLanguageMapping>,
    #[serde(default)]
    pub(crate) filenames: Vec<StoredLanguageMapping>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredLanguageMapping {
    pub(crate) pattern: String,
    pub(crate) language: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxSettings {
    pub(crate) mode: Option<SyntaxMode>,
    pub(crate) colorscheme: Option<StoredSyntaxThemeConfig>,
    pub(crate) theme: Option<StoredSyntaxThemeConfig>,
    pub(crate) layout: Option<LayoutSetting>,
    pub(crate) live_reload: Option<bool>,
    pub(crate) syntax_highlighting: Option<bool>,
    #[serde(default, alias = "word_wrap", alias = "wrap_lines")]
    pub(crate) line_wrapping: bool,
    #[serde(default)]
    pub(crate) colors: ColorOverrides,
    #[serde(default, flatten)]
    pub(crate) color_overrides: ColorOverrides,
    #[serde(default, alias = "background_transparent", alias = "transparent_bg")]
    pub(crate) transparent_background: bool,
    #[serde(default)]
    pub(crate) diff: StoredDiffSettings,
    #[serde(default)]
    pub(crate) notifications: StoredNotificationSettings,
    #[serde(default)]
    pub(crate) limits: StoredSyntaxLimits,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredNotificationSettings {
    pub(crate) mode: Option<NotificationMode>,
    pub(crate) corner: Option<ToastCorner>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) max_visible: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationMode {
    #[default]
    Default,
    Debug,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToastCorner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredDiffSettings {
    pub(crate) line_background: Option<DiffBackground>,
    pub(crate) gutter_background: Option<DiffGutterBackground>,
    pub(crate) inline_background: Option<DiffBackground>,
    #[serde(alias = "word_background", alias = "word_diff_background")]
    pub(crate) word_background: Option<DiffBackground>,
    pub(crate) sign_style: Option<DiffSignStyle>,
    #[serde(
        alias = "context_lines",
        alias = "context_expand",
        alias = "expand_context"
    )]
    pub(crate) context_expansion: Option<StoredDiffContextExpansion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredDiffContextExpansion {
    Lines(usize),
    Mode(StoredDiffContextExpansionMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StoredDiffContextExpansionMode {
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredSyntaxThemeConfig {
    Name(String),
    Table(StoredSyntaxThemeTable),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxThemeTable {
    pub(crate) source: Option<SyntaxThemeSource>,
    pub(crate) name: Option<String>,
    pub(crate) path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredSyntaxLimits {
    pub(crate) max_source_kib: Option<usize>,
    pub(crate) max_line_kib: Option<usize>,
    pub(crate) cache_entries: Option<usize>,
    pub(crate) queue_entries: Option<usize>,
    pub(crate) prefetch_viewports: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSettings {
    pub mode: SyntaxMode,
    pub theme: SyntaxThemeConfig,
    pub layout: Option<LayoutSetting>,
    pub live_reload: bool,
    pub syntax_highlighting: bool,
    pub line_wrapping: bool,
    pub colors: ColorOverrides,
    pub transparent_background: bool,
    pub diff: DiffSettings,
    pub notifications: NotificationSettings,
    pub limits: SyntaxLimits,
}

impl Default for SyntaxSettings {
    fn default() -> Self {
        Self {
            mode: SyntaxMode::Builtin,
            theme: SyntaxThemeConfig::default(),
            layout: None,
            live_reload: true,
            syntax_highlighting: true,
            line_wrapping: false,
            colors: ColorOverrides::default(),
            transparent_background: false,
            diff: DiffSettings::default(),
            notifications: NotificationSettings::default(),
            limits: SyntaxLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositiveCount(std::num::NonZeroUsize);

impl PositiveCount {
    pub fn new(value: usize) -> Self {
        Self(std::num::NonZeroUsize::new(value.max(1)).expect("positive count"))
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl From<usize> for PositiveCount {
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotificationTimeoutMs(u64);

impl NotificationTimeoutMs {
    pub fn new(value: u64) -> Self {
        Self(value.min(MAX_NOTIFICATION_TIMEOUT_MS))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for NotificationTimeoutMs {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationSettings {
    mode: NotificationMode,
    corner: ToastCorner,
    timeout_ms: NotificationTimeoutMs,
    max_visible: PositiveCount,
}

impl NotificationSettings {
    pub fn new(
        mode: NotificationMode,
        corner: ToastCorner,
        timeout_ms: u64,
        max_visible: usize,
    ) -> Self {
        Self {
            mode,
            corner,
            timeout_ms: NotificationTimeoutMs::new(timeout_ms),
            max_visible: PositiveCount::new(max_visible),
        }
    }

    pub fn mode(self) -> NotificationMode {
        self.mode
    }

    pub fn corner(self) -> ToastCorner {
        self.corner
    }

    pub fn timeout(self) -> NotificationTimeoutMs {
        self.timeout_ms
    }

    pub fn visible_count(self) -> PositiveCount {
        self.max_visible
    }

    pub fn timeout_ms(self) -> u64 {
        self.timeout_ms.get()
    }

    pub fn max_visible(self) -> usize {
        self.max_visible.get()
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self::new(NotificationMode::Default, ToastCorner::TopRight, 1_500, 3)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ColorOverrides {
    #[serde(alias = "background")]
    pub bg: Option<String>,
    #[serde(alias = "foreground")]
    pub fg: Option<String>,
    pub header: Option<String>,
    pub file: Option<String>,
    pub hunk: Option<String>,
    pub notice: Option<String>,
    pub cursor: Option<String>,
    #[serde(alias = "cursor_line")]
    pub cursor_line_bg: Option<String>,
    pub muted: Option<String>,
    pub gutter_bg: Option<String>,
    pub empty_diff: Option<String>,
    pub search_match_fg: Option<String>,
    pub search_match_bg: Option<String>,
    pub statusline_fg: Option<String>,
    pub statusline_bg: Option<String>,
    pub statusline_accent_fg: Option<String>,
    pub statusline_accent_bg: Option<String>,
    pub statusline_info_fg: Option<String>,
    pub statusline_info_bg: Option<String>,
    pub addition_fg: Option<String>,
    pub addition_gutter_bg: Option<String>,
    pub addition_bg: Option<String>,
    pub addition_inline_bg: Option<String>,
    pub deletion_fg: Option<String>,
    pub deletion_gutter_bg: Option<String>,
    pub deletion_bg: Option<String>,
    pub deletion_inline_bg: Option<String>,
    pub attribute: Option<String>,
    pub comment: Option<String>,
    pub constant: Option<String>,
    pub constructor: Option<String>,
    pub function: Option<String>,
    pub keyword: Option<String>,
    pub label: Option<String>,
    pub module: Option<String>,
    pub number: Option<String>,
    pub operator: Option<String>,
    pub property: Option<String>,
    pub punctuation: Option<String>,
    pub string: Option<String>,
    pub tag: Option<String>,
    pub r#type: Option<String>,
    pub variable: Option<String>,
}

impl ColorOverrides {
    pub(crate) fn overlay(self, overrides: Self) -> Self {
        Self {
            bg: overrides.bg.or(self.bg),
            fg: overrides.fg.or(self.fg),
            header: overrides.header.or(self.header),
            file: overrides.file.or(self.file),
            hunk: overrides.hunk.or(self.hunk),
            notice: overrides.notice.or(self.notice),
            cursor: overrides.cursor.or(self.cursor),
            cursor_line_bg: overrides.cursor_line_bg.or(self.cursor_line_bg),
            muted: overrides.muted.or(self.muted),
            gutter_bg: overrides.gutter_bg.or(self.gutter_bg),
            empty_diff: overrides.empty_diff.or(self.empty_diff),
            search_match_fg: overrides.search_match_fg.or(self.search_match_fg),
            search_match_bg: overrides.search_match_bg.or(self.search_match_bg),
            statusline_fg: overrides.statusline_fg.or(self.statusline_fg),
            statusline_bg: overrides.statusline_bg.or(self.statusline_bg),
            statusline_accent_fg: overrides.statusline_accent_fg.or(self.statusline_accent_fg),
            statusline_accent_bg: overrides.statusline_accent_bg.or(self.statusline_accent_bg),
            statusline_info_fg: overrides.statusline_info_fg.or(self.statusline_info_fg),
            statusline_info_bg: overrides.statusline_info_bg.or(self.statusline_info_bg),
            addition_fg: overrides.addition_fg.or(self.addition_fg),
            addition_gutter_bg: overrides.addition_gutter_bg.or(self.addition_gutter_bg),
            addition_bg: overrides.addition_bg.or(self.addition_bg),
            addition_inline_bg: overrides.addition_inline_bg.or(self.addition_inline_bg),
            deletion_fg: overrides.deletion_fg.or(self.deletion_fg),
            deletion_gutter_bg: overrides.deletion_gutter_bg.or(self.deletion_gutter_bg),
            deletion_bg: overrides.deletion_bg.or(self.deletion_bg),
            deletion_inline_bg: overrides.deletion_inline_bg.or(self.deletion_inline_bg),
            attribute: overrides.attribute.or(self.attribute),
            comment: overrides.comment.or(self.comment),
            constant: overrides.constant.or(self.constant),
            constructor: overrides.constructor.or(self.constructor),
            function: overrides.function.or(self.function),
            keyword: overrides.keyword.or(self.keyword),
            label: overrides.label.or(self.label),
            module: overrides.module.or(self.module),
            number: overrides.number.or(self.number),
            operator: overrides.operator.or(self.operator),
            property: overrides.property.or(self.property),
            punctuation: overrides.punctuation.or(self.punctuation),
            string: overrides.string.or(self.string),
            tag: overrides.tag.or(self.tag),
            r#type: overrides.r#type.or(self.r#type),
            variable: overrides.variable.or(self.variable),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffSettings {
    pub line_background: DiffBackground,
    pub gutter_background: DiffGutterBackground,
    pub inline_background: DiffBackground,
    pub sign_style: DiffSignStyle,
    pub context_expansion: DiffContextExpansion,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            line_background: DiffBackground::Subtle,
            gutter_background: DiffGutterBackground::Delta,
            inline_background: DiffBackground::Strong,
            sign_style: DiffSignStyle::Bold,
            context_expansion: DiffContextExpansion::Full,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffContextExpansion {
    Lines(usize),
    Full,
}

impl DiffContextExpansion {
    pub fn expand_count(self, available: usize) -> usize {
        match self {
            Self::Lines(lines) => available.min(lines.max(1)),
            Self::Full => available,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffBackground {
    None,
    #[default]
    Subtle,
    Strong,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffGutterBackground {
    Base,
    #[default]
    Delta,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffSignStyle {
    Normal,
    #[default]
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutSetting {
    #[serde(alias = "auto", alias = "responsive")]
    Dynamic,
    Split,
    Unified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxMode {
    Enabled,
    #[default]
    Builtin,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxThemeConfig {
    Builtin { name: Option<String> },
    Ansi,
    Base16 { path: PathBuf },
    Base16MissingPath,
}

impl SyntaxThemeConfig {
    pub fn source(&self) -> SyntaxThemeSource {
        match self {
            Self::Builtin { .. } => SyntaxThemeSource::Builtin,
            Self::Ansi => SyntaxThemeSource::Ansi,
            Self::Base16 { .. } | Self::Base16MissingPath => SyntaxThemeSource::Base16,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Builtin { name } => name.as_deref(),
            Self::Ansi | Self::Base16 { .. } | Self::Base16MissingPath => None,
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Self::Base16 { path } => Some(path),
            Self::Builtin { .. } | Self::Ansi | Self::Base16MissingPath => None,
        }
    }
}

impl Default for SyntaxThemeConfig {
    fn default() -> Self {
        Self::Builtin {
            name: Some("system".to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxThemeSource {
    #[default]
    Builtin,
    Ansi,
    Base16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxLimits {
    pub max_source_bytes: usize,
    pub max_line_bytes: usize,
    pub cache_entries: usize,
    pub queue_entries: usize,
    pub prefetch_viewports: usize,
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES,
            max_line_bytes: DEFAULT_MAX_HIGHLIGHT_LINE_BYTES,
            cache_entries: DEFAULT_HIGHLIGHT_CACHE_ENTRIES,
            queue_entries: DEFAULT_HIGHLIGHT_QUEUE_ENTRIES,
            prefetch_viewports: DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxLanguageEnablement {
    Enabled,
    Disabled,
}

impl SyntaxLanguageEnablement {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxGrammarState {
    Bundled,
    Unavailable,
}

impl SyntaxGrammarState {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Bundled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxHighlightState {
    Ready,
    Unavailable,
}

impl SyntaxHighlightState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageStatus {
    pub language: String,
    pub enablement: SyntaxLanguageEnablement,
    pub grammar: SyntaxGrammarState,
    pub highlighting: SyntaxHighlightState,
    pub version: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxAddResult {
    pub added: Vec<String>,
    pub already_enabled: Vec<String>,
    pub without_highlights: Vec<String>,
    pub custom_mappings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxAddOptions {
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyntaxAvailableFilter {
    #[default]
    All,
    Installed,
    Enabled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxUpdateResult {
    pub bundled: Vec<String>,
    pub unavailable: Vec<String>,
    pub without_highlights: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxRemoveResult {
    pub removed: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxCleanResult {
    pub stale_records_removed: usize,
    pub enabled_languages_kept: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDoctorIssue {
    pub language: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDoctorReport {
    pub statuses: Vec<SyntaxLanguageStatus>,
    pub issues: Vec<SyntaxDoctorIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageSet {
    pub(crate) enabled: BTreeSet<String>,
    pub(crate) extensions: Vec<StoredLanguageMapping>,
    pub(crate) filenames: Vec<StoredLanguageMapping>,
}

impl SyntaxLanguageSet {
    pub fn load() -> MarkResult<Self> {
        let settings = load_settings()?;
        Self::load_with_mode(settings.mode)
    }

    pub fn load_with_mode(mode: SyntaxMode) -> MarkResult<Self> {
        let config = load_config()?;
        let installed = installed_language_set();
        Ok(Self {
            enabled: enabled_language_set_for_mode(mode, &config, &installed),
            extensions: config.extensions,
            filenames: config.filenames,
        })
    }

    pub fn from_enabled_languages(languages: &[String]) -> Self {
        let config = load_config().unwrap_or_default();
        Self {
            enabled: language_vec_to_set(languages),
            extensions: config.extensions,
            filenames: config.filenames,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self
            .enabled
            .iter()
            .any(|language| self.is_highlight_ready(language))
    }

    pub fn language_for_path(&self, path: &str) -> Option<String> {
        let language = detect_custom_language_from_path(path, &self.extensions, &self.filenames)
            .or_else(|| detect_language_name(path))?;
        let language = normalize_language_name(language);
        self.is_highlight_ready(&language).then_some(language)
    }

    pub fn is_highlight_ready(&self, language: &str) -> bool {
        self.enabled.contains(language) && has_highlights(language)
    }
}

pub struct SyntaxHighlighter {
    pub(crate) highlighter: mark_textmate::TextMateHighlighter,
    pub(crate) loaded_languages: BTreeSet<String>,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self {
            highlighter: mark_textmate::TextMateHighlighter::new(),
            loaded_languages: BTreeSet::new(),
        }
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(&mut self, language: &str, source: &str) -> MarkResult<HighlightedText> {
        let language = normalize_language_name(language.to_owned());
        let highlighted = self
            .highlighter
            .highlight(&language, source)
            .map_err(|error| MarkError::Usage(error.to_string()))?;
        self.loaded_languages.insert(language);
        Ok(highlighted)
    }
}
