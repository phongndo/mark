use super::*;

#[test]
fn hunk_focus_moves_between_hunks_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));

    app.next_hunk();
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));

    app.next_hunk();
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_2)));

    app.previous_hunk();
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));

    app.previous_hunk();
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));
}

#[test]
fn layout_toggle_resets_manual_hunk_focus_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));

    app.next_hunk();
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.viewport.manual_hunk_focus, Some((FILE_0, HUNK_1)));
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));

    app.toggle_layout();

    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.viewport.manual_hunk_focus, None);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));
}

#[test]
fn j_and_k_move_hunk_focus_when_diff_fits_viewport() {
    let changeset = changeset_with_hunks_at(PathBuf::from("/repo"), &[1, 2, 3]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .expect("j should be handled");
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .expect("j should be handled");
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_2)));

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .expect("k should be handled");
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_1)));

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .expect("k should be handled");
    assert_eq!(app.viewport.scroll, 0);
    assert_eq!(app.focused_hunk_for_viewport(20), Some((FILE_0, HUNK_0)));
}

#[test]
fn bracket_hunk_navigation_centers_hunk_that_fits_viewport() {
    let changeset =
        changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 8), (20, 4), (40, 10)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    let range = app
        .document
        .model
        .hunk_row_range(0, 1)
        .expect("target hunk should have rows");

    app.next_hunk();

    let hunk_center = range
        .start
        .saturating_add(range.end.saturating_sub(range.start).saturating_sub(1) / 2);
    assert_eq!(
        app.viewport.scroll + viewport_center_offset(app.viewport.viewport_rows),
        hunk_center
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_1)));
}

#[test]
fn hunk_navigation_keeps_adjacent_file_header_with_oversized_hunk() {
    let changeset = changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(1, 20)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.set_scroll(5);

    let hunk_row = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("target hunk should have a header row");
    app.focus_hunk_row(hunk_row);

    assert_eq!(
        app.viewport.scroll,
        app.document.model.file_start_row(0).unwrap()
    );
    assert_eq!(
        app.document.model.row(app.viewport.scroll),
        Some(UiRow::FileHeader(FILE_0))
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_0)));
}

#[test]
fn hunk_navigation_keeps_file_header_before_collapsed_context() {
    let changeset = changeset_with_hunk_line_counts(PathBuf::from("/repo"), &[(233, 20)]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(9);
    app.set_scroll(5);

    let hunk_row = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("target hunk should have a header row");
    app.focus_hunk_row(hunk_row);

    assert_eq!(
        app.viewport.scroll,
        app.document.model.file_start_row(0).unwrap()
    );
    assert_eq!(
        app.document.model.row(app.viewport.scroll),
        Some(UiRow::FileHeader(FILE_0))
    );
    assert!(matches!(
        app.document.model.row(app.viewport.scroll + 1),
        Some(UiRow::Collapsed { .. })
    ));
    assert_eq!(
        app.document.model.row(app.viewport.scroll + 2),
        Some(UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_0
        })
    );
    assert_eq!(app.focused_hunk_for_viewport(9), Some((FILE_0, HUNK_0)));
}

#[test]
fn focused_hunk_editor_target_uses_new_path_and_visible_line() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);
    app.set_scroll(40);

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 44,
        })
    );
}

#[test]
fn editor_reload_restores_the_focused_line_at_its_viewport_row() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let replacement = changeset_with_context_lines_at(repo, 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);
    app.set_scroll(40);

    assert_eq!(app.focused_hunk_editor_target().unwrap().line, 44);
    app.replace_path_changeset(Path::new("file.rs"), replacement);
    app.set_scroll(0);
    app.restore_editor_view_for_test(Path::new("file.rs"), 44, 5);

    assert_eq!(app.viewport.scroll, 40);
    assert_eq!(app.focused_hunk_editor_target().unwrap().line, 44);
}

#[test]
fn editor_reload_restores_anchor_through_annotation_rows() {
    use crate::annotation::AnnotationKey;
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let replacement = changeset_with_context_lines_at(repo, 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_width(80);
    app.set_viewport_rows(11);

    let annotated_row = (0..app.document.model.len())
        .find(|row| app.editor_line_at_hunk_row(*row, 0, 0) == Some(43))
        .expect("line above the editor anchor should be rendered");
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

    app.replace_path_changeset(Path::new("file.rs"), replacement);
    app.set_scroll(0);
    app.restore_editor_view_for_test(Path::new("file.rs"), 44, 5);

    let anchor_viewport_row = plan_diff_viewport_rows(&app, app.viewport.viewport_rows)
        .into_iter()
        .enumerate()
        .find_map(|(viewport_row, slot)| match slot.kind {
            ViewportSlotKind::DiffVisual { model_row, .. }
                if app.editor_line_at_hunk_row(model_row, 0, 0) == Some(44) =>
            {
                Some(viewport_row)
            }
            _ => None,
        })
        .expect("editor anchor should be visible");
    assert_eq!(anchor_viewport_row, 5);
}

#[test]
fn editor_reload_finds_anchor_between_long_deletion_runs() {
    let mut changeset = changeset_with_context_lines(3);
    let hunk = &mut changeset.files[0].hunks_mut()[0];
    let mut lines = vec![DiffLine::addition(1, "first")];
    lines.extend((1..=300).map(|line| DiffLine::deletion(line, "before")));
    lines.push(DiffLine::addition(2, "target"));
    lines.extend((301..=600).map(|line| DiffLine::deletion(line, "after")));
    lines.push(DiffLine::addition(3, "last"));
    hunk.ranges = HunkLineRanges::new(1, 600, 1, 3);
    hunk.lines = lines;

    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);
    app.restore_editor_view_for_test(Path::new("file.rs"), 2, 5);

    assert_eq!(app.focused_hunk_editor_target().unwrap().line, 2);
}

#[test]
fn editor_reload_does_not_restore_after_navigation_during_reload() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let replacement = changeset_with_context_lines_at(repo, 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);
    app.set_scroll(40);

    let navigation = EditorReloadNavigation {
        scroll: app.viewport.scroll,
        selected_file: app.sidebar.selected_file,
        manual_hunk_focus: app.viewport.manual_hunk_focus,
        layout: app.viewport.layout,
        line_wrapping: app.viewport.line_wrapping,
    };
    let (tx, rx) = oneshot::channel();
    assert!(
        tx.send(EditorScopedReload {
            path: PathBuf::from("file.rs"),
            changeset: Ok(replacement),
            view_anchor: Some(EditorViewAnchor {
                line: 44,
                viewport_row: 5,
            }),
        })
        .is_ok()
    );
    app.jobs.editor_reload = Some(EditorReloadWorker {
        generation: app.document.generation,
        navigation,
        job: AsyncJob::new(rx),
    });

    app.set_scroll(0);
    assert!(app.drain_editor_reload());

    assert_eq!(app.viewport.scroll, 0);
    assert_ne!(app.focused_hunk_editor_target().unwrap().line, 44);
}

#[test]
fn editor_reload_does_not_restore_after_layout_change_during_reload() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_context_lines_at(repo.clone(), 1, 100);
    let replacement = changeset_with_context_lines_at(repo, 1, 100);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(11);

    let navigation = EditorReloadNavigation {
        scroll: app.viewport.scroll,
        selected_file: app.sidebar.selected_file,
        manual_hunk_focus: app.viewport.manual_hunk_focus,
        layout: app.viewport.layout,
        line_wrapping: app.viewport.line_wrapping,
    };
    let (tx, rx) = oneshot::channel();
    assert!(
        tx.send(EditorScopedReload {
            path: PathBuf::from("file.rs"),
            changeset: Ok(replacement),
            view_anchor: Some(EditorViewAnchor {
                line: 44,
                viewport_row: 5,
            }),
        })
        .is_ok()
    );
    app.jobs.editor_reload = Some(EditorReloadWorker {
        generation: app.document.generation,
        navigation,
        job: AsyncJob::new(rx),
    });

    app.toggle_layout();
    assert_eq!(app.viewport.scroll, navigation.scroll);
    assert!(app.drain_editor_reload());

    assert_eq!(app.viewport.layout, DiffLayoutMode::Split);
    assert_eq!(app.viewport.scroll, 0);
}

#[test]
fn focused_hunk_editor_target_uses_manual_focus_when_diff_fits_viewport() {
    let repo = PathBuf::from("/repo");
    let changeset = changeset_with_hunks_at(repo.clone(), &[10, 20, 30]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(20);

    app.next_hunk();
    app.next_hunk();

    assert_eq!(
        app.focused_hunk_editor_target(),
        Some(EditorTarget {
            path: repo.join("file.rs"),
            line: 30,
        })
    );
}

#[test]
fn debug_notifications_emit_terminal_event_toasts() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.notifications.toasts = Toasts::new(NotificationSettings::new(
        NotificationMode::Debug,
        ToastCorner::TopRight,
        1_500,
        3,
    ));
    let (_tx, rx) = mpsc::channel(1);
    let mut events = crate::event_reader::TerminalEventReader::from_receiver(rx);
    let mut live_diff = None;

    let should_quit = handle_event(
        &mut app,
        Event::Resize(120, 40),
        &mut live_diff,
        &mut events,
    )
    .expect("resize should be handled");

    assert!(!should_quit);
    assert_eq!(
        app.notifications.toasts.latest_text(),
        Some("event: resize 120x40")
    );
}

#[test]
fn file_separator_line_draws_rule_across_full_width() {
    let theme = DiffTheme::default();
    let line = file_separator_line(DiffLayoutMode::Unified, 24, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 24);
    assert_eq!(text, "────────────────────────");
    assert_eq!(line.spans[0].style.bg, Some(base_bg(theme)));
    assert_eq!(line.spans[0].style.fg, Some(theme.empty_diff));
}

#[test]
fn ui_model_expands_context_before_hunk_from_nearest_lines() {
    let step = default_context_expand_step();
    let changeset = changeset_with_hunk_at(PathBuf::from("/repo"), 50);
    let mut expansions = HashMap::new();

    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);
    assert_eq!(
        model.row(1),
        Some(UiRow::Collapsed {
            file: FILE_0,
            hunk: HUNK_0,
            old_start: 1,
            new_start: 1,
            lines: 49,
            expanded: 0,
        })
    );

    expansions.insert(
        ContextKey {
            file: FILE_0,
            hunk: HUNK_0,
        },
        step,
    );
    let model = UiModel::new(&changeset, DiffLayoutMode::Unified, &expansions);

    assert_eq!(
        model.row(1),
        Some(UiRow::Collapsed {
            file: FILE_0,
            hunk: HUNK_0,
            old_start: 1,
            new_start: 1,
            lines: 29,
            expanded: step as u32,
        })
    );
    assert_eq!(
        model.row(2),
        Some(UiRow::ContextLine {
            file: FILE_0,
            old_line: 30,
            new_line: 30,
        })
    );
    assert_eq!(
        model.row(22),
        Some(UiRow::ContextHide {
            file: FILE_0,
            hunk: HUNK_0,
            lines: step,
        })
    );
    assert_eq!(
        model.row(23),
        Some(UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_0
        })
    );
}

#[test]
fn full_context_expansion_config_shows_all_remaining_lines() {
    let repo = temp_test_dir("full-context-expansion");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let text = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.join("file.rs"), text).expect("context file should be written");
    let changeset = changeset_with_hunk_at(repo, 50);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.theme.diff.context_expansion = DiffContextExpansion::Full;

    assert!(app.expand_context_at_row(1));
    assert_eq!(
        app.document.context_expansions.get(&ContextKey {
            file: FILE_0,
            hunk: HUNK_0
        }),
        Some(&49)
    );
    assert_eq!(
        app.document.model.row(1),
        Some(UiRow::ContextLine {
            file: FILE_0,
            old_line: 1,
            new_line: 1,
        })
    );
    assert_eq!(
        app.document.model.row(49),
        Some(UiRow::ContextLine {
            file: FILE_0,
            old_line: 49,
            new_line: 49,
        })
    );
    assert_eq!(
        app.document.model.row(50),
        Some(UiRow::ContextHide {
            file: FILE_0,
            hunk: HUNK_0,
            lines: 49,
        })
    );
    assert_eq!(
        app.document.model.row(51),
        Some(UiRow::HunkHeader {
            file: FILE_0,
            hunk: HUNK_0
        })
    );
}

