use super::*;

#[test]
fn parse_patch_bytes_reports_explicit_limits() {
    let patch = Arc::<[u8]>::from(
        b"diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
-old
+new
"
        .as_slice(),
    );

    let error = parse_patch_bytes_limited(
        patch,
        DiffLimits {
            max_diff_rows: Some(1),
            ..DiffLimits::default()
        },
    )
    .expect_err("two diff rows should exceed the limit");

    assert_eq!(error.limit, "diff rows");
    assert_eq!(error.max, 1);
    assert_eq!(error.actual, 2);
}

#[test]
fn malformed_hunk_counts_do_not_drive_allocations_or_overflow_lines() {
    let max = usize::MAX;
    let patch = format!(
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -{max},{max} +{max},{max} @@\n line\n"
    );

    let text_files = parse_patch(&patch);
    let byte_files = parse_patch_bytes(Arc::from(patch.into_bytes().into_boxed_slice()));

    assert_eq!(text_files.len(), 1);
    assert_eq!(text_files[0].hunks()[0].lines.len(), 1);
    assert_eq!(byte_files, text_files);
    assert_eq!(
        byte_files[0].hunks()[0].lines[0].old_line(),
        Some(u32::MAX as usize)
    );
    assert_eq!(
        byte_files[0].hunks()[0].lines[0].new_line(),
        Some(u32::MAX as usize)
    );
}

#[test]
fn malformed_hunk_count_boundaries_never_panic() {
    let mut counts = vec![usize::MAX, usize::MAX - 1];
    if let Ok(above_u32) = usize::try_from(u64::from(u32::MAX) + 1) {
        counts.push(above_u32);
    }
    for count in counts {
        let patch = format!("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,{count} +1,1 @@\n+new\n");
        let files = parse_patch_bytes(Arc::from(patch.into_bytes().into_boxed_slice()));
        assert_eq!(files[0].hunks()[0].lines.len(), 1);
    }
}
