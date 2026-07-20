use super::*;

#[test]
fn default_layout_uses_split_only_when_terminal_is_wide_enough() {
    assert_eq!(
        default_layout_for_width(MIN_SPLIT_WIDTH - 1),
        DiffLayoutMode::Unified
    );
    assert_eq!(
        default_layout_for_width(MIN_SPLIT_WIDTH),
        DiffLayoutMode::Split
    );
}

#[test]
fn hunk_focus_uses_sliding_viewport_anchor() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    assert_eq!(app.focused_hunk_for_viewport(5), Some((FILE_0, HUNK_0)));

    app.set_scroll(1);
    assert_eq!(app.focused_hunk_for_viewport(5), Some((FILE_0, HUNK_1)));

    app.set_scroll(usize::MAX);
    assert_eq!(app.focused_hunk_for_viewport(5), Some((FILE_0, HUNK_2)));
    assert_eq!(app.viewport.scroll, app.max_scroll());
}

#[test]
fn bracket_hunk_navigation_places_oversized_hunk_at_top() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 2), (20, 20), (60, 2)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    let range = app
        .document
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");

    app.next_hunk();

    assert_eq!(app.viewport.scroll, range.start - 1);
    assert!(matches!(
        app.document.model.row(app.viewport.scroll),
        Some(UiRow::Collapsed { .. })
    ));
    assert_eq!(
        app.document.model.row(app.viewport.scroll + 1),
        Some(UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_1
        })
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_1)));
}

#[test]
fn hunk_navigation_centers_with_surrounding_collapsed_context() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 2), (20, 2), (40, 2)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);

    let range = app
        .document
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");
    let range_start = range.start - 1;
    let range_end = range.end + 1;
    assert!(matches!(
        app.document.model.row(range_start),
        Some(UiRow::Collapsed { .. })
    ));
    assert!(matches!(
        app.document.model.row(range.end),
        Some(UiRow::Collapsed { .. })
    ));

    app.next_hunk();

    let center = range_start.saturating_add(range_end.saturating_sub(range_start + 1) / 2);
    assert_eq!(
        app.viewport.scroll + viewport_center_offset(app.viewport.viewport_rows),
        center
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_1)));
}

#[test]
fn hunk_navigation_keeps_target_visible_after_expanded_pre_hunk_context() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[50, 100]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.document.context_expansions.insert(
        ContextKey {
            file: FILE_0,
            hunk: HUNK_1,
        },
        default_context_expand_step(),
    );
    app.document.model = UiModel::new(
        &app.document.changeset,
        app.viewport.layout,
        &app.document.context_expansions,
    );
    let hunk_row = app
        .document
        .model
        .hunk_start_row(0, 1)
        .expect("target hunk should have a header row");
    assert!(matches!(
        app.document.model.row(hunk_row - 1),
        Some(UiRow::Collapsed { .. })
    ));
    assert!(matches!(
        app.document.model.row(hunk_row - 2),
        Some(UiRow::ContextLine { .. })
    ));

    app.next_hunk();

    assert!(hunk_row >= app.viewport.scroll);
    assert!(hunk_row < app.viewport.scroll + app.viewport.viewport_rows);
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_1)));
}

#[test]
fn hunk_navigation_includes_expanded_context_before_hunk() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[50, 100, 150]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(8);
    app.document.context_expansions.insert(
        ContextKey {
            file: FILE_0,
            hunk: HUNK_1,
        },
        3,
    );
    app.document.model = UiModel::new(
        &app.document.changeset,
        app.viewport.layout,
        &app.document.context_expansions,
    );
    let context_start = app
        .document
        .model
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                UiRow::ContextHide {
                    file: FILE_0,
                    hunk: HUNK_1,
                    ..
                }
            )
        })
        .expect("expanded context should have a hide control");
    let hunk_row = app
        .document
        .model
        .hunk_start_row(0, 1)
        .expect("target hunk should have a header row");

    app.next_hunk();

    assert_eq!(app.viewport.scroll, context_start);
    assert!(hunk_row >= app.viewport.scroll);
    assert!(hunk_row < app.viewport.scroll + app.viewport.viewport_rows);
    assert_eq!(
        app.document.model.row(app.viewport.scroll),
        Some(UiRow::ContextHide {
            file: FILE_0,
            hunk: HUNK_1,
            lines: 3
        })
    );
    assert_eq!(app.focused_hunk_for_viewport(8), Some((FILE_0, HUNK_1)));
}

#[test]
fn full_file_hunk_navigation_centers_each_change() {
    let repo = temp_test_dir("full-file-hunk-navigation");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        (1..=220)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("source file should be written");
    let changeset = changeset_with_hunks_at(repo.clone(), &[20, 100, 180]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.toggle_full_file();
    assert!(app.expand_trailing_context_for_key(0, 3));
    app.set_scroll(0);
    app.viewport.manual_hunk_focus = None;

    for hunk in [HUNK_0, HUNK_1, HUNK_2] {
        app.next_hunk();

        let range = app
            .document
            .model
            .hunk_row_range(0, hunk.get())
            .expect("target hunk should have rows");
        let center = range
            .start
            .saturating_add(range.end.saturating_sub(range.start + 1) / 2);
        assert_eq!(
            app.viewport.scroll + viewport_center_offset(app.viewport.viewport_rows),
            center
        );
        assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, hunk)));
    }

    app.previous_hunk();
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_1)));

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn selecting_file_scrolls_file_to_top_and_focuses_its_first_hunk() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(7);

    app.select_file(1);

    assert_eq!(
        app.viewport.scroll,
        app.document.model.file_start_row(1).unwrap()
    );
    assert_eq!(app.sidebar.selected_file, FILE_1);
    assert_eq!(app.focused_hunk_for_viewport(7), Some((FILE_1, HUNK_0)));
}

#[test]
fn selecting_file_keeps_first_hunk_visible_when_header_top_would_hide_it() {
    let mut changeset = changeset_with_files(&["a.rs", "b.rs"]);
    {
        let hunk = &mut changeset.files[1].hunks_mut()[0];
        hunk.ranges = HunkLineRanges::new(10, 1, 10, 1);
        hunk.lines = vec![DiffLine::context(10, 10, "line 10".to_owned())];
    }
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(2);

    let file_start = app
        .document
        .model
        .file_start_row(1)
        .expect("selected file should have a header row");
    let hunk_row = app
        .document
        .model
        .hunk_start_row(1, 0)
        .expect("selected file should have a first hunk");
    assert!(hunk_row >= file_start.saturating_add(2));

    app.select_file(1);

    assert_eq!(app.sidebar.selected_file, FILE_1);
    assert!(visible_hunk_keys(&app).contains(&(1, 0)));
    assert_eq!(app.focused_hunk_for_viewport(2), Some((FILE_1, HUNK_0)));
}