#[test]
fn b_clears_file_sidebar_resize_state() {
    let changeset = changeset_with_files(&["a.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.sidebar.file_sidebar_render_width = 30;
    app.viewport.viewport_width = 70;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 29,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("resize should start");

    assert!(app.sidebar.file_sidebar_resizing);
    assert_eq!(app.sidebar.file_sidebar_width, Some(30));

    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("b should be handled");

    assert!(!app.sidebar.file_sidebar_open);
    assert!(!app.sidebar.file_sidebar_resizing);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 49,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .expect("drag should no longer resize after sidebar closes");

    assert_eq!(app.sidebar.file_sidebar_width, Some(30));
}

#[test]
fn file_filter_and_grep_filter_compose_and_render_together() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .expect("f should open file filter");
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
        .expect("file filter input should be handled");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep file filter");

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .expect("slash should open grep filter");
    for character in "line 1".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .expect("grep filter input should be handled");
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter should keep grep filter");

    assert_eq!(visible_paths(&app), vec!["b.rs"]);
    assert!(line_text(&filter_bar_line(&app, 80)).starts_with("@b  /line 1"));

    let should_quit = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("escape should clear active filters");
    assert!(!should_quit);
    assert_eq!(app.filters.file_filter, "");
    assert_eq!(app.filters.grep_filter, "");
    assert_eq!(visible_paths(&app), vec!["a.rs", "b.rs", "c.rs"]);
    assert!(!filter_bar_visible(&app));
}

#[test]
fn n_and_p_navigate_grep_by_line_not_match_count() {
    let changeset = changeset_with_line_texts(&[
        "line line line",
        "line",
        "other 2",
        "other 3",
        "other 4",
        "other 5",
        "other 6",
        "other 7",
    ]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(5);
    app.filters.grep_filter = "line".to_owned();
    app.apply_filters(PostFilterNavigation::JumpToGrep);

    assert_eq!(
        app.filters.grep_matches,
        vec![ModelRow::new(2), ModelRow::new(3)]
    );
    assert_eq!(app.current_grep_match_row(), Some(2));
    assert_eq!(app.viewport.scroll, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("n should move to next matching line");

    assert_eq!(app.current_grep_match_row(), Some(3));
    assert_eq!(
        app.viewport.scroll + viewport_center_offset(app.viewport.viewport_rows),
        3
    );
}

#[test]
fn wrapped_grep_selection_stays_on_visible_continuation_row() {
    let changeset = changeset_with_line_texts(&["needle abcdefghijkl", "other", "needle second"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.viewport.line_wrapping = true;
    app.set_viewport_width(18);
    app.set_viewport_rows(2);
    app.filters.grep_filter = "needle".to_owned();
    app.apply_filters(PostFilterNavigation::JumpToGrep);

    assert_eq!(app.filters.grep_matches.len(), 2);
    let first = app.filters.grep_matches[0];
    let second = app.filters.grep_matches[1];
    let continuation_scroll = app
        .wrapped_visual_scroll_for_model_row(first.get())
        .saturating_add(1);

    app.set_scroll(continuation_scroll);

    assert_eq!(app.current_grep_match_row(), Some(first.get()));

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .expect("n should move to the next grep match");

    assert_eq!(app.current_grep_match_row(), Some(second.get()));
}

#[test]
fn grep_highlight_uses_logical_text_across_rendered_spans() {
    let theme = DiffTheme::default();
    let line = Line::from(vec![
        Span::styled("fo", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("obar", Style::default()),
    ]);
    let target = grep_highlight_target_for_columns("foobar".to_owned(), &line.spans, 0, 6, 0)
        .expect("target should map rendered spans to logical text");

    let rendered = highlighted_grep_text_line(line, "foobar", vec![target], theme);

    assert_eq!(line_text(&rendered), "foobar");
    assert_eq!(rendered.spans[0].content.as_ref(), "fo");
    assert_eq!(rendered.spans[0].style.bg, Some(theme.search_match_bg));
    assert_eq!(rendered.spans[0].style.fg, Some(theme.search_match_fg));
    assert!(
        rendered.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(rendered.spans[1].content.as_ref(), "obar");
    assert_eq!(rendered.spans[1].style.bg, Some(theme.search_match_bg));
}

#[test]
fn grep_highlight_expands_matches_to_grapheme_boundaries() {
    let theme = DiffTheme::default();

    for (text, query) in [("❤️", "❤"), ("👩‍💻", "👩")] {
        let line = Line::from(Span::styled(text, Style::default()));
        let target =
            grep_highlight_target_for_columns(text.to_owned(), &line.spans, 0, text.width(), 0)
                .expect("target should cover the rendered grapheme");

        let rendered = highlighted_grep_text_line(line, query, vec![target], theme);

        assert_eq!(line_text(&rendered), text);
        assert_eq!(rendered.spans.len(), 1);
        assert_eq!(rendered.spans[0].content.as_ref(), text);
        assert_eq!(rendered.spans[0].style.bg, Some(theme.search_match_bg));
    }
}

#[test]
fn grep_highlight_expands_matches_across_span_split_graphemes() {
    let theme = DiffTheme::default();
    let line = Line::from(vec![
        Span::styled("❤", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("\u{fe0f}", Style::default()),
    ]);
    let target = grep_highlight_target_for_columns("❤️".to_owned(), &line.spans, 0, 2, 0)
        .expect("target should cover both rendered spans");

    let rendered = highlighted_grep_text_line(line, "❤", vec![target], theme);

    assert_eq!(line_text(&rendered), "❤️");
    assert_eq!(rendered.spans.len(), 2);
    assert!(
        rendered
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.search_match_bg))
    );
    assert!(
        rendered.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn grep_highlight_hunk_header_uses_terminal_safe_text() {
    let mut changeset = changeset_with_line_text("body");
    changeset.files[0].hunks_mut()[0].header = "@@ -1 +1 @@ before\tneedle".to_owned();
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.filters.grep_filter = "needle".to_owned();

    let row = app
        .document
        .model
        .row(1)
        .expect("hunk header should be visible");
    let rendered = render_row(&mut app, 1, row, 80);

    assert!(line_text(&rendered).contains("before    needle"));
    assert!(
        rendered
            .spans
            .iter()
            .any(|span| span.content.contains("needle")
                && span.style.bg == Some(app.config.theme.search_match_bg)),
        "grep should highlight text after terminal-expanded hunk header tabs"
    );
}

#[test]
fn grep_highlight_hunk_header_uses_normalized_context() {
    let mut changeset = changeset_with_line_text("body");
    changeset.files[0].hunks_mut()[0].header = "@@ -1 +1 @@     def needle".to_owned();
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.filters.grep_filter = "needle".to_owned();

    let row = app
        .document
        .model
        .row(1)
        .expect("hunk header should be visible");
    let rendered = render_row(&mut app, 1, row, 80);

    assert!(line_text(&rendered).contains("@@ -1 +1 @@ def needle"));
    assert!(
        rendered
            .spans
            .iter()
            .any(|span| span.content.contains("needle")
                && span.style.bg == Some(app.config.theme.search_match_bg)),
        "grep should highlight text after hunk header context whitespace is normalized"
    );
}

#[test]
fn grep_highlight_ignores_unified_gutter_numbers() {
    let changeset = changeset_with_line_text("abc");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.filters.grep_filter = "1".to_owned();
    app.apply_filters(PostFilterNavigation::Preserve);

    let row = app
        .document
        .model
        .row(2)
        .expect("diff line should be visible");
    let rendered = render_row(&mut app, 2, row, 32);

    assert!(line_text(&rendered).contains('1'));
    assert!(
        rendered
            .spans
            .iter()
            .all(|span| span.style.bg != Some(app.config.theme.search_match_bg)),
        "grep should not highlight line numbers when only the gutter matches"
    );
}

#[test]
fn wrapped_split_context_line_highlights_grep_on_continuation_rows() {
    let theme = DiffTheme::default();
    let line = DiffLine::context(12, 12, "prefix needle suffix".to_owned());

    let lines = render_split_context_line_wrapped(&line, None, 0, 30, theme, "needle");
    let highlighted_line = lines
        .iter()
        .find(|line| line_text(line).contains("needle"))
        .expect("wrapped context line should render the grep match");

    assert!(
        highlighted_line
            .spans
            .iter()
            .any(|span| span.content.contains("needle")
                && span.style.bg == Some(theme.search_match_bg)),
        "grep match should be highlighted in wrapped split context lines"
    );
}

#[test]
fn file_sidebar_position_is_limited_to_rendered_body_rows() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.sidebar.file_sidebar_render_width = 20;
    app.set_viewport_rows(3);

    assert!(app.is_file_sidebar_position(0, 3));
    assert!(!app.is_file_sidebar_position(0, 4));
}

#[test]
fn error_log_separator_fills_width() {
    assert_eq!(error_log_separator(0), "");
    assert_eq!(error_log_separator(4), "erro");
    assert_eq!(error_log_separator(12), "error ──────");
    assert_eq!(error_log_separator(12).width(), 12);
}

#[test]
fn error_log_header_shows_copy_command_on_right() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_error_log("reload failed");

    let line = error_log_header_line(&app, 40);
    let text = line_text(&line);

    assert_eq!(text.width(), 40);
    assert!(text.starts_with("error "));
    assert!(text.ends_with("[Copy All (Ctrl-Shift-C)]"));
    assert_eq!(line.spans[2].style.fg, Some(app.config.theme.deletion_fg));
}

#[test]
fn transparent_background_applies_to_entire_error_log_pane() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.theme =
        DiffTheme::catppuccin_mocha().with_transparent_background_override(Some(true));
    app.set_error_log("reload failed:\nfatal: bad revision");

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 4))
        .expect("test terminal should be created");
    terminal
        .draw(|frame| {
            let area = frame.area();
            draw_error_log(frame, &app, area);
        })
        .expect("error log draw should succeed");

    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            assert_eq!(
                buffer.cell((x, y)).expect("cell should exist").bg,
                Color::Reset
            );
        }
    }
}

#[test]
fn error_log_header_uses_configured_copy_command() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        copy_error_log = "space y"
        "#,
    )
    .expect("keymap should parse");
    app.set_error_log("reload failed");

    let text = line_text(&error_log_header_line(&app, 32));

    assert_eq!(text.width(), 32);
    assert!(text.ends_with("[Copy All (Space y)]"));
}

#[test]
fn copy_error_log_without_log_shows_notice_without_writing() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let mut output = Vec::new();

    app.copy_error_log_to_writer(&mut output);

    assert!(output.is_empty());
    assert_eq!(
        app.notifications.toasts.latest_text(),
        Some("no error log to copy")
    );
}

#[test]
fn notices_expire_after_ttl() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.set_notice("reloaded");
    let expires_at = app
        .notifications
        .toasts
        .visible()
        .next()
        .unwrap()
        .expires_at;
    assert_eq!(app.notifications.toasts.latest_text().unwrap(), "reloaded");
    app.runtime.dirty = false;

    app.expire_toasts(expires_at - Duration::from_millis(1));
    assert!(!app.notifications.toasts.is_empty());
    assert!(!app.runtime.dirty);

    app.expire_toasts(expires_at);
    assert!(app.notifications.toasts.is_empty());
    assert!(app.runtime.dirty);
}

#[test]
fn notices_clamp_oversized_timeout() {
    let mut toasts = Toasts::new(NotificationSettings::new(
        NotificationMode::Default,
        ToastCorner::TopRight,
        u64::MAX,
        3,
    ));

    let before = Instant::now();
    assert!(toasts.push(ToastLevel::Info, "saved"));
    let after = Instant::now();

    let expires_at = toasts.visible().next().unwrap().expires_at;
    assert!(expires_at >= before);
    assert!(expires_at <= after + Duration::from_millis(MAX_NOTIFICATION_TIMEOUT_MS));
}

