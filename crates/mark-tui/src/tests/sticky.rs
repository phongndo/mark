use super::*;

fn unlabeled_hunk_changeset(body_lines: usize) -> Changeset {
    let mut lines = vec![DiffLine::addition(1, "fn example() {")];
    lines.extend((0..body_lines).map(|offset| DiffLine::addition(offset + 2, "    body_line")));
    lines.push(DiffLine::addition(body_lines + 2, "}"));
    let line_count = lines.len();

    Changeset {
        repo: PathBuf::from("/repo").into(),
        title: "test".to_owned(),
        files: vec![mark_diff::DiffFile {
            change: mark_diff::FileChange::from_status(
                mark_diff::FileStatus::Added,
                None,
                Some("file.rs".to_owned()),
            ),
            additions: line_count,
            deletions: 0,
            body: mark_diff::DiffFileBody::Text {
                hunks: vec![mark_diff::DiffHunk {
                    header: format!("@@ -0,0 +1,{line_count} @@"),
                    ranges: HunkLineRanges::new(0, 0, 1, line_count),
                    lines,
                }],
            },
        }],
        raw_patch: mark_diff::Changeset::empty_raw_patch(),
    }
}

fn oversized_hunk_app(viewport_rows: usize) -> DiffApp {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        unlabeled_hunk_changeset(40),
        DiffLayoutMode::Unified,
    );
    app.config.annotation_targeting = AnnotationTargeting::Hints;
    app.set_viewport_rows(viewport_rows);
    app.set_viewport_width(80);
    app
}

#[test]
fn oversized_hunk_pins_real_hunk_header_with_fallback_label() {
    let mut app = oversized_hunk_app(8);
    let header = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("hunk header");
    app.set_scroll(header.saturating_add(5));

    let lines = build_diff_viewport_lines(&mut app, 80, 8);
    let sticky = line_text(&lines[0]);
    assert!(sticky.contains("@@ -0,0 +1,42 @@"), "{sticky:?}");
    assert!(sticky.contains("fn example() {"), "{sticky:?}");
}

#[test]
fn visible_unlabeled_hunk_header_uses_the_same_fallback_label() {
    let mut app = oversized_hunk_app(8);
    let header = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("hunk header");
    app.set_scroll(header);

    let lines = build_diff_viewport_lines(&mut app, 80, 8);
    let in_flow = line_text(&lines[0]);
    assert!(in_flow.contains("@@ -0,0 +1,42 @@"), "{in_flow:?}");
    assert!(in_flow.contains("fn example() {"), "{in_flow:?}");
    assert!(!line_text(&lines[1]).contains("@@"));
}

#[test]
fn small_hunk_does_not_pin_after_its_end_is_visible() {
    let mut app = DiffApp::new(
        DiffOptions::default(),
        unlabeled_hunk_changeset(2),
        DiffLayoutMode::Unified,
    );
    app.config.annotation_targeting = AnnotationTargeting::Hints;
    app.set_viewport_rows(3);
    app.set_viewport_width(80);
    let header = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("hunk header");
    app.set_scroll(header.saturating_add(2));

    let lines = build_diff_viewport_lines(&mut app, 80, 3);
    assert!(!line_text(&lines[0]).contains("@@"));
}

#[test]
fn wrapped_oversized_hunk_pins_real_header_with_fallback_label() {
    let mut app = oversized_hunk_app(8);
    app.viewport.line_wrapping = true;
    app.set_viewport_width(32);
    let header = app
        .document
        .model
        .hunk_start_row(0, 0)
        .expect("hunk header");
    let header_visual = app.wrapped_visual_scroll_for_model_row(header);
    app.set_scroll(header_visual.saturating_add(5));

    let lines = build_diff_viewport_lines(&mut app, 32, 8);
    let sticky = line_text(&lines[0]);
    assert!(sticky.contains("@@ -0,0"), "{sticky:?}");
    assert!(sticky.contains("fn ex"), "{sticky:?}");
}
