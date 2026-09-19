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
