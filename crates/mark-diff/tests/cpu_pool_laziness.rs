use std::sync::Arc;

#[test]
fn small_and_single_section_patches_do_not_start_cpu_pool() {
    assert!(!mark_runtime::is_cpu_pool_started());

    let small_patch = Arc::<[u8]>::from(
        b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
            .as_slice(),
    );
    assert_eq!(mark_diff::parse_patch_bytes(small_patch).len(), 1);
    assert!(!mark_runtime::is_cpu_pool_started());

    let padding = "x".repeat(8 * 1024 * 1024);
    let single_section_patch = Arc::<[u8]>::from(
        format!(
            "diff --git a/a.txt b/a.txt\nindex {padding}\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
        )
        .into_bytes(),
    );
    assert_eq!(mark_diff::parse_patch_bytes(single_section_patch).len(), 1);
    assert!(!mark_runtime::is_cpu_pool_started());
}
