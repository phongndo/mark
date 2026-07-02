use super::*;
use mark_core::MarkError;
use std::{
    collections::BTreeSet,
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_syntax_test_dir(name: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "mark-syntax-{name}-{}-{}",
        std::process::id(),
        NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("test dir should be created");
    path
}

#[test]
fn maps_extensions_to_language_names() {
    assert_eq!(normalize_language_name("rs".to_owned()), "rust");
    assert_eq!(normalize_language_name("rust".to_owned()), "rust");
    assert_eq!(normalize_language_name("shell".to_owned()), "bash");
    assert_eq!(normalize_language_name("c++".to_owned()), "cpp");
    assert_eq!(normalize_language_name("cc".to_owned()), "cpp");
    assert_eq!(normalize_language_name("cxx".to_owned()), "cpp");
    assert_eq!(normalize_language_name("js".to_owned()), "javascript");
    assert_eq!(normalize_language_name("ts".to_owned()), "typescript");
    assert_eq!(normalize_language_name("src/lib.rs".to_owned()), "rust");
}

#[test]
fn maps_common_basenames_to_language_names() {
    assert_eq!(normalize_language_name("Makefile".to_owned()), "make");
    assert_eq!(
        normalize_language_name("CMakeLists.txt".to_owned()),
        "cmake"
    );
    assert_eq!(
        normalize_language_name("BUILD.bazel".to_owned()),
        "starlark"
    );
    assert_eq!(normalize_language_name(".clang-format".to_owned()), "yaml");
}

#[test]
fn splits_highlighted_segments_by_line() {
    let mut lines = vec![HighlightedLine::default(), HighlightedLine::default()];
    let mut line = 0;
    push_source_segment(
        &mut lines,
        &mut line,
        10,
        b"hello\nworld",
        Some(SyntaxClass::String),
    );

    assert_eq!(line, 1);
    assert_eq!(lines[0].segments[0].byte_start, 10);
    assert_eq!(lines[0].segments[0].byte_end, 15);
    assert_eq!(lines[1].segments[0].byte_start, 16);
    assert_eq!(lines[1].segments[0].byte_end, 21);
    assert_eq!(lines[1].segments[0].class, Some(SyntaxClass::String));
}

#[test]
fn maps_scope_names_to_coarse_classes() {
    assert_eq!(syntax_class("keyword.function"), Some(SyntaxClass::Keyword));
    assert_eq!(
        syntax_class("entity.name.function"),
        Some(SyntaxClass::Function)
    );
    assert_eq!(syntax_class("typewriter"), None);
}

#[test]
fn detects_languages_by_path() {
    assert_eq!(
        detect_language_from_path("src/lib.rs").as_deref(),
        Some("rust")
    );
    assert_eq!(
        detect_language_from_path("Makefile").as_deref(),
        Some("make")
    );
    assert_eq!(
        detect_language_from_path("CMakeLists.txt").as_deref(),
        Some("cmake")
    );
    assert_eq!(
        detect_language_from_path(".clang-format").as_deref(),
        Some("yaml")
    );
    assert_eq!(
        detect_language_from_path("WORKSPACE").as_deref(),
        Some("starlark")
    );
}

#[test]
fn detects_custom_languages_by_extension_and_filename() {
    let extensions = vec![StoredLanguageMapping {
        pattern: "foo.bar".to_owned(),
        language: "customlang".to_owned(),
    }];
    let filenames = vec![StoredLanguageMapping {
        pattern: "Customfile".to_owned(),
        language: "customfilelang".to_owned(),
    }];

    assert_eq!(
        detect_custom_language_from_path("src/example.foo.bar", &extensions, &filenames).as_deref(),
        Some("customlang")
    );
    assert_eq!(
        detect_custom_language_from_path("src/CUSTOMFILE", &extensions, &filenames).as_deref(),
        Some("customfilelang")
    );
    assert_eq!(
        detect_custom_language_from_path("src/example.rs", &extensions, &filenames),
        None
    );

    let overlapping_extensions = vec![
        StoredLanguageMapping {
            pattern: "bar".to_owned(),
            language: "barlang".to_owned(),
        },
        StoredLanguageMapping {
            pattern: "foo.bar".to_owned(),
            language: "customlang".to_owned(),
        },
    ];
    assert_eq!(
        detect_custom_language_from_path("src/example.foo.bar", &overlapping_extensions, &[])
            .as_deref(),
        Some("customlang")
    );
    assert_eq!(
        detect_custom_language_from_path("src/example.bar", &overlapping_extensions, &[])
            .as_deref(),
        Some("barlang")
    );
}

#[test]
fn validates_custom_language_inputs() {
    assert!(ensure_safe_language_name("custom_lang1").is_ok());
    assert!(ensure_safe_language_name("custom-lang").is_err());
    assert_eq!(normalize_custom_extension(".foo.bar").unwrap(), "foo.bar");
    assert!(normalize_custom_extension("../foo").is_err());
    assert_eq!(
        normalize_custom_filename("Makefile.custom").unwrap(),
        "Makefile.custom"
    );
    assert!(normalize_custom_filename("dir/file").is_err());
}

#[test]
fn textmate_highlights_rust() {
    let mut highlighter = SyntaxHighlighter::new();

    let highlighted = highlighter
        .highlight("rust", "fn main() {\n    let value = 1;\n}")
        .expect("rust should highlight");

    assert_eq!(highlighted.lines.len(), 3);
    assert!(!highlighted.lines[0].segments.is_empty());
    assert!(highlighter.loaded_languages.contains("rust"));
}

#[test]
fn core_languages_are_bundled() {
    for language in CORE_LANGUAGES {
        assert!(
            mark_textmate::has_language(language),
            "core language should be bundled: {language}"
        );
    }
}

#[test]
fn removing_core_languages_is_rejected() {
    let requested = BTreeSet::from(["rust".to_owned(), "ada".to_owned()]);

    let error = reject_core_language_removal(&requested)
        .unwrap_err()
        .to_string();

    assert!(error.contains("cannot remove core syntax languages: rust"));
    assert!(!error.contains("ada"));
}

#[test]
fn update_all_targets_configured_and_bundled_languages() {
    let config = StoredSyntaxConfig {
        languages: vec!["ruby".to_owned(), "shell".to_owned()],
        ..StoredSyntaxConfig::default()
    };
    let installed = BTreeSet::from(["elixir".to_owned()]);

    let languages = update_all_language_set(&config, &installed);

    assert_eq!(
        languages,
        BTreeSet::from(["bash".to_owned(), "elixir".to_owned(), "ruby".to_owned()])
    );
}

#[test]
fn syntax_settings_default_to_builtin_system_colorscheme() {
    let settings = parse_settings("").expect("empty settings should parse");

    assert_eq!(settings.mode, SyntaxMode::Builtin);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Builtin);
    assert_eq!(settings.theme.name(), Some("system"));
    assert_eq!(settings.layout, None);
    assert!(settings.live_reload);
    assert!(settings.syntax_highlighting);
    assert!(!settings.line_wrapping);
    assert!(!settings.transparent_background);
    assert_eq!(settings.diff, DiffSettings::default());
    assert_eq!(settings.limits, SyntaxLimits::default());
}

