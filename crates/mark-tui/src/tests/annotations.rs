use super::*;

#[test]
fn hunk_focus_uses_rendered_rows_when_annotations_hide_model_rows() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);

    let annotated_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                UiRow::UnifiedLine {
                    file: FILE_0,
                    hunk: HUNK_0,
                    ..
                }
            )
        })
        .expect("first hunk should have a rendered line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document
            .model
            .row(annotated_row)
            .expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "one\ntwo\nthree".to_owned());
    app.set_scroll(annotated_row.saturating_sub(1));

    let rendered_hunks: Vec<_> = plan_diff_viewport_rows(&app, app.viewport.viewport_rows)
        .into_iter()
        .filter_map(|slot| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. } => app
                .document
                .model
                .row(model_row)
                .and_then(|row| row.hunk_key()),
            _ => None,
        })
        .collect();
    assert!(rendered_hunks.contains(&(0, 0)));
    assert!(!rendered_hunks.contains(&(0, 1)));
    assert_eq!(
        app.focused_hunk_for_viewport(app.viewport.viewport_rows),
        Some((FILE_0, HUNK_0))
    );
    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 10,
        })
    );
}

#[test]
fn hunk_navigation_scrolls_past_annotation_blocks_to_show_target_hunk() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);

    let annotated_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                UiRow::UnifiedLine {
                    file: FILE_0,
                    hunk: HUNK_0,
                    ..
                }
            )
        })
        .expect("first hunk should have a rendered line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document
            .model
            .row(annotated_row)
            .expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "one\ntwo\nthree".to_owned());
    app.set_scroll(annotated_row.saturating_sub(1));

    assert_eq!(
        app.focused_hunk_for_viewport(app.viewport.viewport_rows),
        Some((FILE_0, HUNK_0))
    );

    app.next_hunk();

    let target_hunk_row = app
        .document
        .model
        .hunk_start_row(0, 1)
        .expect("target hunk should have a header row");
    let rendered_rows: Vec<_> = plan_diff_viewport_rows(&app, app.viewport.viewport_rows)
        .into_iter()
        .filter_map(|slot| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. } => Some(model_row),
            _ => None,
        })
        .collect();
    assert!(rendered_rows.contains(&target_hunk_row));
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_1)));
    assert_eq!(
        app.focused_hunk_for_viewport(app.viewport.viewport_rows),
        Some((FILE_0, HUNK_1))
    );

    app.next_hunk();
    assert_eq!(
        app.focused_hunk_for_viewport(app.viewport.viewport_rows),
        Some((FILE_0, HUNK_2))
    );
}

#[test]
fn max_scroll_reaches_rows_hidden_by_saved_annotation() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let lines: Vec<&str> = (0..5).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(app.document.model.len());

    let annotated_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document
            .model
            .row(annotated_row)
            .expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "note".to_owned());

    assert!(app.max_scroll() > annotated_row);
    app.set_scroll(app.max_scroll());
    let rendered_rows: Vec<_> = plan_diff_viewport_rows(&app, app.viewport.viewport_rows)
        .into_iter()
        .filter_map(|slot| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. } => Some(model_row),
            _ => None,
        })
        .collect();
    assert!(rendered_rows.contains(&(app.document.model.len() - 1)));
}

#[test]
fn max_scroll_for_short_annotation_at_end_avoids_blank_viewport() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let lines: Vec<&str> = (0..100).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(app.document.model.len());

    let annotated_row = app
        .document
        .model
        .rows
        .iter()
        .rposition(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("last unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document
            .model
            .row(annotated_row)
            .expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "note".to_owned());

    assert!(app.max_scroll() < annotated_row);
    app.set_scroll(usize::MAX);
    let plans = plan_diff_viewport_rows(&app, app.viewport.viewport_rows);

    assert_eq!(plans.len(), app.viewport.viewport_rows);
    assert!(plans.iter().any(|slot| matches!(
        slot.kind,
        ViewportSlotKind::DiffVisual { model_row, .. } if model_row == annotated_row
    )));
}

#[test]
fn annotation_navigation_centers_and_advances_from_centered_annotation() {
    use crate::annotation::AnnotationKey;

    let lines = vec!["line"; 100];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);

    let unified_rows = app
        .document
        .model
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| matches!(row, UiRow::UnifiedLine { .. }).then_some(index))
        .collect::<Vec<_>>();
    let first_row = unified_rows[19];
    let second_row = unified_rows[59];
    for row in [first_row, second_row] {
        let key = AnnotationKey::from_ui_row(
            &app.document.changeset,
            app.document.model.row(row).expect("annotated row"),
        )
        .expect("annotation key");
        app.annotations_state
            .annotations
            .insert(key, "note".to_owned());
    }

    let first_anchor = app.annotation_anchor_visual_scroll(first_row);
    let second_anchor = app.annotation_anchor_visual_scroll(second_row);
    let center = viewport_center_offset(app.viewport.viewport_rows);

    app.handle_key(KeyEvent::new(KeyCode::Char('}'), KeyModifiers::NONE))
        .expect("next annotation should be handled");
    assert_eq!(app.viewport.scroll + center, first_anchor);

    app.handle_key(KeyEvent::new(KeyCode::Char('}'), KeyModifiers::NONE))
        .expect("next annotation should advance from centered annotation");
    assert_eq!(app.viewport.scroll + center, second_anchor);

    app.handle_key(KeyEvent::new(KeyCode::Char('{'), KeyModifiers::NONE))
        .expect("previous annotation should advance from centered annotation");
    assert_eq!(app.viewport.scroll + center, first_anchor);
}

#[test]
fn annotation_navigation_renders_target_after_previous_saved_block() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let lines = vec!["line"; 40];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);

    let unified_rows = app
        .document
        .model
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| matches!(row, UiRow::UnifiedLine { .. }).then_some(index))
        .collect::<Vec<_>>();
    let first_row = unified_rows[8];
    let second_row = unified_rows[10];
    for (row, note) in [(first_row, "one\ntwo\nthree"), (second_row, "target")] {
        let key = AnnotationKey::from_ui_row(
            &app.document.changeset,
            app.document.model.row(row).expect("annotated row"),
        )
        .expect("annotation key");
        app.annotations_state
            .annotations
            .insert(key, note.to_owned());
    }

    app.move_annotation(1);
    app.move_annotation(1);

    fn rendered_rows(app: &DiffApp) -> Vec<usize> {
        plan_diff_viewport_rows(app, app.viewport.viewport_rows)
            .into_iter()
            .filter_map(|slot| match slot.kind {
                ViewportSlotKind::DiffVisual { model_row, .. } => Some(model_row),
                _ => None,
            })
            .collect::<Vec<_>>()
    }

    assert!(rendered_rows(&app).contains(&second_row));

    app.move_annotation(-1);
    assert!(rendered_rows(&app).contains(&first_row));
}

