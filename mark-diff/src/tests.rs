use super::*;
use crate::{
    difftool::rewrite_difftool_patch_paths,
    git_io::{StderrCapture, parse_numstat, temp_index_path},
};
use std::{env, io::Write, process::Stdio};

#[test]
fn parse_patch_omits_no_newline_at_end_of_file_marker() {
    let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n line\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks[0].lines.len(), 3);
    assert!(
        files[0].hunks[0]
            .lines
            .iter()
            .all(|line| line.kind != DiffLineKind::Meta)
    );
}

#[test]
fn parse_patch_reads_file_hunks_and_line_numbers() {
    let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,3 @@\n one\n-two\n+two changed\n+three\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_path(), "a.txt");
    assert_eq!(files[0].additions, 2);
    assert_eq!(files[0].deletions, 1);
    assert_eq!(files[0].hunks[0].lines[0].old_line, Some(1));
    assert_eq!(files[0].hunks[0].lines[0].new_line, Some(1));
    assert_eq!(files[0].hunks[0].lines[1].old_line, Some(2));
    assert_eq!(files[0].hunks[0].lines[1].new_line, None);
    assert_eq!(files[0].hunks[0].lines[2].old_line, None);
    assert_eq!(files[0].hunks[0].lines[2].new_line, Some(2));
}

#[test]
fn parse_patch_stats_counts_without_storing_hunk_lines() {
    let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,3 @@\n one\n-two\n+two changed\n+three\ndiff --git a/blob.bin b/blob.bin\nBinary files a/blob.bin and b/blob.bin differ\n";

    let stats = parse_patch_stats(BufReader::new(patch.as_bytes())).unwrap();

    assert_eq!(stats.files.len(), 2);
    assert_eq!(stats.files[0].display_path(), "a.txt");
    assert_eq!(stats.files[0].additions, 2);
    assert_eq!(stats.files[0].deletions, 1);
    assert_eq!(stats.files[1].display_path(), "blob.bin");
    assert!(stats.files[1].is_binary);
    assert_eq!(stats.totals.files, 2);
    assert_eq!(stats.totals.additions, 2);
    assert_eq!(stats.totals.deletions, 1);
    assert_eq!(stats.totals.binary_files, 1);
}

#[test]
fn parse_patch_stats_counts_non_utf8_hunk_lines() {
    let patch = b"diff --git a/bytes.txt b/bytes.txt\n--- a/bytes.txt\n+++ b/bytes.txt\n@@ -1 +1 @@\n-\xff\n+\xfe\n";

    let stats = parse_patch_stats(BufReader::new(patch.as_slice())).unwrap();

    assert_eq!(stats.files.len(), 1);
    assert_eq!(stats.files[0].display_path(), "bytes.txt");
    assert_eq!(stats.files[0].additions, 1);
    assert_eq!(stats.files[0].deletions, 1);
    assert_eq!(stats.totals.files, 1);
    assert_eq!(stats.totals.additions, 1);
    assert_eq!(stats.totals.deletions, 1);
}

#[test]
fn render_bytes_stat_matches_full_changeset_stat_for_patch() {
    let patch = Arc::<[u8]>::from(
            b"--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+next\n--- a/b.txt\n+++ b/b.txt\n@@ -2 +2 @@\n-left\n+right\n"
                .as_slice(),
        );
    let options = DiffOptions {
        source: DiffSource::Patch(PatchSource::Stdin(patch)),
        stat: true,
        include_untracked: false,
        ..DiffOptions::default()
    };

    let streamed = String::from_utf8(render_bytes(options.clone()).unwrap()).unwrap();
    let full = render_stat(&load_review_ref(&options).unwrap());

    assert_eq!(streamed, full);
}