#[test]
fn settings_write_path_preserves_legacy_settings_source() {
    let dir = temp_syntax_test_dir("settings-write-path");
    let settings_path = dir.join("config.toml");
    let legacy_settings_path = dir.join("syntax.toml");

    assert_eq!(
        crate::paths::settings_write_path_from_paths(
            settings_path.clone(),
            legacy_settings_path.clone()
        ),
        settings_path
    );

    fs::write(&legacy_settings_path, "colorscheme = \"ansi\"\n")
        .expect("legacy settings should be written");
    assert_eq!(
        crate::paths::settings_write_path_from_paths(
            settings_path.clone(),
            legacy_settings_path.clone()
        ),
        legacy_settings_path
    );

    fs::write(&settings_path, "line_wrapping = true\n").expect("settings should be written");
    assert_eq!(
        crate::paths::settings_write_path_from_paths(settings_path.clone(), legacy_settings_path),
        settings_path
    );
}

#[test]
fn syntax_settings_supports_persistent_ui_settings() {
    let settings = parse_settings(
        r#"
layout = "unified"
live_reload = false
syntax_highlighting = false
line_wrapping = true

[notifications]
mode = "debug"
corner = "bottom-left"
timeout_ms = 2500
max_visible = 5
"#,
    )
    .expect("settings should parse");

    assert_eq!(settings.layout, Some(LayoutSetting::Unified));
    assert!(!settings.live_reload);
    assert!(!settings.syntax_highlighting);
    assert!(settings.line_wrapping);
    assert_eq!(settings.notifications.mode(), NotificationMode::Debug);
    assert_eq!(settings.notifications.corner(), ToastCorner::BottomLeft);
    assert_eq!(settings.notifications.timeout_ms(), 2_500);
    assert_eq!(settings.notifications.max_visible(), 5);

    let settings = parse_settings("layout = \"dynamic\"\n").expect("settings should parse");
    assert_eq!(settings.layout, Some(LayoutSetting::Dynamic));
}

