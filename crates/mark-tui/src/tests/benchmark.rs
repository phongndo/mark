use super::*;

#[test]
fn syntax_benchmark_finishes_jobs_from_the_last_random_viewport() {
    let mut changeset = changeset_with_line_text("line first");
    let mut last_file = changeset_with_context_lines(4_000).files.remove(0);
    set_test_file_modified(&mut last_file, "last.rs");
    changeset.files.push(last_file);
    let report = crate::benchmark_diff_view(
        changeset,
        Some(vec!["rust".into()]),
        DiffBenchmarkOptions {
            width: 80,
            viewport_rows: 1,
            max_scroll_steps: 1,
            ..Default::default()
        },
    );
    assert!(report.syntax.jobs_queued > 0);
    assert_eq!(report.syntax.jobs_failed, 0);
    assert_eq!(report.syntax.jobs_skipped, 0);
    assert_eq!(report.syntax.jobs_evicted, 0);
    assert_eq!(report.syntax.jobs_completed, report.syntax.jobs_queued);
}

#[test]
fn unified_wrapped_benchmark_scrolls_continuation_rows() {
    let mut changeset = changeset_with_replacement_pair();
    for line in &mut changeset.files[0].hunks_mut()[0].lines {
        *line.text_mut() = "wrapped界 ".repeat(400);
    }
    let options = DiffBenchmarkOptions {
        width: 60,
        viewport_rows: 4,
        scroll_step: 3,
        max_scroll_steps: 20,
        line_wrapping: true,
        ..Default::default()
    };
    let split = crate::benchmark_diff_view(changeset.clone(), None, options);
    let unified = crate::benchmark_diff_view(
        changeset,
        None,
        DiffBenchmarkOptions {
            unified: true,
            ..options
        },
    );

    // Each logical line wraps into more than max_scroll_steps visual rows.
    assert_eq!(unified.warm_scroll_steps, 20);
    assert_eq!(split.warm_scroll_steps, 20);
    // Unified shows the replaced pair on two model rows; split pairs them.
    assert_eq!(unified.row_count, split.row_count + 1);
}
