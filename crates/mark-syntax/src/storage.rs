use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::{
    BASENAME_LANGUAGES, CORE_LANGUAGES, DiffContextExpansion, DiffSettings, LANGUAGE_ALIASES,
    LEGACY_CONFIG_FILE, NotificationSettings, StoredDiffContextExpansion,
    StoredDiffContextExpansionMode, StoredDiffSettings, StoredLanguageMapping,
    StoredNotificationSettings, StoredSyntaxConfig, StoredSyntaxLimits, StoredSyntaxSettings,
    StoredSyntaxThemeConfig, StoredSyntaxThemeTable, SyntaxLimits, SyntaxMode, SyntaxSettings,
    SyntaxThemeConfig, SyntaxThemeSource, config_path, load_settings,
};
use mark_core::{MarkError, MarkResult};

pub(crate) fn config_home() -> MarkResult<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    #[cfg(windows)]
    {
        if let Some(path) = env::var_os("APPDATA").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        if let Some(path) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path).join("AppData").join("Roaming"));
        }
    }

    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config"))
        .ok_or_else(|| MarkError::Usage("could not determine config directory".to_owned()))
}

pub(crate) fn load_config() -> MarkResult<StoredSyntaxConfig> {
    let path = config_path()?;
    load_config_from_path(&path)
}

pub(crate) fn load_config_from_path(path: &Path) -> MarkResult<StoredSyntaxConfig> {
    if path.exists() {
        return read_config(path);
    }

    let legacy_path = legacy_config_path_for(path);
    if legacy_path.exists() {
        return read_config(&legacy_path);
    }

    Ok(StoredSyntaxConfig::default())
}

fn legacy_config_path_for(path: &Path) -> PathBuf {
    path.with_file_name(LEGACY_CONFIG_FILE)
}

fn read_config(path: &Path) -> MarkResult<StoredSyntaxConfig> {
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents).map_err(Into::into)
}

pub(crate) fn save_config(config: &StoredSyntaxConfig) -> MarkResult<()> {
    let path = config_path()?;
    write_config(&path, config)
}

fn write_config(path: &Path, config: &StoredSyntaxConfig) -> MarkResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec_pretty(config)?;
    fs::write(path, contents)?;
    Ok(())
}

pub(crate) fn parse_settings(contents: &str) -> Result<SyntaxSettings, toml::de::Error> {
    let stored: StoredSyntaxSettings = toml::from_str(contents)?;
    Ok(settings_from_stored(stored))
}

pub(crate) fn settings_from_stored(stored: StoredSyntaxSettings) -> SyntaxSettings {
    let colorscheme = stored.colorscheme.or(stored.theme);

    SyntaxSettings {
        mode: stored.mode.unwrap_or_default(),
        theme: colorscheme
            .map(theme_config_from_stored)
            .unwrap_or_default(),
        layout: stored.layout,
        live_reload: stored.live_reload.unwrap_or(true),
        syntax_highlighting: stored.syntax_highlighting.unwrap_or(true),
        line_wrapping: stored.line_wrapping,
        colors: stored.colors.overlay(stored.color_overrides),
        transparent_background: stored.transparent_background,
        diff: diff_from_stored(stored.diff),
        notifications: notifications_from_stored(stored.notifications),
        limits: limits_from_stored(stored.limits),
    }
}

pub(crate) fn notifications_from_stored(
    stored: StoredNotificationSettings,
) -> NotificationSettings {
    let defaults = NotificationSettings::default();
    NotificationSettings::new(
        stored.mode.unwrap_or(defaults.mode()),
        stored.corner.unwrap_or(defaults.corner()),
        stored.timeout_ms.unwrap_or(defaults.timeout_ms()),
        stored.max_visible.unwrap_or(defaults.max_visible()),
    )
}