#[test]
fn tab_file_navigation_focuses_first_hunk_and_updates_selected_file() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(7);

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .expect("tab should be handled");

    assert_eq!(app.sidebar.selected_file, FILE_1);
    assert_eq!(app.focused_hunk_for_viewport(7), Some((FILE_1, HUNK_0)));

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .expect("tab should be handled");
    assert_eq!(app.sidebar.selected_file, FILE_2);
    assert_eq!(app.focused_hunk_for_viewport(7), Some((FILE_2, HUNK_0)));

    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .expect("shift-tab should be handled");
    assert_eq!(app.sidebar.selected_file, FILE_1);
    assert_eq!(app.focused_hunk_for_viewport(7), Some((FILE_1, HUNK_0)));
}

#[test]
fn selecting_current_file_preserves_focused_hunk() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    app.next_hunk();
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_1)));

    app.select_file(0);

    assert_eq!(app.sidebar.selected_file, FILE_0);
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_1)));
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));
}

#[test]
fn replace_loaded_diff_clears_manual_hunk_focus() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    app.next_hunk();
    app.next_hunk();
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_2)));
    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 30,
        })
    );

    app.replace_loaded_diff(
        DiffOptions::default(),
        changeset_with_hunks_at(repo.clone(), &[100, 200, 300]),
    );

    assert_eq!(app.viewport.manual_hunk_focus, None);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));
    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 100,
        })
    );
}

#[test]
fn focused_hunk_editor_target_falls_back_to_hunk_start() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[20, 40]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);
    app.set_scroll(1);

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 40,
        })
    );
}

#[test]
fn focused_hunk_editor_target_skips_deleted_files() {
    let mut changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    set_test_file_deleted(&mut changeset.files[0]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    assert_eq!(app.focused_hunk_editor_target(), None);
}

#[test]
fn focused_hunk_editor_target_skips_show_sources() {
    let changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    let mut app = DiffApp::new(
        DiffOptions {
            source: DiffSource::Show("HEAD~1".into()),
            ..DiffOptions::default()
        },
        changeset,
        DiffLayoutMode::Unified,
    );
    app.set_viewport_rows(5);

    assert_eq!(app.focused_hunk_editor_target(), None);
    assert_eq!(app.focused_hunk_editor_reload_request(), None);
}

#[test]
fn editor_reload_behavior_supports_worktree_backed_diffs() {
    let changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert_eq!(
        app.editor_reload_behavior(false, Some(Path::new("src/file.rs"))),
        EditorReloadBehavior::None
    );
    assert_eq!(
        app.editor_reload_behavior(true, Some(Path::new("src/file.rs"))),
        EditorReloadBehavior::ScopedAsync
    );
    assert_eq!(
        app.editor_reload_behavior(true, None),
        EditorReloadBehavior::Sync
    );

    app.document.options.source = DiffSource::Base("main".into());
    assert_eq!(
        app.editor_reload_behavior(true, Some(Path::new("src/file.rs"))),
        EditorReloadBehavior::ScopedAsync
    );

    app.document.options.source = DiffSource::Branch {
        base: "main".into(),
        head: "feature".into(),
    };
    assert_eq!(
        app.editor_reload_behavior(true, Some(Path::new("src/file.rs"))),
        EditorReloadBehavior::None
    );
}

#[test]
fn focused_editor_reload_request_preserves_rename_pair() {
    let mut changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    set_test_file_renamed(&mut changeset.files[0], "old.rs", "new.rs");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);

    let request = app.focused_hunk_editor_reload_request().unwrap();

    assert_eq!(request.path, PathBuf::from("new.rs"));
    assert_eq!(
        request.pathspecs,
        vec![PathBuf::from("old.rs"), PathBuf::from("new.rs")]
    );
}

#[test]
fn explicit_layout_ignores_saved_layout_preference() {
    let settings = SyntaxSettings {
        layout: Some(LayoutSetting::Unified),
        ..SyntaxSettings::default()
    };

    assert_eq!(
        layout_override_from_settings(&settings, true),
        Some(DiffLayoutMode::Unified)
    );
    assert_eq!(layout_override_from_settings(&settings, false), None);

    let settings = SyntaxSettings {
        layout: Some(LayoutSetting::Dynamic),
        ..SyntaxSettings::default()
    };
    assert_eq!(layout_override_from_settings(&settings, true), None);
}

#[test]
fn static_explicit_layout_builds_model_before_saved_layout_preference() {
    let app = DiffApp::new_static_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_replacement_pair(),
        DiffLayoutMode::Split,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            layout: Some(LayoutSetting::Unified),
            ..SyntaxSettings::default()
        },
    );

    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.layout_override, Some(DiffLayoutMode::Split));
    assert!(
        app.document
            .model
            .rows
            .iter()
            .any(|row| matches!(row, UiRow::SplitLine { .. }))
    );
    assert!(
        app.document
            .model
            .rows
            .iter()
            .all(|row| !matches!(row, UiRow::UnifiedLine { .. }))
    );
}