#[test]
fn file_sidebar_tracks_selected_file() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.set_viewport_rows(4);

    app.sidebar.selected_file = FILE_4;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.sidebar.selected_file, FILE_4);
    assert_eq!(app.sidebar.file_sidebar_scroll, 1);

    app.sidebar.selected_file = FILE_1;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.sidebar.selected_file, FILE_1);
    assert_eq!(app.sidebar.file_sidebar_scroll, 1);
}

#[test]
fn grouped_file_sidebar_tracks_selected_file_rows() {
    let changeset = changeset_with_files(&["src/a.rs", "src/b.rs", "docs/c.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.set_viewport_rows(2);
    app.sidebar.selected_file = FILE_2;

    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.sidebar.file_sidebar_scroll, 3);
    let lines = file_sidebar_lines(&app, 24, 2);
    assert!(line_text(&lines[0]).contains("docs/"));
    assert!(line_text(&lines[1]).contains(" M c.rs"));
}

#[test]
fn file_sidebar_opens_at_hunk_default_width() {
    let changeset = changeset_with_files(&["a.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;

    assert_eq!(file_sidebar_width(&app, 100), 34);
}

#[test]
fn transparent_background_applies_to_file_sidebar_base() {
    let changeset = changeset_with_files(&["a.rs", "b.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.theme =
        DiffTheme::catppuccin_mocha().with_transparent_background_override(Some(true));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 3))
        .expect("test terminal should be created");
    terminal
        .draw(|frame| {
            let area = frame.area();
            draw_file_sidebar(frame, &app, area);
        })
        .expect("file sidebar draw should succeed");

    let buffer = terminal.backend().buffer();
    for y in 1..buffer.area.height {
        for x in 0..buffer.area.width {
            assert_eq!(
                buffer.cell((x, y)).expect("cell should exist").bg,
                Color::Reset
            );
        }
    }

    assert_eq!(
        buffer.cell((0, 0)).expect("selected cell should exist").bg,
        app.config.theme.gutter_bg
    );
    assert_eq!(
        buffer
            .cell((19, 0))
            .expect("separator cell should exist")
            .bg,
        Color::Reset
    );
}

#[test]
fn replace_changeset_keeps_remapped_file_sidebar_selection_visible() {
    let changeset = changeset_with_files(&["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.set_viewport_rows(2);
    app.sidebar.selected_file = FILE_4;
    app.ensure_file_sidebar_selection_visible(app.visible_file_sidebar_rows());

    assert_eq!(app.sidebar.file_sidebar_scroll, 3);

    app.replace_changeset(changeset_with_files(&[
        "new.rs",
        "other.rs",
        "third.rs",
        "fourth.rs",
        "fifth.rs",
    ]));

    assert_eq!(app.sidebar.selected_file, FILE_0);
    assert_eq!(app.sidebar.file_sidebar_scroll, 0);
}

#[test]
fn file_sidebar_renders_changed_file_summary() {
    let mut changeset = changeset_with_files(&["src/lib.rs", "README.md"]);
    set_test_file_added(&mut changeset.files[1]);
    changeset.files[1].additions = 12;
    changeset.files[1].deletions = 0;
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.sidebar.file_sidebar_open = true;
    app.sidebar.selected_file = FILE_1;

    let lines = file_sidebar_lines(&app, 24, 3);
    let additions = lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+12")
        .expect("additions should render as their own span");
    let deletions = lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-0")
        .expect("deletions should render as their own span");

    assert!(line_text(&lines[0]).contains("src/"));
    assert_eq!(lines[0].spans[0].style.fg, Some(DiffTheme::default().muted));
    assert!(line_text(&lines[1]).contains(" M lib.rs"));
    assert!(!line_text(&lines[1]).contains("src/"));
    assert!(line_text(&lines[2]).contains(" A README.md"));
    assert!(line_text(&lines[2]).contains("+12 -0"));
    assert_eq!(lines[1].spans[0].content.as_ref(), " M ");
    assert_eq!(lines[1].spans[0].style.fg, Some(DiffTheme::default().hunk));
    assert!(
        lines[1].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(
        lines[1].spans[1].style.fg,
        Some(DiffTheme::default().foreground)
    );
    assert_eq!(additions.style.fg, Some(DiffTheme::default().addition_fg));
    assert_eq!(deletions.style.fg, Some(DiffTheme::default().deletion_fg));
    assert!(
        lines[2].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn file_sidebar_truncates_long_paths_before_stats() {
    let mut changeset =
        changeset_with_files(&["src/runtime/test_runner/expect/toMatchInlineSnapshot.rs"]);
    set_test_file_added(&mut changeset.files[0]);
    changeset.files[0].additions = 1290;
    changeset.files[0].deletions = 3910;
    let app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    let lines = file_sidebar_lines(&app, 32, 2);
    let text = line_text(&lines[1]);

    assert_eq!(text.width(), 32);
    assert!(text.contains("..."));
    assert!(text.contains("+1290 -3910"));
}

#[test]
fn statusline_header_right_aligns_current_file() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_viewport_rows(3);
    app.select_file(1);

    let line = statusline_header_line(&app, 80);
    let text = line_text(&line);

    assert_eq!(text.width(), 80);
    assert!(text.starts_with(" All changes  HEAD → working tree"));
    assert!(text.contains("README.md 2/3"));
    assert!(text.ends_with("% "));

    let selector = line.spans.first().expect("selector block should render");
    assert_eq!(
        selector.style.fg,
        Some(app.config.theme.statusline_accent_fg)
    );
    assert_eq!(
        selector.style.bg,
        Some(app.config.theme.statusline_accent_bg)
    );

    let file = line.spans.last().expect("file block should render");
    assert_eq!(file.style.fg, Some(app.config.theme.statusline_info_fg));
    assert_eq!(file.style.bg, Some(app.config.theme.statusline_info_bg));
    assert!(file.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn statusline_header_uses_theme_statusline_colors() {
    let changeset = changeset_with_files(&["src/lib.rs"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.theme.statusline_accent_fg = Color::Rgb(1, 2, 3);
    app.config.theme.statusline_accent_bg = Color::Rgb(4, 5, 6);
    app.config.theme.statusline_info_fg = Color::Rgb(7, 8, 9);
    app.config.theme.statusline_info_bg = Color::Rgb(10, 11, 12);

    let line = statusline_header_line(&app, 80);
    let selector = line.spans.first().expect("selector block should render");
    let file = line.spans.last().expect("file block should render");

    assert_eq!(selector.style.fg, Some(Color::Rgb(1, 2, 3)));
    assert_eq!(selector.style.bg, Some(Color::Rgb(4, 5, 6)));
    assert_eq!(file.style.fg, Some(Color::Rgb(7, 8, 9)));
    assert_eq!(file.style.bg, Some(Color::Rgb(10, 11, 12)));
}

#[test]
fn statusline_header_hides_pending_diff_load() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let options = DiffOptions {
        source: DiffSource::Worktree,
        ..DiffOptions::default()
    };
    app.jobs.pending_diff_load = Some(pending_diff_load(options));

    let line = statusline_header_line(&app, 80);
    let text = line_text(&line);

    assert_eq!(text.width(), 80);
    assert!(!text.contains("loading diff"));
}

#[test]
fn statusline_header_hides_pending_review_load() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.jobs.pending_review_load = Some(pending_review_load());

    let line = statusline_header_line(&app, 80);
    let text = line_text(&line);

    assert_eq!(text.width(), 80);
    assert!(!text.contains("loading review"));
}

#[test]
fn notice_toasts_render_as_overlay() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 10))
        .expect("test terminal should be created");

    app.set_notice("editor closed; reloading");

    let line = statusline_header_line(&app, 120);
    let text = line_text(&line);
    assert_eq!(text.width(), 120);
    assert!(!text.contains("editor closed; reloading"));

    terminal
        .draw(|frame| crate::render::draw(frame, &mut app))
        .expect("draw should succeed");
    let rows = buffer_rows(terminal.backend().buffer());
    assert!(
        rows.iter()
            .any(|row| row.contains("editor closed; reloading"))
    );
}

#[test]
fn statusline_header_hides_pending_live_reload() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.mark_live_reload_pending();

    let line = statusline_header_line(&app, 80);
    let text = line_text(&line);

    assert_eq!(text.width(), 80);
    assert!(!text.contains("refreshing diff"));
    assert!(!text.contains("loading diff"));
}

#[test]
fn live_reload_suppresses_toast_in_default_notification_mode() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);

    app.mark_live_reload_pending();

    assert!(app.notifications.toasts.is_empty());
}

#[test]
fn live_reload_emits_success_toast_in_debug_notification_mode() {
    let changeset = changeset_with_files(&["src/lib.rs", "README.md", "docs/guide.md"]);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.notifications.toasts = Toasts::new(NotificationSettings::new(
        NotificationMode::Debug,
        ToastCorner::TopRight,
        1_500,
        3,
    ));

    app.mark_live_reload_pending();

    let toast = app.notifications.toasts.visible().next().unwrap();
    assert_eq!(toast.text, "refreshing");
    assert_eq!(toast.level, ToastLevel::Success);
}

#[test]
fn line_wrapping_wraps_long_unified_rows() {
    let changeset = changeset_with_line_text("abcdefghijkl");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.viewport.line_wrapping = true;

    let row_index = 2;
    let row = app
        .document
        .model
        .row(row_index)
        .expect("diff line should exist");
    let lines = render_row_wrapped_with_focus(&mut app, row_index, row, 18, None);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 3);
    assert!(rendered[0].contains("abcd"));
    assert!(rendered[1].contains("efgh"));
    assert!(rendered[2].contains("ijkl"));
}

#[test]
fn line_wrapping_preserves_wide_glyphs_at_unified_wrap_boundary() {
    let changeset = changeset_with_line_text("abc界def");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.viewport.line_wrapping = true;

    let row_index = 2;
    let row = app
        .document
        .model
        .row(row_index)
        .expect("diff line should exist");
    let lines = render_row_wrapped_with_focus(&mut app, row_index, row, 18, None);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(rendered.len(), 3);
    assert!(rendered[0].contains("abc"));
    assert!(rendered[1].contains("界de"));
    assert!(rendered[2].contains('f'));
}

#[test]
fn line_wrapping_preserves_wide_glyphs_at_split_wrap_boundary() {
    let changeset = changeset_with_line_text("abc界def");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.viewport.line_wrapping = true;

    let row_index = 2;
    let row = app
        .document
        .model
        .row(row_index)
        .expect("diff line should exist");
    let lines = render_row_wrapped_with_focus(&mut app, row_index, row, 24, None);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(rendered.len(), 3);
    assert!(rendered[0].contains("abc"));
    assert!(rendered[1].contains("界de"));
    assert!(rendered[2].contains('f'));
}

#[test]
fn line_wrapping_splits_expanded_tabs_at_visual_boundaries() {
    assert_eq!(wrapped_line_start_columns("abc\tdef", 6), vec![0, 6]);
    assert_eq!(wrapped_line_count("abc\tdef", 6), 2);
    assert_eq!(wrapped_line_start_columns("\t", 2), vec![0, 2]);
}

#[test]
fn line_wrapping_keeps_emoji_sequences_on_one_visual_row() {
    assert_eq!(wrapped_line_start_columns("👩‍💻", 2), vec![0]);
    assert_eq!(wrapped_line_count("👩‍💻", 2), 1);
    assert_eq!(wrapped_line_start_columns("👩‍💻a", 2), vec![0, 2]);
    assert_eq!(wrapped_line_start_columns("❤️a", 2), vec![0, 2]);
}

#[test]
fn file_header_truncates_path_before_delta() {
    let file = mark_diff::DiffFile {
        change: FileChange::from_status(
            FileStatus::Modified,
            Some("src/runtime/test_runner/expect/toMatchInlineSnapshot.rs".to_owned()),
            Some("src/runtime/test_runner/expect/toMatchInlineSnapshot.rs".to_owned()),
        ),
        additions: 1290,
        deletions: 3910,
        body: mark_diff::DiffFileBody::Text { hunks: Vec::new() },
    };

    let theme = DiffTheme::default();
    let line = file_header_line(&file, 32, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 32);
    assert!(text.starts_with("M "));
    assert!(text.contains("..."));
    assert!(text.ends_with("+1290 -3910"));
    assert_eq!(line.spans[0].content.as_ref(), "M");
    assert_eq!(line.spans[0].style.fg, Some(theme.hunk));
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(line.spans[2].style.fg, Some(theme.foreground));

    let additions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+1290")
        .expect("additions should render as a separate span");
    let deletions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-3910")
        .expect("deletions should render as a separate span");

    assert_eq!(additions.style.fg, Some(theme.addition_fg));
    assert_eq!(deletions.style.fg, Some(theme.deletion_fg));
}

#[test]
fn hunk_header_uses_raw_location_context_and_delta() {
    let hunk = mark_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        ranges: HunkLineRanges::new(200, 2, 211, 3),
        lines: vec![
            DiffLine::context(200, 211, "context".to_owned()),
            DiffLine::deletion(201, "old".to_owned()),
            DiffLine::addition(212, "new".to_owned()),
            DiffLine::addition(213, "again".to_owned()),
        ],
    };

    let theme = DiffTheme::default();
    let text = line_text(&Line::from(hunk_header_spans(
        &hunk,
        48,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    )));

    assert_eq!(text.width(), 48);
    assert!(text.starts_with("@@ -200,2 +211,3 @@ render_diff_hunk"));
    assert!(text.ends_with("+2 -1"));
}

#[test]
fn hunk_header_fits_emoji_sequences_with_terminal_width() {
    let hunk = mark_diff::DiffHunk {
        header: "@@ -1 +1 @@ 👩‍💻❤️abc".to_owned(),
        ranges: HunkLineRanges::new(1, 1, 1, 1),
        lines: vec![DiffLine::addition(1, "new".to_owned())],
    };

    let theme = DiffTheme::default();
    let text = line_text(&Line::from(hunk_header_spans(
        &hunk,
        22,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    )));

    assert_eq!(text, "@@ -1 +1 @@ 👩‍💻❤️abc +1");
    assert_eq!(text.width(), 22);
}

#[test]
fn hunk_header_line_matches_unified_gutter() {
    let hunk = mark_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        ranges: HunkLineRanges::new(200, 2, 211, 3),
        lines: vec![DiffLine::addition(211, "new".to_owned())],
    };

    let theme = DiffTheme::default();
    let line = hunk_header_line(&hunk, 64, theme);
    let text = line_text(&line);

    assert_eq!(text.width(), 64);
    assert!(text.starts_with(&format!("{DIFF_INDICATOR} @@ -200,2 +211,3 @@")));
    assert!(text.contains("@@ -200,2 +211,3 @@ render_diff_hunk"));
    assert!(text.ends_with("+1"));
    let old_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-200,2")
        .expect("old range should render as a separate span");
    let new_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+211,3")
        .expect("new range should render as a separate span");
    let context = line
        .spans
        .iter()
        .find(|span| span.content.as_ref().contains("render_diff_hunk"))
        .expect("context should render as a separate span");
    let additions = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+1")
        .expect("additions should render as a separate span");

    assert_eq!(old_range.style.fg, Some(theme.deletion_fg));
    assert_eq!(new_range.style.fg, Some(theme.addition_fg));
    assert_eq!(context.style.fg, Some(theme.foreground));
    assert_eq!(additions.style.fg, Some(theme.addition_fg));
    assert!(
        line.spans
            .iter()
            .all(|span| span.style.bg == Some(line_gutter_bg(DiffLineKind::Meta, theme)))
    );
}