pub(crate) fn diff_from_stored(stored: StoredDiffSettings) -> DiffSettings {
    let defaults = DiffSettings::default();
    DiffSettings {
        line_background: stored.line_background.unwrap_or(defaults.line_background),
        gutter_background: stored
            .gutter_background
            .unwrap_or(defaults.gutter_background),
        inline_background: stored
            .inline_background
            .or(stored.word_background)
            .unwrap_or(defaults.inline_background),
        sign_style: stored.sign_style.unwrap_or(defaults.sign_style),
        context_expansion: stored
            .context_expansion
            .map(diff_context_expansion_from_stored)
            .unwrap_or(defaults.context_expansion),
    }
}

pub(crate) fn diff_context_expansion_from_stored(
    stored: StoredDiffContextExpansion,
) -> DiffContextExpansion {
    match stored {
        StoredDiffContextExpansion::Lines(lines) => DiffContextExpansion::Lines(lines.max(1)),
        StoredDiffContextExpansion::Mode(StoredDiffContextExpansionMode::Full) => {
            DiffContextExpansion::Full
        }
    }
}

pub(crate) fn theme_config_from_stored(stored: StoredSyntaxThemeConfig) -> SyntaxThemeConfig {
    match stored {
        StoredSyntaxThemeConfig::Name(name) => theme_config_from_name(name),
        StoredSyntaxThemeConfig::Table(table) => theme_config_from_table(table),
    }
}

pub(crate) fn theme_config_from_name(name: String) -> SyntaxThemeConfig {
    let name = name.trim().to_owned();
    if let Some(source) = theme_source_from_name(&name) {
        return theme_config_from_source(source, None, None);
    }

    SyntaxThemeConfig::Builtin {
        name: (!name.is_empty()).then_some(name),
    }
}

pub(crate) fn theme_config_from_table(table: StoredSyntaxThemeTable) -> SyntaxThemeConfig {
    let name = table
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let source = table
        .source
        .or_else(|| name.as_deref().and_then(theme_source_from_name))
        .or_else(|| table.path.as_ref().map(|_| SyntaxThemeSource::Base16))
        .unwrap_or_default();
    let name = if theme_source_from_name(name.as_deref().unwrap_or_default()).is_some() {
        None
    } else {
        name
    };

    theme_config_from_source(source, name, table.path)
}

fn theme_config_from_source(
    source: SyntaxThemeSource,
    name: Option<String>,
    path: Option<PathBuf>,
) -> SyntaxThemeConfig {
    match source {
        SyntaxThemeSource::Builtin => SyntaxThemeConfig::Builtin { name },
        SyntaxThemeSource::Ansi => SyntaxThemeConfig::Ansi,
        SyntaxThemeSource::Base16 => path
            .map(|path| SyntaxThemeConfig::Base16 { path })
            .unwrap_or(SyntaxThemeConfig::Base16MissingPath),
    }
}

pub(crate) fn theme_source_from_name(name: &str) -> Option<SyntaxThemeSource> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ansi" | "terminal" => Some(SyntaxThemeSource::Ansi),
        "base16" => Some(SyntaxThemeSource::Base16),
        _ => None,
    }
}

pub(crate) fn limits_from_stored(stored: StoredSyntaxLimits) -> SyntaxLimits {
    let defaults = SyntaxLimits::default();
    SyntaxLimits {
        max_source_bytes: kib_or_default(stored.max_source_kib, defaults.max_source_bytes),
        max_line_bytes: kib_or_default(stored.max_line_kib, defaults.max_line_bytes),
        cache_entries: non_zero_or_default(stored.cache_entries, defaults.cache_entries),
        queue_entries: non_zero_or_default(stored.queue_entries, defaults.queue_entries),
        prefetch_viewports: stored
            .prefetch_viewports
            .unwrap_or(defaults.prefetch_viewports),
    }
}

pub(crate) fn kib_or_default(kib: Option<usize>, default: usize) -> usize {
    kib.and_then(|kib| kib.checked_mul(1024))
        .filter(|bytes| *bytes > 0)
        .unwrap_or(default)
}

pub(crate) fn non_zero_or_default(value: Option<usize>, default: usize) -> usize {
    value.filter(|value| *value > 0).unwrap_or(default)
}