#[test]
fn raw_blob_range_stays_in_hunk_view_when_full_file_is_requested() {
    let repo = temp_test_dir("raw-blob-range-full-file");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("file.rs"), "old\n").expect("old file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-qm", "old"]);
    let old_blob = git_stdout(&repo, &["rev-parse", "HEAD:file.rs"]);
    fs::write(repo.join("file.rs"), "new\n").expect("new file should be written");
    git(&repo, &["commit", "-qam", "new"]);
    let new_blob = git_stdout(&repo, &["rev-parse", "HEAD:file.rs"]);
    let options = DiffOptions {
        source: DiffSource::Range {
            left: old_blob.into(),
            right: new_blob.into(),
        },
        ..DiffOptions::default()
    };
    let changeset = changeset_with_hunk_at(repo.clone(), 1);
    let mut app = DiffApp::new(options, changeset, DiffLayoutMode::Unified);

    app.toggle_full_file();

    assert!(app.viewport.full_file);
    assert!(!app.full_file_mode_active());
    assert!(app.document.context_expansions.is_empty());
    assert!(
        (0..app.document.model.len())
            .any(|row| matches!(app.document.model.row(row), Some(UiRow::HunkHeader { .. })))
    );

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn full_file_context_normalizes_zero_count_insertion_ranges() {
    let repo = temp_test_dir("zero-count-insertion-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(repo.join("file.rs"), "one\ntwo\ninserted\nthree\nfour\n")
        .expect("source file should be written");
    let mut changeset = changeset_with_hunk_at(repo.clone(), 3);
    let hunk = &mut changeset.files[0].hunks_mut()[0];
    hunk.header = "@@ -2,0 +3 @@".to_owned();
    hunk.ranges = HunkLineRanges::new(2, 0, 3, 1);
    hunk.lines = vec![DiffLine::addition(3, "inserted".to_owned())];

    let trailing_key = ContextKey {
        file: FILE_0,
        hunk: HUNK_1,
    };
    let trailing = HashMap::from([(trailing_key, 2)]);
    let expansions = full_file_context_expansions(&changeset, &DiffOptions::default(), &trailing);
    assert_eq!(
        expansions.get(&ContextKey {
            file: FILE_0,
            hunk: HUNK_0,
        }),
        Some(&2)
    );

    let model = UiModel::new_with_trailing_context_and_controls(
        &changeset,
        DiffLayoutMode::Unified,
        &expansions,
        &trailing,
        false,
    );
    let context_lines = model
        .iter_rows()
        .filter_map(|row| match row {
            UiRow::ContextLine {
                old_line, new_line, ..
            } => Some((old_line, new_line)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(context_lines, vec![(1, 1), (2, 2), (3, 4), (4, 5)]);

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn trailing_context_starts_after_a_zero_count_new_range() {
    let repo = temp_test_dir("zero-count-deletion-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(repo.join("file.rs"), "two\nthree\nfour\nfive\n")
        .expect("source file should be written");
    let mut changeset = changeset_with_hunk_at(repo.clone(), 1);
    let hunk = &mut changeset.files[0].hunks_mut()[0];
    hunk.header = "@@ -1 +0,0 @@".to_owned();
    hunk.ranges = HunkLineRanges::new(1, 1, 0, 0);
    hunk.lines = vec![DiffLine::deletion(1, "one".to_owned())];
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.toggle_full_file();
    app.set_viewport_rows(app.document.model.len().max(1));

    assert!(app.discover_trailing_context_for_viewport());
    for _ in 0..1_000 {
        app.drain_trailing_context_worker();
        if app.jobs.trailing_context_worker.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        app.jobs.trailing_context_worker.is_none(),
        "trailing context worker did not finish"
    );

    let key = ContextKey {
        file: FILE_0,
        hunk: HUNK_1,
    };
    assert_eq!(app.document.trailing_context_lines.get(&key), Some(&4));
    assert!(app.document.model.context_line_row(FILE_0, 4).is_some());

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn trailing_context_discovery_rejects_oversized_source_lines() {
    let repo = temp_test_dir("trailing-context-line-limit");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        format!("{}\ntail\n", "x".repeat(1024 * 1024 + 1)),
    )
    .expect("context file should be written");
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_hunk_at(repo.clone(), 1),
        DiffLayoutMode::Unified,
    );
    app.toggle_full_file();
    app.set_viewport_rows(app.document.model.len().max(1));

    assert!(app.discover_trailing_context_for_viewport());
    for _ in 0..1_000 {
        app.drain_trailing_context_worker();
        if app.jobs.trailing_context_worker.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        app.jobs.trailing_context_worker.is_none(),
        "trailing context worker did not finish"
    );

    let key = ContextKey {
        file: FILE_0,
        hunk: HUNK_1,
    };
    assert_eq!(app.document.trailing_context_lines.get(&key), Some(&0));
    assert!(!app.document.trailing_context_sides.contains_key(&key));

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn full_file_setting_starts_with_unchanged_lines_visible() {
    let repo = temp_test_dir("configured-full-file");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let app = DiffApp::new_static_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_hunk_at(repo, 20),
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            ..SyntaxSettings::default()
        },
    );

    assert!(app.viewport.full_file);
    assert!(app.overlays.options_menu_draft.full_file);
    assert_eq!(
        app.document.trailing_context_lines.get(&ContextKey {
            file: FILE_0,
            hunk: HunkIndex::new(1),
        }),
        Some(&10)
    );
    assert!(matches!(
        app.document.model.row(1),
        Some(UiRow::ContextLine {
            old_line: 1,
            new_line: 1,
            ..
        })
    ));
    assert!(app.document.model.context_line_row(FILE_0, 30).is_some());
    assert!((0..app.document.model.len()).all(|row| !matches!(
        app.document.model.row(row),
        Some(UiRow::Collapsed { .. } | UiRow::ContextHide { .. } | UiRow::HunkHeader { .. })
    )));
}

#[test]
fn static_full_file_mode_omits_limit_rejected_source_expansions() {
    let repo = temp_test_dir("static-full-file-line-limit");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(repo.join("file.rs"), "x".repeat(1024 * 1024 + 1))
        .expect("context file should be written");
    let app = DiffApp::new_static_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_hunk_at(repo.clone(), 50_000_000),
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            ..SyntaxSettings::default()
        },
    );

    assert!(app.viewport.full_file);
    assert!(!app.document.context_cache.is_empty());
    assert!(
        app.document
            .context_cache
            .values()
            .all(|entry| matches!(entry, ContextSourceEntry::Unavailable))
    );
    assert!(app.document.context_expansions.is_empty());
    assert!(app.document.model.len() < 100);

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn wrapped_full_file_setting_preloads_context_for_interactive_startup() {
    let repo = temp_test_dir("configured-wrapped-full-file");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        (1..=30)
            .map(|_| "x".repeat(80))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("context file should be written");
    let mut app = DiffApp::new_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_hunk_at(repo, 20),
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            line_wrapping: true,
            ..SyntaxSettings::default()
        },
    );
    app.set_viewport_width(18);

    assert!(matches!(
        app.document.context_cache.get(&ContextSourceKey {
            file: FILE_0,
            side: DiffSide::New,
        }),
        Some(ContextSourceEntry::Lines(_))
    ));
    let hunk_row = app
        .document
        .model
        .diff_line_row(FILE_0, HUNK_0, LINE_0)
        .expect("hunk row should be visible")
        .get();
    let scroll_before_render = app.wrapped_visual_scroll_for_model_row(hunk_row);
    assert!(scroll_before_render > hunk_row);

    app.context_line_text(0, 1, 1);

    assert_eq!(
        app.wrapped_visual_scroll_for_model_row(hunk_row),
        scroll_before_render
    );
}

#[test]
fn full_file_viewport_context_loads_on_a_worker() {
    let repo = temp_test_dir("async-full-file-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        (1..=30)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("context file should be written");
    let mut app = DiffApp::new_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_hunk_at(repo.clone(), 20),
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            ..SyntaxSettings::default()
        },
    );

    assert!(app.prepare_full_file_context_for_viewport(20));
    assert!(matches!(
        app.document.context_cache.get(&ContextSourceKey {
            file: FILE_0,
            side: DiffSide::New,
        }),
        Some(ContextSourceEntry::Loading)
    ));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while app.jobs.context_load_worker.is_some() && std::time::Instant::now() < deadline {
        app.drain_context_load_worker();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert!(app.jobs.context_load_worker.is_none());
    assert!(matches!(
        app.document.context_cache.get(&ContextSourceKey {
            file: FILE_0,
            side: DiffSide::New,
        }),
        Some(ContextSourceEntry::Lines(_))
    ));
    assert_eq!(app.context_line_text(0, 1, 1), "line 1");
    fs::remove_dir_all(repo).expect("test directory should be removed");
}

#[test]
fn disabling_full_file_cancels_pending_context_width_load() {
    let repo = temp_test_dir("cancel-full-file-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        std::iter::once("x".repeat(200))
            .chain((2..=30).map(|line| format!("line {line}")))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("context file should be written");
    let mut app = DiffApp::new_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset_with_hunk_at(repo.clone(), 20),
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            ..SyntaxSettings::default()
        },
    );
    let diff_width = app.document.search_index.max_line_width();

    assert!(app.prepare_full_file_context_for_viewport(20));
    assert!(app.jobs.context_load_worker.is_some());
    app.set_full_file(false);

    assert!(app.jobs.context_load_worker.is_none());
    assert!(
        app.document
            .context_cache
            .values()
            .all(|entry| !matches!(entry, ContextSourceEntry::Loading))
    );
    assert_eq!(app.document.max_line_width, diff_width);
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(!app.drain_context_load_worker());
    assert_eq!(app.document.max_line_width, diff_width);

    fs::remove_dir_all(repo).expect("test directory should be removed");
}