#[test]
fn hunk_header_truncates_context_before_delta() {
    let hunk = mark_diff::DiffHunk {
        header: "@@ -1 +1 @@ render_diff_hunk_with_a_really_long_name".to_owned(),
        ranges: HunkLineRanges::new(1, 1, 1, 1),
        lines: vec![
            DiffLine::deletion(1, "old".to_owned()),
            DiffLine::addition(1, "new".to_owned()),
        ],
    };

    let theme = DiffTheme::default();
    let text = line_text(&Line::from(hunk_header_spans(
        &hunk,
        32,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    )));

    assert_eq!(text.width(), 32);
    assert!(text.starts_with("@@ -1 +1 @@ render"));
    assert!(text.contains("..."));
    assert!(text.ends_with("+1 -1"));
}

#[test]
fn hunk_header_truncates_location_without_collapsing_range_styles() {
    let hunk = mark_diff::DiffHunk {
        header: "@@ -200,2 +211,3 @@ render_diff_hunk".to_owned(),
        ranges: HunkLineRanges::new(200, 2, 211, 3),
        lines: vec![DiffLine::context(200, 211, "context".to_owned())],
    };

    let theme = DiffTheme::default();
    let line = Line::from(hunk_header_spans(
        &hunk,
        17,
        theme,
        line_gutter_bg(DiffLineKind::Meta, theme),
    ));
    let text = line_text(&line);

    assert_eq!(text, "@@ -200,2 +211...");
    assert_eq!(text.width(), 17);

    let old_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "-200,2")
        .expect("old range should keep its own span when truncated");
    let new_range = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "+211")
        .expect("new range should keep its own span when truncated");

    assert_eq!(old_range.style.fg, Some(theme.deletion_fg));
    assert_eq!(new_range.style.fg, Some(theme.addition_fg));
}

#[test]
fn content_spans_fall_back_when_syntax_text_mismatches_diff_text() {
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text("wrong"),
        segments: vec![mark_syntax::SyntaxSegment {
            byte_start: 0,
            byte_end: 5,
            class: Some(SyntaxClass::Keyword),
            scope_stack: Default::default(),
        }],
        scope_table: Default::default(),
    };

    let spans = content_spans_at_scroll(
        "right",
        Some(&syntax),
        &[],
        DiffLineKind::Addition,
        8,
        DiffTheme::default(),
        0,
    );
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "right   ");
    assert_eq!(spans.len(), 1);
}

#[test]
fn content_spans_expand_tabs_and_escape_controls_before_rendering() {
    let text = "\tif (ok)\u{1b}";
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![mark_syntax::SyntaxSegment {
            byte_start: 0,
            byte_end: text.len(),
            class: Some(SyntaxClass::Keyword),
            scope_stack: Default::default(),
        }],
        scope_table: Default::default(),
    };

    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Addition,
        20,
        DiffTheme::default(),
        0,
    );
    let rendered = span_text(&spans);

    assert_eq!(rendered, "    if (ok)\\u{1b}   ");
    assert!(!rendered.contains('\t'));
    assert!(!rendered.contains('\u{1b}'));
}

#[test]
fn github_high_contrast_resolves_exact_scopes_and_modifiers() {
    let text = "\\begin";
    let (table, scope_stack) = mark_syntax::HighlightScopeTable::from_scope_names(&[
        "text.tex.latex",
        "support.function.general.tex",
        "punctuation.definition.function.tex",
    ]);
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment::new(0, text.len(), Some(SyntaxClass::Function))
                .with_scope_stack(scope_stack),
        ],
        scope_table: std::sync::Arc::new(table),
    };
    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Context,
        text.len(),
        DiffTheme::github_dark_high_contrast(),
        0,
    );
    assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x91, 0xcb, 0xff)));

    let text = "bold";
    let (table, scope_stack) = mark_syntax::HighlightScopeTable::from_scope_names(&[
        "text.html.markdown",
        "markup.bold.markdown",
    ]);
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment::new(0, text.len(), None).with_scope_stack(scope_stack),
        ],
        scope_table: std::sync::Arc::new(table),
    };
    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Context,
        text.len(),
        DiffTheme::github_dark_high_contrast(),
        0,
    );
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn exact_theme_uses_default_foreground_for_unmatched_properties() {
    let text = "plain";
    let (table, scope_stack) =
        mark_syntax::HighlightScopeTable::from_scope_names(&["unmatched.custom"]);
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment::new(0, text.len(), None).with_scope_stack(scope_stack),
        ],
        scope_table: std::sync::Arc::new(table),
    };
    let theme = DiffTheme::github_dark();
    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Context,
        text.len(),
        theme,
        0,
    );

    assert_eq!(theme.foreground, Color::Rgb(0xc9, 0xd1, 0xd9));
    assert_eq!(spans[0].style.fg, Some(Color::Rgb(0xe6, 0xed, 0xf3)));
}

#[test]
fn exact_theme_preserves_configured_base_colors_for_unmatched_scopes() {
    let text = "plain";
    let (table, scope_stack) =
        mark_syntax::HighlightScopeTable::from_scope_names(&["unmatched.custom"]);
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment::new(0, text.len(), None).with_scope_stack(scope_stack),
        ],
        scope_table: std::sync::Arc::new(table),
    };
    let theme = DiffTheme::github_dark_high_contrast()
        .with_color_overrides(&mark_syntax::ColorOverrides {
            fg: Some("#123456".to_owned()),
            bg: Some("#654321".to_owned()),
            ..Default::default()
        })
        .unwrap();
    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Context,
        text.len(),
        theme,
        0,
    );

    assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x12, 0x34, 0x56)));
    assert_eq!(spans[0].style.bg, Some(Color::Rgb(0x65, 0x43, 0x21)));
}

#[test]
fn scope_aware_user_rules_apply_after_exact_theme_and_respect_diff_background() {
    let text = "call";
    let (table, scope_stack) = mark_syntax::HighlightScopeTable::from_scope_names(&[
        "source.test",
        "support.function.custom",
    ]);
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment::new(0, text.len(), Some(SyntaxClass::Function))
                .with_scope_stack(scope_stack),
        ],
        scope_table: std::sync::Arc::new(table),
    };
    let theme = DiffTheme::github_dark_high_contrast()
        .with_syntax_rules(&[mark_syntax::SyntaxRuleOverride {
            scope: "support.function".to_owned(),
            foreground: Some("#123456".to_owned()),
            background: Some("#654321".to_owned()),
            font_style: Some("bold underline".to_owned()),
        }])
        .unwrap();
    let context = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Context,
        text.len(),
        theme,
        0,
    );
    assert_eq!(context[0].style.fg, Some(Color::Rgb(0x12, 0x34, 0x56)));
    assert_eq!(context[0].style.bg, Some(Color::Rgb(0x65, 0x43, 0x21)));
    assert!(context[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(context[0].style.add_modifier.contains(Modifier::UNDERLINED));

    let addition = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Addition,
        text.len(),
        theme,
        0,
    );
    assert_eq!(addition[0].style.bg, Some(theme.addition_bg));
}

#[test]
fn content_spans_align_inline_emphasis_to_grapheme_boundaries() {
    let text = "❤️a";
    let variation_selector_start = '❤'.len_utf8();
    let variation_selector_end = variation_selector_start + '\u{fe0f}'.len_utf8();
    let theme = DiffTheme::default();

    // Inline tokenization can isolate only the variation selector for `❤a` -> `❤️a`.
    let spans = content_spans_at_scroll(
        text,
        None,
        &[InlineRange {
            byte_start: variation_selector_start,
            byte_end: variation_selector_end,
        }],
        DiffLineKind::Addition,
        3,
        theme,
        0,
    );

    assert_eq!(span_text(&spans), text);
    assert_eq!(span_text(&spans).width(), 3);
    assert_eq!(spans[0].content.as_ref(), "❤️");
    assert_eq!(spans[0].style.bg, Some(theme.addition_inline_bg));
}

