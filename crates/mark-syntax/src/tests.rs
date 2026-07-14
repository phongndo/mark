use super::*;
use mark_core::MarkError;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn normalizes_language_aliases_with_the_bundled_catalog() {
    assert_eq!(normalize_language_name("rust".to_owned()), "rust");
    assert_eq!(normalize_language_name("shell".to_owned()), "bash");
    assert_eq!(normalize_language_name("c++".to_owned()), "cpp");
    assert_eq!(normalize_language_name("cc".to_owned()), "cpp");
    assert_eq!(normalize_language_name("cxx".to_owned()), "cpp");
    assert_eq!(normalize_language_name("js".to_owned()), "javascript");
    assert_eq!(normalize_language_name("jsx".to_owned()), "jsx");
    assert_eq!(normalize_language_name("ts".to_owned()), "typescript");
    assert_eq!(normalize_language_name("tsx".to_owned()), "tsx");
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
    assert_eq!(normalize_language_name(".gitignore".to_owned()), "ignore");
    assert_eq!(normalize_language_name("gitignore".to_owned()), "ignore");
    assert_eq!(normalize_language_name("ignorefile".to_owned()), "ignore");
    assert_eq!(normalize_language_name("git-ignore".to_owned()), "ignore");
    assert_eq!(
        normalize_language_name(".dockerignore".to_owned()),
        "ignore"
    );
}