#[test]
fn queue_editor_scoped_reload_marks_dirty_for_terminal_repaint() {
    let changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 20);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.runtime.dirty = false;

    app.queue_editor_scoped_reload(EditorReloadRequest {
        path: PathBuf::from("src/file.rs"),
        pathspecs: vec![PathBuf::from("src/file.rs")],
        view_anchor: None,
    });

    assert!(app.runtime.dirty);
    assert!(app.jobs.pending_editor_reload.is_some());
}

#[test]
fn live_reload_started_state_marks_pending_until_loaded() {
    let changeset = changeset_with_files(&["src/lib.rs"]);
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset.clone(),
        DiffLayoutMode::Unified,
    );
    let reload_options = app.document.options.clone();
    let (reload_tx, mut reload_rx) = mpsc::channel(2);

    reload_tx
        .try_send(LiveDiffReload::Started)
        .expect("started reload should send");
    drain_live_reloads(&mut app, Some((&reload_options, &mut reload_rx)));

    assert_eq!(
        app.jobs.live_updates.status(),
        Some(LiveReloadStatus::Pending)
    );
    app.runtime.dirty = false;

    reload_tx
        .try_send(LiveDiffReload::Loaded(Ok(changeset)))
        .expect("loaded reload should send");
    drain_live_reloads(&mut app, Some((&reload_options, &mut reload_rx)));

    assert_eq!(app.jobs.live_updates.status(), Some(LiveReloadStatus::Idle));
    assert!(app.runtime.dirty);
}

#[test]
fn watcher_failure_is_visible_and_suppresses_automatic_restart() {
    let changeset = changeset_with_files(&["src/lib.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let reload_options = app.document.options.clone();
    let (reload_tx, mut reload_rx) = mpsc::channel(1);

    reload_tx
        .try_send(LiveDiffReload::WatcherFailed("watch overflow".to_owned()))
        .expect("watcher failure should send");
    drain_live_reloads(&mut app, Some((&reload_options, &mut reload_rx)));

    assert_eq!(
        app.jobs.live_diff_failed_options.as_ref(),
        Some(&app.document.options)
    );
    assert_eq!(
        app.notifications.error_log.as_deref(),
        Some("live reload watcher failed: watch overflow")
    );
}

#[test]
fn live_reloads_from_previous_diff_options_are_discarded() {
    let previous_options = DiffOptions::default();
    let current_options = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(
        current_options.clone(),
        changeset_with_files(&["current.rs"]),
        DiffLayoutMode::Unified,
    );
    let (reload_tx, mut reload_rx) = mpsc::channel(3);

    reload_tx
        .try_send(LiveDiffReload::Started)
        .expect("started reload should send");
    reload_tx
        .try_send(LiveDiffReload::Loaded(Ok(changeset_with_files(&[
            "stale.rs",
        ]))))
        .expect("loaded reload should send");
    reload_tx
        .try_send(LiveDiffReload::WatcherFailed("stale watcher".to_owned()))
        .expect("watcher failure should send");
    drain_live_reloads(&mut app, Some((&previous_options, &mut reload_rx)));

    assert_eq!(app.document.options, current_options);
    assert_eq!(app.document.changeset.files[0].display_path(), "current.rs");
    assert_eq!(app.jobs.live_updates.status(), Some(LiveReloadStatus::Idle));
    assert!(app.jobs.live_diff_failed_options.is_none());
    assert!(app.notifications.error_log.is_none());
    assert!(matches!(
        reload_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn grep_jump_scrolls_past_annotation_blocks_to_show_match() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let changeset = changeset_with_line_texts(&["annotated", "other", "needle", "tail"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);

    let annotated_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                UiRow::UnifiedLine {
                    file: FILE_0,
                    hunk: HUNK_0,
                    line: LINE_0
                }
            )
        })
        .expect("annotated row should exist");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document
            .model
            .row(annotated_row)
            .expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "note".to_owned());

    app.filters.grep_filter = "needle".to_owned();
    app.apply_filters(PostFilterNavigation::JumpToGrep);

    let match_row = app
        .current_grep_match_row()
        .expect("grep match should be selected");
    let rendered_rows: Vec<_> = plan_diff_viewport_rows(&app, app.viewport.viewport_rows)
        .into_iter()
        .filter_map(|slot| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. } => Some(model_row),
            _ => None,
        })
        .collect();
    assert!(rendered_rows.contains(&match_row));
}

#[test]
fn question_mark_key_opens_help_menu_and_filters_when_open() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(!app.overlays.help_menu_is_open());

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
        .expect("? should be handled");
    assert!(!should_quit);
    assert!(app.overlays.help_menu_is_open());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
        .expect("? should filter help");
    assert!(app.overlays.help_menu_is_open());
    assert_eq!(app.overlays.help_menu_input, "?");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("Esc should close help");
    assert!(!app.overlays.help_menu_is_open());
}

#[test]
fn question_mark_key_filters_branch_menu() {
    let options = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(options, changeset, DiffLayoutMode::Unified);
    app.refs.open_branch_menu(BranchMenu::Head);
    app.refs.comparison_branches = branch_names(&["main", "feature/header"]);

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT))
        .expect("? should be handled by branch filter");

    assert!(!should_quit);
    assert!(!app.overlays.help_menu_is_open());
    assert_eq!(app.refs.branch_menu.input, "?");
}

#[test]
fn copy_marks_writes_structured_json_to_clipboard_sequence() {
    use crate::annotation::AnnotationKey;

    let mut changeset = changeset_with_line_text("hello");
    changeset.files[0].hunks_mut()[0].lines[0] = DiffLine::addition(1, "hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    app.annotations_state
        .annotations
        .insert(key, "Needs \"escaping\"\nand context".to_owned());
    let expected = concat!(
        "{\n",
        "  \"version\": 1,\n",
        "  \"marks\": [\n",
        "    {\n",
        "      \"path\": \"file.rs\",\n",
        "      \"new_line\": 1,\n",
        "      \"body\": \"Needs \\\"escaping\\\"\\nand context\"\n",
        "    }\n",
        "  ]\n",
        "}"
    );
    let mut output = Vec::new();

    app.copy_marks_to_writer(&mut output);

    assert_eq!(app.marks_clipboard_json().as_deref(), Some(expected));
    assert_eq!(
        String::from_utf8(output).expect("OSC 52 sequence should be UTF-8"),
        osc52_clipboard_sequence(expected)
    );
    assert_eq!(app.notifications.toasts.latest_text(), Some("marks copied"));
}

#[test]
fn copy_marks_omits_annotations_without_current_diff_line() {
    use crate::annotation::{AnnotationKey, AnnotationSide};

    let changeset = changeset_with_replacement_pair();
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let new_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { line: LINE_1, .. }))
        .expect("addition line");
    let old_key = AnnotationKey {
        path: "file.rs".into(),
        side: AnnotationSide::Old,
        line: 1,
    };
    let new_key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(new_row).expect("new diff row"),
    )
    .expect("new-side key");
    assert_eq!(old_key.side, AnnotationSide::Old);
    assert_eq!(new_key.side, AnnotationSide::New);
    app.annotations_state
        .annotations
        .insert(old_key.clone(), "old note".to_owned());
    app.annotations_state
        .annotations
        .insert(new_key, "new note".to_owned());

    let mut replacement = changeset_with_line_text("new");
    set_test_file_added(&mut replacement.files[0]);
    replacement.files[0].additions = 1;
    {
        let hunk = &mut replacement.files[0].hunks_mut()[0];
        hunk.ranges = HunkLineRanges::new(hunk.old_start(), 0, hunk.new_start(), hunk.new_count());
        hunk.lines[0] = DiffLine::addition(1, "new");
    }
    app.replace_loaded_diff(DiffOptions::default(), replacement);

    let expected = concat!(
        "{\n",
        "  \"version\": 1,\n",
        "  \"marks\": [\n",
        "    {\n",
        "      \"path\": \"file.rs\",\n",
        "      \"new_line\": 1,\n",
        "      \"body\": \"new note\"\n",
        "    }\n",
        "  ]\n",
        "}"
    );

    assert_eq!(app.marks_clipboard_json().as_deref(), Some(expected));
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&old_key)
            .map(String::as_str),
        Some("old note")
    );
}

