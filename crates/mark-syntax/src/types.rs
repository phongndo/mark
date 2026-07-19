use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, AtomicUsize, Ordering, fence},
    },
};

use crate::{
    detect_custom_language_from_path, detect_language_name, enabled_language_set_for_mode,
    has_highlights, installed_language_set, language_vec_to_set, load_config, load_settings,
    normalize_language_name,
};
use mark_core::{MarkError, MarkResult};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use unicode_width::UnicodeWidthChar;

pub const DEFAULT_ANNOTATION_HINT_KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxClass {
    Attribute,
    Comment,
    Constant,
    Constructor,
    Function,
    Keyword,
    Label,
    Module,
    Number,
    Operator,
    Property,
    Punctuation,
    String,
    Tag,
    Type,
    Variable,
}

/// A compact reference to one complete, ordered TextMate scope stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ScopeStackRef(pub u32);

/// An interned TextMate scope name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ScopeAtomId(pub u32);

// Rendering can resolve a base theme and a post-theme scope override for each
// segment. Keep both generations warm instead of making them evict each other.
const STYLE_CACHE_SLOTS: usize = 2;
// Stable slot epochs are always even. Writers publish this sentinel while a
// slot is being reset, then advance its previous epoch by two.
const STYLE_CACHE_UPDATING_EPOCH: u64 = 1;

/// Immutable scope data shared by every line in one highlighting result.
///
/// Entry zero is always the empty stack. Keeping this table separate from
/// segments allows themes to be changed without tokenizing the source again.
#[derive(Debug)]
pub struct HighlightScopeTable {
    atoms: Vec<Arc<str>>,
    stacks: Vec<Arc<[ScopeAtomId]>>,
    resolved_styles: Vec<[AtomicU64; STYLE_CACHE_SLOTS]>,
    style_cache_generations: [AtomicU64; STYLE_CACHE_SLOTS],
    style_cache_epochs: [AtomicU64; STYLE_CACHE_SLOTS],
    style_cache_next_slot: AtomicUsize,
    style_cache_lock: RwLock<()>,
    style_cache_hits: AtomicU64,
    style_cache_misses: AtomicU64,
    style_cache_stats_enabled: bool,
}

impl Clone for HighlightScopeTable {
    fn clone(&self) -> Self {
        Self {
            atoms: self.atoms.clone(),
            stacks: self.stacks.clone(),
            resolved_styles: (0..self.stacks.len())
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            style_cache_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_epochs: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_next_slot: AtomicUsize::new(0),
            style_cache_lock: RwLock::new(()),
            style_cache_hits: AtomicU64::new(0),
            style_cache_misses: AtomicU64::new(0),
            style_cache_stats_enabled: self.style_cache_stats_enabled,
        }
    }
}

impl PartialEq for HighlightScopeTable {
    fn eq(&self, other: &Self) -> bool {
        self.atoms == other.atoms && self.stacks == other.stacks
    }
}

impl Eq for HighlightScopeTable {}

impl Default for HighlightScopeTable {
    fn default() -> Self {
        Self {
            atoms: Vec::new(),
            stacks: vec![Arc::from([])],
            resolved_styles: vec![std::array::from_fn(|_| AtomicU64::new(0))],
            style_cache_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_epochs: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_next_slot: AtomicUsize::new(0),
            style_cache_lock: RwLock::new(()),
            style_cache_hits: AtomicU64::new(0),
            style_cache_misses: AtomicU64::new(0),
            style_cache_stats_enabled: style_cache_stats_enabled(),
        }
    }
}

impl HighlightScopeTable {
    pub(crate) fn empty_shared() -> Arc<Self> {
        static EMPTY: std::sync::OnceLock<Arc<HighlightScopeTable>> = std::sync::OnceLock::new();
        Arc::clone(EMPTY.get_or_init(|| Arc::new(HighlightScopeTable::default())))
    }