#[test]
fn content_spans_preserve_width_when_syntax_splits_graphemes() {
    let text = "❤️a";
    let heart_end = '❤'.len_utf8();
    let emoji_end = heart_end + '\u{fe0f}'.len_utf8();
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment {
                byte_start: 0,
                byte_end: heart_end,
                class: Some(SyntaxClass::Keyword),
                scope_stack: Default::default(),
            },
            mark_syntax::SyntaxSegment {
                byte_start: heart_end,
                byte_end: emoji_end,
                class: Some(SyntaxClass::Operator),
                scope_stack: Default::default(),
            },
            mark_syntax::SyntaxSegment {
                byte_start: emoji_end,
                byte_end: text.len(),
                class: None,
                scope_stack: Default::default(),
            },
        ],
        scope_table: Default::default(),
    };

    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[],
        DiffLineKind::Addition,
        3,
        DiffTheme::default(),
        0,
    );

    assert_eq!(span_text(&spans), text);
    assert_eq!(span_text(&spans).width(), 3);
}

#[test]
fn split_empty_cells_use_default_gutter_and_hatched_fill() {
    let spans = split_cell_spans_at_scroll(
        None,
        None,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index: 0,
            width: 12,
            theme: DiffTheme::default(),
        },
        0,
    );

    assert_eq!(span_text(&spans), "▌        ╱  ");
    assert_eq!(spans[0].content.as_ref(), DIFF_INDICATOR);
    assert_eq!(spans[0].style.fg, Some(DiffTheme::default().muted));
    assert_eq!(spans[0].style.bg, Some(DiffTheme::default().gutter_bg));
    assert_eq!(spans[1].content.as_ref(), "       ");
    assert_eq!(spans[1].style.bg, Some(DiffTheme::default().gutter_bg));
    assert_eq!(spans[2].style.fg, Some(DiffTheme::default().empty_diff));
}

#[test]
fn split_empty_cells_can_disable_hatched_fill() {
    let mut theme = DiffTheme::default();
    theme.decorations.empty_fill = false;
    let spans = split_cell_spans_at_scroll(
        None,
        None,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index: 0,
            width: 12,
            theme,
        },
        0,
    );

    assert_eq!(span_text(&spans), "▌           ");
}

#[test]
fn minimal_decorations_use_plain_diff_chrome() {
    let mut theme = DiffTheme::default();
    theme.decorations.mode = DecorationMode::Minimal;
    theme.decorations.empty_fill = true;

    let spans = split_cell_spans_at_scroll(
        None,
        None,
        &[],
        SplitCellRender {
            side: SplitSide::Old,
            row_index: 0,
            width: 12,
            theme,
        },
        0,
    );
    assert_eq!(span_text(&spans), "            ");

    let separator = file_separator_line(DiffLayoutMode::Split, 12, theme);
    assert_eq!(line_text(&separator), "            ");

    let context = context_show_line(20, true, "", 32, theme);
    let context = line_text(&context);
    assert!(context.contains("show 20 more unchanged lines"));
    assert!(!context.contains('▾'));

    let help = help_menu_row_line(
        HelpMenuRow::Binding(HelpMenuKey::Static("↑/↓"), "move"),
        32,
        theme,
        &Keymap::default(),
    );
    let help = line_text(&help);
    assert!(help.contains("up/down"));
    assert!(!help.contains('↑'));
}

#[test]
fn split_wrapped_empty_cells_follow_visual_rows() {
    let changeset = Changeset {
        repo: PathBuf::from("/repo").into(),
        title: "test".to_owned(),
        files: vec![mark_diff::DiffFile {
            change: FileChange::from_status(
                FileStatus::Modified,
                Some("file.rs".to_owned()),
                Some("file.rs".to_owned()),
            ),
            additions: 2,
            deletions: 0,
            body: mark_diff::DiffFileBody::Text {
                hunks: vec![mark_diff::DiffHunk {
                    header: "@@ -0,0 +1,2 @@".to_owned(),
                    ranges: HunkLineRanges::new(0, 0, 1, 2),
                    lines: vec![
                        DiffLine::addition(1, "abcdefgh".to_owned()),
                        DiffLine::addition(2, "ijkl".to_owned()),
                    ],
                }],
            },
        }],
        raw_patch: mark_diff::Changeset::empty_raw_patch(),
    };
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.config.theme.decorations.empty_fill = true;
    app.viewport.line_wrapping = true;
    app.set_viewport_width(24);

    let first_row = app
        .document
        .model
        .row(2)
        .expect("first addition row should exist");
    let second_row = app
        .document
        .model
        .row(3)
        .expect("second addition row should exist");
    let first = render_row_wrapped_with_focus(&mut app, 2, first_row, 24, None);
    let second = render_row_wrapped_with_focus(&mut app, 3, second_row, 24, None);

    let left_width = 12usize;
    let content_offset = 1 + GUTTER_WIDTH.min(left_width.saturating_sub(1));
    let content_width = split_cell_content_width(left_width);
    let left_fill = |line: &Line<'_>| {
        line_text(line)
            .chars()
            .skip(content_offset)
            .take(content_width)
            .collect::<String>()
    };
    let first_visual_row = app.wrapped_visual_scroll_for_model_row(2);
    let second_visual_row = app.wrapped_visual_scroll_for_model_row(3);

    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 1);
    assert_eq!(first_visual_row, 2);
    assert_eq!(second_visual_row, 4);
    assert_eq!(
        left_fill(&first[0]),
        empty_diff_fill_from(content_width, first_visual_row, content_offset, true)
    );
    assert_eq!(
        left_fill(&first[1]),
        empty_diff_fill_from(content_width, first_visual_row + 1, content_offset, true)
    );
    assert_eq!(
        left_fill(&second[0]),
        empty_diff_fill_from(content_width, second_visual_row, content_offset, true)
    );
    assert_ne!(left_fill(&first[0]), left_fill(&first[1]));
}

#[test]
fn line_gutters_use_theme_background() {
    let theme = DiffTheme::default();
    let line = DiffLine::context(7, 7, "same".to_owned());

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

    assert_eq!(rendered.spans[0].style.fg, Some(theme.muted));
    assert_eq!(rendered.spans[0].style.bg, Some(theme.gutter_bg));
    assert_eq!(rendered.spans[1].style.fg, Some(theme.foreground));
    assert_eq!(rendered.spans[1].style.bg, Some(theme.gutter_bg));
}

#[test]
fn changed_line_gutters_use_delta_colors_and_bold_signs() {
    let theme = DiffTheme::default();
    let line = DiffLine::addition(7, "added".to_owned());

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, theme, 0);

    assert_eq!(rendered.spans[0].style.bg, Some(theme.addition_gutter_bg));
    assert_eq!(rendered.spans[1].style.fg, Some(theme.addition_fg));
    assert_eq!(rendered.spans[1].style.bg, Some(theme.addition_gutter_bg));
    assert_eq!(rendered.spans[2].content.as_ref(), "+");
    assert_eq!(rendered.spans[2].style.fg, Some(theme.addition_fg));
    assert_eq!(rendered.spans[2].style.bg, Some(theme.addition_gutter_bg));
    assert!(
        rendered.spans[2]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(rendered.spans[3].style.fg, Some(theme.foreground));
    assert_eq!(rendered.spans[3].style.bg, Some(theme.addition_bg));
}

#[test]
fn split_view_uses_right_indicator_as_separator() {
    let changeset = changeset_with_context_lines(1);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);

    let rendered = render_split_line_with_focus(
        &mut app,
        SplitLineRender {
            file: 0,
            hunk: 0,
            left: Some(0),
            right: Some(0),
            row_index: 0,
            width: 24,
            focused: false,
        },
    );
    let text = line_text(&rendered);

    assert!(!text.contains('│'));
    assert_eq!(text.chars().nth(12), Some('▌'));
}

#[test]
fn unified_diff_content_scrolls_horizontally() {
    let line = DiffLine::context(1, 1, "abcdef".to_owned());

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 18, DiffTheme::default(), 2);

    assert!(line_text(&rendered).ends_with("cdef"));
}

#[test]
fn split_diff_content_scrolls_horizontally() {
    let changeset = changeset_with_line_text("abcdef");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    app.viewport.horizontal_scroll = 2;

    let rendered = render_split_line_with_focus(
        &mut app,
        SplitLineRender {
            file: 0,
            hunk: 0,
            left: Some(0),
            right: Some(0),
            row_index: 0,
            width: 24,
            focused: false,
        },
    );

    assert_eq!(line_text(&rendered), "▌    1  cdef▌    1  cdef");
}

#[test]
fn diff_lines_start_with_change_indicator() {
    let line = DiffLine::addition(3, "new".to_owned());

    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, 24, DiffTheme::default(), 0);

    assert_eq!(rendered.spans[0].content.as_ref(), DIFF_INDICATOR);
    assert_eq!(
        rendered.spans[0].style.fg,
        Some(DiffTheme::default().addition_fg)
    );
    assert!(!line_text(&rendered).contains(EMPTY_DIFF_FILL));
}

#[test]
fn highlighted_mouse_diff_content_line_highlights_only_code_columns() {
    let line = DiffLine::context(1, 1, "code".to_owned());
    let width = 24;
    let theme = DiffTheme::default();
    let rendered = render_unified_line_at_scroll(&line, None, &[], 0, width, theme, 0);
    let highlighted = highlighted_mouse_diff_content_line(
        rendered.clone(),
        DiffLayoutMode::Unified,
        width,
        theme,
    );

    assert_eq!(highlighted.spans[0].style, rendered.spans[0].style);
    let original_code = rendered
        .spans
        .iter()
        .find(|span| span.content.as_ref().contains("code"))
        .expect("content span");
    let highlighted_code = highlighted
        .spans
        .iter()
        .find(|span| span.content.as_ref().contains("code"))
        .expect("content span");
    assert_ne!(highlighted_code.style.bg, original_code.style.bg);
    assert_eq!(highlighted_code.style.bg, Some(theme.cursor_line_bg));
    assert!(unified_content_start_column(width) > 0);
}

#[test]
fn annotation_add_button_opens_input_under_hovered_line() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 10,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(5);
    app.viewport.scroll = 0;

    let code_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.viewport.scroll = code_row;
    app.update_diff_mouse_hover(38, 1);

    let hover_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 3);
    let button_span = hover_lines[0].spans.last().expect("add button span");
    assert_eq!(button_span.style.bg, Some(app.config.theme.cursor_line_bg));

    assert!(app.handle_diff_click(38, 1));
    assert!(app.annotations_state.annotation_draft.is_some());

    let lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    assert_eq!(lines.len(), 4);
    assert!(line_text(&lines[1]).starts_with("file.rs R1 "));
    assert!(line_text(&lines[1]).ends_with("[x]"));
    assert!(
        lines[1]
            .spans
            .iter()
            .any(|span| span.style.fg == Some(app.config.theme.hunk))
    );
    assert!(line_text(&lines[2]).contains(INPUT_CURSOR));
    let footer = line_text(&lines[3]);
    assert!(footer.starts_with("─"));
    assert!(footer.ends_with("[✓]"));
    assert_eq!(
        lines[3].spans.last().and_then(|span| span.style.fg),
        Some(app.config.theme.addition_fg)
    );

    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    assert!(app.handle_diff_click(38, 4));

    assert!(app.annotations_state.annotation_draft.is_none());
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note")
    );

    let lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    let close_row = lines
        .iter()
        .position(|line| line_text(line).ends_with("[x]"))
        .expect("saved annotation close row");
    let body_row = lines
        .iter()
        .position(|line| line_text(line).contains("note"))
        .expect("saved annotation body row");
    let edit_row = lines
        .iter()
        .position(|line| line_text(line).ends_with("[↻]"))
        .expect("saved annotation edit row");
    assert!(close_row < body_row);
    assert!(body_row < edit_row);
    assert_eq!(
        lines[edit_row].spans.last().and_then(|span| span.style.fg),
        Some(app.config.theme.search_match_bg)
    );

    let diff_y = app.viewport.rendered_diff_area.expect("diff area").y;
    assert!(app.handle_diff_click(38, diff_y.saturating_add(edit_row as u16)));
    assert!(app.annotations_state.annotation_draft.is_some());
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note!")
    );

    let lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 6);
    let close_row = lines
        .iter()
        .position(|line| line_text(line).ends_with("[x]"))
        .expect("saved annotation close row");
    app.update_diff_mouse_hover(38, diff_y.saturating_add(close_row as u16));
    assert!(app.handle_diff_click(38, diff_y.saturating_add(close_row as u16)));
    assert!(!app.annotations_state.annotations.contains_key(&key));
}