#[test]
fn copy_marks_includes_marks_on_collapsed_context_lines() {
    use crate::annotation::AnnotationKey;

    let repo = temp_test_dir("copy-collapsed-context-mark");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(app.expand_context_at_row(1));
    let context_row = app
        .document
        .model
        .rows
        .iter()
        .copied()
        .find(|row| matches!(row, UiRow::ContextLine { .. }))
        .expect("expanded context line");
    let key =
        AnnotationKey::from_ui_row(&app.document.changeset, context_row).expect("context key");
    app.annotations_state
        .annotations
        .insert(key.clone(), "context note".to_owned());

    assert!(app.hide_context(0, 0));
    assert!(
        !app.document.model.rows.iter().any(|row| matches!(
            row,
            UiRow::ContextLine { new_line, .. } if *new_line == key.line
        )),
        "marked context line should be collapsed"
    );

    let expected = format!(
        concat!(
            "{{\n",
            "  \"version\": 1,\n",
            "  \"marks\": [\n",
            "    {{\n",
            "      \"path\": \"file.rs\",\n",
            "      \"new_line\": {},\n",
            "      \"body\": \"context note\"\n",
            "    }}\n",
            "  ]\n",
            "}}"
        ),
        key.line
    );

    assert_eq!(
        app.marks_clipboard_json().as_deref(),
        Some(expected.as_str())
    );
}

fn finish_trailing_context_discovery(app: &mut DiffApp) {
    for _ in 0..1_000 {
        app.drain_trailing_context_worker();
        if app.jobs.trailing_context_worker.is_none() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("trailing context worker did not finish");
}

#[test]
fn revision_backed_trailing_context_discovery_runs_on_a_worker() {
    let repo = temp_test_dir("trailing-context-control");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "mark@example.com"]);
    git(&repo, &["config", "user.name", "Mark Test"]);
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-qm", "initial"]);
    let changeset = changeset_with_hunk_at(repo, 50);
    let options = DiffOptions {
        source: DiffSource::Range {
            left: "HEAD".into(),
            right: "HEAD".into(),
        },
        ..DiffOptions::default()
    };
    let mut app = DiffApp::new(options, changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(app.document.model.len());

    assert!(app.discover_trailing_context_for_viewport());
    assert!(app.jobs.trailing_context_worker.is_some());
    assert!(app.document.trailing_context_lines.is_empty());
    finish_trailing_context_discovery(&mut app);
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::Collapsed {
            hunk,
            new_start: 51,
            lines: 30,
            ..
        }) if hunk.get() == 1
    ));
    let control_row = app.document.model.len() - 1;
    assert!(app.handle_context_at_row(control_row));
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::ContextLine { new_line: 80, .. })
    ));
}

#[test]
fn full_file_mode_expands_discovered_trailing_context() {
    let repo = temp_test_dir("full-file-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.toggle_full_file();
    app.set_viewport_rows(app.document.model.len());

    assert!(app.discover_trailing_context_for_viewport());
    finish_trailing_context_discovery(&mut app);

    assert!(app.viewport.full_file);
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::ContextLine { new_line: 80, .. })
    ));
    assert!(app.document.model.rows.iter().all(|row| {
        !matches!(
            row,
            UiRow::Collapsed { .. } | UiRow::ContextHide { .. } | UiRow::HunkHeader { .. }
        )
    }));
}

#[test]
fn trailing_context_discovery_skips_oversized_sources_without_caching_them() {
    let repo = temp_test_dir("oversized-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let changeset = changeset_with_hunk_at(repo.clone(), 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let discovery_byte_limit = app
        .config
        .syntax_limits
        .max_source_bytes
        .min(mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES);
    let oversized = vec![b'x'; discovery_byte_limit + 1];
    fs::write(repo.join("file.rs"), oversized).expect("context file should be written");
    app.set_viewport_rows(app.document.model.len());

    assert!(app.discover_trailing_context_for_viewport());
    finish_trailing_context_discovery(&mut app);
    assert!(app.document.context_cache.is_empty());
    assert_eq!(
        app.document.trailing_context_lines.get(&ContextKey {
            file: FileIndex::new(0),
            hunk: HunkIndex::new(1),
        }),
        Some(&0)
    );
}

#[test]
fn cached_full_file_diff_retries_capped_trailing_context_discovery() {
    let repo = temp_test_dir("cached-capped-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let changeset = changeset_with_hunk_at(repo.clone(), 50);
    let options = DiffOptions::default();
    let mut app = DiffApp::new(options.clone(), changeset.clone(), DiffLayoutMode::Unified);
    let discovery_byte_limit = app
        .config
        .syntax_limits
        .max_source_bytes
        .min(mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES);
    let filler = "x".repeat(discovery_byte_limit / 80 + 2);
    let oversized = (1..=80)
        .map(|line| format!("line {line} {filler}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), oversized).expect("context file should be written");
    app.toggle_full_file();

    let key = ContextKey {
        file: FILE_0,
        hunk: HUNK_1,
    };
    let mut cached = diff_cache_entry(options.clone(), changeset);
    cached.trailing_context_lines.insert(key, 0);
    app.replace_cached_diff(options, cached, BranchMetadataPolicy::Preserve);

    assert!(!app.document.trailing_context_lines.contains_key(&key));
    app.set_viewport_rows(app.document.model.len());
    assert!(app.discover_trailing_context_for_viewport());
    finish_trailing_context_discovery(&mut app);
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::ContextLine { new_line: 80, .. })
    ));

    fs::remove_dir_all(repo).expect("repo directory should be removed");
}

#[test]
fn cached_diff_restores_trailing_context_metadata_with_its_model() {
    let repo = temp_test_dir("cached-oversized-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let changeset = changeset_with_hunk_at(repo.clone(), 50);
    let options = DiffOptions::default();
    let mut app = DiffApp::new(options.clone(), changeset.clone(), DiffLayoutMode::Unified);
    let discovery_byte_limit = app
        .config
        .syntax_limits
        .max_source_bytes
        .min(mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES);
    let filler = "x".repeat(discovery_byte_limit / 80 + 2);
    let oversized = (1..=80)
        .map(|line| format!("line {line} {filler}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), oversized).expect("context file should be written");

    assert!(app.expand_trailing_context_for_key(0, 1));
    assert!(app.hide_context(0, 1));
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::Collapsed { hunk, .. }) if hunk.get() == 1
    ));

    let other_options = DiffOptions {
        source: DiffSource::Base("main".into()),
        ..DiffOptions::default()
    };
    app.cache_loaded_diff(other_options.clone(), changeset);
    app.start_diff_load(other_options, "switch failed");
    assert!(app.jobs.pending_diff_load.is_none());
    app.start_diff_load(options, "switch back failed");
    assert!(app.jobs.pending_diff_load.is_none());

    let key = ContextKey {
        file: FileIndex::new(0),
        hunk: HunkIndex::new(1),
    };
    assert_eq!(app.document.trailing_context_lines.get(&key), Some(&30));
    assert!(!app.discover_trailing_context_for_viewport());
    let control_row = app.document.model.len() - 1;
    assert!(app.handle_context_at_row(control_row));
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::ContextLine { new_line: 80, .. })
    ));
}