pub(crate) fn enabled_language_set() -> MarkResult<BTreeSet<String>> {
    let settings = load_settings()?;
    let config = load_config()?;
    let available = installed_language_set();
    Ok(enabled_language_set_for_mode(
        settings.mode,
        &config,
        &available,
    ))
}

pub(crate) fn enabled_language_set_for_mode(
    mode: SyntaxMode,
    config: &StoredSyntaxConfig,
    available: &BTreeSet<String>,
) -> BTreeSet<String> {
    match mode {
        SyntaxMode::Enabled => enabled_language_set_from_config(config),
        SyntaxMode::Builtin | SyntaxMode::All => available.clone(),
    }
}

pub(crate) fn enabled_language_set_from_config(config: &StoredSyntaxConfig) -> BTreeSet<String> {
    let mut enabled = language_vec_to_set(&config.languages);
    enabled.extend(core_enabled_language_set());
    enabled
}

pub(crate) fn bundled_highlight_language_set() -> BTreeSet<String> {
    mark_textmate::available_languages().into_iter().collect()
}

pub(crate) fn core_enabled_language_set() -> BTreeSet<String> {
    CORE_LANGUAGES
        .iter()
        .filter_map(|language| mark_textmate::canonical_language(language))
        .collect()
}

pub(crate) fn reject_core_language_removal(requested: &BTreeSet<String>) -> MarkResult<()> {
    let core = core_enabled_language_set();
    let blocked = requested
        .intersection(&core)
        .cloned()
        .collect::<Vec<String>>();
    if blocked.is_empty() {
        return Ok(());
    }

    Err(MarkError::Usage(format!(
        "cannot remove core syntax languages: {}; use `mark --no-syntax` to disable syntax for a run",
        blocked.join(", ")
    )))
}

pub(crate) fn installed_language_set() -> BTreeSet<String> {
    bundled_highlight_language_set()
}

pub(crate) fn language_vec_to_set(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

pub(crate) fn normalize_language_names(languages: &[String]) -> BTreeSet<String> {
    languages
        .iter()
        .cloned()
        .map(normalize_language_name)
        .filter(|language| !language.is_empty())
        .collect()
}

pub(crate) fn normalize_language_name(language: String) -> String {
    let language = language.trim().to_ascii_lowercase();
    if language.is_empty() {
        return String::new();
    }
    if let Some(language) = detect_language_from_basename(&language) {
        return canonical_language_name(language);
    }
    let language = language.trim_start_matches('.');
    canonical_language_name(language)
}

fn canonical_language_name(language: &str) -> String {
    let language = language_alias(language).unwrap_or(language);
    mark_textmate::canonical_language(language)
        .or_else(|| mark_textmate::detect_language_from_path(language))
        .unwrap_or_else(|| language.to_owned())
}

pub(crate) fn detect_language_name(path: &str) -> Option<String> {
    detect_language_from_basename(path)
        .map(str::to_owned)
        .or_else(|| mark_textmate::detect_language_from_path(path))
}

pub(crate) fn language_alias(language: &str) -> Option<&'static str> {
    LANGUAGE_ALIASES
        .iter()
        .find_map(|(alias, target)| (*alias == language).then_some(*target))
}

pub(crate) fn detect_language_from_basename(path: &str) -> Option<&'static str> {
    let name = Path::new(path).file_name()?.to_str()?;
    BASENAME_LANGUAGES
        .iter()
        .find_map(|(basename, language)| name.eq_ignore_ascii_case(basename).then_some(*language))
}

pub(crate) fn detect_custom_language_from_path(
    path: &str,
    extensions: &[StoredLanguageMapping],
    filenames: &[StoredLanguageMapping],
) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?.to_ascii_lowercase();

    filenames
        .iter()
        .find(|mapping| name.eq_ignore_ascii_case(&mapping.pattern))
        .map(|mapping| mapping.language.clone())
        .or_else(|| {
            extensions
                .iter()
                .enumerate()
                .filter_map(|(index, mapping)| {
                    extension_mapping_match_len(&name, &mapping.pattern)
                        .map(|length| (length, index, mapping))
                })
                .max_by_key(|candidate| (candidate.0, candidate.1))
                .map(|(_, _, mapping)| mapping.language.clone())
        })
}

