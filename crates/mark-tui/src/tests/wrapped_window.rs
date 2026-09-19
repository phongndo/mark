use super::*;

#[test]
fn wrapped_windows_match_full_rows_across_unicode_and_layouts() {
    const ATOMS: &[&str] = &[
        "a",
        " ",
        "\t",
        "\0",
        "é",
        "e\u{301}",
        "界",
        "👩‍💻",
        "ｶﾞ",
        "\u{200b}",
        "needle",
    ];
    let mut state = 0x9b17_284d_06ec_5f31u64;
    for case in 0..48 {
        let mut text = String::new();
        for _ in 0..24 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            text.push_str(ATOMS[state as usize % ATOMS.len()]);
        }
        let mut changeset = changeset_with_replacement_pair();
        *changeset.files[0].hunks_mut()[0].lines[0].text_mut() = text;
        *changeset.files[0].hunks_mut()[0].lines[1].text_mut() = "short needle\t界".into();
        for layout in [DiffLayoutMode::Unified, DiffLayoutMode::Split] {
            let mut app = DiffApp::new(DiffOptions::default(), changeset.clone(), layout);
            app.viewport.line_wrapping = true;
            app.filters.grep_filter = "needle".into();
            for width in [0, 1, 15, 18, 23, 24, 25, 39, 80] {
                app.set_viewport_width(width);
                for row_index in 0..app.document.model.len() {
                    let row = app.document.model.row(row_index).unwrap();
                    let focused = Some((FILE_0, HUNK_0));
                    let full = render_row_wrapped_with_focus(
                        &mut app,
                        row_index,
                        row,
                        width,
                        focused,
                        0..usize::MAX,
                    );
                    for start in [
                        0,
                        1,
                        full.len() / 2,
                        full.len().saturating_sub(1),
                        full.len(),
                        usize::MAX,
                    ] {
                        for len in [0, 1, 3, 40] {
                            let window = render_row_wrapped_with_focus(
                                &mut app,
                                row_index,
                                row,
                                width,
                                focused,
                                start..start.saturating_add(len),
                            );
                            let expected = full
                                .iter()
                                .skip(start)
                                .take(len)
                                .cloned()
                                .collect::<Vec<_>>();
                            assert_eq!(
                                window, expected,
                                "case={case}, layout={layout:?}, width={width}, row={row_index}, start={start}, len={len}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn wrapped_context_windows_preserve_syntax_and_grep_styles() {
    let text = "let needle = \"界👩‍💻\";\t// e\u{301} end";
    let line = DiffLine::context(11, 17, text);
    let syntax = mark_syntax::SyntaxHighlighter::new()
        .highlight("rust", text)
        .unwrap();
    let syntax = &syntax.lines[0];
    for width in [0, 1, 17, 25, 39, 80] {
        let full = render_split_context_line_wrapped(
            &line,
            Some(syntax),
            7,
            width,
            DiffTheme::default(),
            "needle",
            0..usize::MAX,
        );
        for start in 0..=full.len() + 1 {
            let window = render_split_context_line_wrapped(
                &line,
                Some(syntax),
                7,
                width,
                DiffTheme::default(),
                "needle",
                start..start + 2,
            );
            assert_eq!(
                window,
                full.iter().skip(start).take(2).cloned().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn wrapped_viewport_keeps_annotations_after_the_final_continuation() {
    use crate::annotation::{AnnotationDraft, AnnotationKey};
    use crate::render::viewport_plan::{ViewportSlotKind, plan_diff_viewport_rows};

    for layout in [DiffLayoutMode::Unified, DiffLayoutMode::Split] {
        let mut app = DiffApp::new(
            DiffOptions::default(),
            changeset_with_line_texts(&["abc界\tneedle👩‍💻end", "following"]),
            layout,
        );
        app.viewport.line_wrapping = true;
        app.set_viewport_rows(8);
        for width in [25, 39, 24] {
            app.set_viewport_width(width);
            let row_index = 2;
            let key = AnnotationKey::from_ui_row(
                &app.document.changeset,
                app.document.model.row(row_index).unwrap(),
            )
            .unwrap();
            app.annotations_state
                .annotations
                .insert(key.clone(), "window note".into());
            for compose in [false, true] {
                app.annotations_state.annotation_draft = compose.then(|| AnnotationDraft {
                    key: key.clone(),
                    model_row_index: row_index,
                    input: "window note".into(),
                    cursor: 0,
                });
                let start = app.wrapped_visual_scroll_for_model_row(row_index);
                let height = app.wrapped_visual_height_for_model_row(row_index);
                for offset in 0..height {
                    app.viewport.scroll = start + offset;
                    let plan = plan_diff_viewport_rows(&app, 8);
                    let rendered = build_diff_viewport_lines(&mut app, width, 8);
                    assert_eq!(rendered.len(), plan.len());
                    for (screen_row, slot) in plan.iter().enumerate() {
                        if let ViewportSlotKind::DiffVisual {
                            visual_scroll,
                            model_row,
                        } = slot.kind
                        {
                            assert_eq!(
                                app.model_row_at_scroll(visual_scroll).unwrap().0,
                                model_row
                            );
                        }
                        match slot.kind {
                            ViewportSlotKind::AnnotationSaved { block_row: 1, .. }
                            | ViewportSlotKind::AnnotationCompose { block_row: 1, .. } => {
                                assert!(line_text(&rendered[screen_row]).contains("window note"));
                                assert!(screen_row >= height - offset);
                            }
                            _ => {}
                        }
                    }
                }
            }
            app.annotations_state.annotation_draft = None;
        }
    }
}