#[test]
fn trailing_context_discovery_tries_old_side_after_oversized_new_side() {
    let repo = temp_test_dir("old-side-trailing-context");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "mark@example.com"]);
    git(&repo, &["config", "user.name", "Mark Test"]);
    let old_text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), old_text).expect("old source should be written");
    git(&repo, &["add", "file.rs"]);
    git(&repo, &["commit", "-qm", "initial"]);

    let changeset = changeset_with_hunk_at(repo.clone(), 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let discovery_byte_limit = app
        .config
        .syntax_limits
        .max_source_bytes
        .min(mark_syntax::DEFAULT_MAX_HIGHLIGHT_SOURCE_BYTES);
    fs::write(repo.join("file.rs"), vec![b'x'; discovery_byte_limit + 1])
        .expect("oversized new source should be written");
    app.set_viewport_rows(app.document.model.len());

    assert!(app.discover_trailing_context_for_viewport());
    finish_trailing_context_discovery(&mut app);
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::Collapsed {
            hunk,
            new_start: 51,
            lines: 30,
            ..
        }) if hunk.get() == 1
    ));
    let key = ContextKey {
        file: FileIndex::new(0),
        hunk: HunkIndex::new(1),
    };
    assert_eq!(
        app.document.trailing_context_sides.get(&key),
        Some(&DiffSide::Old)
    );
    let control_row = app.document.model.len() - 1;
    assert!(app.handle_context_at_row(control_row));
    assert!(matches!(
        app.document.model.rows.last(),
        Some(UiRow::ContextLine { new_line: 80, .. })
    ));
    assert_eq!(app.context_source_side(0), Some(DiffSide::Old));
}

#[test]
fn copy_marks_includes_marks_on_collapsed_trailing_context_lines() {
    use crate::annotation::AnnotationKey;

    let repo = temp_test_dir("copy-collapsed-trailing-context-mark");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    assert!(app.expand_trailing_context_for_key(0, 1));
    let context_row = app
        .document
        .model
        .rows
        .iter()
        .copied()
        .find(|row| matches!(row, UiRow::ContextLine { new_line: 51, .. }))
        .expect("expanded trailing context line");
    let key =
        AnnotationKey::from_ui_row(&app.document.changeset, context_row).expect("context key");
    app.annotations_state
        .annotations
        .insert(key.clone(), "trailing context note".to_owned());

    assert!(app.hide_context(0, 1));
    assert!(
        !app.document.model.rows.iter().any(|row| matches!(
            row,
            UiRow::ContextLine { new_line, .. } if *new_line == key.line
        )),
        "marked trailing context line should be collapsed"
    );
    app.annotations_state.annotations.insert(
        AnnotationKey {
            path: key.path.clone(),
            side: key.side,
            line: 81,
        },
        "stale trailing note".to_owned(),
    );

    let expected = format!(
        concat!(
            "{{\n",
            "  \"version\": 1,\n",
            "  \"marks\": [\n",
            "    {{\n",
            "      \"path\": \"file.rs\",\n",
            "      \"new_line\": {},\n",
            "      \"body\": \"trailing context note\"\n",
            "    }}\n",
            "  ]\n",
            "}}"
        ),
        key.line
    );

    assert_eq!(
        app.marks_clipboard_json().as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn copy_marks_without_marks_shows_notice_without_writing() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let mut output = Vec::new();

    app.copy_marks_to_writer(&mut output);

    assert!(output.is_empty());
    assert_eq!(
        app.notifications.toasts.latest_text(),
        Some("no marks to copy")
    );
}

#[test]
fn osc52_clipboard_sequence_base64_encodes_text() {
    assert_eq!(osc52_clipboard_sequence("abc"), "\x1b]52;c;YWJj\x07");
    assert_eq!(osc52_clipboard_sequence("mark"), "\x1b]52;c;bWFyaw==\x07");
}

#[test]
fn notices_mark_dirty_when_configured_max_visible_is_zero() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.notifications.toasts = Toasts::new(NotificationSettings::new(
        NotificationMode::Default,
        ToastCorner::TopRight,
        1_500,
        0,
    ));
    app.runtime.dirty = false;
    app.runtime.terminal_clear_requested = true;

    app.set_notice("editor closed");

    assert!(app.runtime.dirty);
    assert!(app.runtime.terminal_clear_requested);
    assert_eq!(
        app.notifications.toasts.latest_text(),
        Some("editor closed")
    );
}

#[test]
fn viewport_width_change_clamps_scroll_after_saved_annotation_rewraps() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(6);
    app.set_viewport_width(8);

    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state.annotations.insert(
        key,
        "one two three four five six seven eight nine ten".to_owned(),
    );

    app.set_scroll(app.max_scroll());
    let narrow_scroll = app.viewport.scroll;
    assert!(narrow_scroll > 0);

    app.set_viewport_width(80);

    assert!(narrow_scroll > app.max_scroll());
    assert_eq!(app.viewport.scroll, app.max_scroll());
    let lines = build_diff_viewport_lines(&mut app, 80, 6);
    assert!(lines.iter().any(|line| line_text(line).contains("hello")));
}