#[test]
fn full_file_source_limit_rejects_growth_before_context_materialization() {
    let repo = temp_test_dir("full-file-context-limit");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(repo.join("file.rs"), "0123456789").expect("source file should be written");
    let source = FullFileSource {
        repo: repo.clone().into(),
        kind: FullFileSourceKind::Worktree {
            path: "file.rs".into(),
        },
    };

    assert!(matches!(
        load_full_file_source_limited(&source, 4),
        Err(SyntaxSkipReason::TooLarge)
    ));
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert!(matches!(
        load_full_file_source_limited_cancellable(&source, 32, &cancelled),
        Err(SyntaxSkipReason::NoSource)
    ));
    fs::remove_dir_all(repo).expect("test directory should be removed");
}

#[cfg(unix)]
#[test]
fn full_file_worktree_source_rejects_symlink_escape() {
    let root = temp_test_dir("full-file-context-symlink");
    let repo = root.join("repo");
    let outside = root.join("outside");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::create_dir_all(&outside).expect("outside directory should be created");
    fs::write(outside.join("secret.rs"), "secret").expect("outside file should be written");
    std::os::unix::fs::symlink(&outside, repo.join("escape"))
        .expect("escape symlink should be created");
    let source = FullFileSource {
        repo: repo.into(),
        kind: FullFileSourceKind::Worktree {
            path: "escape/secret.rs".into(),
        },
    };

    assert!(matches!(
        load_full_file_source_limited(&source, 32),
        Err(SyntaxSkipReason::NoPath)
    ));
    fs::remove_dir_all(root).expect("test directory should be removed");
}

#[test]
fn wrapped_full_file_startup_loads_only_viewport_context() {
    let repo = temp_test_dir("wrapped-full-file-viewport-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let paths = ["one.rs", "two.rs", "three.rs", "four.rs"];
    for path in paths {
        fs::write(
            repo.join(path),
            (1..=80)
                .map(|line| format!("{path} line {line} {}", "x".repeat(80)))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("source file should be written");
    }
    let mut changeset = changeset_with_files(&paths);
    changeset.repo = repo.clone().into();
    for file in &mut changeset.files {
        let hunk = &mut file.hunks_mut()[0];
        hunk.header = "@@ -50 +50 @@".to_owned();
        hunk.ranges = HunkLineRanges::new(50, 1, 50, 1);
        hunk.lines = vec![DiffLine::context(50, 50, "changed".to_owned())];
    }

    let app = DiffApp::new_with_explicit_layout_and_settings(
        DiffOptions::default(),
        changeset,
        DiffLayoutMode::Unified,
        SyntaxStartupMode::Disabled,
        SyntaxSettings {
            full_file: true,
            line_wrapping: true,
            ..SyntaxSettings::default()
        },
    );

    assert!(app.full_file_mode_active());
    assert!(!app.document.context_cache.is_empty());
    assert!(
        app.document
            .context_cache
            .keys()
            .all(|key| key.file == FILE_0)
    );

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn file_changed_since_compares_target_fingerprint() {
    let dir = temp_test_dir("file-changed-since");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("file.txt");
    fs::write(&path, "a").unwrap();

    let before = FileFingerprint::read(&path);
    assert!(!file_changed_since(&path, before));

    fs::write(&path, "abcd").unwrap();
    assert!(file_changed_since(&path, before));

    let missing = dir.join("missing.txt");
    assert!(!file_changed_since(&missing, None));
    assert!(file_changed_since(&missing, before));
}

#[test]
fn path_changeset_replaces_only_edited_file() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let replacement = changeset_with_files(&["b.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.replace_path_changeset(Path::new("b.rs"), replacement);

    assert_eq!(visible_paths(&app), vec!["a.rs", "b.rs", "c.rs"]);
    assert_eq!(
        app.document.changeset.files[0].hunks()[0].lines[0].text(),
        "line 0"
    );
    assert_eq!(
        app.document.changeset.files[1].hunks()[0].lines[0].text(),
        "line 0"
    );
    assert_eq!(
        app.document.changeset.files[2].hunks()[0].lines[0].text(),
        "line 2"
    );
}

#[test]
fn path_changeset_removes_file_when_diff_disappears() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut replacement = changeset_with_files(&[]);
    replacement.repo = PathBuf::from("/repo").into();
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.replace_path_changeset(Path::new("b.rs"), replacement);

    assert_eq!(visible_paths(&app), vec!["a.rs", "c.rs"]);
}

#[test]
fn ui_model_inserts_file_separator_between_files() {
    let changeset = changeset_with_files(&["a.rs", "b.rs"]);
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &HashMap::new());

    assert_eq!(model.file_start_row(0), Some(0));
    assert_eq!(model.file_start_row(1), Some(4));
    assert_eq!(model.row(3), Some(UiRow::FileSeparator));
    assert_eq!(model.row(4), Some(UiRow::FileHeader(FILE_1)));
    assert_eq!(model.file_at_row(3), Some(0));
    assert_eq!(model.file_at_row(4), Some(1));
}

#[test]
fn ui_model_expands_context_after_previous_hunk_downward() {
    let step = default_context_expand_step();
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[50, 100]);
    let mut expansions = HashMap::new();

    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);
    assert_eq!(
        model.row(4),
        Some(UiRow::Collapsed {
            file: FILE_0,
            hunk: HUNK_1,
            old_start: 51,
            new_start: 51,
            lines: 49,
            expanded: 0,
        })
    );

    expansions.insert(
        ContextKey {
            file: FILE_0,
            hunk: HUNK_1,
        },
        step,
    );
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);

    assert_eq!(
        model.row(4),
        Some(UiRow::ContextHide {
            file: FILE_0,
            hunk: HUNK_1,
            lines: step,
        })
    );
    assert_eq!(
        model.row(5),
        Some(UiRow::ContextLine {
            file: FILE_0,
            old_line: 51,
            new_line: 51,
        })
    );
    assert_eq!(
        model.row(25),
        Some(UiRow::Collapsed {
            file: FILE_0,
            hunk: HUNK_1,
            old_start: 71,
            new_start: 71,
            lines: 29,
            expanded: step as u32,
        })
    );
    assert_eq!(
        model.row(26),
        Some(UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_1
        })
    );
}