#[test]
fn split_annotation_add_button_uses_current_side_for_paired_row() {
    use crate::annotation::{AnnotationKey, AnnotationSide};

    let changeset = changeset_with_replacement_pair();
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

    let row = app.document.model.row(split_row).expect("row");
    let keys = AnnotationKey::candidates_from_ui_row(&app.document.changeset, row);
    assert_eq!(keys.len(), 1);
    let key = keys[0].clone();
    assert_eq!(key.side, AnnotationSide::New);
    let left_width = app.viewport.viewport_width / 2;
    let old_button_column = (left_width - 1) as u16;
    app.update_diff_mouse_hover(old_button_column, 1);

    let hover_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 3);
    let hover_text = line_text(&hover_lines[0]);
    let (old_button_text, _) = skip_display_prefix(&hover_text, left_width - 4);
    assert!(
        !old_button_text.starts_with(" [+]"),
        "paired row should not expose an old-side add button: {hover_text:?}"
    );
    assert!(hover_text.ends_with(" [+]"));
    assert!(!app.handle_diff_click(old_button_column, 1));
    assert!(app.annotations_state.annotation_draft.is_none());

    let new_button_column = (app.viewport.viewport_width - 1) as u16;
    app.update_diff_mouse_hover(new_button_column, 1);
    let hover_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 3);
    assert!(line_text(&hover_lines[0]).ends_with(" [+]"));

    assert!(app.handle_diff_click(new_button_column, 1));
    let draft = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .expect("draft");
    assert_eq!(draft.key, key);

    for character in "new note".chars() {
        app.handle_annotation_input_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ));
    }
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("new note")
    );
    let json = app.marks_clipboard_json().expect("marks JSON");
    assert!(json.contains("\"old_line\": 1"));
    assert!(json.contains("\"new_line\": 1"));
}

#[test]
fn annotation_pairing_spans_no_newline_meta_line() {
    use crate::annotation::{AnnotationKey, AnnotationSide};

    let mut changeset = changeset_with_replacement_pair();
    changeset.files[0].hunks_mut()[0]
        .lines
        .insert(1, DiffLine::meta("\\ No newline at end of file".to_owned()));

    let mut app = DiffApp::new(
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
    let addition_row = app
        .document
        .model
        .rows
        .iter()
        .position(|row| matches!(row, UiRow::UnifiedLine { line: LINE_2, .. }))
        .expect("addition line");

    assert_eq!(
        AnnotationKey::from_ui_row(
            &app.document.changeset,
            app.document.model.row(deletion_row).expect("row")
        ),
        None
    );
    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(addition_row).expect("row"),
    )
    .expect("addition key");
    assert_eq!(key.side, AnnotationSide::New);
    app.annotations_state
        .annotations
        .insert(key, "note".to_owned());
    let json = app.marks_clipboard_json().expect("marks JSON");
    assert!(json.contains("\"old_line\": 1"));
    assert!(json.contains("\"new_line\": 1"));

    let split_app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Split);
    let split_deletion_row = split_app
        .document
        .model
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                UiRow::SplitLine {
                    left,
                    right,
                    ..
                } if left.get() == Some(LINE_0) && right.get().is_none()
            )
        })
        .expect("split deletion row");
    assert_eq!(
        AnnotationKey::from_ui_row(
            &split_app.document.changeset,
            split_app
                .document
                .model
                .row(split_deletion_row)
                .expect("row"),
        ),
        None
    );
}

#[test]
fn split_annotation_add_button_uses_right_edge_for_deletion_only_row() {
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

    let row = app.document.model.row(split_row).expect("row");
    let keys = AnnotationKey::candidates_from_ui_row(&app.document.changeset, row);
    assert_eq!(keys.len(), 1);
    let key = keys[0].clone();
    assert_eq!(key.side, AnnotationSide::Old);

    let left_width = app.viewport.viewport_width / 2;
    let old_button_column = (left_width - 1) as u16;
    app.update_diff_mouse_hover(old_button_column, 1);
    let hover_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 60, 3);
    let hover_text = line_text(&hover_lines[0]);
    let (old_button_text, _) = skip_display_prefix(&hover_text, left_width - 4);
    assert!(
        !old_button_text.starts_with(" [+]"),
        "deletion-only row should not expose a left-side add button: {hover_text:?}"
    );
    assert!(hover_text.ends_with(" [+]"));
    assert!(!app.handle_diff_click(old_button_column, 1));
    assert!(app.annotations_state.annotation_draft.is_none());

    let new_button_column = (app.viewport.viewport_width - 1) as u16;
    app.update_diff_mouse_hover(new_button_column, 1);
    assert!(app.handle_diff_click(new_button_column, 1));
    let draft = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .expect("draft");
    assert_eq!(draft.key, key);
}

#[test]
fn annotation_save_preserves_body_whitespace_but_deletes_blank_drafts() {
    use crate::annotation::{AnnotationDraft, AnnotationKey};

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.config.keymap = Keymap::parse(
        r#"
        [keymap.global]
        edit_hunk = "e"
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
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");

    let body = "  indented\n    code  ";
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key: key.clone(),
        model_row_index: code_row,
        input: body.to_owned(),
        cursor: body.len(),
    });
    let save_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(app.handle_annotation_input_key(save_key));
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some(body)
    );
    assert!(
        app.marks_clipboard_json()
            .expect("marks JSON")
            .contains("\"body\": \"  indented\\n    code  \"")
    );

    let blank = " \n\t ";
    app.annotations_state.annotation_draft = Some(AnnotationDraft {
        key: key.clone(),
        model_row_index: code_row,
        input: blank.to_owned(),
        cursor: blank.len(),
    });
    let save_key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(app.handle_annotation_input_key(save_key));
    assert!(!app.annotations_state.annotations.contains_key(&key));
}

#[test]
fn annotation_button_hit_tests_use_diff_relative_columns() {
    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 10,
        y: 1,
        width: 40,
        height: 5,
    });
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
    app.update_diff_mouse_hover(48, 1);

    assert!(!app.handle_diff_click(38, 1));
    assert!(app.annotations_state.annotation_draft.is_none());

    assert!(app.handle_diff_click(48, 1));
    assert!(app.annotations_state.annotation_draft.is_some());
}

#[test]
fn annotation_draft_opened_on_last_row_scrolls_block_into_view() {
    let lines: Vec<&str> = (0..10).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 4,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(4);
    let code_row = app
        .document
        .model
        .rows
        .iter()
        .rposition(|row| matches!(row, UiRow::UnifiedLine { .. }))
        .expect("unified line");
    app.set_scroll(app.max_scroll());
    let previous_scroll = app.viewport.scroll;
    let viewport_row = code_row.saturating_sub(app.viewport.scroll) as u16;

    app.update_diff_mouse_hover(38, viewport_row.saturating_add(1));
    assert!(app.handle_diff_click(38, viewport_row.saturating_add(1)));

    let draft_row = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .map(|draft| draft.model_row_index)
        .expect("draft");
    assert_eq!(draft_row, code_row);
    assert!(app.viewport.scroll > previous_scroll);
    assert!(
        crate::render::viewport_plan::compose_block_bottom_viewport_row(&app, draft_row).is_some()
    );

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 4);
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR))
    );
    assert!(rendered.iter().any(|line| line_text(line).ends_with("[✓]")));
}

#[test]
fn annotation_hidden_compose_footer_is_not_submit_target() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 2,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(2);
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

    let draft_row = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .map(|draft| draft.model_row_index)
        .expect("draft");
    assert_eq!(
        crate::render::viewport_plan::compose_block_bottom_viewport_row(&app, draft_row),
        None
    );
    for character in "note".chars() {
        app.handle_annotation_input_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE,
        ));
    }

    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 2);
    assert!(line_text(&rendered[1]).ends_with("[x]"));
    assert!(!rendered.iter().any(|line| line_text(line).ends_with("[✓]")));
    assert!(app.handle_diff_click(38, 2));

    assert!(app.annotations_state.annotation_draft.is_none());
    let row = app.document.model.row(code_row).expect("row");
    let key = AnnotationKey::from_ui_row(&app.document.changeset, row).expect("key");
    assert!(!app.annotations_state.annotations.contains_key(&key));
}

#[test]
fn annotation_input_scrolls_back_to_draft_above_viewport() {
    let lines: Vec<&str> = (0..12).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 4,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(4);
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
    let draft_row = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .map(|draft| draft.model_row_index)
        .expect("draft");

    app.set_scroll(code_row.saturating_add(5));
    assert_eq!(
        crate::render::viewport_plan::compose_block_bottom_viewport_row(&app, draft_row),
        None
    );
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert!(
        crate::render::viewport_plan::compose_block_bottom_viewport_row(&app, draft_row).is_some()
    );
    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 4);
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR))
    );
}

#[test]
fn long_annotation_draft_stays_visible_when_footer_cannot_fit() {
    let lines: Vec<&str> = (0..20).map(|_| "line").collect();
    let changeset = changeset_with_line_texts(&lines);
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 4,
    });
    app.set_viewport_width(40);
    app.set_viewport_rows(4);
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
    let draft_row = app
        .annotations_state
        .annotation_draft
        .as_ref()
        .map(|draft| draft.model_row_index)
        .expect("draft");

    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    app.handle_annotation_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.viewport.scroll, draft_row);
    assert_eq!(
        crate::render::viewport_plan::compose_block_bottom_viewport_row(&app, draft_row),
        None
    );
    assert!(
        crate::render::viewport_plan::compose_block_top_viewport_row(&app, draft_row).is_some()
    );
    let rendered = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 4);
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains(INPUT_CURSOR))
    );
}

#[test]
fn filter_input_blocks_annotation_hover_drafts() {
    use crate::annotation::AnnotationKey;

    let changeset = changeset_with_line_text("hello");
    let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
    app.set_rendered_diff_area(Rect {
        x: 0,
        y: 1,
        width: 40,
        height: 5,
    });
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
    app.update_diff_mouse_hover(38, 1);
    assert_eq!(app.viewport.mouse_hover, Some((38, 0)));

    app.open_filter_input(DiffFilterKind::File);
    assert_eq!(app.viewport.mouse_hover, None);
    assert!(!app.handle_diff_click(38, 1));

    let key = AnnotationKey::from_ui_row(
        &app.document.changeset,
        app.document.model.row(code_row).expect("annotated row"),
    )
    .expect("key");
    app.annotations_state
        .annotations
        .insert(key.clone(), "note".to_owned());
    assert!(!app.handle_diff_click(38, 2));
    assert_eq!(
        app.annotations_state
            .annotations
            .get(&key)
            .map(String::as_str),
        Some("note")
    );
    assert!(!app.handle_diff_click(38, 4));

    assert!(app.annotations_state.annotation_draft.is_none());
    assert_eq!(app.filters.filter_input, Some(DiffFilterKind::File));
}

#[test]
fn diff_modals_suppress_stale_mouse_hover_highlight() {
    type ModalOpener = (&'static str, fn(&mut DiffApp));
    let modal_openers: [ModalOpener; 7] = [
        ("help menu", |app| app.toggle_help_menu()),
        ("options menu", |app| app.open_options_menu()),
        ("diff menu", |app| app.open_diff_menu()),
        ("review input", |app| app.open_review_input()),
        ("branch menu", |app| {
            app.refs.comparison_branches = branch_names(&["main", "topic"]);
            app.toggle_branch_menu(BranchMenu::Head);
        }),
        ("commit menu", |app| {
            app.refs.comparison_commits = vec![GitCommit {
                sha: "abcdef0".into(),
                subject: "commit".to_owned(),
            }];
            app.toggle_commit_menu();
        }),
        ("color scheme picker", |app| {
            app.open_options_menu();
            app.open_color_scheme_picker();
        }),
    ];

    for (label, open_modal) in modal_openers {
        let changeset = changeset_with_line_text("hello");
        let mut app = DiffApp::new(DiffOptions::default(), changeset, DiffLayoutMode::Unified);
        app.set_rendered_diff_area(Rect {
            x: 0,
            y: 1,
            width: 40,
            height: 5,
        });
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
        app.update_diff_mouse_hover(38, 1);

        let hovered_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 5);
        assert!(
            line_text(&hovered_lines[0]).contains("[+]"),
            "{label} baseline should show hover add button"
        );

        open_modal(&mut app);
        assert!(
            app.diff_modal_blocks_mouse_hover(),
            "{label} should block diff mouse hover"
        );

        let modal_lines = crate::render::diff::build_diff_viewport_lines(&mut app, 40, 5);
        assert!(
            !line_text(&modal_lines[0]).contains("[+]"),
            "{label} should hide stale hover add button"
        );
        assert!(
            !modal_lines[0]
                .spans
                .iter()
                .any(|span| span.style.bg == Some(app.config.theme.cursor_line_bg)),
            "{label} should hide stale hover highlight"
        );
    }
}