#[test]
fn annotation_target_mode_labels_the_entire_viewport_and_selects_a_hint() {
    use std::collections::HashSet;

    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(80);
    app.set_viewport_rows(app.document.model.len());

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");

    let mode = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .expect("annotation target mode");
    let visible_hunks = mode
        .targets
        .iter()
        .filter_map(|target| {
            app.document
                .model
                .row(target.model_row_index)
                .and_then(UiRow::hunk_key)
                .map(|(_, hunk)| hunk)
        })
        .collect::<HashSet<_>>();
    assert_eq!(visible_hunks, HashSet::from([0, 1, 2]));

    let target = mode
        .targets
        .iter()
        .find(|target| target.hint == "a")
        .cloned()
        .expect("focused hunk should receive the easiest hint");
    assert_eq!(
        app.document
            .model
            .row(target.model_row_index)
            .and_then(UiRow::hunk_key)
            .map(|(_, hunk)| hunk),
        Some(0)
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("hint should select its line");

    let draft = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .expect("selected line should open a draft");
    assert_eq!(draft.key, target.key);
    assert!(app.annotations_state.annotation_target_mode.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("draft text should be entered");
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .expect("draft should save");
    assert!(app.annotations_state.annotation_target_mode.is_none());
    assert!(!app.annotations_state.sticky_annotation_draft);
}

#[test]
fn annotation_target_mode_filters_multi_key_hints_and_supports_backspace() {
    let lines = vec!["line"; 40];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(80);
    app.set_viewport_rows(app.document.model.len());

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.iter().find(|target| target.hint.len() == 2))
        .cloned()
        .expect("tall viewport should use a two-key hint");
    let mut characters = target.hint.chars();
    let first = characters.next().expect("first hint character");

    app.handle_key(KeyEvent::new(KeyCode::Char(first), KeyModifiers::NONE))
        .expect("first hint character should filter targets");
    assert_eq!(
        app.annotations_state
            .annotation_target_mode
            .as_ref()
            .map(|mode| mode.prefix.as_str()),
        Some(&target.hint[..1])
    );

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        .expect("backspace should clear the target prefix");
    assert_eq!(
        app.annotations_state
            .annotation_target_mode
            .as_ref()
            .map(|mode| mode.prefix.as_str()),
        Some("")
    );

    for character in target.hint.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("hint should be accepted");
    }
    assert_eq!(
        app.annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| &draft.key),
        Some(&target.key)
    );
}

#[test]
fn no_op_reload_preserves_in_progress_annotation_target_mode() {
    let lines = vec!["line"; 40];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(
        DiffOptions::default(),
        changeset.clone(),
        DiffLayoutMode::Unified,
    );
    app.set_viewport_width(80);
    app.set_viewport_rows(app.document.model.len());

    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT))
        .expect("sticky annotation target mode should open");
    let first = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.iter().find(|target| target.hint.len() == 2))
        .and_then(|target| target.hint.chars().next())
        .expect("tall viewport should use a two-key hint");
    app.handle_key(KeyEvent::new(KeyCode::Char(first), KeyModifiers::NONE))
        .expect("first hint character should filter targets");
    let expected_mode = app
        .annotations_state
        .annotation_target_mode
        .clone()
        .expect("target mode should remain open after a partial hint");
    assert!(expected_mode.sticky);
    assert_eq!(expected_mode.prefix, first.to_string());

    app.replace_loaded_diff(DiffOptions::default(), changeset);

    assert_eq!(
        app.annotations_state.annotation_target_mode.as_ref(),
        Some(&expected_mode)
    );
}

#[test]
fn annotation_target_mode_shows_only_matching_hint_suffixes() {
    let lines = vec!["line"; 40];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(80);
    app.set_viewport_rows(app.document.model.len());

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    assert!(line_text(&statusline_header_line(&app, 80)).contains("targets · type hint · Esc"));
    let mode = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .expect("annotation target mode");
    let target = mode
        .targets
        .iter()
        .find(|target| target.hint.chars().count() == 2)
        .cloned()
        .expect("tall viewport should use a two-key hint");
    let first = target.hint.chars().next().expect("first hint character");
    let nonmatching_scroll = mode
        .targets
        .iter()
        .find(|candidate| !candidate.hint.starts_with(first))
        .map(|candidate| candidate.visual_scroll)
        .expect("a nonmatching target");

    app.handle_key(KeyEvent::new(KeyCode::Char(first), KeyModifiers::NONE))
        .expect("first hint character should filter targets");

    let expected_suffix = target.hint.strip_prefix(first).expect("remaining suffix");
    assert_eq!(
        app.annotation_target_hint_at_visual_scroll(target.visual_scroll)
            .map(|(hint, _, _)| hint),
        Some(expected_suffix)
    );
    assert!(
        app.annotation_target_hint_at_visual_scroll(nonmatching_scroll)
            .is_none()
    );
    let header = line_text(&statusline_header_line(&app, 80));
    assert!(header.contains(&format!("{first}… ·")));
    assert!(header.contains("matches · Backspace · Esc"));
}

#[test]
fn annotation_target_mode_uses_annotation_accent_for_existing_marks() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("code row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "existing note".to_owned());

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let hint = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.first())
        .map(|target| target.hint.clone())
        .expect("existing target");
    let rendered = build_diff_viewport_lines(&mut app, 40, 5);
    let hint_span = rendered[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == hint)
        .expect("hint span");

    assert_eq!(hint_span.style.fg, Some(app.config.theme.hunk));
    assert!(hint_span.style.add_modifier.contains(Modifier::UNDERLINED));
    assert_ne!(hint_span.style.bg, Some(app.config.theme.search_match_bg));
}

#[test]
fn annotation_target_mode_uses_configured_keys_and_optional_uppercase_display() {
    let changeset = changeset_with_line_texts(&["one", "two", "three"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(app.document.model.len());
    app.config.syntax_settings.annotations.hint_keys = "arst".to_owned();
    app.config.syntax_settings.annotations.uppercase_hints = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.iter().find(|target| target.hint == "a"))
        .cloned()
        .expect("configured first hint");
    assert!(
        app.annotations_state
            .annotation_target_mode
            .as_ref()
            .expect("target mode")
            .targets
            .iter()
            .all(|target| target
                .hint
                .chars()
                .all(|character| "arst".contains(character)))
    );

    let viewport_rows = app.viewport.viewport_rows;
    let rendered = build_diff_viewport_lines(&mut app, 40, viewport_rows);
    assert!(rendered.iter().any(|line| line.spans.iter().any(|span| {
        span.content.as_ref() == "A" && span.style.bg == Some(app.config.theme.search_match_bg)
    })));

    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT))
        .expect("uppercase form of configured hint should be accepted");
    assert_eq!(
        app.annotations_state
            .annotation_draft
            .as_ref()
            .map(|draft| &draft.key),
        Some(&target.key)
    );
}