#[test]
fn context_source_side_uses_loaded_fallback_side() {
    let repo = temp_test_dir("context-source-side-fallback");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);

    let text = (1..=10)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("old file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    fs::remove_file(repo.join("file.rs")).expect("worktree file should be removed");

    let changeset = changeset_with_hunk_at(repo.clone(), 5);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    let (side, lines) = app
        .ensure_context_lines(0)
        .expect("old context should load after new side fails");
    assert_eq!(side, DiffSide::Old);
    assert_eq!(lines.get(0), Some("line 1"));
    assert_eq!(app.context_source_side(0), Some(DiffSide::Old));

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn collapsed_context_control_is_minimal_and_readable() {
    let theme = DiffTheme::default();
    let line = context_show_line(20, false, "▴", 72, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 72);
    assert!(text.starts_with(DIFF_INDICATOR));
    assert!(text.contains("▴ show 20 unchanged lines"));
    assert_eq!(line.spans[1].style.fg, Some(theme.muted));
    assert_eq!(
        line.spans[0].style.bg,
        Some(line_gutter_bg(DiffLineKind::Meta, theme))
    );

    let hide = context_hide_line(20, "▾", 24, theme);
    let hide_text = line_text(&hide);
    assert!(hide_text.contains("▾ hide 20 unchanged"));
    assert_eq!(hide.spans[1].style.fg, Some(theme.muted));

    assert_eq!(context_expand_marker(0), "▴");
    assert_eq!(context_expand_marker(1), "▾");
    assert_eq!(context_hide_marker(0), "▾");
    assert_eq!(context_hide_marker(1), "▴");

    let more = context_show_line(20, true, "▾", 72, theme);
    assert!(line_text(&more).contains("▾ show 20 more unchanged lines"));
}

#[test]
fn responsive_layout_preserves_manual_unified_choice_on_wide_resize() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

    app.toggle_layout();
    assert_eq!(app.viewport.layout, DiffLayoutMode::Unified);

    app.apply_responsive_layout(MIN_SPLIT_WIDTH + 40);

    assert_eq!(app.viewport.layout, DiffLayoutMode::Unified);
}

#[test]
fn responsive_layout_preserves_manual_split_choice_on_narrow_resize() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.toggle_layout();
    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.layout_override, Some(DiffLayoutMode::Split));

    app.apply_responsive_layout(MIN_SPLIT_WIDTH - 1);
    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.layout_override, Some(DiffLayoutMode::Split));

    app.apply_responsive_layout(MIN_SPLIT_WIDTH + 40);
    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
}

#[test]
fn explicit_layout_preserves_split_choice_on_narrow_resize() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new_with_explicit_layout(
        DiffOptions::default(),
        changeset,
        DiffLayoutMode::Split,
        SyntaxStartupMode::Disabled,
    );

    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.layout_override, Some(DiffLayoutMode::Split));
    assert_eq!(app.overlays.options_menu_draft.layout, LayoutSetting::Split);

    app.apply_responsive_layout(MIN_SPLIT_WIDTH - 1);

    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.layout_override, Some(DiffLayoutMode::Split));
}

#[test]
fn live_reload_invalidation_clears_cache_without_visible_pending_state() {
    let changeset = changeset_with_files(&["src/lib.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let options = DiffOptions {
        source: DiffSource::Worktree,
        ..DiffOptions::default()
    };

    app.cache_loaded_diff(options, changeset_with_files(&["cached.rs"]));
    assert!(!app.jobs.diff_cache.is_empty());

    app.mark_live_reload_invalidated();

    assert_eq!(
        app.jobs.live_updates.status(),
        Some(LiveReloadStatus::Invalidated)
    );
    assert!(app.jobs.diff_cache.is_empty());

    let line = statusline_header_line(&app, 80);
    assert!(!line_text(&line).contains("refreshing diff"));
}

#[test]
fn explicit_diff_load_returns_before_replacing_changeset() {
    let changeset = changeset_with_files(&["src/lib.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let patch = Arc::<[u8]>::from(
        b"diff --git a/other.rs b/other.rs\n--- a/other.rs\n+++ b/other.rs\n@@ -1 +1 @@\n-old\n+new\n"
            .as_slice(),
    );
    let options = DiffOptions {
        source: DiffSource::Patch(PatchSource::Text {
            label: "test patch".into(),
            patch,
        }),
        local_untracked: mark_diff::UntrackedMode::Exclude,
        ..DiffOptions::default()
    };

    app.start_diff_load(options, "diff unavailable");

    assert!(app.jobs.pending_diff_load.is_some());
    assert_eq!(app.document.changeset.files[0].display_path(), "src/lib.rs");

    for _ in 0..100 {
        app.drain_pending_diff_load();
        if app.jobs.pending_diff_load.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }

    assert!(app.jobs.pending_diff_load.is_none());
    assert_eq!(app.document.changeset.files[0].display_path(), "other.rs");
    assert_eq!(app.document.options.source, DiffSource::Patch(PatchSource::Text {
        label: "test patch".into(),
        patch: Arc::<[u8]>::from(
            b"diff --git a/other.rs b/other.rs\n--- a/other.rs\n+++ b/other.rs\n@@ -1 +1 @@\n-old\n+new\n"
                .as_slice(),
        ),
    }));
}

#[test]
fn file_filter_edit_with_active_grep_preserves_current_grep_match() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);
    app.filters.grep_filter = "line".to_owned();
    app.apply_filters(PostFilterNavigation::JumpToGrep);
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("n should move to next grep match");
    let scroll_before_file_filter = app.viewport.scroll;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .expect("f should open file filter");
    for character in "rs".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("file filter input should be handled");
    }

    assert_eq!(visible_paths(&app), vec!["a.rs", "b.rs", "c.rs"]);
    assert_eq!(app.current_grep_match_row(), Some(6));
    assert_eq!(app.viewport.scroll, scroll_before_file_filter);
}

#[test]
fn configured_leader_diff_type_bindings_cycle_choices() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        next_diff_type = "space n"
        previous_diff_type = "space p"
        "#,
    )
    .expect("keymap should parse");

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .expect("leader should be handled");
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("leader n should cycle diff type");
    let load = app
        .jobs
        .pending_diff_load
        .as_ref()
        .expect("leader n should queue diff load");
    assert_eq!(load.options.source, DiffSource::Show("HEAD".into()));
    assert!(app.input.key_prefix_pending.is_none());

    app.jobs.pending_diff_load = None;
    app.document.options = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .expect("leader should be handled");
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
        .expect("leader p should cycle diff type");
    let load = app
        .jobs
        .pending_diff_load
        .as_ref()
        .expect("leader p should queue diff load");
    assert_eq!(load.options.source, DiffSource::Worktree);
    assert!(app.input.key_prefix_pending.is_none());
}