    /// Builds a small standalone table for diagnostics and theme tooling.
    pub fn from_scope_names(scopes: &[&str]) -> (Self, ScopeStackRef) {
        let atoms = scopes
            .iter()
            .map(|scope| Arc::<str>::from(*scope))
            .collect::<Vec<_>>();
        let stack = (0..atoms.len())
            .map(|index| ScopeAtomId(index as u32))
            .collect::<Vec<_>>();
        (
            Self {
                atoms,
                stacks: vec![Arc::from([]), Arc::from(stack)],
                resolved_styles: (0..2)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                style_cache_generations: std::array::from_fn(|_| AtomicU64::new(0)),
                style_cache_epochs: std::array::from_fn(|_| AtomicU64::new(0)),
                style_cache_next_slot: AtomicUsize::new(0),
                style_cache_lock: RwLock::new(()),
                style_cache_hits: AtomicU64::new(0),
                style_cache_misses: AtomicU64::new(0),
                style_cache_stats_enabled: style_cache_stats_enabled(),
            },
            ScopeStackRef(1),
        )
    }

    pub fn stack(&self, stack: ScopeStackRef) -> Option<&[ScopeAtomId]> {
        self.stacks.get(stack.0 as usize).map(AsRef::as_ref)
    }

    pub fn atom(&self, atom: ScopeAtomId) -> Option<&str> {
        self.atoms.get(atom.0 as usize).map(AsRef::as_ref)
    }

    pub fn stack_names(&self, stack: ScopeStackRef) -> impl Iterator<Item = &str> {
        self.stack(stack)
            .unwrap_or_default()
            .iter()
            .filter_map(|atom| self.atom(*atom))
    }

    pub fn stack_count(&self) -> usize {
        self.stacks.len()
    }

    pub fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    pub fn memory_bytes(&self) -> usize {
        let cached_styles =
            self.resolved_styles.capacity() * std::mem::size_of::<[AtomicU64; STYLE_CACHE_SLOTS]>();
        std::mem::size_of::<Self>()
            .saturating_add(self.atoms.len() * std::mem::size_of::<Arc<str>>())
            .saturating_add(self.atoms.iter().map(|atom| atom.len()).sum::<usize>())
            .saturating_add(self.stacks.len() * std::mem::size_of::<Arc<[ScopeAtomId]>>())
            .saturating_add(
                self.stacks
                    .iter()
                    .map(|stack| stack.len() * std::mem::size_of::<ScopeAtomId>())
                    .sum::<usize>(),
            )
            .saturating_add(cached_styles)
    }