#[test]
fn annotation_target_navigation_keys_cancel_and_continue_navigation() {
    let lines = vec!["line"; 100];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(10);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let scroll = app.viewport.scroll;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .expect("down should navigate");
    assert_eq!(app.viewport.scroll, scroll + 1);
    assert!(app.annotations_state.annotation_target_mode.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should reopen");
    let scroll = app.viewport.scroll;
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .expect("page down should navigate");
    assert_eq!(app.viewport.scroll, scroll + 20);
    assert!(app.annotations_state.annotation_target_mode.is_none());
}

#[test]
fn sticky_annotation_mode_reopens_targets_after_save_and_esc_exits() {
    let changeset = changeset_with_line_texts(&["one", "two", "three"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(app.document.model.len());

    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT))
        .expect("sticky annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .filter(|mode| mode.sticky)
        .and_then(|mode| mode.targets.first())
        .cloned()
        .expect("sticky target");
    for character in target.hint.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("hint should select a line");
    }
    assert!(app.annotations_state.sticky_annotation_draft);

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("draft text should be entered");
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .expect("draft should save");

    assert_eq!(
        app.annotations_state
            .annotations
            .get(&target.key)
            .map(String::as_str),
        Some("n")
    );
    assert!(app.annotations_state.annotation_draft.is_none());
    assert!(
        app.annotations_state
            .annotation_target_mode
            .as_ref()
            .is_some_and(|mode| mode.sticky)
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("Esc should exit sticky targeting");
    assert!(app.annotations_state.annotation_target_mode.is_none());
}

#[test]
fn annotation_target_mode_blocks_modified_navigation_but_preserves_hard_quit() {
    let lines = vec!["line"; 100];
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(10);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let scroll = app.viewport.scroll;
    assert!(
        !app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL,))
            .expect("modified navigation should be consumed")
    );
    assert_eq!(app.viewport.scroll, scroll);
    assert!(app.annotations_state.annotation_target_mode.is_some());

    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL,))
            .expect("Ctrl-C should retain hard-quit behavior")
    );
}

#[test]
fn annotation_target_mode_labels_wrapped_logical_lines_once() {
    let changeset = changeset_with_line_texts(&[
        "a very long logical line that wraps several times",
        "another logical line",
    ]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.viewport.line_wrapping = true;
    app.set_viewport_width(18);
    app.set_viewport_rows(20);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");

    let mode = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .expect("annotation target mode");
    assert_eq!(mode.targets.len(), 2);
    assert_ne!(mode.targets[0].visual_scroll, mode.targets[1].visual_scroll);
}

#[test]
fn unified_hint_replaces_the_line_number_and_preserves_the_diff_sign() {
    let mut changeset = changeset_with_line_text("hello");
    changeset.files[0].hunks_mut()[0].lines[0] = DiffLine::addition(7, "hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(5);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.first())
        .cloned()
        .expect("addition target");

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 5);
    let characters = line_text(&rendered[0]).chars().collect::<Vec<_>>();
    assert_eq!(characters[11], target.hint.chars().next().expect("hint"));
    assert_eq!(characters[13], '+');
}

#[test]
fn split_replacement_hint_replaces_the_current_side_line_number() {
    use crate::annotation::AnnotationSide;

    let changeset = changeset_with_replacement_pair();
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.set_viewport_width(60);
    app.set_viewport_rows(8);
    let split_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::SplitLine { .. }))
        .expect("split line");
    app.viewport.scroll = split_row;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.first())
        .cloned()
        .expect("replacement target");
    assert_eq!(target.key.side, AnnotationSide::New);

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 8);
    let characters = line_text(&rendered[0]).chars().collect::<Vec<_>>();
    assert_eq!(characters[7], '-');
    assert_eq!(characters[35], target.hint.chars().next().expect("hint"));
    assert_eq!(characters[37], '+');
    assert!(rendered[0].spans.iter().any(|span| {
        span.content.as_ref() == target.hint
            && span.style.bg == Some(app.config.theme.search_match_bg)
    }));
}

#[test]
fn split_deletion_only_hint_replaces_the_old_side_line_number() {
    use crate::annotation::AnnotationSide;

    let mut changeset = changeset_with_replacement_pair();
    changeset.files[0].additions = 0;
    {
        let hunk = &mut changeset.files[0].hunks_mut()[0];
        hunk.ranges = HunkLineRanges::new(hunk.old_start(), hunk.old_count(), hunk.new_start(), 0);
        hunk.lines.truncate(1);
    }
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.set_viewport_width(60);
    app.set_viewport_rows(8);
    let split_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::SplitLine { .. }))
        .expect("split line");
    app.viewport.scroll = split_row;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .expect("annotation target mode should open");
    let target = app
        .annotations_state
        .annotation_target_mode
        .as_ref()
        .and_then(|mode| mode.targets.first())
        .cloned()
        .expect("deletion target");
    assert_eq!(target.key.side, AnnotationSide::Old);

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 8);
    let characters = line_text(&rendered[0]).chars().collect::<Vec<_>>();
    assert_eq!(characters[5], target.hint.chars().next().expect("hint"));
    assert_eq!(characters[7], '-');
    assert_eq!(characters[37], ' ');
}

#[test]
fn old_side_annotation_renders_and_edits_on_deletion_only_split_row() {
    use crate::annotation::{AnnotationKey, AnnotationSide};

    let mut changeset = changeset_with_replacement_pair();
    changeset.files[0].additions = 0;
    {
        let hunk = &mut changeset.files[0].hunks_mut()[0];
        hunk.ranges = HunkLineRanges::new(hunk.old_start(), hunk.old_count(), hunk.new_start(), 0);
        hunk.lines.truncate(1);
    }
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 60,
        height: 8,
    });
    app.set_viewport_width(60);
    app.set_viewport_rows(8);
    let split_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::SplitLine { .. }))
        .expect("split line");
    app.viewport.scroll = split_row;
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(split_row).expect("row"),
    )
    .expect("key");
    assert_eq!(key.side, AnnotationSide::Old);
    app.annotations_state
        .annotations
        .insert(key.clone(), "old note".to_owned());

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 8);
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains("old note"))
    );
    let footer_row = rendered
        .iter()
        .position(|line| line_text(line).ends_with("[↻]"))
        .expect("edit footer") as u16;

    assert!(app.handle_diff_click(58, 1 + footer_row));
    let draft = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .expect("draft");
    assert_eq!(draft.key, key);
    assert_eq!(draft.input, "old note");
}