#[test]
fn range_diff_has_no_diff_type_choices() {
    let options = DiffOptions {
        source: DiffSource::Range {
            left: "main".into(),
            right: "feature".into(),
        },
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.refs.branch_base = Some("main".to_owned());
    app.refs.branch_head = Some("feature".to_owned());
    app.refs.current_head = Some("main".to_owned());

    assert!(app.diff_menu_choices().is_empty());
}

#[test]
fn cached_current_diff_does_not_reuse_a_full_file_model() {
    let repo = temp_test_dir("full-file-diff-cache");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.refs.branch_base = Some("main".to_owned());
    app.refs.current_head = Some("feature".to_owned());
    let branch = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(branch.clone(), changeset_with_context_lines_at(repo, 1, 1));

    app.toggle_full_file();
    assert!(
        (0..app.document.model.len())
            .all(|row| !matches!(app.document.model.row(row), Some(UiRow::HunkHeader { .. })))
    );

    app.select_diff_choice(DiffChoice::Branch);
    assert_eq!(app.document.options, branch);
    app.set_full_file(false);
    app.select_diff_choice(DiffChoice::All);

    assert_eq!(app.document.options, DiffOptions::default());
    assert!(
        (0..app.document.model.len())
            .any(|row| matches!(app.document.model.row(row), Some(UiRow::HunkHeader { .. })))
    );
}

#[test]
fn cached_full_file_diff_preloads_wrapped_context_before_scroll_restore() {
    let repo = temp_test_dir("cached-wrapped-full-file");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    fs::write(
        repo.join("file.rs"),
        (1..=30)
            .map(|_| "x".repeat(80))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo.clone(), 20);
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset.clone(),
        DiffLayoutMode::Unified,
    );
    app.toggle_full_file();
    app.set_line_wrapping(true);
    app.set_viewport_width(18);
    let options = DiffOptions::default();

    app.replace_cached_diff(
        options.clone(),
        diff_cache_entry(options, changeset),
        BranchMetadataPolicy::Preserve,
    );

    assert!(matches!(
        app.document.context_cache.get(&ContextSourceKey {
            file: FILE_0,
            side: DiffSide::New,
        }),
        Some(ContextSourceEntry::Lines(_))
    ));
    let hunk_row = app
        .document
        .model
        .diff_line_row(FILE_0, HUNK_0, LINE_0)
        .expect("hunk row should be visible")
        .get();
    let scroll_before_render = app.wrapped_visual_scroll_for_model_row(hunk_row);
    assert!(scroll_before_render > hunk_row);

    app.context_line_text(0, 1, 1);

    assert_eq!(
        app.wrapped_visual_scroll_for_model_row(hunk_row),
        scroll_before_render
    );
}

#[test]
fn cached_full_file_diff_recovers_context_width_before_horizontal_scroll_restore() {
    let repo = temp_test_dir("cached-full-file-context-width");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let wide_line = "x".repeat(200);
    fs::write(
        repo.join("file.rs"),
        std::iter::once(wide_line)
            .chain((2..=80).map(|line| format!("line {line}")))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo.clone(), 50);
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset.clone(),
        DiffLayoutMode::Unified,
    );
    app.toggle_full_file();
    app.set_viewport_width(40);
    app.context_line_text(0, 1, 1);
    app.set_horizontal_scroll(80);
    assert_eq!(app.viewport.horizontal_scroll, 80);
    let options = DiffOptions::default();

    app.replace_cached_diff(
        options.clone(),
        diff_cache_entry(options, changeset),
        BranchMetadataPolicy::Preserve,
    );

    assert!(app.document.max_line_width >= 200);
    assert_eq!(app.viewport.horizontal_scroll, 80);

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn cached_current_diff_rebuilds_model_while_filter_apply_is_pending() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_files(&["all.rs", "filtered.rs"]),
        DiffLayoutMode::Unified,
    );
    app.refs.branch_base = Some("main".to_owned());
    app.refs.current_head = Some("feature".to_owned());
    let branch = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(branch.clone(), changeset_with_files(&["branch.rs"]));

    app.filters.file_filter = "filtered".to_owned();
    app.apply_filters(PostFilterNavigation::Preserve);
    assert_eq!(visible_paths(&app), vec!["filtered.rs"]);

    app.filters.file_filter.clear();
    app.filters.file_filter_input.clear();
    app.jobs.filter_searching = true;

    app.select_diff_choice(DiffChoice::Branch);
    assert_eq!(app.document.options, branch);
    assert_eq!(visible_paths(&app), vec!["branch.rs"]);

    app.select_diff_choice(DiffChoice::All);
    assert_eq!(app.document.options, DiffOptions::default());
    assert_eq!(visible_paths(&app), vec!["all.rs", "filtered.rs"]);
}

#[test]
fn cached_diff_choice_is_not_reused_without_live_invalidator() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_files(&["all.rs"]),
        DiffLayoutMode::Unified,
    );
    let all_changes = DiffOptions {
        source: DiffSource::Worktree,
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(all_changes.clone(), changeset_with_files(&["stale.rs"]));
    app.jobs.live_updates = LiveUpdatesState::DisabledByCli;

    app.cycle_diff_choice(1);
    app.jobs.pending_diff_load = None;
    app.document.options = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };

    app.cycle_diff_choice(1);

    let load = app
        .jobs
        .pending_diff_load
        .as_ref()
        .expect("cycling should queue a fresh diff load");
    assert_eq!(load.options, all_changes);
    assert_eq!(app.document.options.source, DiffSource::Show("HEAD".into()));
    assert_eq!(visible_paths(&app), vec!["all.rs"]);
    assert!(app.jobs.diff_cache.is_empty());
}

#[test]
fn cached_diff_choice_is_not_reused_during_pending_live_reload() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_files(&["all.rs"]),
        DiffLayoutMode::Unified,
    );
    let all_changes = DiffOptions {
        source: DiffSource::Worktree,
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(all_changes.clone(), changeset_with_files(&["stale.rs"]));
    app.mark_live_reload_pending();

    app.cycle_diff_choice(1);
    app.jobs.pending_diff_load = None;
    app.document.options = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };

    app.cycle_diff_choice(1);

    let load = app
        .jobs
        .pending_diff_load
        .as_ref()
        .expect("cycling should queue a fresh diff load");
    assert_eq!(load.options, all_changes);
    assert_eq!(app.document.options.source, DiffSource::Show("HEAD".into()));
    assert_eq!(visible_paths(&app), vec!["all.rs"]);
    assert!(app.jobs.diff_cache.is_empty());
    assert_eq!(
        app.jobs.live_updates.status(),
        Some(LiveReloadStatus::Pending)
    );
}