#[test]
fn legacy_git_ignore_alias_works_through_primary_apis() {
    let languages = SyntaxLanguageSet {
        enabled: BTreeSet::from(["ignore".to_owned()]),
        extensions: Vec::new(),
        filenames: Vec::new(),
    };

    assert_eq!(
        languages.language_for_path(".gitignore").as_deref(),
        Some("ignore")
    );
    assert_eq!(
        languages.language_for_path(".dockerignore").as_deref(),
        Some("ignore")
    );

    let mut highlighter = SyntaxHighlighter::new();
    let highlighted = highlighter
        .highlight("git-ignore", "target/\n*.log\n")
        .expect("the legacy git-ignore id should resolve to the ignore grammar");
    assert!(!highlighted.lines.is_empty());
    assert!(highlighter.loaded_languages.contains("ignore"));
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
fn detects_bundled_language_paths_and_basenames() {
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
    assert_eq!(
        detect_language_from_path("src/module.mjs").as_deref(),
        Some("javascript")
    );
    assert_eq!(
        detect_language_from_path("include/project.h").as_deref(),
        Some("c")
    );
    assert_eq!(
        detect_language_from_path("analysis.r").as_deref(),
        Some("r")
    );
    assert_eq!(
        detect_language_from_path("lint.reek").as_deref(),
        Some("yaml")
    );
    assert_eq!(normalize_language_name("hbs".to_owned()), "handlebars");
    assert_eq!(normalize_language_name("jade".to_owned()), "pug");
}

#[test]
fn tex_extension_uses_vscode_latex_language() {
    assert_eq!(
        detect_language_from_path("homework.tex").as_deref(),
        Some("latex")
    );
    assert!(
        has_language("tex"),
        "plain TeX remains explicitly selectable"
    );
}

#[test]
fn backend_catalog_contains_the_core_pack() {
    assert_eq!(installed_language_set().len(), 256);
    assert!(core_enabled_language_set().contains("rust"));
    assert!(core_enabled_language_set().contains("bash"));
    assert!(core_language_set().contains("rust"));
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
fn upserts_custom_filename_mappings_case_insensitively() {
    let mut filenames = vec![
        StoredLanguageMapping {
            pattern: "Readme".to_owned(),
            language: "rust".to_owned(),
        },
        StoredLanguageMapping {
            pattern: "README".to_owned(),
            language: "text".to_owned(),
        },
    ];

    let updated =
        upsert_filename_mappings(&mut filenames, "markdown", &["README".to_owned()]).unwrap();

    assert_eq!(updated, vec!["README".to_owned()]);
    assert_eq!(
        filenames,
        vec![StoredLanguageMapping {
            pattern: "README".to_owned(),
            language: "markdown".to_owned(),
        }]
    );
    assert_eq!(
        detect_custom_language_from_path("docs/readme", &[], &filenames).as_deref(),
        Some("markdown")
    );

    let unchanged =
        upsert_filename_mappings(&mut filenames, "markdown", &["README".to_owned()]).unwrap();

    assert!(unchanged.is_empty());
    assert_eq!(filenames.len(), 1);
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
fn syntax_add_request_requires_one_language_for_custom_mappings() {
    let options = SyntaxAddOptions {
        extensions: vec!["foo".to_owned()],
        filenames: Vec::new(),
    };

    assert!(SyntaxAddRequest::from_cli(vec!["rust".to_owned()], options.clone()).is_ok());
    assert!(SyntaxAddRequest::from_cli(Vec::new(), options.clone()).is_err());
    assert!(
        SyntaxAddRequest::from_cli(vec!["rust".to_owned(), "ruby".to_owned()], options).is_err()
    );
}

#[test]
fn syntax_add_reports_unavailable_language_without_mutating_config() {
    let mut config = StoredSyntaxConfig {
        languages: vec!["shell".to_owned()],
        extensions: vec![StoredLanguageMapping {
            pattern: "foo".to_owned(),
            language: "rust".to_owned(),
        }],
        filenames: vec![StoredLanguageMapping {
            pattern: "Rustfile".to_owned(),
            language: "rust".to_owned(),
        }],
    };
    let original_config = config.clone();
    let request =
        SyntaxAddRequest::Languages(SyntaxLanguageSelection::new(vec!["typo".to_owned()]).unwrap());

    let result =
        add_languages_to_config(&mut config, request, &BTreeSet::from(["rust".to_owned()]))
            .unwrap();

    assert_eq!(
        result,
        SyntaxAddResult {
            added: Vec::new(),
            already_enabled: Vec::new(),
            unavailable: vec!["typo".to_owned()],
            custom_mappings: Vec::new(),
        }
    );
    assert_eq!(config, original_config);
}

#[test]
fn syntax_add_skips_unavailable_languages_when_adding_mixed_selection() {
    let mut config = StoredSyntaxConfig::default();
    let request = SyntaxAddRequest::Languages(
        SyntaxLanguageSelection::new(vec!["rust".to_owned(), "typo".to_owned()]).unwrap(),
    );

    let result =
        add_languages_to_config(&mut config, request, &BTreeSet::from(["rust".to_owned()]))
            .unwrap();

    assert_eq!(result.added, vec!["rust".to_owned()]);
    assert!(result.already_enabled.is_empty());
    assert_eq!(result.unavailable, vec!["typo".to_owned()]);
    assert_eq!(config.languages, vec!["rust".to_owned()]);
}

#[test]
fn syntax_add_accepts_custom_mappings_for_a_ready_language() {
    let mut config = StoredSyntaxConfig::default();
    let request = SyntaxAddRequest::LanguageWithMappings {
        language: "rust".to_owned(),
        options: SyntaxAddOptions {
            extensions: vec!["rs.in".to_owned()],
            filenames: vec!["Rustfile".to_owned()],
        },
    };

    let result =
        add_languages_to_config(&mut config, request, &BTreeSet::from(["rust".to_owned()]))
            .unwrap();

    assert_eq!(result.added, vec!["rust"]);
    assert_eq!(result.custom_mappings.len(), 2);
}

#[test]
fn syntax_add_rejects_custom_mappings_for_unavailable_language() {
    let mut config = StoredSyntaxConfig {
        languages: vec!["rust".to_owned()],
        ..StoredSyntaxConfig::default()
    };
    let original_config = config.clone();
    let request = SyntaxAddRequest::LanguageWithMappings {
        language: "definitely_missing".to_owned(),
        options: SyntaxAddOptions {
            extensions: vec!["rs".to_owned()],
            filenames: vec!["Cargo.toml".to_owned()],
        },
    };

    let error = add_languages_to_config(&mut config, request, &BTreeSet::new())
        .unwrap_err()
        .to_string();

    assert!(error.contains("cannot add custom mappings"));
    assert!(error.contains("definitely_missing"));
    assert_eq!(config, original_config);
}

#[test]
fn syntax_update_selection_rejects_ambiguous_all_flag() {
    assert!(SyntaxUpdateSelection::from_cli(Vec::new(), true).is_ok());
    assert!(SyntaxUpdateSelection::from_cli(vec!["rust".to_owned()], false).is_ok());
    assert!(SyntaxUpdateSelection::from_cli(Vec::new(), false).is_err());
    assert!(SyntaxUpdateSelection::from_cli(vec!["rust".to_owned()], true).is_err());
}

#[test]
fn direct_highlighting_uses_the_bundled_native_backend() {
    let mut highlighter = SyntaxHighlighter::new();

    let highlighted = highlighter
        .highlight("rust", "fn main() {\n    let value = 1;\n}")
        .unwrap();

    assert_eq!(highlighted.lines.len(), 3);
    assert!(
        highlighted
            .lines
            .iter()
            .any(|line| { line.segments.iter().any(|segment| segment.class.is_some()) })
    );
    assert!(highlighter.loaded_languages.contains("rust"));
}

#[test]
fn bundled_markdown_loads_private_yang_and_twig_dependencies() {
    let mut highlighter = SyntaxHighlighter::new();
    let source = "```yang\nmodule demo {\n namespace \"urn:demo\";\n}\n```\n```twig\n{% if user %}{{ user.name }}{% endif %}\n```";
    let highlighted = highlighter.highlight("markdown", source).unwrap();

    let yang_line = "module demo {";
    assert!(highlighted.lines[1].segments.iter().any(|segment| {
        yang_line.get(segment.byte_start..segment.byte_end) == Some("module")
            && segment.class == Some(SyntaxClass::Keyword)
    }));
    let twig_line = "{% if user %}{{ user.name }}{% endif %}";
    assert!(highlighted.lines[6].segments.iter().any(|segment| {
        twig_line.get(segment.byte_start..segment.byte_end) == Some("if")
            && segment.class == Some(SyntaxClass::Keyword)
    }));
}

#[test]
fn core_language_identity_survives_without_a_backend() {
    let requested = BTreeSet::from(["rust".to_owned(), "ada".to_owned()]);

    let error = reject_core_language_removal(&requested)
        .unwrap_err()
        .to_string();

    assert!(error.contains("cannot remove core syntax languages: rust"));
    assert!(!error.contains("ada"));
    let config = StoredSyntaxConfig::default();
    assert!(reject_unconfigured_core_language_removal(&config, &requested).is_err());
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
fn syntax_update_all_reports_unavailable_configured_languages() {
    let dir = unique_temp_dir("syntax-update-all-configured");

    with_xdg_config_home(&dir, || {
        let mark_dir = dir.join(CONFIG_DIR);
        fs::create_dir_all(&mark_dir).unwrap();
        fs::write(
            mark_dir.join(CONFIG_FILE),
            r#"{"languages": ["definitely_missing"]}"#,
        )
        .unwrap();

        let result = update_languages(SyntaxUpdateSelection::All).unwrap();

        assert!(
            result
                .unavailable
                .contains(&"definitely_missing".to_owned())
        );
    });

    remove_temp_dir(&dir);
}

#[test]
fn load_config_returns_default_when_syntax_json_is_absent() {
    let dir = unique_temp_dir("missing-config");
    let path = dir.join(CONFIG_FILE);

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config, StoredSyntaxConfig::default());
    assert!(!path.exists());

    remove_temp_dir(&dir);
}

#[test]
fn load_config_reads_syntax_json() {
    let dir = unique_temp_dir("syntax-json-config");
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, r#"{"languages": ["rust"]}"#).unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.languages, vec!["rust"]);

    remove_temp_dir(&dir);
}

#[test]
fn load_config_reads_legacy_tree_sitter_json_without_migrating() {
    let dir = unique_temp_dir("legacy-tree-sitter-config");
    let path = dir.join(CONFIG_FILE);
    let legacy_path = dir.join(LEGACY_CONFIG_FILE);
    fs::write(
        &legacy_path,
        r#"{
  "languages": ["ruby", "shell"],
  "parsers": ["ignored legacy parser state"],
  "extensions": [{"pattern": "foo", "language": "ruby"}],
  "filenames": [{"pattern": "Buildfile", "language": "ruby"}]
}"#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.languages, vec!["ruby", "shell"]);
    assert_eq!(
        config.extensions,
        vec![StoredLanguageMapping {
            pattern: "foo".to_owned(),
            language: "ruby".to_owned(),
        }]
    );
    assert_eq!(
        config.filenames,
        vec![StoredLanguageMapping {
            pattern: "Buildfile".to_owned(),
            language: "ruby".to_owned(),
        }]
    );
    assert!(!path.exists());

    remove_temp_dir(&dir);
}

#[cfg(unix)]
#[test]
fn load_config_reads_legacy_tree_sitter_json_from_read_only_config_dir() {
    use std::os::unix::fs::PermissionsExt;

    let dir = unique_temp_dir("read-only-legacy-config");
    let path = dir.join(CONFIG_FILE);
    let legacy_path = dir.join(LEGACY_CONFIG_FILE);
    fs::write(&legacy_path, r#"{"languages": ["ruby"]}"#).unwrap();

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let result = load_config_from_path(&path);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    let config = result.unwrap();
    assert_eq!(config.languages, vec!["ruby"]);
    assert!(!path.exists());

    remove_temp_dir(&dir);
}

#[test]
fn load_config_prefers_syntax_json_over_legacy_tree_sitter_json() {
    let dir = unique_temp_dir("syntax-json-preferred");
    let path = dir.join(CONFIG_FILE);
    let legacy_path = dir.join(LEGACY_CONFIG_FILE);
    fs::write(&path, r#"{"languages": ["rust"]}"#).unwrap();
    fs::write(
        &legacy_path,
        r#"{
  "languages": ["ruby"],
  "extensions": [{"pattern": "foo", "language": "ruby"}]
}"#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.languages, vec!["rust"]);
    assert!(config.extensions.is_empty());
    assert!(config.filenames.is_empty());

    remove_temp_dir(&dir);
}

#[test]
fn load_settings_returns_default_when_config_toml_is_absent() {
    let dir = unique_temp_dir("missing-settings");
    let path = dir.join(SETTINGS_FILE);

    let settings = load_settings_from_path(&path).unwrap();

    assert_eq!(settings, SyntaxSettings::default());
    assert!(!path.exists());

    remove_temp_dir(&dir);
}

#[test]
fn load_settings_reads_config_toml() {
    let dir = unique_temp_dir("config-toml-settings");
    let path = dir.join(SETTINGS_FILE);
    fs::write(
        &path,
        r##"
mode = "enabled"
colorscheme = "ansi"
layout = "unified"
"##,
    )
    .unwrap();

    let settings = load_settings_from_path(&path).unwrap();

    assert_eq!(settings.mode, SyntaxMode::Enabled);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Ansi);
    assert_eq!(settings.layout, Some(LayoutSetting::Unified));

    remove_temp_dir(&dir);
}

#[test]
fn load_settings_falls_back_to_legacy_syntax_toml() {
    let dir = unique_temp_dir("legacy-syntax-toml-settings");
    let path = dir.join(SETTINGS_FILE);
    let legacy_path = dir.join(LEGACY_SETTINGS_FILE);
    fs::write(
        &legacy_path,
        r##"
mode = "enabled"
theme = "ansi"
layout = "split"
"##,
    )
    .unwrap();

    let settings = load_settings_from_path(&path).unwrap();

    assert_eq!(settings.mode, SyntaxMode::Enabled);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Ansi);
    assert_eq!(settings.layout, Some(LayoutSetting::Split));
    assert!(!path.exists());

    remove_temp_dir(&dir);
}

#[test]
fn load_settings_prefers_config_toml_over_legacy_syntax_toml() {
    let dir = unique_temp_dir("config-toml-preferred");
    let path = dir.join(SETTINGS_FILE);
    let legacy_path = dir.join(LEGACY_SETTINGS_FILE);
    fs::write(&path, "colorscheme = \"system\"\n").unwrap();
    fs::write(&legacy_path, "theme = \"ansi\"\n").unwrap();

    let settings = load_settings_from_path(&path).unwrap();

    assert_eq!(settings.theme.source(), SyntaxThemeSource::Builtin);
    assert_eq!(settings.theme.name(), Some("system"));

    remove_temp_dir(&dir);
}

#[test]
fn settings_write_path_preserves_legacy_settings_source() {
    let dir = unique_temp_dir("settings-write-path");
    let path = dir.join(SETTINGS_FILE);
    let legacy_path = dir.join(LEGACY_SETTINGS_FILE);

    assert_eq!(
        settings_write_path_from_paths(path.clone(), legacy_path.clone()),
        path
    );

    fs::write(&legacy_path, "colorscheme = \"ansi\"\n").unwrap();
    assert_eq!(
        settings_write_path_from_paths(path.clone(), legacy_path.clone()),
        legacy_path
    );

    fs::write(&path, "line_wrapping = true\n").unwrap();
    assert_eq!(
        settings_write_path_from_paths(path.clone(), legacy_path),
        path
    );

    remove_temp_dir(&dir);
}

#[test]
fn syntax_settings_default_to_builtin_system_colorscheme() {
    let settings = parse_settings("").expect("empty settings should parse");

    assert_eq!(settings.mode, SyntaxMode::Builtin);
    assert_eq!(settings.theme.source(), SyntaxThemeSource::Builtin);
    assert_eq!(settings.theme.name(), Some("system"));
    assert_eq!(settings.decorations, DecorationSettings::default());
    assert_eq!(settings.layout, None);
    assert!(settings.live_reload);
    assert!(settings.syntax_highlighting);
    assert!(!settings.line_wrapping);
    assert!(!settings.transparent_background);
    assert_eq!(settings.diff, DiffSettings::default());
    assert_eq!(settings.limits, SyntaxLimits::default());
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
fn syntax_settings_supports_persistent_ui_settings() {
    let settings = parse_settings(
        r#"
layout = "unified"
live_reload = false
syntax_highlighting = false
line_wrapping = true

[decorations]
mode = "minimal"
empty_fill = false
no_borders = true

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
    assert_eq!(settings.decorations.mode, DecorationSetting::Minimal);
    assert!(!settings.decorations.empty_fill);
    assert!(settings.decorations.no_borders);
    assert_eq!(settings.notifications.mode(), NotificationMode::Debug);
    assert_eq!(settings.notifications.corner(), ToastCorner::BottomLeft);
    assert_eq!(settings.notifications.timeout_ms(), 2_500);
    assert_eq!(settings.notifications.max_visible(), 5);

    let settings = parse_settings("layout = \"dynamic\"\n").expect("settings should parse");
    assert_eq!(settings.layout, Some(LayoutSetting::Dynamic));
}

#[test]
fn syntax_settings_applies_legacy_empty_fill_only_as_decorations_fallback() {
    let settings = parse_settings("[diff]\nempty_fill = false\n").expect("settings should parse");
    assert!(!settings.decorations.empty_fill);

    let settings = parse_settings(
        r#"
[decorations]
empty_fill = false

[diff]
empty_fill = true
"#,
    )
    .expect("settings should parse");
    assert!(!settings.decorations.empty_fill);
    assert!(settings.diff.empty_fill);

    let settings = parse_settings(
        r#"
[decorations]
empty_fill = true

[diff]
empty_diff_fill = false
"#,
    )
    .expect("settings should parse");
    assert!(settings.decorations.empty_fill);
    assert!(!settings.diff.empty_fill);
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
cache_kib = 8192
queue_entries = 256
queue_kib = 4096
prefetch_viewports = 2
worker_threads = 3

[diff]
line_background = "subtle"
gutter_background = "delta"
inline_background = "strong"
sign_style = "bold"
empty_fill = true
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
    assert_eq!(settings.limits.cache_bytes, 8192 * 1024);
    assert_eq!(settings.limits.queue_entries, 256);
    assert_eq!(settings.limits.queue_bytes, 4096 * 1024);
    assert_eq!(settings.limits.prefetch_viewports, 2);
    assert_eq!(settings.limits.worker_threads, 3);
    assert_eq!(settings.diff.line_background, DiffBackground::Subtle);
    assert_eq!(settings.diff.gutter_background, DiffGutterBackground::Delta);
    assert_eq!(settings.diff.inline_background, DiffBackground::Strong);
    assert_eq!(settings.diff.sign_style, DiffSignStyle::Bold);
    assert!(settings.diff.empty_fill);
    assert!(settings.decorations.empty_fill);
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
fn syntax_settings_accepts_empty_diff_fill_alias() {
    let settings =
        parse_settings("[diff]\nempty_diff_fill = true\n").expect("settings should parse");

    assert!(settings.diff.empty_fill);
    assert!(settings.decorations.empty_fill);
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
fn syntax_settings_supports_scope_aware_rules() {
    let settings = parse_settings(
        r##"
[[syntax_rules]]
scope = "support.function"
foreground = "#91cbff"

[[syntax_rules]]
scope = "entity.name.function"
foreground = "#dbb7ff"
font_style = "bold"
"##,
    )
    .expect("settings should parse");

    assert_eq!(settings.syntax_rules.len(), 2);
    assert_eq!(settings.syntax_rules[0].scope, "support.function");
    assert_eq!(settings.syntax_rules[1].font_style.as_deref(), Some("bold"));
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

    assert!(enabled.contains("definitely_custom_language"));
    assert!(enabled.contains("rust"));
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
fn language_set_falls_back_from_invalid_custom_mappings_to_bundled_detection() {
    let languages = SyntaxLanguageSet {
        enabled: core_enabled_language_set(),
        extensions: vec![StoredLanguageMapping {
            pattern: "rs".to_owned(),
            language: "definitely_missing".to_owned(),
        }],
        filenames: vec![StoredLanguageMapping {
            pattern: "Cargo.toml".to_owned(),
            language: "definitely_missing".to_owned(),
        }],
    };

    assert_eq!(
        languages.language_for_path("src/lib.rs").as_deref(),
        Some("rust")
    );
    assert_eq!(
        languages.language_for_path("Cargo.toml").as_deref(),
        Some("toml")
    );
}

#[test]
fn doctor_reports_stale_enabled_config() {
    let issues = doctor_issues(&[SyntaxLanguageStatus {
        language: "definitely_not_a_language".to_owned(),
        state: SyntaxLanguageState::enabled(SyntaxLanguageRuntimeState::MissingGrammar),
    }]);

    assert_eq!(issues.len(), 1);
    assert!(issues[0].message.contains("no bundled syntax grammar"));
}

#[test]
fn doctor_reports_bundled_languages_as_ready() {
    let dir = unique_temp_dir("doctor-with-native-backend");

    with_xdg_config_home(&dir, || {
        save_config(&StoredSyntaxConfig {
            languages: vec!["ruby".to_owned()],
            ..StoredSyntaxConfig::default()
        })
        .unwrap();
        let settings = settings_path().unwrap();
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(settings, "mode = \"enabled\"\n").unwrap();

        let report = doctor().unwrap();

        let ruby = report
            .statuses
            .iter()
            .find(|status| status.language == "ruby")
            .expect("ruby should be bundled");
        assert!(ruby.state.is_highlight_ready());
        assert!(
            report
                .issues
                .iter()
                .all(|issue| issue.language != "backend")
        );
    });

    remove_temp_dir(&dir);
}

#[test]
fn clean_cache_preserves_valid_bundled_language_configuration() {
    let dir = unique_temp_dir("clean-with-native-backend");

    with_xdg_config_home(&dir, || {
        let config = StoredSyntaxConfig {
            languages: vec!["rust".to_owned()],
            extensions: vec![StoredLanguageMapping {
                pattern: "rs.in".to_owned(),
                language: "rust".to_owned(),
            }],
            ..StoredSyntaxConfig::default()
        };
        save_config(&config).unwrap();

        let result = clean_cache().unwrap();
        assert_eq!(result.stale_records_removed, 0);
        assert_eq!(load_config().unwrap(), config);
    });

    remove_temp_dir(&dir);
}

#[test]
fn clean_config_preserves_core_aliases_and_the_supplied_catalog() {
    let mut config = StoredSyntaxConfig {
        languages: vec![
            "gitignore".to_owned(),
            "shell".to_owned(),
            "definitely_missing".to_owned(),
        ],
        ..StoredSyntaxConfig::default()
    };
    let available = BTreeSet::from(["ignore".to_owned()]);

    let result = clean_language_config(&mut config, &available);

    assert_eq!(result.stale_records_removed, 1);
    assert_eq!(result.enabled_languages_kept, 2);
    assert_eq!(config.languages, vec!["bash", "ignore"]);
}

#[test]
fn clean_cache_normalizes_custom_mapping_language_aliases() {
    let mut config = StoredSyntaxConfig {
        extensions: vec![StoredLanguageMapping {
            pattern: "ignore".to_owned(),
            language: "gitignore".to_owned(),
        }],
        filenames: vec![StoredLanguageMapping {
            pattern: ".dockerignore".to_owned(),
            language: "ignorefile".to_owned(),
        }],
        ..StoredSyntaxConfig::default()
    };
    let available = BTreeSet::from(["ignore".to_owned()]);

    let result = clean_language_config(&mut config, &available);

    assert_eq!(result.stale_records_removed, 0);
    assert_eq!(result.enabled_languages_kept, 0);
    assert_eq!(config.extensions[0].language, "ignore");
    assert_eq!(config.filenames[0].language, "ignore");
}

#[test]
fn remove_languages_removes_alias_custom_mappings() {
    let mut config = StoredSyntaxConfig {
        languages: vec!["gitignore".to_owned(), "shell".to_owned()],
        extensions: vec![
            StoredLanguageMapping {
                pattern: "ignore".to_owned(),
                language: "gitignore".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "sh".to_owned(),
                language: "shell".to_owned(),
            },
        ],
        filenames: vec![
            StoredLanguageMapping {
                pattern: ".gitignore".to_owned(),
                language: "ignorefile".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "Makefile".to_owned(),
                language: "make".to_owned(),
            },
        ],
    };
    let requested = normalize_language_names(&["git-ignore".to_owned()]);

    let result = remove_languages_from_config(&mut config, &requested);

    assert_eq!(
        result,
        SyntaxRemoveResult {
            removed: vec!["ignore".to_owned()],
            missing: Vec::new(),
            kept_core: Vec::new(),
            removed_custom_mappings: vec![
                "*.ignore -> ignore".to_owned(),
                ".gitignore -> ignore".to_owned(),
            ],
        }
    );
    assert_eq!(config.languages, vec!["bash"]);
    assert_eq!(
        config.extensions,
        vec![StoredLanguageMapping {
            pattern: "sh".to_owned(),
            language: "bash".to_owned(),
        }]
    );
    assert_eq!(
        config.filenames,
        vec![StoredLanguageMapping {
            pattern: "Makefile".to_owned(),
            language: "make".to_owned(),
        }]
    );
}

#[test]
fn remove_languages_cleans_core_custom_mappings_without_disabling_core() {
    let mut config = StoredSyntaxConfig {
        languages: vec!["rust".to_owned(), "ruby".to_owned()],
        extensions: vec![
            StoredLanguageMapping {
                pattern: "rs.in".to_owned(),
                language: "rust".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "rb.in".to_owned(),
                language: "ruby".to_owned(),
            },
        ],
        filenames: vec![
            StoredLanguageMapping {
                pattern: "Rustfile".to_owned(),
                language: "rust".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "Rakefile".to_owned(),
                language: "ruby".to_owned(),
            },
        ],
    };
    let requested = BTreeSet::from(["rust".to_owned()]);

    reject_unconfigured_core_language_removal(&config, &requested).unwrap();
    let result = remove_languages_from_config(&mut config, &requested);

    assert_eq!(
        result,
        SyntaxRemoveResult {
            removed: Vec::new(),
            missing: Vec::new(),
            kept_core: vec!["rust".to_owned()],
            removed_custom_mappings: vec![
                "*.rs.in -> rust".to_owned(),
                "Rustfile -> rust".to_owned(),
            ],
        }
    );
    assert_eq!(config.languages, vec!["ruby".to_owned()]);
    assert_eq!(
        config.extensions,
        vec![StoredLanguageMapping {
            pattern: "rb.in".to_owned(),
            language: "ruby".to_owned(),
        }]
    );
    assert_eq!(
        config.filenames,
        vec![StoredLanguageMapping {
            pattern: "Rakefile".to_owned(),
            language: "ruby".to_owned(),
        }]
    );
}

#[test]
fn syntax_rm_preserves_core_identity_while_removing_custom_mappings() {
    let dir = unique_temp_dir("syntax-rm-without-backend");

    with_xdg_config_home(&dir, || {
        save_config(&StoredSyntaxConfig {
            languages: vec!["rust".to_owned()],
            extensions: vec![StoredLanguageMapping {
                pattern: "rs.in".to_owned(),
                language: "rust".to_owned(),
            }],
            filenames: vec![StoredLanguageMapping {
                pattern: "Rustfile".to_owned(),
                language: "rust".to_owned(),
            }],
        })
        .unwrap();

        let result = remove_languages(&["rust".to_owned()]).unwrap();

        assert_eq!(result.removed, Vec::<String>::new());
        assert_eq!(result.missing, Vec::<String>::new());
        assert_eq!(result.kept_core, vec!["rust".to_owned()]);
        assert_eq!(
            result.removed_custom_mappings,
            vec!["*.rs.in -> rust".to_owned(), "Rustfile -> rust".to_owned()]
        );
        assert_eq!(load_config().unwrap(), StoredSyntaxConfig::default());
    });

    remove_temp_dir(&dir);
}

#[test]
fn clean_config_preserves_custom_mappings_in_the_supplied_catalog() {
    let mut config = StoredSyntaxConfig {
        languages: vec!["definitely_missing".to_owned(), "rust".to_owned()],
        extensions: vec![
            StoredLanguageMapping {
                pattern: "rs".to_owned(),
                language: "definitely_missing".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "foo".to_owned(),
                language: "rust".to_owned(),
            },
        ],
        filenames: vec![
            StoredLanguageMapping {
                pattern: "Cargo.toml".to_owned(),
                language: "definitely_missing".to_owned(),
            },
            StoredLanguageMapping {
                pattern: "Rustfile".to_owned(),
                language: "rust".to_owned(),
            },
        ],
    };
    let result = clean_language_config(&mut config, &BTreeSet::from(["rust".to_owned()]));

    assert_eq!(result.stale_records_removed, 3);
    assert_eq!(result.enabled_languages_kept, 1);
    assert_eq!(config.languages, vec!["rust"]);
    assert_eq!(
        config.extensions,
        vec![StoredLanguageMapping {
            pattern: "foo".to_owned(),
            language: "rust".to_owned(),
        }]
    );
    assert_eq!(
        config.filenames,
        vec![StoredLanguageMapping {
            pattern: "Rustfile".to_owned(),
            language: "rust".to_owned(),
        }]
    );
}

#[test]
fn mark_error_import_is_still_usable_for_result_tests() {
    let error = MarkError::Usage("save failed".to_owned()).to_string();
    assert_eq!(error, "save failed");
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "mark-syntax-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn remove_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn with_xdg_config_home<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous = std::env::var_os("XDG_CONFIG_HOME");

    // SAFETY: tests that mutate XDG_CONFIG_HOME serialize through ENV_LOCK and
    // do not spawn threads while the temporary value is installed.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", path) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    // SAFETY: guarded by ENV_LOCK as above; restore the previous process
    // environment value before allowing another test to continue.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