#[test]
fn ansi_theme_uses_terminal_palette_indices() {
    let theme = diff_theme_from_config(&SyntaxThemeConfig::Ansi).expect("ansi theme should load");

    assert_eq!(theme.addition_fg, Color::Indexed(2));
    assert_eq!(
        theme.syntax.color(SyntaxClass::Keyword),
        Some(Color::Indexed(13))
    );
}

#[test]
fn system_theme_preserves_terminal_base_and_uses_owned_diff_colors() {
    let theme = builtin_diff_theme(Some("system")).expect("system theme should load");

    assert_eq!(theme.foreground, Color::Reset);
    assert_eq!(theme.background, Color::Reset);
    assert_eq!(theme.file, Color::Reset);
    assert_eq!(theme.cursor, Color::Reset);
    assert_eq!(theme.cursor_line_bg, Color::Indexed(237));
    assert_ne!(theme.addition_fg, Color::Indexed(2));
    assert_ne!(theme.deletion_fg, Color::Indexed(1));
    assert_eq!(row_bg(DiffLineKind::Addition, theme), theme.addition_bg);
    assert_eq!(
        inline_bg(DiffLineKind::Addition, theme),
        theme.addition_inline_bg
    );
    assert_eq!(
        line_gutter_bg(DiffLineKind::Addition, theme),
        theme.addition_gutter_bg
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::String),
        SyntaxPalette::ansi().color(SyntaxClass::String)
    );
}

#[test]
fn default_theme_alias_uses_system_theme() {
    let theme = builtin_diff_theme(Some("default")).expect("default theme should load");

    assert_eq!(theme, DiffTheme::system());
}

#[test]
fn packaged_builtin_themes_are_available() {
    for choice in BUILTIN_THEMES {
        let name = color_scheme_label(*choice);
        let theme = builtin_diff_theme(Some(name)).expect("built-in theme should load");

        assert_ne!(theme.statusline_accent_bg, Color::Reset);
        assert!(
            theme.syntax.color(SyntaxClass::Keyword).is_some(),
            "{name} should set syntax keyword foreground"
        );
        if name == "system" {
            assert!(theme.exact_syntax.is_none());
        } else {
            assert!(
                theme.exact_syntax.is_some(),
                "{name} should use vendored TextMate rules"
            );
        }
    }
}

#[test]
fn zenbones_tui_colors_match_the_pinned_upstream_theme() {
    let dark = builtin_diff_theme(Some("zenbones-dark")).expect("zenbones dark should load");
    assert_eq!(dark.background, Color::Rgb(0x1c, 0x19, 0x17));
    assert_eq!(dark.foreground, Color::Rgb(0xb4, 0xbd, 0xc3));
    assert_eq!(dark.cursor, Color::Rgb(0xc4, 0xca, 0xcf));
    assert_eq!(dark.cursor_line_bg, Color::Rgb(0x25, 0x21, 0x1f));
    assert_eq!(dark.addition_fg, Color::Rgb(0x81, 0x9b, 0x69));
    assert_eq!(dark.addition_bg, Color::Rgb(0x23, 0x2d, 0x1a));
    assert_eq!(dark.deletion_fg, Color::Rgb(0xde, 0x6e, 0x7c));
    assert_eq!(dark.deletion_bg, Color::Rgb(0x3e, 0x22, 0x25));

    let light = builtin_diff_theme(Some("zenbones-light")).expect("zenbones light should load");
    assert_eq!(light.background, Color::Rgb(0xf0, 0xed, 0xec));
    assert_eq!(light.foreground, Color::Rgb(0x2c, 0x36, 0x3c));
    assert_eq!(light.cursor_line_bg, Color::Rgb(0xe9, 0xe4, 0xe2));
    assert_eq!(light.addition_bg, Color::Rgb(0xcb, 0xe5, 0xb8));
    assert_eq!(light.deletion_bg, Color::Rgb(0xeb, 0xd8, 0xda));
}

#[test]
fn builtin_syntax_palettes_match_upstream_theme_colors() {
    // These expectations mirror the upstream Catppuccin, Gruvbox, and GitHub
    // theme colors for the closest matching Mark syntax classes.
    for (theme, comment, operator, tag, attribute) in [
        (
            DiffTheme::catppuccin_latte(),
            Color::Rgb(0x7c, 0x7f, 0x93),
            Color::Rgb(0x17, 0x92, 0x99),
            Color::Rgb(0x1e, 0x66, 0xf5),
            Color::Rgb(0xdf, 0x8e, 0x1d),
        ),
        (
            DiffTheme::catppuccin_frappe(),
            Color::Rgb(0x94, 0x9c, 0xbb),
            Color::Rgb(0x81, 0xc8, 0xbe),
            Color::Rgb(0x8c, 0xaa, 0xee),
            Color::Rgb(0xe5, 0xc8, 0x90),
        ),
        (
            DiffTheme::catppuccin_macchiato(),
            Color::Rgb(0x93, 0x9a, 0xb7),
            Color::Rgb(0x8b, 0xd5, 0xca),
            Color::Rgb(0x8a, 0xad, 0xf4),
            Color::Rgb(0xee, 0xd4, 0x9f),
        ),
        (
            DiffTheme::catppuccin_mocha(),
            Color::Rgb(0x93, 0x99, 0xb2),
            Color::Rgb(0x94, 0xe2, 0xd5),
            Color::Rgb(0x89, 0xb4, 0xfa),
            Color::Rgb(0xf9, 0xe2, 0xaf),
        ),
    ] {
        assert_eq!(theme.syntax.color(SyntaxClass::Comment), Some(comment));
        assert_eq!(theme.syntax.color(SyntaxClass::Operator), Some(operator));
        assert_eq!(theme.syntax.color(SyntaxClass::Property), Some(operator));
        assert_eq!(theme.syntax.color(SyntaxClass::Tag), Some(tag));
        assert_eq!(theme.syntax.color(SyntaxClass::Attribute), Some(attribute));
        assert_eq!(theme.syntax.color(SyntaxClass::Module), Some(attribute));
        assert_eq!(theme.syntax.color(SyntaxClass::Variable), None);
    }

    for (theme, constant, function, operator, variable, punctuation, property) in [
        (
            DiffTheme::gruvbox_dark(),
            Color::Rgb(0xd3, 0x86, 0x9b),
            Color::Rgb(0xfa, 0xbd, 0x2f),
            Color::Rgb(0x8e, 0xc0, 0x7c),
            Color::Rgb(0x83, 0xa5, 0x98),
            Color::Rgb(0xa8, 0x99, 0x84),
            Color::Rgb(0x68, 0x9d, 0x6a),
        ),
        (
            DiffTheme::gruvbox_light(),
            Color::Rgb(0x8f, 0x3f, 0x71),
            Color::Rgb(0xb5, 0x76, 0x14),
            Color::Rgb(0x42, 0x7b, 0x58),
            Color::Rgb(0x07, 0x66, 0x78),
            Color::Rgb(0x7c, 0x6f, 0x64),
            Color::Rgb(0x68, 0x9d, 0x6a),
        ),
    ] {
        assert_eq!(
            theme.syntax.color(SyntaxClass::Comment),
            Some(Color::Rgb(0x92, 0x83, 0x74))
        );
        assert_eq!(theme.syntax.color(SyntaxClass::Constant), Some(constant));
        assert_eq!(theme.syntax.color(SyntaxClass::Function), Some(function));
        assert_eq!(theme.syntax.color(SyntaxClass::Type), Some(function));
        assert_eq!(theme.syntax.color(SyntaxClass::Operator), Some(operator));
        assert_eq!(theme.syntax.color(SyntaxClass::Tag), Some(operator));
        assert_eq!(theme.syntax.color(SyntaxClass::Variable), Some(variable));
        assert_eq!(
            theme.syntax.color(SyntaxClass::Punctuation),
            Some(punctuation)
        );
        assert_eq!(theme.syntax.color(SyntaxClass::Property), Some(property));
    }

    for (theme, comment, constant, function, tag, string, variable) in [
        (
            DiffTheme::github_light(),
            Color::Rgb(0x6e, 0x77, 0x81),
            Color::Rgb(0x05, 0x50, 0xae),
            Color::Rgb(0x82, 0x50, 0xdf),
            Color::Rgb(0x11, 0x63, 0x29),
            Color::Rgb(0x0a, 0x30, 0x69),
            Color::Rgb(0x95, 0x38, 0x00),
        ),
        (
            DiffTheme::github_dark(),
            Color::Rgb(0x8b, 0x94, 0x9e),
            Color::Rgb(0x79, 0xc0, 0xff),
            Color::Rgb(0xd2, 0xa8, 0xff),
            Color::Rgb(0x7e, 0xe7, 0x87),
            Color::Rgb(0xa5, 0xd6, 0xff),
            Color::Rgb(0xff, 0xa6, 0x57),
        ),
        (
            DiffTheme::github_light_high_contrast(),
            Color::Rgb(0x66, 0x70, 0x7b),
            Color::Rgb(0x02, 0x3b, 0x95),
            Color::Rgb(0x62, 0x2c, 0xbc),
            Color::Rgb(0x02, 0x4c, 0x1a),
            Color::Rgb(0x03, 0x25, 0x63),
            Color::Rgb(0x70, 0x2c, 0x00),
        ),
        (
            DiffTheme::github_dark_high_contrast(),
            Color::Rgb(0xbd, 0xc4, 0xcc),
            Color::Rgb(0x91, 0xcb, 0xff),
            Color::Rgb(0xdb, 0xb7, 0xff),
            Color::Rgb(0x72, 0xf0, 0x88),
            Color::Rgb(0xad, 0xdc, 0xff),
            Color::Rgb(0xff, 0xb7, 0x57),
        ),
    ] {
        assert_eq!(theme.syntax.color(SyntaxClass::Attribute), None);
        assert_eq!(theme.syntax.color(SyntaxClass::Comment), Some(comment));
        assert_eq!(theme.syntax.color(SyntaxClass::Constant), Some(constant));
        assert_eq!(theme.syntax.color(SyntaxClass::Function), Some(function));
        assert_eq!(theme.syntax.color(SyntaxClass::Tag), Some(tag));
        assert_eq!(theme.syntax.color(SyntaxClass::String), Some(string));
        assert_eq!(theme.syntax.color(SyntaxClass::Variable), None);
        assert_eq!(theme.syntax.color(SyntaxClass::Type), Some(variable));
        assert_eq!(theme.syntax.color(SyntaxClass::Property), Some(constant));
        assert_eq!(theme.syntax.color(SyntaxClass::Punctuation), None);
    }

    let theme = DiffTheme::tokyonight();
    assert_eq!(
        theme.syntax.color(SyntaxClass::Comment),
        Some(Color::Rgb(0x51, 0x59, 0x7d))
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::Attribute),
        Some(Color::Rgb(0xbb, 0x9a, 0xf7))
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::Operator),
        Some(Color::Rgb(0x89, 0xdd, 0xff))
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::Property),
        Some(Color::Rgb(0x7d, 0xcf, 0xff))
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::Type),
        Some(Color::Rgb(0x0d, 0xb9, 0xd7))
    );
    assert_eq!(
        theme.syntax.color(SyntaxClass::Tag),
        Some(Color::Rgb(0xf7, 0x76, 0x8e))
    );
}