#[test]
fn diff_prefetch_skips_when_live_reload_is_disabled() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    app.jobs.live_updates = LiveUpdatesState::DisabledByCli;

    app.start_diff_prefetches();

    assert!(app.jobs.pending_diff_prefetch.is_none());
    assert!(app.jobs.diff_prefetch_queue.is_empty());
    assert!(!app.jobs.diff_prefetch_started);
}

#[test]
fn diff_prefetch_skips_for_sources_without_live_reload() {
    let options = DiffOptions {
        source: DiffSource::Range {
            left: "main".into(),
            right: "HEAD".into(),
        },
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        options,
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );

    app.start_diff_prefetches();

    assert!(app.jobs.pending_diff_prefetch.is_none());
    assert!(app.jobs.diff_prefetch_queue.is_empty());
    assert!(!app.jobs.diff_prefetch_started);
}

#[test]
fn cycling_back_to_current_diff_clears_pending_load() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );

    app.cycle_diff_choice(1);
    assert!(app.jobs.pending_diff_load.is_some());

    app.cycle_diff_choice(-1);

    assert_eq!(app.document.options, DiffOptions::default());
    assert!(app.jobs.pending_diff_load.is_none());
}

#[test]
fn reload_invalidates_cached_diff_choices() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    let show = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(show, changeset_with_files(&["show.rs"]));

    app.reload().expect("reload should start");

    assert!(app.jobs.diff_cache.is_empty());
    assert!(app.jobs.pending_diff_prefetch.is_none());
    assert!(app.jobs.diff_prefetch_queue.is_empty());
    assert!(!app.jobs.diff_prefetch_started);
}

#[test]
fn cache_invalidation_preserves_pending_diff_load() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset_with_context_lines(1),
        DiffLayoutMode::Unified,
    );
    let pending_options = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };
    app.jobs.pending_diff_load = Some(pending_diff_load(pending_options.clone()));

    app.invalidate_diff_cache();

    assert_eq!(
        app.jobs
            .pending_diff_load
            .as_ref()
            .map(|load| &load.options),
        Some(&pending_options)
    );
}