#[test]
fn syntax_settings_clamp_notification_values() {
    let settings =
        parse_settings("[notifications]\nmax_visible = 0\ntimeout_ms = 9223372036854775807\n")
            .expect("settings should parse");

    assert_eq!(settings.notifications.max_visible(), 1);
    assert_eq!(
        settings.notifications.timeout_ms(),
        MAX_NOTIFICATION_TIMEOUT_MS
    );
}

#[test]
fn syntax_settings_supports_ansi_colorscheme_and_limits() {
    let settings = parse_settings(
        r#"
mode = "builtin"
colorscheme = "ansi"
transparent_background = true

[limits]
max_source_kib = 64
max_line_kib = 4
cache_entries = 128
queue_entries = 256
prefetch_viewports = 2

[diff]
line_background = "subtle"
gutter_background = "delta"
inline_background = "strong"
sign_style = "bold"
context_expand = 42
"#,
    )
    .expect("settings should parse");

    assert_eq!(settings.mode, SyntaxMode::Builtin);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Ansi);
    assert_eq!(settings.theme.name(), None);
    assert!(settings.transparent_background);
    assert_eq!(settings.limits.max_source_bytes, 64 * 1024);
    assert_eq!(settings.limits.max_line_bytes, 4 * 1024);
    assert_eq!(settings.limits.cache_entries, 128);
    assert_eq!(settings.limits.queue_entries, 256);
    assert_eq!(settings.limits.prefetch_viewports, 2);
    assert_eq!(settings.diff.line_background, DiffBackground::Subtle);
    assert_eq!(settings.diff.gutter_background, DiffGutterBackground::Delta);
    assert_eq!(settings.diff.inline_background, DiffBackground::Strong);
    assert_eq!(settings.diff.sign_style, DiffSignStyle::Bold);
    assert_eq!(
        settings.diff.context_expansion,
        DiffContextExpansion::Lines(42)
    );
}

#[test]
fn syntax_settings_supports_full_context_expansion() {
    let settings =
        parse_settings("[diff]\ncontext_expand = \"full\"\n").expect("settings should parse");

    assert_eq!(settings.diff.context_expansion, DiffContextExpansion::Full);
    assert_eq!(settings.diff.context_expansion.expand_count(123), 123);
}

#[test]
fn syntax_settings_clamps_zero_context_expansion_to_one() {
    let settings = parse_settings("[diff]\ncontext_expand = 0\n").expect("settings should parse");

    assert_eq!(
        settings.diff.context_expansion,
        DiffContextExpansion::Lines(1)
    );
    assert_eq!(settings.diff.context_expansion.expand_count(10), 1);
}

#[test]
fn syntax_settings_supports_legacy_theme_key() {
    let settings = parse_settings("theme = \"ansi\"\n").expect("legacy theme key should parse");

    assert_eq!(settings.theme.source(), SyntaxThemeSource::Ansi);
    assert_eq!(settings.theme.name(), None);
}

#[test]
fn syntax_settings_prefers_colorscheme_over_legacy_theme() {
    let settings = parse_settings("colorscheme = \"system\"\ntheme = \"ansi\"\n")
        .expect("settings should parse");

    assert_eq!(settings.theme.source(), SyntaxThemeSource::Builtin);
    assert_eq!(settings.theme.name(), Some("system"));
}