fn extension_mapping_match_len(filename: &str, extension: &str) -> Option<usize> {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    if extension.is_empty() {
        return None;
    }
    filename
        .ends_with(&format!(".{extension}"))
        .then_some(extension.len())
}

pub(crate) fn has_highlights(language: &str) -> bool {
    mark_textmate::has_language(language)
}

#[cfg(test)]
pub(crate) fn ensure_safe_language_name(language: &str) -> MarkResult<()> {
    let valid = !language.is_empty()
        && language
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(MarkError::Usage(format!(
            "language name must use lowercase ASCII letters, digits, and underscores: {language}"
        )))
    }
}

pub(crate) fn normalize_custom_extension(extension: &str) -> MarkResult<String> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let valid = !extension.is_empty()
        && extension
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.');
    if valid && !extension.contains("..") && !extension.contains('/') && !extension.contains('\\') {
        Ok(extension)
    } else {
        Err(MarkError::Usage(format!(
            "invalid extension mapping: {extension}"
        )))
    }
}

pub(crate) fn normalize_custom_filename(filename: &str) -> MarkResult<String> {
    let filename = filename.trim();
    let valid = !filename.is_empty()
        && !filename.contains('/')
        && !filename.contains('\\')
        && filename != "."
        && filename != "..";
    if valid {
        Ok(filename.to_owned())
    } else {
        Err(MarkError::Usage(format!(
            "invalid filename mapping: {filename}"
        )))
    }
}

pub(crate) fn upsert_extension_mappings(
    mappings: &mut Vec<StoredLanguageMapping>,
    language: &str,
    extensions: &[String],
) -> MarkResult<Vec<String>> {
    let mut added = Vec::new();
    for extension in extensions {
        let pattern = normalize_custom_extension(extension)?;
        if upsert_mapping(mappings, &pattern, language) {
            added.push(pattern);
        }
    }
    Ok(added)
}

pub(crate) fn upsert_filename_mappings(
    mappings: &mut Vec<StoredLanguageMapping>,
    language: &str,
    filenames: &[String],
) -> MarkResult<Vec<String>> {
    let mut added = Vec::new();
    for filename in filenames {
        let pattern = normalize_custom_filename(filename)?;
        if upsert_filename_mapping(mappings, &pattern, language) {
            added.push(pattern);
        }
    }
    Ok(added)
}

fn upsert_filename_mapping(
    mappings: &mut Vec<StoredLanguageMapping>,
    pattern: &str,
    language: &str,
) -> bool {
    if let Some(index) = mappings
        .iter()
        .position(|mapping| mapping.pattern.eq_ignore_ascii_case(pattern))
    {
        let mut changed =
            mappings[index].pattern != pattern || mappings[index].language != language;
        mappings[index].pattern = pattern.to_owned();
        mappings[index].language = language.to_owned();

        let mut cursor = index + 1;
        while cursor < mappings.len() {
            if mappings[cursor].pattern.eq_ignore_ascii_case(pattern) {
                mappings.remove(cursor);
                changed = true;
            } else {
                cursor += 1;
            }
        }
        return changed;
    }
    mappings.push(StoredLanguageMapping {
        pattern: pattern.to_owned(),
        language: language.to_owned(),
    });
    true
}

pub(crate) fn upsert_mapping(
    mappings: &mut Vec<StoredLanguageMapping>,
    pattern: &str,
    language: &str,
) -> bool {
    if let Some(mapping) = mappings
        .iter_mut()
        .find(|mapping| mapping.pattern == pattern)
    {
        let changed = mapping.language != language;
        mapping.language = language.to_owned();
        return changed;
    }
    mappings.push(StoredLanguageMapping {
        pattern: pattern.to_owned(),
        language: language.to_owned(),
    });
    true
}