#[test]
fn renamed_file_annotations_use_side_specific_paths() {
    use crate::annotation::{AnnotationKey, AnnotationSide};

    let mut changeset = changeset_with_replacement_pair();
    set_test_file_renamed(&mut changeset.files[0], "old.rs", "new.rs");

    let app = DiffApp::new(
        DiffOptions::default(),
        changeset.clone(),
        DiffLayoutMode::Unified,
    );
    let deletion_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { line: LINE_0, .. }))
        .expect("deletion line");
    let old_key = AnnotationKey {
        path: "old.rs".into(),
        side: AnnotationSide::Old,
        line: 1,
    };

    let addition_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { line: LINE_1, .. }))
        .expect("addition line");
    assert_eq!(
        AnnotationKey::from_ui_row(
            &app.document.changeset,
            app.document.model.row(deletion_row).expect("deletion row"),
        ),
        None
    );
    let new_key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(addition_row).expect("addition row"),
    )
    .expect("new-side key");
    assert_eq!(new_key.path, "new.rs");
    assert_eq!(new_key.side, AnnotationSide::New);

    let split_app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    let split_row = split_app
        .document
        .model
        .rows
        .iter()
        .find(|row| matches!(row, UiRow::SplitLine { .. }))
        .copied()
        .expect("split row");
    let split_keys =
        AnnotationKey::candidates_from_ui_row(&split_app.document.changeset, split_row);
    assert!(!split_keys.contains(&old_key));
    assert!(split_keys.contains(&new_key));

    let mut export_app = split_app;
    export_app
        .annotations_state
        .annotations
        .insert(new_key, "new note".to_owned());
    let json = export_app.marks_clipboard_json().expect("marks JSON");
    assert!(json.contains("\"path\": \"new.rs\""));
    assert!(json.contains("\"old_line\": 1"));
    assert!(json.contains("\"new_line\": 1"));
    assert!(!json.contains("\"path\": \"old.rs\""));
}

#[test]
fn meta_rows_do_not_render_annotation_add_button() {
    let mut changeset = changeset_with_line_text("placeholder");
    let line = &mut changeset.files[0].hunks_mut()[0].lines[0];
    *line = DiffLine::meta("\\ No newline at end of file");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 5,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(5);
    let meta_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::MetaLine { .. }))
        .expect("meta line");
    app.viewport.scroll = meta_row;
    app.update_diff_mouse_hover(38, 1);

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 5);
    assert!(!line_text(&rendered[0]).contains("[+]"));
    assert!(!app.handle_diff_click(38, 1));
    assert!(app.annotations_state.annotation_draft.is_none());
}

#[test]
fn annotation_height_cache_tracks_text_and_viewport_width() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("row"),
    )
    .expect("key");
    app.annotations_state
        .annotations
        .insert(key.clone(), "one two three four".to_owned());

    let _ = app.max_scroll();
    let first = *app
        .annotations_state
        .annotation_heights
        .borrow()
        .get(&key)
        .expect("cached annotation height");
    assert_eq!(first.width, 8);

    app.annotations_state
        .annotations
        .insert(key.clone(), "abcdefghijklmnopqr".to_owned());
    let replacement_ptr = app.annotations_state.annotations[&key].as_ptr() as usize;
    let _ = app.max_scroll();
    let replacement = *app
        .annotations_state
        .annotation_heights
        .borrow()
        .get(&key)
        .expect("replacement annotation height");
    assert_eq!(replacement.text_ptr, replacement_ptr);
    assert_ne!(replacement.text_ptr, first.text_ptr);

    app.set_viewport_width(20);
    let resized = *app
        .annotations_state
        .annotation_heights
        .borrow()
        .get(&key)
        .expect("resized annotation height");
    assert_eq!(resized.width, 20);
}

#[test]
fn annotation_row_cache_is_invalidated_with_the_view_model() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("row"),
    )
    .expect("key");

    app.annotations_state
        .annotation_rows
        .borrow_mut()
        .insert(key.clone(), Some(usize::MAX));
    app.set_layout(DiffLayoutMode::Split);

    assert_ne!(app.annotation_model_row(&key), Some(usize::MAX));
    assert!(app.annotation_model_row(&key).is_some());
}

#[test]
fn annotation_row_cache_batches_multiple_missing_keys() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let expected = app
        .document
        .model
        .iter_rows()
        .enumerate()
        .filter_map(|(row, model_row)| {
            AnnotationKey::from_ui_row(&app.document.changeset, model_row).map(|key| (key, row))
        })
        .take(3)
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 3);
    for (key, _) in &expected {
        app.annotations_state
            .annotations
            .insert(key.clone(), "note".to_owned());
    }

    app.cache_annotation_model_rows();

    for (key, row) in expected {
        assert_eq!(app.annotation_model_row(&key), Some(row));
    }
}

#[test]
fn annotations_are_keyed_by_path_and_line_across_model_changes() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(6);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    app.annotations_state
        .annotations
        .insert(key, "note".to_owned());

    app.set_layout(DiffLayoutMode::Split);
    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert!(rendered.iter().any(|line| line_text(line).contains("note")));

    let mut replacement = changeset_with_line_text("hello");
    set_test_file_modified(&mut replacement.files[0], "other.rs");
    app.replace_loaded_diff(DiffOptions::default(), replacement);

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert!(!rendered.iter().any(|line| line_text(line).contains("note")));
    assert_eq!(app.marks_clipboard_json(), None);
}

#[test]
fn annotation_input_wraps_words_and_ctrl_s_saves() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 20,
        height: 8,
    });
    app.set_viewport_width(20);
    app.set_viewport_rows(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.update_diff_mouse_hover(18, 1);
    assert!(app.handle_diff_click(18, 1));

    for character in "alpha beta gamma delta".chars() {
        app.handle_annotation_input_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ));
    }

    let lines = crate::render::diff::build_diff_viewport_lines(&mut app, 20, 8);
    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("alpha beta gamma"))
    );
    assert!(rendered.iter().any(|line| line.contains("delta")));
    assert!(rendered.iter().all(|line| line.width() <= 20));

    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for character in "next".chars() {
        app.handle_annotation_input_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ));
    }
    assert!(app.annotations_state.annotation_draft.is_some());

    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    assert!(app.annotations_state.annotation_draft.is_none());
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("alpha beta gamma delta\nnext")
    );
}

#[test]
fn annotation_rendering_preserves_whitespace_while_wrapping() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(12);
    app.set_viewport_rows(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state
        .annotations
        .insert(key, "  indented  code\na\t\tb".to_owned());
    app.viewport.scroll = code_row;

    let rendered: Vec<String> = crate::render::diff::build_diff_viewport_lines(&mut app, 12, 8)
        .iter()
        .map(line_text)
        .collect();

    assert!(rendered.iter().any(|line| line.starts_with("  indented  ")));
    assert!(rendered.iter().any(|line| line.starts_with("code")));
    assert!(rendered.iter().any(|line| line.contains("a        b")));
}

#[test]
fn annotation_rendering_wraps_expanded_tabs_without_panic() {
    let lines = crate::render::annotations::render_annotation_saved_block(
        "\tab",
        4,
        DiffTheme::default(),
        None,
    );

    assert_eq!(
        lines.len(),
        crate::render::annotations::annotation_saved_block_height("\tab", 4)
    );
    assert_eq!(line_text(&lines[1]), "    ");
    assert_eq!(line_text(&lines[2]), "ab  ");
}

#[test]
fn annotation_rendering_preserves_partial_tabs_across_wraps() {
    let lines = crate::render::annotations::render_annotation_saved_block(
        "a\tb",
        4,
        DiffTheme::default(),
        None,
    );

    assert_eq!(line_text(&lines[1]), "a   ");
    assert_eq!(line_text(&lines[2]), " b  ");

    let narrow_lines = crate::render::annotations::render_annotation_saved_block(
        "\t",
        2,
        DiffTheme::default(),
        None,
    );

    assert_eq!(line_text(&narrow_lines[1]), "  ");
    assert_eq!(line_text(&narrow_lines[2]), "  ");
}