    /// Cumulative resolved-style cache hits and misses for benchmark tooling.
    pub fn style_cache_stats(&self) -> (u64, u64) {
        (
            self.style_cache_hits.load(Ordering::Relaxed),
            self.style_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn from_parts(atoms: Vec<Arc<str>>, stacks: Vec<Arc<[ScopeAtomId]>>) -> Self {
        let stack_count = stacks.len();
        Self {
            atoms,
            stacks,
            resolved_styles: (0..stack_count)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            style_cache_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_epochs: std::array::from_fn(|_| AtomicU64::new(0)),
            style_cache_next_slot: AtomicUsize::new(0),
            style_cache_lock: RwLock::new(()),
            style_cache_hits: AtomicU64::new(0),
            style_cache_misses: AtomicU64::new(0),
            style_cache_stats_enabled: style_cache_stats_enabled(),
        }
    }

    pub(crate) fn cached_style(&self, theme: u64, stack: ScopeStackRef) -> (usize, Option<u64>) {
        let (slot, style) = loop {
            // Cached rendering is overwhelmingly a read-only operation. Use
            // a monotonically advancing slot epoch for seqlock-style
            // validation so a warm segment does not acquire the table-wide
            // RwLock. Unlike the theme generation, the epoch cannot return to
            // its prior value when a slot transitions A -> C -> A.
            let mut found = None;
            for (slot, generation) in self.style_cache_generations.iter().enumerate() {
                let epoch = self.style_cache_epochs[slot].load(Ordering::Acquire);
                if epoch == STYLE_CACHE_UPDATING_EPOCH
                    || generation.load(Ordering::Acquire) != theme
                {
                    continue;
                }
                let style = self
                    .resolved_styles
                    .get(stack.0 as usize)
                    .and_then(|styles| styles[slot].load(Ordering::Acquire).checked_sub(1));
                // The acquire keeps the entry load ahead of epoch validation.
                // If it observes a late release publication, the publisher's
                // stable epoch happens-before the validation load, preventing
                // that entry from being paired with stale slot metadata.
                if self.style_cache_epochs[slot].load(Ordering::Relaxed) == epoch {
                    found = Some((slot, style));
                    break;
                }
            }
            if let Some(found) = found {
                break found;
            }

            // Only installing a new theme generation needs exclusive access.
            // `cache_style` retains a shared lock while publishing an entry,
            // so a reset cannot overwrite another generation's value.
            let _write = self
                .style_cache_lock
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self
                .style_cache_generations
                .iter()
                .all(|generation| generation.load(Ordering::Relaxed) != theme)
            {
                let slot =
                    self.style_cache_next_slot.fetch_add(1, Ordering::Relaxed) % STYLE_CACHE_SLOTS;
                // Mark the slot unstable before clearing it. The write lock
                // serializes installers, while the stamped epoch lets
                // lock-free readers detect every reuse, including A -> C -> A.
                let previous_epoch = self.style_cache_epochs[slot]
                    .swap(STYLE_CACHE_UPDATING_EPOCH, Ordering::Acquire);
                debug_assert_ne!(previous_epoch, STYLE_CACHE_UPDATING_EPOCH);
                fence(Ordering::Release);
                for styles in &self.resolved_styles {
                    styles[slot].store(0, Ordering::Relaxed);
                }
                self.style_cache_generations[slot].store(theme, Ordering::Release);
                self.style_cache_epochs[slot]
                    .store(previous_epoch.wrapping_add(2), Ordering::Release);
            }
        };
        if self.style_cache_stats_enabled {
            if style.is_some() {
                self.style_cache_hits.fetch_add(1, Ordering::Relaxed);
            } else {
                self.style_cache_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Return the slot even on a miss so cache_style can populate exactly
        // the generation reserved by this lookup.
        (slot, style)
    }

    pub(crate) fn cache_style(&self, theme: u64, stack: ScopeStackRef, slot: usize, style: u64) {
        let _read = self
            .style_cache_lock
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.style_cache_generations[slot].load(Ordering::Acquire) == theme
            && let Some(entries) = self.resolved_styles.get(stack.0 as usize)
        {
            // Readers validate the slot without taking the lock. Publish with
            // release ordering so a reader that observes this entry cannot
            // validate it against generation metadata from before this slot
            // was installed.
            entries[slot].store(style + 1, Ordering::Release);
        }
    }
}

fn style_cache_stats_enabled() -> bool {
    // Read once: `HighlightScopeTable::default()` runs per tokenized source,
    // and `getenv` takes a process-global lock.
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MARK_TEXTMATE_THEME_CACHE_STATS").is_some())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub class: Option<SyntaxClass>,
    /// Exact TextMate scopes. `class` is retained only as a coarse fallback.
    pub scope_stack: ScopeStackRef,
}

impl SyntaxSegment {
    pub fn new(byte_start: usize, byte_end: usize, class: Option<SyntaxClass>) -> Self {
        debug_assert!(byte_start <= byte_end);
        Self {
            byte_start,
            byte_end,
            class,
            scope_stack: ScopeStackRef::default(),
        }
    }

    pub fn with_scope_stack(mut self, scope_stack: ScopeStackRef) -> Self {
        self.scope_stack = scope_stack;
        self
    }

    pub fn len(&self) -> usize {
        self.byte_end.saturating_sub(self.byte_start)
    }

    pub fn is_empty(&self) -> bool {
        self.byte_start >= self.byte_end
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedLine {
    pub fingerprint: LineTextFingerprint,
    pub segments: Vec<SyntaxSegment>,
    pub scope_table: Arc<HighlightScopeTable>,
}

impl Default for HighlightedLine {
    fn default() -> Self {
        Self::new("")
    }
}

impl HighlightedLine {
    pub fn new(text: &str) -> Self {
        Self {
            fingerprint: LineTextFingerprint::from_text(text),
            segments: Vec::new(),
            scope_table: HighlightScopeTable::empty_shared(),
        }
    }

    pub fn matches_text(&self, text: &str) -> bool {
        self.fingerprint.matches(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineTextFingerprint {
    byte_len: usize,
    hash: u64,
}

impl Default for LineTextFingerprint {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl LineTextFingerprint {
    pub fn from_text(text: &str) -> Self {
        Self {
            byte_len: text.len(),
            hash: stable_text_hash(text.as_bytes()),
        }
    }

    pub fn byte_len(self) -> usize {
        self.byte_len
    }

    pub fn matches(self, text: &str) -> bool {
        self.byte_len == text.len() && self.hash == stable_text_hash(text.as_bytes())
    }

    pub(crate) fn without_trailing_byte(self, byte: u8) -> Self {
        // FNV-1a update is `(hash ^ byte) * PRIME`. PRIME is odd, so it has a
        // multiplicative inverse modulo 2^64 and the final byte can be removed
        // without hashing the line a second time.
        const PRIME_INVERSE: u64 = 0xce96_5057_aff6_957b;
        Self {
            byte_len: self.byte_len.saturating_sub(1),
            hash: self.hash.wrapping_mul(PRIME_INVERSE) ^ u64::from(byte),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedText {
    pub lines: Vec<HighlightedLine>,
}

fn stable_text_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub(crate) const CONFIG_DIR: &str = "mark";
pub(crate) const CONFIG_FILE: &str = "syntax.json";
pub(crate) const LEGACY_CONFIG_FILE: &str = "tree-sitter.json";
pub(crate) const SETTINGS_FILE: &str = "config.toml";
pub(crate) const LEGACY_SETTINGS_FILE: &str = "syntax.toml";
pub(crate) const BUNDLED_GRAMMAR_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_HIGHLIGHT_LINE_BYTES: usize = 8 * 1024;
pub const DEFAULT_HIGHLIGHT_CACHE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_QUEUE_ENTRIES: usize = 512;
pub const DEFAULT_HIGHLIGHT_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_HIGHLIGHT_QUEUE_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS: usize = 1;
pub const MAX_NOTIFICATION_TIMEOUT_MS: u64 = 10_000;

pub(crate) const CORE_LANGUAGES: &[&str] = &[
    "rust",
    "c",
    "cpp",
    "python",
    "typescript",
    "javascript",
    "jsx",
    "tsx",
    "bash",
    "toml",
    "json",
    "yaml",
    "markdown",
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
    ("git-ignore", "ignore"),
    ("gitignore", "ignore"),
    ("hcl", "terraform"),
    ("ignorefile", "ignore"),
    ("ipynb", "json"),
    ("js", "javascript"),
    ("jsx", "jsx"),
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
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".dockerignore", "ignore"),
    (".gitignore", "ignore"),
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
    pub(crate) decorations: Option<StoredDecorationSettings>,
    pub(crate) layout: Option<LayoutSetting>,
    pub(crate) live_reload: Option<bool>,
    pub(crate) syntax_highlighting: Option<bool>,
    #[serde(alias = "show_full_file")]
    pub(crate) full_file: Option<bool>,
    #[serde(default, alias = "word_wrap", alias = "wrap_lines")]
    pub(crate) line_wrapping: bool,
    #[serde(default)]
    pub(crate) colors: ColorOverrides,
    #[serde(default)]
    pub(crate) syntax_rules: Vec<SyntaxRuleOverride>,
    #[serde(default, flatten)]
    pub(crate) color_overrides: ColorOverrides,
    #[serde(default, alias = "background_transparent", alias = "transparent_bg")]
    pub(crate) transparent_background: Option<bool>,
    #[serde(default)]
    pub(crate) diff: StoredDiffSettings,
    #[serde(default)]
    pub(crate) notifications: StoredNotificationSettings,
    #[serde(default)]
    pub(crate) annotations: AnnotationSettings,
    #[serde(default)]
    pub(crate) limits: StoredSyntaxLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AnnotationSettings {
    #[serde(
        default = "default_annotation_hint_keys",
        deserialize_with = "deserialize_annotation_hint_keys"
    )]
    pub hint_keys: String,
    #[serde(default)]
    pub uppercase_hints: bool,
}

impl Default for AnnotationSettings {
    fn default() -> Self {
        Self {
            hint_keys: default_annotation_hint_keys(),
            uppercase_hints: false,
        }
    }
}

fn default_annotation_hint_keys() -> String {
    DEFAULT_ANNOTATION_HINT_KEYS.to_owned()
}

fn deserialize_annotation_hint_keys<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let hint_keys = String::deserialize(deserializer)?;
    let mut seen = HashSet::new();
    let mut count = 0usize;
    for character in hint_keys.chars() {
        count += 1;
        if character.is_control() || UnicodeWidthChar::width(character) != Some(1) {
            return Err(D::Error::custom(
                "annotations.hint_keys must contain only printable single-width characters",
            ));
        }
        if !seen.insert(character.to_ascii_lowercase()) {
            return Err(D::Error::custom(
                "annotations.hint_keys characters must be unique (ignoring ASCII case)",
            ));
        }
    }
    if count < 2 {
        return Err(D::Error::custom(
            "annotations.hint_keys must contain at least two characters",
        ));
    }
    Ok(hint_keys)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredNotificationSettings {
    pub(crate) mode: Option<NotificationMode>,
    pub(crate) corner: Option<ToastCorner>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) max_visible: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum StoredDecorationSettings {
    Mode(DecorationSetting),
    Table(StoredDecorationSettingsTable),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StoredDecorationSettingsTable {
    pub(crate) mode: Option<DecorationSetting>,
    pub(crate) empty_fill: Option<bool>,
    #[serde(default)]
    pub(crate) no_borders: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorationSettings {
    pub mode: DecorationSetting,
    pub empty_fill: bool,
    pub no_borders: bool,
}

impl Default for DecorationSettings {
    fn default() -> Self {
        Self {
            mode: DecorationSetting::Auto,
            empty_fill: true,
            no_borders: false,
        }
    }
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
    #[serde(alias = "show_full_file")]
    pub(crate) full_file: Option<bool>,
    pub(crate) line_background: Option<DiffBackground>,
    pub(crate) gutter_background: Option<DiffGutterBackground>,
    pub(crate) inline_background: Option<DiffBackground>,
    #[serde(alias = "word_background", alias = "word_diff_background")]
    pub(crate) word_background: Option<DiffBackground>,
    pub(crate) sign_style: Option<DiffSignStyle>,
    #[serde(alias = "empty_diff_fill")]
    pub(crate) empty_fill: Option<bool>,
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
    pub(crate) cache_kib: Option<usize>,
    pub(crate) queue_entries: Option<usize>,
    pub(crate) queue_kib: Option<usize>,
    pub(crate) prefetch_viewports: Option<usize>,
    pub(crate) worker_threads: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSettings {
    pub mode: SyntaxMode,
    pub theme: SyntaxThemeConfig,
    pub decorations: DecorationSettings,
    pub layout: Option<LayoutSetting>,
    pub live_reload: bool,
    pub syntax_highlighting: bool,
    pub full_file: bool,
    pub line_wrapping: bool,
    pub colors: ColorOverrides,
    pub syntax_rules: Vec<SyntaxRuleOverride>,
    pub transparent_background: bool,
    /// The explicitly configured transparency value, if any. When absent,
    /// theme-local transparency is preserved.
    pub transparent_background_override: Option<bool>,
    pub diff: DiffSettings,
    pub notifications: NotificationSettings,
    pub annotations: AnnotationSettings,
    pub limits: SyntaxLimits,
}

impl Default for SyntaxSettings {
    fn default() -> Self {
        Self {
            mode: SyntaxMode::Builtin,
            theme: SyntaxThemeConfig::default(),
            decorations: DecorationSettings::default(),
            layout: None,
            live_reload: true,
            syntax_highlighting: true,
            full_file: false,
            line_wrapping: false,
            colors: ColorOverrides::default(),
            syntax_rules: Vec::new(),
            transparent_background: false,
            transparent_background_override: None,
            diff: DiffSettings::default(),
            notifications: NotificationSettings::default(),
            annotations: AnnotationSettings::default(),
            limits: SyntaxLimits::default(),
        }
    }
}

/// A post-theme TextMate selector override from `[[syntax_rules]]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct SyntaxRuleOverride {
    pub scope: String,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub font_style: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecorationSetting {
    #[default]
    Auto,
    Fancy,
    Minimal,
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
    pub empty_fill: bool,
    pub context_expansion: DiffContextExpansion,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            line_background: DiffBackground::Subtle,
            gutter_background: DiffGutterBackground::Delta,
            inline_background: DiffBackground::Strong,
            sign_style: DiffSignStyle::Bold,
            empty_fill: false,
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
    pub cache_bytes: usize,
    pub queue_entries: usize,
    pub queue_bytes: usize,
    pub prefetch_viewports: usize,
    pub worker_threads: usize,
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES,
            max_line_bytes: DEFAULT_MAX_HIGHLIGHT_LINE_BYTES,
            cache_entries: DEFAULT_HIGHLIGHT_CACHE_ENTRIES,
            cache_bytes: DEFAULT_HIGHLIGHT_CACHE_BYTES,
            queue_entries: DEFAULT_HIGHLIGHT_QUEUE_ENTRIES,
            queue_bytes: DEFAULT_HIGHLIGHT_QUEUE_BYTES,
            prefetch_viewports: DEFAULT_HIGHLIGHT_PREFETCH_VIEWPORTS,
            worker_threads: default_highlight_worker_threads(),
        }
    }
}

pub fn default_highlight_worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|cores| (cores.get() / 2).clamp(1, 4))
        .unwrap_or(1)
}

impl SyntaxLimits {
    pub(crate) fn engine_line_cache_entries(self) -> usize {
        // The outer TUI cache is file/hunk sized. A single outer entry can
        // contain many source lines, so give the tokenizer a proportional
        // line cache rather than limiting it to only a few hundred lines.
        self.cache_entries.saturating_mul(64).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxGrammarSource {
    Bundled,
}

impl SyntaxGrammarSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxGrammarInfo {
    version: String,
    source: SyntaxGrammarSource,
}

impl SyntaxGrammarInfo {
    pub fn bundled(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            source: SyntaxGrammarSource::Bundled,
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn source(&self) -> SyntaxGrammarSource {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxLanguageRuntimeState {
    Ready(SyntaxGrammarInfo),
    MissingHighlights(SyntaxGrammarInfo),
    MissingGrammar,
}

impl SyntaxLanguageRuntimeState {
    pub fn into_available(self) -> Option<SyntaxAvailableRuntimeState> {
        match self {
            Self::Ready(grammar) => Some(SyntaxAvailableRuntimeState::Ready(grammar)),
            Self::MissingHighlights(grammar) => {
                Some(SyntaxAvailableRuntimeState::MissingHighlights(grammar))
            }
            Self::MissingGrammar => None,
        }
    }

    pub fn grammar(&self) -> Option<&SyntaxGrammarInfo> {
        match self {
            Self::Ready(grammar) | Self::MissingHighlights(grammar) => Some(grammar),
            Self::MissingGrammar => None,
        }
    }

    pub fn is_grammar_available(&self) -> bool {
        self.grammar().is_some()
    }

    pub fn is_highlight_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxAvailableRuntimeState {
    Ready(SyntaxGrammarInfo),
    MissingHighlights(SyntaxGrammarInfo),
}

impl SyntaxAvailableRuntimeState {
    pub fn grammar(&self) -> &SyntaxGrammarInfo {
        match self {
            Self::Ready(grammar) | Self::MissingHighlights(grammar) => grammar,
        }
    }

    pub fn is_highlight_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxLanguageState {
    Enabled(SyntaxLanguageRuntimeState),
    Disabled(SyntaxAvailableRuntimeState),
}

impl SyntaxLanguageState {
    pub fn enabled(runtime: SyntaxLanguageRuntimeState) -> Self {
        Self::Enabled(runtime)
    }

    pub fn disabled(runtime: SyntaxAvailableRuntimeState) -> Self {
        Self::Disabled(runtime)
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    pub fn grammar(&self) -> Option<&SyntaxGrammarInfo> {
        match self {
            Self::Enabled(runtime) => runtime.grammar(),
            Self::Disabled(runtime) => Some(runtime.grammar()),
        }
    }

    pub fn is_grammar_available(&self) -> bool {
        self.grammar().is_some()
    }

    pub fn is_highlight_ready(&self) -> bool {
        match self {
            Self::Enabled(runtime) => runtime.is_highlight_ready(),
            Self::Disabled(runtime) => runtime.is_highlight_ready(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageStatus {
    pub language: String,
    pub state: SyntaxLanguageState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxAddResult {
    pub added: Vec<String>,
    pub already_enabled: Vec<String>,
    pub unavailable: Vec<String>,
    pub custom_mappings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxAddOptions {
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
}

impl SyntaxAddOptions {
    pub fn has_mappings(&self) -> bool {
        !self.extensions.is_empty() || !self.filenames.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxAddRequest {
    Languages(SyntaxLanguageSelection),
    LanguageWithMappings {
        language: String,
        options: SyntaxAddOptions,
    },
}

impl SyntaxAddRequest {
    pub fn from_cli(languages: Vec<String>, options: SyntaxAddOptions) -> MarkResult<Self> {
        if options.has_mappings() {
            match languages.as_slice() {
                [language] => Ok(Self::LanguageWithMappings {
                    language: language.clone(),
                    options,
                }),
                [] => Err(MarkError::Usage("provide at least one language".to_owned())),
                _ => Err(MarkError::Usage(
                    "use --ext or --filename with exactly one language".to_owned(),
                )),
            }
        } else {
            SyntaxLanguageSelection::new(languages).map(Self::Languages)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxLanguageSelection {
    languages: Vec<String>,
}

impl SyntaxLanguageSelection {
    pub fn new(languages: Vec<String>) -> MarkResult<Self> {
        if languages.is_empty() {
            Err(MarkError::Usage("provide at least one language".to_owned()))
        } else {
            Ok(Self { languages })
        }
    }

    pub fn as_slice(&self) -> &[String] {
        &self.languages
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxUpdateSelection {
    All,
    Languages(SyntaxLanguageSelection),
}

impl SyntaxUpdateSelection {
    pub fn from_cli(languages: Vec<String>, all: bool) -> MarkResult<Self> {
        match (all, languages.is_empty()) {
            (true, true) => Ok(Self::All),
            (true, false) => Err(MarkError::Usage(
                "use `mark syntax update --all` without language names".to_owned(),
            )),
            (false, true) => Err(MarkError::Usage(
                "provide at least one language or use --all".to_owned(),
            )),
            (false, false) => SyntaxLanguageSelection::new(languages).map(Self::Languages),
        }
    }
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxRemoveResult {
    pub removed: Vec<String>,
    pub missing: Vec<String>,
    pub kept_core: Vec<String>,
    pub removed_custom_mappings: Vec<String>,
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
        if let Some(language) =
            detect_custom_language_from_path(path, &self.extensions, &self.filenames)
        {
            let language = normalize_language_name(language);
            if self.is_highlight_ready(&language) {
                return Some(language);
            }
        }

        let language = normalize_language_name(detect_language_name(path)?);
        self.is_highlight_ready(&language).then_some(language)
    }

    pub fn is_highlight_ready(&self, language: &str) -> bool {
        self.enabled.contains(language) && has_highlights(language)
    }
}

#[derive(Default)]
pub struct SyntaxHighlighter {
    pub(crate) engine: crate::engine::SyntaxEngine,
    pub(crate) loaded_languages: BTreeSet<String>,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(&mut self, language: &str, source: &str) -> MarkResult<HighlightedText> {
        let language = normalize_language_name(language.to_owned());
        let highlighted = self.engine.highlight(&language, source)?;
        self.loaded_languages.insert(language);
        Ok(highlighted)
    }

    /// Enables low-overhead native-engine counters for diagnostics and
    /// benchmarks. Highlight output is unaffected.
    pub fn set_engine_counters_enabled(&mut self, enabled: bool) {
        self.engine.set_counters_enabled(enabled);
    }

    pub fn take_engine_counters(&mut self) -> crate::engine::counters::EngineCounters {
        self.engine.take_counters()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_cache_slot_epoch_changes_across_theme_aba() {
        let (table, stack) =
            HighlightScopeTable::from_scope_names(&["source.test", "entity.name.test"]);
        let install = |theme, style| {
            let (slot, cached) = table.cached_style(theme, stack);
            assert!(cached.is_none());
            table.cache_style(theme, stack, slot, style);
            slot
        };

        let theme_a = 10;
        let a_slot = install(theme_a, 100);
        let first_a_epoch = table.style_cache_epochs[a_slot].load(Ordering::Relaxed);
        install(20, 200);
        assert_eq!(install(30, 300), a_slot);
        install(40, 400);
        assert_eq!(install(theme_a, 100), a_slot);

        let second_a_epoch = table.style_cache_epochs[a_slot].load(Ordering::Relaxed);
        assert_ne!(first_a_epoch, second_a_epoch);
        assert_eq!(table.cached_style(theme_a, stack).1, Some(100));
    }
}