#[test]
fn render_bytes_stat_matches_full_changeset_stat_for_repo_source() {
    let test_dir = temp_test_dir("repo-stat-equivalence");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("rename.txt"), "same\n").expect("renamed file should be written");
    fs::write(repo.join("binary.bin"), b"\0base\n").expect("binary file should be written");
    git(["add", "rename.txt", "binary.bin"], &repo);
    git(["commit", "-q", "-m", "fixtures"], &repo);

    fs::write(repo.join("base.txt"), "base\nnext\n").expect("tracked file should change");
    fs::write(repo.join("binary.bin"), b"\0changed\n").expect("binary file should change");
    fs::write(repo.join("untracked.txt"), "new\n").expect("untracked file should be written");
    git(["mv", "rename.txt", "renamed.txt"], &repo);
    let options = DiffOptions {
        repo: Some(repo.clone()),
        stat: true,
        ..DiffOptions::default()
    };

    let streamed = String::from_utf8(render_bytes(options.clone()).unwrap()).unwrap();
    let full = render_stat(&load_review_ref(&options).unwrap());

    assert_eq!(streamed, full);
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn base_branch_diff_includes_committed_staged_and_untracked_changes() {
    let test_dir = temp_test_dir("base-branch-all-changes");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    git(["branch", "-M", "main"], &repo);
    git(["checkout", "-q", "-b", "feature"], &repo);

    fs::write(repo.join("committed.txt"), "committed\n").expect("committed file should be written");
    git(["add", "committed.txt"], &repo);
    git(["commit", "-q", "-m", "committed"], &repo);
    fs::write(repo.join("staged.txt"), "staged\n").expect("staged file should be written");
    git(["add", "staged.txt"], &repo);
    fs::write(repo.join("untracked.txt"), "untracked\n").expect("untracked file should be written");

    let changeset = load_review_ref(&DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Base("main".to_owned()),
        ..DiffOptions::default()
    })
    .expect("base branch diff should load");
    let paths = changeset
        .files
        .iter()
        .map(DiffFile::display_path)
        .collect::<Vec<_>>();

    assert!(paths.contains(&"committed.txt"));
    assert!(paths.contains(&"staged.txt"));
    assert!(paths.contains(&"untracked.txt"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn base_branch_diff_reports_unknown_base_revision() {
    let test_dir = temp_test_dir("unknown-base-revision");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    let error = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Base("missing-branch".to_owned()),
        ..DiffOptions::default()
    })
    .expect_err("missing base should fail before git merge-base");

    assert_eq!(error.to_string(), "unknown base revision `missing-branch`");
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_reports_unknown_revision() {
    let test_dir = temp_test_dir("unknown-range-revision");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    let error = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD".to_owned(),
            right: "missing-branch".to_owned(),
        },
        ..DiffOptions::default()
    })
    .expect_err("missing range side should fail before git diff");

    assert_eq!(error.to_string(), "unknown revision `missing-branch`");
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_accepts_pathspec_right_operand() {
    let test_dir = temp_test_dir("range-pathspec-revision");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::create_dir_all(repo.join("src")).expect("source directory should be created");
    fs::write(repo.join("src/lib.rs"), "one\n").expect("lib file should be written");
    fs::write(repo.join("src/other.rs"), "one\n").expect("other file should be written");
    git(["add", "src/lib.rs", "src/other.rs"], &repo);
    git(["commit", "-q", "-m", "add sources"], &repo);
    fs::write(repo.join("src/lib.rs"), "two\n").expect("lib file should change");
    fs::write(repo.join("src/other.rs"), "two\n").expect("other file should change");

    let options = DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD".to_owned(),
            right: "src/lib.rs".to_owned(),
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = render(options.clone()).expect("pathspec range should render");
    assert!(patch.contains("diff --git a/src/lib.rs b/src/lib.rs"));
    assert!(patch.contains("+two"));
    assert!(!patch.contains("src/other.rs"));

    let stat = render(DiffOptions {
        stat: true,
        ..options
    })
    .expect("pathspec range stat should render");
    assert!(stat.contains("src/lib.rs"));
    assert!(!stat.contains("src/other.rs"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_accepts_treeish_revisions() {
    let test_dir = temp_test_dir("range-treeish-revisions");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    let base_tree = git_output(["rev-parse", "HEAD^{tree}"], &repo);
    fs::write(repo.join("base.txt"), "changed\n").expect("base file should change");
    git(["add", "base.txt"], &repo);
    git(["commit", "-q", "-m", "change base"], &repo);

    let patch = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: base_tree,
            right: "HEAD".to_owned(),
        },
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect("tree object range should render");
    assert!(patch.contains("+changed"));

    let stat = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD~1^{tree}".to_owned(),
            right: "HEAD".to_owned(),
        },
        include_untracked: false,
        stat: true,
        ..DiffOptions::default()
    })
    .expect("tree-ish range stat should render");
    assert!(stat.contains("base.txt"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_accepts_multi_object_left_revision() {
    let test_dir = temp_test_dir("range-multi-object-revision");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    git(["branch", "-M", "main"], &repo);
    git(["checkout", "-q", "-b", "side"], &repo);
    fs::write(repo.join("side.txt"), "side\n").expect("side file should be written");
    git(["add", "side.txt"], &repo);
    git(["commit", "-q", "-m", "side"], &repo);
    git(["checkout", "-q", "main"], &repo);
    git(["merge", "-q", "--no-ff", "side", "-m", "merge"], &repo);

    let stat = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD^@".to_owned(),
            right: "HEAD".to_owned(),
        },
        include_untracked: false,
        stat: true,
        ..DiffOptions::default()
    })
    .expect("multi-object range should render");

    assert!(stat.contains("side.txt"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_accepts_rev_path_tree_revisions() {
    let test_dir = temp_test_dir("range-rev-path-tree-revisions");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    fs::create_dir_all(repo.join("src")).expect("source directory should be created");
    fs::write(repo.join("src/file.txt"), "one\n").expect("source file should be written");
    git(["add", "src/file.txt"], &repo);
    git(["commit", "-q", "-m", "add source"], &repo);

    fs::write(repo.join("src/file.txt"), "two\n").expect("source file should change");
    git(["add", "src/file.txt"], &repo);
    git(["commit", "-q", "-m", "change source"], &repo);

    let patch = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD~1:src".to_owned(),
            right: "HEAD:src".to_owned(),
        },
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect("rev:path tree range should render");

    assert!(patch.contains("+two"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn range_diff_accepts_rev_path_blob_revisions() {
    let test_dir = temp_test_dir("range-rev-path-blob-revisions");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    fs::write(repo.join("file.txt"), "one\n").expect("file should be written");
    git(["add", "file.txt"], &repo);
    git(["commit", "-q", "-m", "add file"], &repo);

    fs::write(repo.join("file.txt"), "two\n").expect("file should change");
    git(["add", "file.txt"], &repo);
    git(["commit", "-q", "-m", "change file"], &repo);

    let options = DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Range {
            left: "HEAD~1:file.txt".to_owned(),
            right: "HEAD:file.txt".to_owned(),
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = render(options.clone()).expect("rev:path blob range should render");
    assert!(patch.contains("-one"));
    assert!(patch.contains("+two"));

    let stat = render(DiffOptions {
        stat: true,
        ..options
    })
    .expect("rev:path blob range stat should render");
    assert!(stat.contains("file.txt"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn base_branch_diff_keeps_commitish_validation() {
    let test_dir = temp_test_dir("base-treeish-revision");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    let error = render(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Base("HEAD^{tree}".to_owned()),
        ..DiffOptions::default()
    })
    .expect_err("merge-base diffs should still require commit-ish base revisions");

    assert_eq!(error.to_string(), "unknown base revision `HEAD^{tree}`");
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn load_review_ref_path_limits_tracked_and_untracked_files() {
    let test_dir = temp_test_dir("path-scoped-review");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("other.txt"), "other\n").expect("other file should be written");
    git(["add", "other.txt"], &repo);
    git(["commit", "-q", "-m", "other"], &repo);

    fs::write(repo.join("base.txt"), "base changed\n").expect("base file should change");
    fs::write(repo.join("other.txt"), "other changed\n").expect("other file should change");
    fs::write(repo.join("new.txt"), "new\n").expect("untracked file should be written");
    let options = DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    };

    let tracked = load_review_ref_path(&options, Path::new("base.txt")).unwrap();
    assert_eq!(tracked.files.len(), 1);
    assert_eq!(tracked.files[0].display_path(), "base.txt");

    let untracked = load_review_ref_path(&options, Path::new("new.txt")).unwrap();
    assert_eq!(untracked.files.len(), 1);
    assert_eq!(untracked.files[0].display_path(), "new.txt");

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn load_review_ref_paths_preserves_scoped_rename_metadata() {
    let test_dir = temp_test_dir("path-scoped-rename");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    let base = (1..=20)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    fs::write(repo.join("old.txt"), base).expect("old file should be written");
    git(["add", "old.txt"], &repo);
    git(["commit", "-q", "-m", "old"], &repo);

    git(["mv", "old.txt", "new.txt"], &repo);
    let changed = (1..=20)
        .map(|line| {
            if line == 20 {
                "line changed\n".to_owned()
            } else {
                format!("line {line}\n")
            }
        })
        .collect::<String>();
    fs::write(repo.join("new.txt"), changed).expect("new file should be changed");
    let options = DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    };

    let new_only = load_review_ref_path(&options, Path::new("new.txt")).unwrap();
    assert_eq!(new_only.files[0].status, FileStatus::Added);

    let paired = load_review_ref_paths(
        &options,
        &[PathBuf::from("old.txt"), PathBuf::from("new.txt")],
    )
    .unwrap();

    assert_eq!(paired.files.len(), 1);
    assert_eq!(paired.files[0].status, FileStatus::Renamed);
    assert_eq!(paired.files[0].old_path.as_deref(), Some("old.txt"));
    assert_eq!(paired.files[0].new_path.as_deref(), Some("new.txt"));
    assert_eq!(paired.files[0].additions, 1);
    assert_eq!(paired.files[0].deletions, 1);

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn parse_numstat_reads_regular_renamed_and_binary_files() {
    let numstat =
        b"2\t1\tsrc/lib.rs\x00-\t-\timage.bin\x000\t0\t\x00old/name.rs\x00new/name.rs\x00";

    let stats = parse_numstat(numstat.as_slice()).unwrap();

    assert_eq!(stats.files.len(), 3);
    assert_eq!(stats.files[0].display_path(), "src/lib.rs");
    assert_eq!(stats.files[1].display_path(), "image.bin");
    assert!(stats.files[1].is_binary);
    assert_eq!(stats.files[2].display_path(), "new/name.rs");
    assert_eq!(stats.totals.files, 3);
    assert_eq!(stats.totals.additions, 2);
    assert_eq!(stats.totals.deletions, 1);
    assert_eq!(stats.totals.binary_files, 1);
}

#[test]
fn parse_patch_reads_plain_unified_diff_without_git_header() {
    let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_path(), "a.txt");
    assert_eq!(files[0].additions, 1);
    assert_eq!(files[0].deletions, 1);
}

#[test]
fn plain_unified_file_headers_wait_for_completed_hunks() {
    let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n--- old marker\n+++ new marker\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old\n+new\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 2);
    assert_eq!(files[0].display_path(), "a.txt");
    assert_eq!(files[0].hunks[0].lines[0].text, "-- old marker");
    assert_eq!(files[0].hunks[0].lines[1].text, "++ new marker");
    assert_eq!(files[1].display_path(), "b.txt");
}

#[test]
fn parse_patch_dequotes_git_c_style_paths() {
    let patch = "diff --git \"a/name\\twith\\\"quote\\\\.txt\" \"b/name\\twith\\\"quote\\\\.txt\"\n--- \"a/name\\twith\\\"quote\\\\.txt\"\n+++ \"b/name\\twith\\\"quote\\\\.txt\"\n@@ -1 +1 @@\n-old\n+new\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].old_path.as_deref(),
        Some("name\twith\"quote\\.txt")
    );
    assert_eq!(
        files[0].new_path.as_deref(),
        Some("name\twith\"quote\\.txt")
    );
    assert_eq!(files[0].display_path(), "name\twith\"quote\\.txt");
}

#[test]
fn parse_patch_dequotes_git_octal_utf8_paths() {
    let patch = "diff --git \"a/\\303\\251.txt\" \"b/\\303\\251.txt\"\n--- \"a/\\303\\251.txt\"\n+++ \"b/\\303\\251.txt\"\n@@ -1 +1 @@\n-old\n+new\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].old_path.as_deref(), Some("é.txt"));
    assert_eq!(files[0].new_path.as_deref(), Some("é.txt"));
    assert_eq!(files[0].display_path(), "é.txt");
}

#[test]
fn parse_patch_preserves_crlf_payloads() {
    let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\r\n+old\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks[0].lines[0].text, "old\r");
    assert_eq!(files[0].hunks[0].lines[1].text, "old");
}

#[test]
fn parse_patch_dequotes_rename_and_copy_metadata_paths() {
    let renamed = parse_patch(
        "diff --git \"a/old\\tname.txt\" \"b/new\\tname.txt\"\nsimilarity index 100%\nrename from \"old\\tname.txt\"\nrename to \"new\\tname.txt\"\n",
    );
    assert_eq!(renamed[0].old_path.as_deref(), Some("old\tname.txt"));
    assert_eq!(renamed[0].new_path.as_deref(), Some("new\tname.txt"));

    let copied = parse_patch(
        "diff --git \"a/src\\\"file.txt\" \"b/copy\\\"file.txt\"\nsimilarity index 100%\ncopy from \"src\\\"file.txt\"\ncopy to \"copy\\\"file.txt\"\n",
    );
    assert_eq!(copied[0].old_path.as_deref(), Some("src\"file.txt"));
    assert_eq!(copied[0].new_path.as_deref(), Some("copy\"file.txt"));
}

#[test]
fn stat_rendering_escapes_terminal_control_characters_in_paths() {
    let patch = Arc::<[u8]>::from(
            b"diff --git \"a/evil\\033]52;c;AAAA\\007.txt\" \"b/evil\\033]52;c;AAAA\\007.txt\"\n--- \"a/evil\\033]52;c;AAAA\\007.txt\"\n+++ \"b/evil\\033]52;c;AAAA\\007.txt\"\n@@ -1 +1 @@\n-old\n+new\n"
                .as_slice(),
        );
    let output = render(DiffOptions {
        source: DiffSource::Patch(PatchSource::Stdin(patch)),
        stat: true,
        ..DiffOptions::default()
    })
    .expect("stat output should render");

    assert!(!output.as_bytes().contains(&0x1b));
    assert!(!output.as_bytes().contains(&0x07));
    assert!(output.contains("\\u{1b}]52;c;AAAA\\u{7}.txt"));
}

#[test]
fn parse_patch_preserves_binary_paths_with_spaces() {
    let patch = "diff --git a/my file.bin b/my file.bin\nindex 1111111..2222222 100644\nGIT binary patch\nliteral 1\nKcmZQz1ONa4\n\n";

    let files = parse_patch(patch);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].old_path.as_deref(), Some("my file.bin"));
    assert_eq!(files[0].new_path.as_deref(), Some("my file.bin"));
    assert_eq!(files[0].display_path(), "my file.bin");
    assert!(files[0].is_binary);
}

#[test]
fn rename_or_copy_status_wins_over_later_mode_headers() {
    let renamed = parse_patch(
        "diff --git a/old.txt b/new.txt\nrename from old.txt\nrename to new.txt\nold mode 100644\nnew mode 100755\n",
    );
    assert_eq!(renamed[0].status, FileStatus::Renamed);

    let copied = parse_patch(
        "diff --git a/source.txt b/copy.txt\ncopy from source.txt\ncopy to copy.txt\nold mode 100644\nnew mode 100755\n",
    );
    assert_eq!(copied[0].status, FileStatus::Copied);
}

#[test]
fn view_model_indexes_file_and_hunk_rows() {
    let changeset = Changeset {
        repo: PathBuf::from("/repo"),
        title: "test".to_owned(),
        files: parse_patch(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n",
        ),
        raw_patch: Vec::new(),
    };
    let model = DiffViewModel::new(&changeset);

    assert_eq!(model.file_start_row(0), Some(0));
    assert_eq!(model.file_at_row(3), Some(0));
    assert_eq!(model.next_hunk_row(0), Some(1));
    assert_eq!(model.previous_hunk_row(4), Some(1));
}

#[test]
fn patch_file_source_renders_without_git_repo() {
    let test_dir = temp_test_dir("patch-file-source");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    let patch_path = test_dir.join("change.diff");
    let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
    fs::write(&patch_path, patch).expect("patch file should be written");

    let output = render(DiffOptions {
        source: DiffSource::Patch(PatchSource::File(patch_path)),
        ..DiffOptions::default()
    })
    .expect("patch source should render");

    assert_eq!(output, patch);
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn show_source_renders_commit_patch() {
    let test_dir = temp_test_dir("show-source");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "base\nchanged\n").expect("file should change");
    git(["add", "base.txt"], &repo);
    git(["commit", "-q", "-m", "change"], &repo);

    let expected = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "show",
            "--format=",
            "--binary",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "-m",
            "--end-of-options",
            "HEAD",
        ])
        .output()
        .expect("git show should run");
    assert!(
        expected.status.success(),
        "git show failed: {}",
        String::from_utf8_lossy(&expected.stderr)
    );

    let actual = render_bytes(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Show("HEAD".to_owned()),
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect("show source should render");

    assert_eq!(actual, expected.stdout);

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(repo),
            source: DiffSource::Show("HEAD".to_owned()),
            include_untracked: false,
            stat: true,
            ..DiffOptions::default()
        })
        .expect("show source stats should render"),
    )
    .expect("stat should be utf-8");
    assert!(stat.contains("base.txt"));
    assert!(stat.contains("1 files changed, 1 insertions(+), 0 deletions(-)"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn show_source_stat_peels_annotated_tag() {
    let test_dir = temp_test_dir("show-annotated-tag-stat");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "base\nnext\n").expect("file should change");
    git(["commit", "-q", "-am", "change"], &repo);
    git(["tag", "-a", "--no-sign", "v1.0", "-m", "release"], &repo);

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(repo.clone()),
            source: DiffSource::Show("v1.0".to_owned()),
            include_untracked: false,
            stat: true,
            ..DiffOptions::default()
        })
        .expect("show source stats should render"),
    )
    .expect("stat should be utf-8");

    assert!(stat.contains("base.txt"));
    assert!(stat.contains("1 files changed, 1 insertions(+), 0 deletions(-)"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn show_source_patch_peels_annotated_tag() {
    let test_dir = temp_test_dir("show-annotated-tag-patch");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "base\nnext\n").expect("file should change");
    git(["commit", "-q", "-am", "change"], &repo);
    git(
        [
            "tag",
            "-a",
            "--no-sign",
            "v1.0",
            "-m",
            "release tag metadata",
        ],
        &repo,
    );

    let patch = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(repo.clone()),
            source: DiffSource::Show("v1.0".to_owned()),
            include_untracked: false,
            ..DiffOptions::default()
        })
        .expect("show source patch should render"),
    )
    .expect("patch should be utf-8");

    assert!(patch.starts_with("diff --git a/base.txt b/base.txt"));
    assert!(!patch.contains("tag v1.0"));
    assert!(!patch.contains("release tag metadata"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn show_source_stat_preserves_valid_revspec() {
    let test_dir = temp_test_dir("show-revspec-stat");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "base\nnext\n").expect("file should change");
    git(["commit", "-q", "-am", "change"], &repo);

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(repo.clone()),
            source: DiffSource::Show("HEAD^!".to_owned()),
            include_untracked: false,
            stat: true,
            ..DiffOptions::default()
        })
        .expect("show source stats should render valid revspec"),
    )
    .expect("stat should be utf-8");

    assert!(stat.contains("base.txt"));
    assert!(stat.contains("1 files changed, 1 insertions(+), 0 deletions(-)"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn show_source_renders_merge_commit_as_parseable_parent_diffs() {
    let test_dir = temp_test_dir("show-merge-source");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    git(["checkout", "-q", "-b", "left"], &repo);
    fs::write(repo.join("base.txt"), "left\n").expect("left file should change");
    git(["commit", "-q", "-am", "left"], &repo);

    git(["checkout", "-q", "-b", "right", "HEAD~1"], &repo);
    fs::write(repo.join("base.txt"), "right\n").expect("right file should change");
    git(["commit", "-q", "-am", "right"], &repo);

    git(["checkout", "-q", "left"], &repo);
    let merge = Command::new("git")
        .current_dir(&repo)
        .args(["merge", "--no-ff", "right", "-m", "merge"])
        .output()
        .expect("git merge should run");
    assert!(!merge.status.success(), "merge should conflict");
    fs::write(repo.join("base.txt"), "merged\n").expect("merge should be resolved");
    git(["add", "base.txt"], &repo);
    git(["commit", "-q", "--no-edit"], &repo);

    let changeset = load(DiffOptions {
        repo: Some(repo.clone()),
        source: DiffSource::Show("HEAD".to_owned()),
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect("show source should load merge diff");

    assert_eq!(changeset.files.len(), 2);
    assert!(
        changeset.files.iter().all(|file| !file.hunks.is_empty()),
        "merge parent diffs should parse into hunks"
    );
    let raw_patch = String::from_utf8_lossy(&changeset.raw_patch);
    assert!(raw_patch.contains("diff --git a/base.txt b/base.txt"));
    assert!(!raw_patch.contains("diff --cc"));
    assert!(!raw_patch.contains("@@@"));

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(repo),
            source: DiffSource::Show("HEAD".to_owned()),
            include_untracked: false,
            stat: true,
            ..DiffOptions::default()
        })
        .expect("show source stats should render"),
    )
    .expect("stat should be utf-8");
    assert!(stat.contains("2 files changed, 2 insertions(+), 2 deletions(-)"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn render_bytes_preserves_non_utf8_git_diff_output() {
    let test_dir = temp_test_dir("non-utf8-diff");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    fs::write(repo.join("bytes.txt"), b"same\n\xff\n")
        .expect("non-UTF-8 base file should be written");
    git(["add", "bytes.txt"], &repo);
    git(["commit", "-q", "-m", "bytes"], &repo);
    fs::write(repo.join("bytes.txt"), b"same\n\xfe\n")
        .expect("non-UTF-8 worktree file should be written");

    let expected = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--end-of-options",
            "HEAD",
        ])
        .output()
        .expect("git diff should run");
    assert!(
        expected.status.success(),
        "git diff failed: {}",
        String::from_utf8_lossy(&expected.stderr)
    );
    assert!(expected.stdout.contains(&0xff));
    assert!(expected.stdout.contains(&0xfe));

    let actual = render_bytes(DiffOptions {
        repo: Some(repo.clone()),
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect("diff bytes should render");

    assert_eq!(actual, expected.stdout);
    let error = render(DiffOptions {
        repo: Some(repo),
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect_err("text rendering should reject non-UTF-8 output");
    assert!(error.to_string().contains("not valid UTF-8"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn patch_stdin_source_parses_stats_without_raw_patch_retention() {
    let patch = Arc::<[u8]>::from(
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+again\n".as_slice(),
        );
    let options = DiffOptions {
        source: DiffSource::Patch(PatchSource::Stdin(patch)),
        stat: true,
        ..DiffOptions::default()
    };

    let changeset = load_review_ref(&options).expect("patch source should parse");

    assert_eq!(changeset.files.len(), 1);
    assert_eq!(changeset.files[0].additions, 2);
    assert_eq!(changeset.files[0].deletions, 1);
    assert!(changeset.raw_patch.is_empty());
}

#[test]
fn patch_text_source_uses_label_title() {
    let patch = Arc::<[u8]>::from(
        b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
            .as_slice(),
    );
    let options = DiffOptions {
        source: DiffSource::Patch(PatchSource::Text {
            label: "github pr owner/repo#1".to_owned(),
            patch,
        }),
        ..DiffOptions::default()
    };

    let changeset = load_review_ref(&options).expect("patch source should parse");

    assert_eq!(changeset.title, "github pr owner/repo#1");
    assert_eq!(changeset.files.len(), 1);
}

#[test]
fn difftool_source_renders_file_pair_with_display_path() {
    let test_dir = temp_test_dir("difftool-file-pair");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("local.tmp"), "old\n").expect("left file should be written");
    fs::write(test_dir.join("remote.tmp"), "new\nnext\n").expect("right file should be written");

    let options = DiffOptions {
        repo: Some(test_dir.clone()),
        source: DiffSource::Difftool {
            left: PathBuf::from("local.tmp"),
            right: PathBuf::from("remote.tmp"),
            path: Some(PathBuf::from("src/example.rs")),
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = render(options.clone()).expect("difftool patch should render");
    assert!(patch.contains("diff --git a/src/example.rs b/src/example.rs"));
    assert!(patch.contains("--- a/src/example.rs"));
    assert!(patch.contains("+++ b/src/example.rs"));
    assert!(!patch.contains("local.tmp"));
    assert!(!patch.contains("remote.tmp"));

    let changeset = load_review_ref(&options).expect("difftool changeset should load");
    assert_eq!(changeset.title, "git difftool: src/example.rs");
    assert_eq!(changeset.files.len(), 1);
    assert_eq!(changeset.files[0].display_path(), "src/example.rs");
    assert_eq!(changeset.files[0].additions, 2);
    assert_eq!(changeset.files[0].deletions, 1);

    let stat = render(DiffOptions {
        stat: true,
        ..options
    })
    .expect("difftool stat should render");
    assert!(stat.contains("src/example.rs"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn difftool_path_rewrite_ignores_hunk_body_header_like_lines() {
    let patch = b"diff --git a/left b/right\n--- a/left\n+++ b/right\n@@ -1,2 +1,2 @@\n context\n--- deleted heading\n+++ added heading\n";

    let rewritten = String::from_utf8(rewrite_difftool_patch_paths(patch, "shown.txt"))
        .expect("rewritten patch should be utf-8");

    assert!(rewritten.contains("--- a/shown.txt"));
    assert!(rewritten.contains("+++ b/shown.txt"));
    assert!(rewritten.contains("--- deleted heading"));
    assert!(rewritten.contains("+++ added heading"));
}

#[test]
fn difftool_path_rewrite_preserves_non_utf8_hunk_bytes() {
    let patch = b"diff --git a/left b/right\n--- a/left\n+++ b/right\n@@ -1 +1 @@\n-\xff\n+\xfe\n";

    let rewritten = rewrite_difftool_patch_paths(patch, "shown.txt");

    assert!(rewritten.starts_with(b"diff --git a/shown.txt b/shown.txt\n"));
    assert!(rewritten.contains(&0xff));
    assert!(rewritten.contains(&0xfe));
    assert!(rewritten.windows(3).all(|window| window != b"\xef\xbf\xbd"));
}

#[test]
fn difftool_stat_counts_non_utf8_text_hunks() {
    let test_dir = temp_test_dir("difftool-stat-non-utf8");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("local.tmp"), b"same\n\xff\n").expect("left file should be written");
    fs::write(test_dir.join("remote.tmp"), b"same\n\xfe\n").expect("right file should be written");

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            repo: Some(test_dir.clone()),
            source: DiffSource::Difftool {
                left: PathBuf::from("local.tmp"),
                right: PathBuf::from("remote.tmp"),
                path: Some(PathBuf::from("bytes.txt")),
            },
            include_untracked: false,
            stat: true,
            ..DiffOptions::default()
        })
        .expect("difftool stat should render"),
    )
    .expect("stat should be utf-8");

    assert!(stat.contains("bytes.txt"));
    assert!(stat.contains("1 insertions(+)"));
    assert!(stat.contains("1 deletions(-)"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[cfg(unix)]
#[test]
fn difftool_source_drops_mode_only_temp_file_changes() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = temp_test_dir("difftool-mode-only");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("local.tmp"), "same\n").expect("left file should be written");
    fs::write(test_dir.join("remote.tmp"), "same\n").expect("right file should be written");
    fs::set_permissions(
        test_dir.join("local.tmp"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("left file mode should be set");
    fs::set_permissions(
        test_dir.join("remote.tmp"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("right file mode should be set");

    let options = DiffOptions {
        repo: Some(test_dir.clone()),
        source: DiffSource::Difftool {
            left: PathBuf::from("local.tmp"),
            right: PathBuf::from("remote.tmp"),
            path: Some(PathBuf::from("mode-only.txt")),
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = render_bytes(options.clone()).expect("patch should render");
    assert!(patch.is_empty());

    let stat = String::from_utf8(
        render_bytes(DiffOptions {
            stat: true,
            ..options.clone()
        })
        .expect("stat should render"),
    )
    .expect("stat should be utf-8");
    assert!(!stat.contains("mode-only.txt"));
    assert!(stat.contains("0 files changed"));

    let changeset = load_review_ref(&options).expect("difftool changeset should load");
    assert!(changeset.files.is_empty());

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[cfg(unix)]
#[test]
fn difftool_source_suppresses_temp_file_mode_changes() {
    use std::os::unix::fs::PermissionsExt;

    let test_dir = temp_test_dir("difftool-temp-mode");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("local.tmp"), "#!/bin/sh\necho old\n")
        .expect("left file should be written");
    fs::write(test_dir.join("remote.tmp"), "#!/bin/sh\necho new\n")
        .expect("right file should be written");
    fs::set_permissions(
        test_dir.join("local.tmp"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("left file mode should be set");
    fs::set_permissions(
        test_dir.join("remote.tmp"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("right file mode should be set");

    let options = DiffOptions {
        repo: Some(test_dir.clone()),
        source: DiffSource::Difftool {
            left: PathBuf::from("local.tmp"),
            right: PathBuf::from("remote.tmp"),
            path: Some(PathBuf::from("bin/script.sh")),
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = String::from_utf8(render_bytes(options.clone()).expect("patch should render"))
        .expect("patch should be utf-8");
    assert!(!patch.contains("old mode "));
    assert!(!patch.contains("new mode "));
    assert!(patch.contains("-echo old"));
    assert!(patch.contains("+echo new"));

    let changeset = load_review_ref(&options).expect("difftool changeset should load");
    assert_eq!(changeset.files[0].status, FileStatus::Modified);

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[cfg(unix)]
#[test]
fn difftool_source_uses_left_display_path_for_deleted_pair() {
    let test_dir = temp_test_dir("difftool-deleted-pair");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("old-name.txt"), "gone\n").expect("left file should be written");

    let options = DiffOptions {
        repo: Some(test_dir.clone()),
        source: DiffSource::Difftool {
            left: PathBuf::from("old-name.txt"),
            right: PathBuf::from("/dev/null"),
            path: None,
        },
        include_untracked: false,
        ..DiffOptions::default()
    };

    let patch = String::from_utf8(render_bytes(options.clone()).expect("patch should render"))
        .expect("patch should be utf-8");
    assert!(patch.contains("diff --git a/old-name.txt b/old-name.txt"));
    assert!(patch.contains("--- a/old-name.txt"));
    assert!(patch.contains("+++ /dev/null"));
    assert!(!patch.contains("a/null b/null"));

    let stat = render(DiffOptions {
        stat: true,
        ..options.clone()
    })
    .expect("stat should render");
    assert!(stat.contains("old-name.txt"));
    assert!(!stat.contains(" null"));

    let changeset = load_review_ref(&options).expect("difftool changeset should load");
    assert_eq!(changeset.files[0].display_path(), "old-name.txt");
    assert_eq!(changeset.files[0].status, FileStatus::Deleted);

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn difftool_source_rejects_missing_input_paths() {
    let test_dir = temp_test_dir("difftool-missing-input");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::write(test_dir.join("local.tmp"), "left\n").expect("left file should be written");

    let error = render_bytes(DiffOptions {
        repo: Some(test_dir.clone()),
        source: DiffSource::Difftool {
            left: PathBuf::from("local.tmp"),
            right: PathBuf::from("missing.tmp"),
            path: Some(PathBuf::from("src/example.rs")),
        },
        include_untracked: false,
        ..DiffOptions::default()
    })
    .expect_err("missing difftool input should fail");

    let message = error.to_string();
    assert!(message.contains("git difftool pair diff failed"));
    assert!(message.contains("Could not access"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn render_untracked_empty_and_noeol_files_as_applyable_patch() {
    let test_dir = temp_test_dir("untracked-exact");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    fs::write(repo.join("empty.txt"), "").expect("empty file should be written");
    fs::write(repo.join("noeol.txt"), "no newline").expect("noeol file should be written");

    git(
        [
            "clone",
            "-q",
            repo.to_str().unwrap(),
            destination.to_str().unwrap(),
        ],
        &test_dir,
    );
    let patch = render(DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    })
    .expect("diff should render");

    git_apply(&destination, patch.as_bytes());
    assert_eq!(fs::read(destination.join("empty.txt")).unwrap(), b"");
    assert_eq!(
        fs::read(destination.join("noeol.txt")).unwrap(),
        b"no newline"
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn render_unborn_head_worktree_diff_against_empty_tree() {
    let test_dir = temp_test_dir("unborn-head-diff");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    git(["init", "-q"], &repo);
    git(["config", "user.email", "test@example.com"], &repo);
    git(["config", "user.name", "Test"], &repo);
    fs::write(repo.join("new.txt"), "new\n").expect("new file should be written");

    let output = render(DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    })
    .expect("unborn HEAD diff should render");

    assert!(output.contains("diff --git a/new.txt b/new.txt"));
    assert!(output.contains("new file mode"));
    assert!(output.contains("+new"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn render_unborn_sha256_head_worktree_diff_against_empty_tree() {
    let test_dir = temp_test_dir("unborn-sha256-head-diff");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    fs::create_dir_all(&repo).expect("repo directory should be created");
    let init = Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q", "--object-format=sha256"])
        .output()
        .expect("git init should run");
    if !init.status.success() {
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
        return;
    }

    fs::write(repo.join("new.txt"), "new\n").expect("new file should be written");

    let output = render(DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    })
    .expect("unborn SHA-256 HEAD diff should render");

    assert!(output.contains("diff --git a/new.txt b/new.txt"));
    assert!(output.contains("new file mode"));
    assert!(output.contains("+new"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[cfg(unix)]
#[test]
fn render_untracked_symlink_as_symlink_without_reading_target() {
    let test_dir = temp_test_dir("untracked-symlink");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);

    fs::write(test_dir.join("secret.txt"), "outside secret\n")
        .expect("target file should be written");
    std::os::unix::fs::symlink("../secret.txt", repo.join("link.txt"))
        .expect("symlink should be created");

    git(
        [
            "clone",
            "-q",
            repo.to_str().unwrap(),
            destination.to_str().unwrap(),
        ],
        &test_dir,
    );
    let patch = render(DiffOptions {
        repo: Some(repo.clone()),
        ..DiffOptions::default()
    })
    .expect("diff should render");

    assert!(patch.contains("new file mode 120000"));
    assert!(patch.contains("+../secret.txt"));
    assert!(!patch.contains("outside secret"));

    git_apply(&destination, patch.as_bytes());
    let target = fs::read_link(destination.join("link.txt")).unwrap();
    assert_eq!(target, PathBuf::from("../secret.txt"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn revision_operands_cannot_be_reinterpreted_as_git_diff_options() {
    let test_dir = temp_test_dir("revision-option-boundary");
    let repo = test_dir.join("repo");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    init_repo(&repo);
    let output_path = test_dir.join("poc.diff");

    let result = render(DiffOptions {
        repo: Some(repo),
        source: DiffSource::Range {
            left: format!("--output={}", output_path.display()),
            right: "HEAD".to_owned(),
        },
        ..DiffOptions::default()
    });

    assert!(result.is_err());
    assert!(!output_path.exists());
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn temp_index_paths_are_adjacent_to_source_index() {
    let index = PathBuf::from("/repo/.git/worktrees/feature/index");
    let temp = temp_index_path(&index, 0).expect("temp index path should resolve");

    assert_eq!(temp.parent(), index.parent());
    assert!(
        temp.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".mark-diff-index-")
    );
}

#[cfg(unix)]
#[test]
fn stderr_capture_temp_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let stderr = StderrCapture::new().expect("stderr capture should be created");
    let path = stderr.path.clone();
    let mode = fs::metadata(&path)
        .expect("stderr capture should exist")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(mode, 0o600);
    stderr.discard();
    assert!(!path.exists());
}

#[test]
fn stderr_capture_drop_removes_temp_file() {
    let stderr = StderrCapture::new().expect("stderr capture should be created");
    let path = stderr.path.clone();

    assert!(path.exists());
    drop(stderr);
    assert!(!path.exists());
}

fn temp_test_dir(name: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "mark-diff-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}

fn init_repo(repo: &Path) {
    fs::create_dir_all(repo).expect("repo directory should be created");
    git(["init", "-q"], repo);
    git(["config", "user.email", "test@example.com"], repo);
    git(["config", "user.name", "Test"], repo);
    fs::write(repo.join("base.txt"), "base\n").expect("base file should be written");
    git(["add", "base.txt"], repo);
    git(["commit", "-q", "-m", "init"], repo);
}

fn git<const N: usize>(args: [&str; N], cwd: &Path) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_apply(repo: &Path, patch: &[u8]) {
    let mut child = Command::new("git")
        .current_dir(repo)
        .args(["apply", "--binary"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git apply should start");
    child
        .stdin
        .as_mut()
        .expect("stdin should be open")
        .write_all(patch)
        .expect("patch should be written");
    let output = child.wait_with_output().expect("git apply should finish");
    assert!(
        output.status.success(),
        "git apply failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N], cwd: &Path) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