#[test]
fn annotation_rendering_preserves_partial_control_escapes_across_wraps() {
    let lines = crate::render::annotations::render_annotation_saved_block(
        "x\u{1}y",
        4,
        DiffTheme::default(),
        None,
    );

    assert_eq!(line_text(&lines[1]), r"x\u{");
    assert_eq!(line_text(&lines[2]), "1}y ");
}

#[test]
fn annotation_input_supports_native_cursor_shortcuts() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 8,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.update_diff_mouse_hover(38, 1);
    assert!(app.handle_diff_click(38, 1));

    for character in "hello world".chars() {
        app.handle_annotation_input_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ));
    }
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::SUPER));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some(">hello world!\n")
    );
}

#[test]
fn annotation_save_and_cancel_use_configured_keybindings() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        save_mark = "ctrl-enter"
        cancel_mark = "ctrl-x"
        "#,
    )
    .expect("keymap should parse");
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 8,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.update_diff_mouse_hover(38, 1);
    assert!(app.handle_diff_click(38, 1));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(app.annotations_state.annotation_draft.is_some());
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));

    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("n")
    );

    let rendered = build_diff_viewport_lines(&mut app, 40, 8);
    let edit_row = rendered
        .iter()
        .position(|line| line_text(line).ends_with("[↻]"))
        .expect("saved annotation edit footer") as u16
        + app.viewport.rendered_diff_area.expect("diff area").y;
    assert!(app.handle_diff_click(38, edit_row));
    assert!(app.annotations_state.annotation_draft.is_some());
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(app.annotations_state.annotation_draft.is_none());
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("n")
    );
}

#[test]
fn annotation_cancel_binding_preempts_overlapping_edit_hunk_shortcut() {
    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        edit_hunk = "ctrl-x"
        cancel_mark = "ctrl-x"
        "#,
    )
    .expect("keymap should parse");
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 8,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(8);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.update_diff_mouse_hover(38, 1);
    assert!(app.handle_diff_click(38, 1));
    assert!(app.annotations_state.annotation_draft.is_some());

    assert!(!handle_test_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
    ));

    assert!(app.annotations_state.annotation_draft.is_none());
    assert!(app.annotations_state.annotations.is_empty());
}

#[test]
fn annotation_typing_e_does_not_open_editor_shortcut() {
    use crate::annotation::{AnnotationDraft, AnnotationKey};

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key,
        model_row_index: code_row,
        input: String::new(),
        cursor: 0,
    });

    assert!(!handle_test_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)
    ));

    let draft = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .expect("draft should remain open");
    assert_eq!(draft.input, "e");
    assert_eq!(draft.cursor, 1);
}

#[test]
fn annotation_save_binding_preempts_overlapping_edit_hunk_shortcut() {
    use crate::annotation::{AnnotationDraft, AnnotationKey};

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        edit_hunk = "ctrl-g"
        save_mark = "ctrl-g"
        "#,
    )
    .expect("keymap should parse");
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key: key.clone(),
        model_row_index: code_row,
        input: "note".to_owned(),
        cursor: "note".len(),
    });

    assert!(!handle_test_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
    ));

    assert!(app.annotations_state.annotation_draft.is_none());
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note")
    );
}

#[test]
fn annotation_draft_bindings_preempt_hard_quit_key() {
    use crate::annotation::{AnnotationDraft, AnnotationKey};

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        save_mark = "ctrl-c"
        "#,
    )
    .expect("keymap should parse");
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("annotation key");
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key: key.clone(),
        model_row_index: code_row,
        input: "note".to_owned(),
        cursor: "note".len(),
    });

    assert!(!handle_test_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));

    assert!(app.annotations_state.annotation_draft.is_none());
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note")
    );

    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        cancel_mark = "ctrl-c"
        "#,
    )
    .expect("keymap should parse");
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key: key.clone(),
        model_row_index: code_row,
        input: "discard".to_owned(),
        cursor: "discard".len(),
    });

    assert!(!handle_test_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));

    assert!(app.annotations_state.annotation_draft.is_none());
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note")
    );
}

#[test]
fn annotation_compose_scrolls_with_annotated_line() {
    let lines: Vec<&str> = (0..12).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(40);
    app.set_viewport_rows(6);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 6,
    });
    let compose_viewport_row = (code_row - app.viewport.scroll) as u16;
    app.update_diff_mouse_hover(38, compose_viewport_row.saturating_add(1));
    assert!(app.handle_diff_click(38, compose_viewport_row.saturating_add(1)));
    assert!(app.annotations_state.annotation_draft.is_some());

    let before = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert!(
        before
            .iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR)),
        "compose visible when annotated line is in view"
    );

    app.set_scroll(code_row.saturating_add(6));
    let after = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert!(
        !after
            .iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR)),
        "compose should scroll with the line, not stick on screen"
    );

    app.set_scroll(code_row);
    let back = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert!(
        back.iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR)),
        "compose returns when line scrolls back into view"
    );
}

#[test]
fn inline_emphasis_marks_changed_tokens_in_paired_lines() {
    let lines = vec![
        DiffLine::deletion(1, "let count = 1;".to_owned()),
        DiffLine::addition(1, "let total = 2;".to_owned()),
    ];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert_eq!(
        range_texts(lines[0].text(), &emphasis[0].ranges),
        ["count", "1"]
    );
    assert_eq!(
        range_texts(lines[1].text(), &emphasis[1].ranges),
        ["total", "2"]
    );
}

#[test]
fn closed_queue_marks_full_file_source_skipped() {
    let queue = SyntaxWorkerQueue::new(8, 0, usize::MAX);
    queue.close();
    let mut syntax = syntax_runtime_with_queue(queue);
    let source_id = SyntaxSourceId {
        generation: 0,
        file: 0,
        side: DiffSide::New,
        kind: SyntaxSourceKind::FullFile,
    };
    let key = SyntaxKey {
        source: source_id,
        language_hash: 1,
        theme_id: SYNTAX_THEME_ID,
    };

    assert!(!syntax.queue_job(
        key,
        "rust".to_owned(),
        full_file_syntax_job_source(),
        SyntaxPriority::Visible,
        None,
    ));

    assert!(syntax.skipped_sources.contains(&source_id));
    assert_eq!(syntax.stats.jobs_skipped, 1);
    assert_eq!(syntax.stats.jobs_rejected, 0);
}

#[test]
fn hunk_source_keeps_single_line_without_trailing_newline_marker() {
    let lines = vec![DiffLine::addition(1, "let value = 1;".to_owned())];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "let value = 1;");
    assert_eq!(source.line_map, vec![Some(0)]);
    assert_eq!(source.source_lines, 1);
}