#[test]
fn transparent_background_only_resets_diff_base_background() {
    let theme = DiffTheme::catppuccin_mocha().with_transparent_background_override(Some(true));
    let spans = content_spans_at_scroll(
        "changed",
        None,
        &[InlineRange {
            byte_start: 0,
            byte_end: 7,
        }],
        DiffLineKind::Addition,
        8,
        theme,
        0,
    );
    let context = render_unified_line_at_scroll(
        &DiffLine::context(7, 7, "same".to_owned()),
        None,
        &[],
        0,
        24,
        theme,
        0,
    );
    let changeset = changeset_with_files(&["file.rs"]);
    let file_header = file_header_line(&changeset.files[0], 32, theme);
    let file_separator = file_separator_line(DiffLayoutMode::Unified, 8, theme);

    assert_eq!(base_bg(theme), theme.background);
    assert_eq!(diff_base_bg(theme), Color::Reset);
    assert_eq!(header_bg(theme), theme.gutter_bg);
    assert_eq!(statusline_bg(theme), theme.statusline_bg);
    assert_eq!(
        line_gutter_bg(DiffLineKind::Addition, theme),
        theme.addition_gutter_bg
    );
    assert_eq!(row_bg(DiffLineKind::Addition, theme), theme.addition_bg);
    assert_eq!(spans[0].style.bg, Some(theme.addition_inline_bg));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(context.spans[0].style.bg, Some(theme.gutter_bg));
    assert_eq!(context.spans[1].style.bg, Some(theme.gutter_bg));
    assert_eq!(context.spans[2].style.bg, Some(Color::Reset));
    assert!(
        file_header
            .spans
            .iter()
            .all(|span| span.style.bg == Some(Color::Reset))
    );
    assert_eq!(file_separator.spans[0].style.bg, Some(Color::Reset));
}

#[test]
fn textmate_palette_uses_fallbacks_for_unmatched_scopes() {
    let theme = builtin_diff_theme(Some("mfd")).expect("MFD should load");
    let background = RgbColor::new(0x7a, 0x8b, 0x69);
    let foreground = RgbColor::new(0x1e, 0x2d, 0x1e);

    // MFD has no markup.inserted/markup.deleted selectors. Its editor
    // foreground must not be mistaken for a selector match.
    assert_eq!(theme.addition_fg, foreground.color());
    assert_eq!(
        theme.deletion_fg,
        background.blend(foreground, 0.84).color()
    );
    assert_ne!(theme.addition_fg, theme.deletion_fg);
    assert_ne!(theme.addition_bg, theme.deletion_bg);
}

#[test]
fn custom_theme_inherits_builtin_and_applies_partial_overrides() {
    let base = builtin_diff_theme(Some("nord")).expect("nord should load");
    let theme = parse_custom_colorscheme(
        r##"
extends = "nord"
transparent_background = true

[colors]
bg = "#010203"
statusline_accent_bg = "#112233"
addition_fg = "bright-green"
keyword = "#aabbcc"
"##,
    )
    .expect("custom theme should parse")
    .expect("native custom theme should be detected");

    assert_eq!(theme.background, Color::Rgb(0x01, 0x02, 0x03));
    assert_eq!(theme.foreground, base.foreground);
    assert_eq!(theme.statusline_accent_bg, Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(theme.addition_fg, Color::LightGreen);
    assert_eq!(
        theme.syntax.color(SyntaxClass::Keyword),
        Some(Color::Rgb(0xaa, 0xbb, 0xcc))
    );
    assert!(theme.transparent_background);
    assert!(theme.exact_syntax.is_some());

    assert!(
        theme
            .with_transparent_background_override(None)
            .transparent_background,
        "an omitted global setting should preserve theme-local transparency"
    );
    assert!(
        !theme
            .with_transparent_background_override(Some(false))
            .transparent_background,
        "an explicit global setting should override theme-local transparency"
    );
}

#[test]
fn legacy_base16_colors_table_is_not_a_native_custom_theme() {
    let contents = r##"
scheme = "Legacy"

[colors]
base00 = "#000000"
base01 = "#111111"
base02 = "#222222"
base03 = "#333333"
base04 = "#444444"
base05 = "#555555"
base06 = "#666666"
base07 = "#777777"
base08 = "#888888"
base09 = "#999999"
base0A = "#aaaaaa"
base0B = "#bbbbbb"
base0C = "#cccccc"
base0D = "#dddddd"
base0E = "#eeeeee"
base0F = "#ffffff"
"##;

    assert!(parse_base16_scheme(contents).is_some());
    assert!(
        parse_custom_colorscheme(contents)
            .expect("legacy Base16 TOML should not fail native validation")
            .is_none()
    );
}

#[test]
fn custom_theme_rejects_unknown_parent() {
    let error = parse_custom_colorscheme("extends = \"not-a-theme\"\n[colors]\nfg = \"white\"\n")
        .expect_err("unknown custom parent should fail");

    assert!(
        error
            .to_string()
            .contains("unknown built-in theme 'not-a-theme'")
    );
}

#[test]
fn custom_theme_reports_unknown_color_keys() {
    let error = parse_custom_colorscheme("[colors]\nbackgroun = \"#010203\"\n")
        .expect_err("misspelled custom color should fail");

    assert!(
        error
            .to_string()
            .contains("unknown custom theme color 'backgroun'")
    );
}

#[test]
fn malformed_custom_theme_reports_toml_error() {
    let error = parse_custom_colorscheme("extends = \"nord\"\n[colors\nbg = \"#010203\"\n")
        .expect_err("malformed custom theme should fail as TOML");

    assert!(error.to_string().contains("invalid custom theme"));
}

#[test]
fn base16_theme_parser_accepts_yaml_or_toml_lines() {
    let scheme = parse_base16_scheme(
        r##"
base00: "#000000"
base01: "111111"
base02: "222222"
base03: "333333"
base04 = "444444"
base05 = "555555"
base06 = "666666"
base07 = "777777"
base08 = "888888"
base09 = "999999"
base0A = "aaaaaa"
base0B = "bbbbbb"
base0C = "cccccc"
base0D = "dddddd"
base0E = "eeeeee"
base0F = "ffffff"
"##,
    )
    .expect("base16 scheme should parse");
    let theme = DiffTheme::base16(scheme);

    assert_eq!(theme.muted, Color::Rgb(51, 51, 51));
    assert_eq!(
        theme.syntax.color(SyntaxClass::String),
        Some(Color::Rgb(187, 187, 187))
    );
}

#[test]
fn inline_emphasis_leaves_unpaired_changed_lines_to_line_style() {
    let lines = vec![DiffLine::deletion(1, "removed line".to_owned())];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert!(emphasis[0].ranges.is_empty());
}

#[test]
fn lazy_inline_emphasis_matches_eager_emphasis() {
    let lines = vec![
        DiffLine::deletion(1, "let count = 1;".to_owned()),
        DiffLine::addition(1, "let total = 2;".to_owned()),
        DiffLine::deletion(2, "removed only".to_owned()),
        DiffLine::context(3, 2, "context".to_owned()),
        DiffLine::addition(3, "added only".to_owned()),
        DiffLine::deletion(4, "alpha beta".to_owned()),
        DiffLine::deletion(5, "gamma".to_owned()),
        DiffLine::addition(4, "alpha zeta".to_owned()),
        DiffLine::addition(5, "delta".to_owned()),
    ];
    let expected = compute_hunk_inline_emphasis(&lines);
    let mut cache = InlineHunkEmphasisCache::new(&lines);

    for index in [7, 5, 2, 4, 0, 1, 8, 6, 3] {
        assert_eq!(
            cache.ranges_for_line(&lines, index),
            expected[index].ranges,
            "lazy emphasis should match eager emphasis for line {index}"
        );
        assert_eq!(
            cache.ranges_for_line(&lines, index),
            expected[index].ranges,
            "cached emphasis should be stable for line {index}"
        );
    }
}

#[test]
fn inline_diff_skips_expensive_long_line_pairs() {
    let lines = vec![
        DiffLine::deletion(1, "a".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1)),
        DiffLine::addition(1, "b".repeat(MAX_INLINE_DIFF_LINE_BYTES + 1)),
    ];

    let emphasis = compute_hunk_inline_emphasis(&lines);

    assert!(emphasis[0].ranges.is_empty());
    assert!(emphasis[1].ranges.is_empty());
}

#[test]
fn content_spans_layers_inline_emphasis_over_syntax() {
    let text = "let value = 2;";
    let number_start = text.find('2').unwrap();
    let syntax = HighlightedLine {
        fingerprint: mark_syntax::LineTextFingerprint::from_text(text),
        segments: vec![
            mark_syntax::SyntaxSegment {
                byte_start: 0,
                byte_end: 12,
                class: Some(SyntaxClass::Keyword),
                scope_stack: Default::default(),
            },
            mark_syntax::SyntaxSegment {
                byte_start: 12,
                byte_end: 13,
                class: Some(SyntaxClass::Number),
                scope_stack: Default::default(),
            },
            mark_syntax::SyntaxSegment {
                byte_start: 13,
                byte_end: 14,
                class: Some(SyntaxClass::Punctuation),
                scope_stack: Default::default(),
            },
        ],
        scope_table: Default::default(),
    };

    let spans = content_spans_at_scroll(
        text,
        Some(&syntax),
        &[InlineRange {
            byte_start: number_start,
            byte_end: number_start + 1,
        }],
        DiffLineKind::Addition,
        20,
        DiffTheme::default(),
        0,
    );
    let number = spans
        .iter()
        .find(|span| span.content.as_ref() == "2")
        .expect("number span should be split out for inline emphasis");

    assert_eq!(
        number.style.fg,
        syntax_fg(SyntaxClass::Number, DiffTheme::default())
    );
    assert_eq!(
        number.style.bg,
        Some(DiffTheme::default().addition_inline_bg)
    );
    assert!(number.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn oversized_lines_disable_hunk_highlighting() {
    let limits = SyntaxLimits::default();
    let lines = vec![
        DiffLine::context(1, 1, "x".repeat(limits.max_line_bytes + 1)),
        DiffLine::context(2, 2, "let value = 1;".to_owned()),
    ];

    assert_eq!(
        build_hunk_source(&lines, DiffSide::New, limits).unwrap_err(),
        SyntaxSkipReason::TooLarge
    );
}

#[test]
fn hunk_source_excludes_diff_meta_lines_and_preserves_empty_lines() {
    let lines = vec![
        DiffLine::context(1, 1, "let a = 1;".to_owned()),
        DiffLine::meta("\\ No newline at end of file".to_owned()),
        DiffLine::addition(2, String::new()),
    ];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "let a = 1;\n");
    assert_eq!(source.line_map, vec![Some(0), None, Some(1)]);
    assert_eq!(source.source_lines, 2);
}

#[test]
fn hunk_source_preserves_leading_empty_lines() {
    let lines = vec![
        DiffLine::addition(1, String::new()),
        DiffLine::addition(2, "let value = 1;".to_owned()),
    ];

    let source = build_hunk_source(&lines, DiffSide::New, SyntaxLimits::default()).unwrap();

    assert_eq!(source.text, "\nlet value = 1;");
    assert_eq!(source.line_map, vec![Some(0), Some(1)]);
    assert_eq!(source.source_lines, 2);
}

#[test]
fn full_file_line_map_uses_absolute_line_numbers() {
    let lines = vec![
        DiffLine::deletion(10, "old".to_owned()),
        DiffLine::addition(11, "new".to_owned()),
        DiffLine::context(12, 12, "same".to_owned()),
    ];

    assert_eq!(
        build_full_file_line_map(&lines, DiffSide::Old).unwrap(),
        vec![Some(9), None, Some(11)]
    );
    assert_eq!(
        build_full_file_line_map(&lines, DiffSide::New).unwrap(),
        vec![None, Some(10), Some(11)]
    );
}