#[test]
fn live_diff_filter_ignores_non_state_git_paths() {
    let repo = std::env::temp_dir().join("mark-tui-live-filter-repo");
    let other = std::env::temp_dir().join("mark-tui-live-filter-other");
    let filter = LiveDiffFilter {
        repo: repo.clone(),
        git_state_paths: vec![
            repo.join(".git/index"),
            repo.join(".git/index.lock"),
            repo.join(".git/refs"),
        ],
        exact_paths: Vec::new(),
    };

    assert!(filter.is_relevant_path(Path::new("src/lib.rs")));
    assert!(filter.is_relevant_path(&repo.join("src/lib.rs")));
    assert!(filter.is_relevant_path(&repo.join(".git/index")));
    assert!(filter.is_relevant_path(&repo.join(".git/index.lock")));
    assert!(filter.is_relevant_path(&repo.join(".git/refs/heads/main")));
    assert!(!filter.is_relevant_path(&repo.join(".git/logs/HEAD")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git")));
    assert!(!filter.is_relevant_path(&repo.join("vendor/plugin/.git/index.lock")));
    assert!(!filter.is_relevant_path(&repo.join("vendor/plugin/.git/objects/tmp")));
    assert!(!filter.is_relevant_path(&repo.join("vendor/plugin/.git/logs/HEAD")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git/HEAD")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git/index")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git/refs/heads/main")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git/info")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/.git/info/exclude")));
    assert!(!filter.is_relevant_path(&repo.join("vendor/plugin/.git/info/attributes")));
    assert!(filter.is_relevant_path(&repo.join("vendor/plugin/src/lib.rs")));
    assert!(!filter.is_relevant_path(&other.join("file.rs")));
}

#[test]
fn live_diff_watch_paths_upgrade_to_recursive() {
    let mut spec = LiveDiffWatchSpec::new(Path::new("repo"));

    spec.add_watch_path(
        PathBuf::from("repo/.git"),
        notify::RecursiveMode::NonRecursive,
    );
    spec.add_watch_path(PathBuf::from("repo/.git"), notify::RecursiveMode::Recursive);

    let watch_path = spec
        .watch_paths
        .iter()
        .find(|watch_path| watch_path.path == Path::new("repo/.git"))
        .unwrap();
    assert_eq!(watch_path.mode, notify::RecursiveMode::Recursive);
}

#[test]
fn empty_diff_fill_is_blank_unless_enabled() {
    assert_eq!(empty_diff_fill_from(8, 0, 0, false), "        ");
    assert_eq!(empty_diff_fill_from(8, 0, 0, true), "╱  ╱  ╱ ");
    assert_eq!(empty_diff_fill_from(8, 1, 0, true), "  ╱  ╱  ");
    assert_eq!(empty_diff_fill_from(8, 2, 0, true), " ╱  ╱  ╱");
}

#[test]
fn focused_hunk_highlights_diff_indicators() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let theme = app.config.theme;

    let header = render_row_with_focus(
        &mut app,
        1,
        UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_0,
        },
        24,
        Some((FILE_0, HUNK_0)),
    );
    assert_eq!(header.spans[0].style.fg, Some(theme.hunk));
    assert!(header.spans[0].style.add_modifier.contains(Modifier::BOLD));

    let row = app
        .document
        .model
        .row(2)
        .expect("diff line should be visible");
    let focused = render_row_with_focus(&mut app, 2, row, 24, Some((FILE_0, HUNK_0)));
    let unfocused = render_row_with_focus(&mut app, 2, row, 24, Some((FILE_0, HUNK_1)));

    assert_eq!(focused.spans[0].style.fg, Some(theme.hunk));
    assert!(focused.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(unfocused.spans[0].style.fg, Some(theme.muted));
    assert!(
        !unfocused.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn highlight_cache_evicts_least_recently_used_entry() {
    let mut cache = LruCache::new(2);
    let first = syntax_key(0);
    let second = syntax_key(1);
    let third = syntax_key(2);

    cache.insert(first, 1);
    cache.insert(second, 2);
    assert_eq!(cache.get(&first), Some(&1));

    cache.insert(third, 3);

    assert_eq!(cache.get(&second), None);
    assert_eq!(cache.get(&first), Some(&1));
    assert_eq!(cache.get(&third), Some(&3));
}

#[test]
fn oversized_hunks_fall_back_to_plain_diff_text() {
    let limits = SyntaxLimits::default();
    let text = "x".repeat(limits.max_line_bytes);
    let line_count = (limits.max_source_bytes / limits.max_line_bytes) + 2;
    let lines = (0..line_count)
        .map(|index| DiffLine::context(index + 1, index + 1, text.clone()))
        .collect::<Vec<_>>();

    assert_eq!(
        build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
        SyntaxSkipReason::TooLarge
    );
}

#[test]
fn full_file_sources_cover_diff_modes_and_statuses() {
    let repo = temp_test_dir("full-file-source-modes");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("old.rs"), "old\n").expect("old source should be written");
    fs::write(repo.join("new.rs"), "new\n").expect("new source should be written");
    git(&repo, &["add", "old.rs", "new.rs"]);
    git(&repo, &["commit", "-qm", "init"]);
    git(&repo, &["branch", "left"]);
    git(&repo, &["branch", "right"]);
    let file = mark_diff::DiffFile {
        change: mark_diff::FileChange::from_status(
            mark_diff::FileStatus::Renamed,
            Some("old.rs".to_owned()),
            Some("new.rs".to_owned()),
        ),
        additions: 0,
        deletions: 0,
        body: mark_diff::DiffFileBody::Text { hunks: Vec::new() },
    };

    assert_eq!(
        full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: "HEAD".into(),
            path: "old.rs".into(),
        }
    );
    assert_eq!(
        full_file_source(&repo, &DiffOptions::default(), &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::Worktree {
            path: "new.rs".into(),
        }
    );

    let base = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &base, &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitMergeBase {
            base: "main".into(),
            head: "HEAD".into(),
            path: "old.rs".into(),
        }
    );
    assert_eq!(
        full_file_source(&repo, &base, &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::Worktree {
            path: "new.rs".into(),
        }
    );

    let range = DiffOptions {
        source: DiffSource::Range {
            left: "left".into(),
            right: "right".into(),
        },
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &range, &file, DiffSide::New)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: "right".into(),
            path: "new.rs".into(),
        }
    );

    let object_range = DiffOptions {
        source: DiffSource::Range {
            left: "HEAD:old.rs".into(),
            right: "HEAD:new.rs".into(),
        },
        ..DiffOptions::default()
    };
    assert!(full_file_source(&repo, &object_range, &file, DiffSide::Old).is_none());
    assert!(full_file_source(&repo, &object_range, &file, DiffSide::New).is_none());

    let show = DiffOptions {
        source: DiffSource::Show("HEAD".into()),
        ..DiffOptions::default()
    };
    assert!(full_file_source(&repo, &show, &file, DiffSide::Old).is_none());
    assert!(full_file_source(&repo, &show, &file, DiffSide::New).is_none());

    let patch = DiffOptions {
        source: DiffSource::Patch(mark_diff::PatchSource::Stdin(Arc::from(&b""[..]))),
        ..DiffOptions::default()
    };
    assert!(full_file_source(&repo, &patch, &file, DiffSide::New).is_none());

    let mut deleted = file.clone();
    deleted.change = mark_diff::FileChange::from_status(
        mark_diff::FileStatus::Deleted,
        Some("old.rs".to_owned()),
        None,
    );
    assert!(full_file_source(&repo, &DiffOptions::default(), &deleted, DiffSide::New).is_none());

    let reflog_revision = "HEAD@{2030-01-01 20:43:02 -0700}";
    let reflog_range = DiffOptions {
        source: DiffSource::Range {
            left: reflog_revision.into(),
            right: reflog_revision.into(),
        },
        ..DiffOptions::default()
    };
    let old_reflog_source = full_file_source(&repo, &reflog_range, &file, DiffSide::Old)
        .expect("timestamped reflog revision should support full-file mode");
    let new_reflog_source = full_file_source(&repo, &reflog_range, &file, DiffSide::New)
        .expect("timestamped reflog revision should support full-file mode");
    assert_eq!(load_full_file_source(&old_reflog_source).unwrap(), "old\n");
    assert_eq!(load_full_file_source(&new_reflog_source).unwrap(), "new\n");

    let commit_search_range = DiffOptions {
        source: DiffSource::Range {
            left: ":/init".into(),
            right: ":/init".into(),
        },
        ..DiffOptions::default()
    };
    let old_commit_search_source =
        full_file_source(&repo, &commit_search_range, &file, DiffSide::Old)
            .expect("commit-search revision should support full-file mode");
    let new_commit_search_source =
        full_file_source(&repo, &commit_search_range, &file, DiffSide::New)
            .expect("commit-search revision should support full-file mode");
    assert_eq!(
        load_full_file_source(&old_commit_search_source).unwrap(),
        "old\n"
    );
    assert_eq!(
        load_full_file_source(&new_commit_search_source).unwrap(),
        "new\n"
    );

    let tree = git_stdout(&repo, &["rev-parse", "HEAD^{tree}"]);
    let tree_range = DiffOptions {
        source: DiffSource::Range {
            left: tree.clone().into(),
            right: "HEAD".into(),
        },
        ..DiffOptions::default()
    };
    assert_eq!(
        full_file_source(&repo, &tree_range, &file, DiffSide::Old)
            .unwrap()
            .kind,
        FullFileSourceKind::GitRevision {
            rev: tree.into(),
            path: "old.rs".into(),
        }
    );

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn reload_re_resolves_commit_search_range_sources() {
    let repo = temp_test_dir("commit-search-range-reload");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("file.rs"), "old\n").expect("old source should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-qm", "base old"]);

    let options = DiffOptions {
        source: DiffSource::Range {
            left: ":/base".into(),
            right: ":/base".into(),
        },
        ..DiffOptions::default()
    };
    let changeset = changeset_with_hunk_at(repo.clone(), 1);
    let mut app = DiffApp::new(options.clone(), changeset.clone(), DiffLayoutMode::Unified);
    let old_source = full_file_source(
        &repo,
        &options,
        &app.document.changeset.files[0],
        DiffSide::New,
    )
    .expect("initial commit-search source should resolve");
    assert_eq!(load_full_file_source(&old_source).unwrap(), "old\n");

    fs::write(repo.join("file.rs"), "new\n").expect("new source should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-qm", "base new"]);
    app.replace_loaded_diff(options.clone(), changeset);

    let refreshed_source = full_file_source(
        &repo,
        &options,
        &app.document.changeset.files[0],
        DiffSide::New,
    )
    .expect("reloaded commit-search source should resolve");
    assert_eq!(load_full_file_source(&refreshed_source).unwrap(), "new\n");

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn full_file_source_loads_worktree_and_revision_contents() {
    let repo = temp_test_dir("full-file-source");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);

    fs::write(repo.join("file.rs"), "fn old() {}\n").expect("old file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-q", "-m", "init"]);

    fs::write(repo.join("file.rs"), "fn new() {}\n").expect("new file should be written");
    assert_eq!(
        load_full_file_source(&FullFileSource {
            repo: repo.clone().into(),
            kind: FullFileSourceKind::GitRevision {
                rev: "HEAD".into(),
                path: "file.rs".into(),
            },
        })
        .unwrap(),
        "fn old() {}\n"
    );
    assert_eq!(
        load_full_file_source(&FullFileSource {
            repo: repo.clone().into(),
            kind: FullFileSourceKind::Worktree {
                path: "file.rs".into(),
            },
        })
        .unwrap(),
        "fn new() {}\n"
    );

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}