#[test]
fn syntax_settings_supports_color_overrides() {
    let settings = parse_settings(
        r##"
colorscheme = "system"
bg = "#111315"
addition_bg = "#1f3025"

[colors]
addition_bg = "#222222"
deletion_bg = "#372526"
statusline_accent_bg = "#334455"
"##,
    )
    .expect("settings should parse");

    assert_eq!(settings.colors.bg.as_deref(), Some("#111315"));
    assert_eq!(settings.colors.addition_bg.as_deref(), Some("#1f3025"));
    assert_eq!(settings.colors.deletion_bg.as_deref(), Some("#372526"));
    assert_eq!(
        settings.colors.statusline_accent_bg.as_deref(),
        Some("#334455")
    );
}

#[test]
fn syntax_settings_supports_word_background_alias() {
    let settings = parse_settings(
        r#"
[diff]
line_background = "none"
word_background = "subtle"
sign_style = "normal"
"#,
    )
    .expect("settings should parse");

    assert_eq!(settings.diff.line_background, DiffBackground::None);
    assert_eq!(settings.diff.inline_background, DiffBackground::Subtle);
    assert_eq!(settings.diff.sign_style, DiffSignStyle::Normal);
}

#[test]
fn syntax_settings_accept_background_transparent_alias() {
    let settings =
        parse_settings("background_transparent = true").expect("settings should parse alias");

    assert!(settings.transparent_background);
}

#[test]
fn syntax_settings_supports_base16_colorscheme_table() {
    let settings = parse_settings(
        r#"
mode = "all"

[colorscheme]
source = "base16"
path = "~/themes/example.yaml"
"#,
    )
    .expect("settings should parse");

    assert_eq!(settings.mode, SyntaxMode::All);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Base16);
    assert_eq!(
        settings.theme.path().cloned(),
        Some(PathBuf::from("~/themes/example.yaml"))
    );
}

#[test]
fn syntax_modes_choose_enabled_languages_without_downloads() {
    let config = StoredSyntaxConfig {
        languages: vec!["definitely_custom_language".to_owned()],
        ..StoredSyntaxConfig::default()
    };
    let available = BTreeSet::from(["elixir".to_owned(), "rust".to_owned()]);

    let enabled = enabled_language_set_for_mode(SyntaxMode::Enabled, &config, &available);
    let builtin = enabled_language_set_for_mode(SyntaxMode::Builtin, &config, &available);
    let all = enabled_language_set_for_mode(SyntaxMode::All, &config, &available);

    assert!(enabled.contains("rust"));
    assert!(enabled.contains("definitely_custom_language"));
    assert!(!builtin.contains("definitely_custom_language"));
    assert!(builtin.contains("rust"));
    assert!(all.contains("rust"));
    assert!(all.contains("elixir"));
    assert!(!all.contains("definitely_custom_language"));
}

#[test]
fn language_set_falls_back_when_grammar_is_missing() {
    let languages = SyntaxLanguageSet {
        enabled: BTreeSet::from(["definitely_missing".to_owned()]),
        extensions: Vec::new(),
        filenames: Vec::new(),
    };

    assert!(!languages.is_highlight_ready("definitely_missing"));
    assert!(languages.is_empty());
}

#[test]
fn doctor_reports_stale_enabled_config() {
    let issues = doctor_issues(&[SyntaxLanguageStatus {
        language: "definitely_not_a_language".to_owned(),
        enablement: SyntaxLanguageEnablement::Enabled,
        grammar: SyntaxGrammarState::Unavailable,
        highlighting: SyntaxHighlightState::Unavailable,
        version: None,
        source: None,
    }]);

    assert_eq!(issues.len(), 1);
    assert!(issues[0].message.contains("no bundled TextMate grammar"));
}

#[test]
fn clean_cache_removes_stale_language_config() {
    let result = SyntaxCleanResult {
        stale_records_removed: 2,
        enabled_languages_kept: 3,
    };
    assert_eq!(result.stale_records_removed, 2);
    assert_eq!(result.enabled_languages_kept, 3);
}

#[test]
fn mark_error_import_is_still_usable_for_result_tests() {
    let error = MarkError::Usage("save failed".to_owned()).to_string();
    assert_eq!(error, "save failed");
}
