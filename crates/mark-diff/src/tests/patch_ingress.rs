use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct CountingReader {
    bytes: Arc<[u8]>,
    offset: usize,
    consumed: Arc<AtomicUsize>,
}

impl std::io::Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let available = &self.bytes[self.offset..];
        let read = available.len().min(buffer.len());
        buffer[..read].copy_from_slice(&available[..read]);
        self.offset += read;
        self.consumed.fetch_add(read, Ordering::Relaxed);
        Ok(read)
    }
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
fn patch_input_limit_stops_reading_after_the_first_excess_byte() {
    let consumed = Arc::new(AtomicUsize::new(0));
    let reader = CountingReader {
        bytes: Arc::from(&b"0123456789abcdef"[..]),
        offset: 0,
        consumed: Arc::clone(&consumed),
    };

    let error = read_patch_input_limited(reader, Some(4)).unwrap_err();

    assert!(error.to_string().contains("patch bytes limit: 5 > 4"));
    assert_eq!(consumed.load(Ordering::Relaxed), 5);
}

#[test]
fn streaming_patch_limit_stops_at_the_first_excess_byte() {
    let consumed = Arc::new(AtomicUsize::new(0));
    let reader = CountingReader {
        bytes: Arc::from(&b"0123456789abcdef"[..]),
        offset: 0,
        consumed: Arc::clone(&consumed),
    };
    let mut output = Vec::new();

    let error = copy_to_writer_limited(reader, &mut output, Some(4)).unwrap_err();

    assert!(error.to_string().contains("patch bytes limit: 5 > 4"));
    assert_eq!(output, b"0123");
    assert_eq!(consumed.load(Ordering::Relaxed), 5);
}

#[test]
fn patch_file_limit_is_enforced_during_ingress() {
    let test_dir = temp_test_dir("patch-file-limit");
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    let patch_path = test_dir.join("change.diff");
    fs::write(&patch_path, b"0123456789abcdef").expect("patch file should be written");
    let options = DiffOptions {
        source: DiffSource::Patch(PatchSource::File(patch_path)),
        ..DiffOptions::default()
    };

    let error = load_review_ref_limited(
        &options,
        DiffLimits {
            max_patch_bytes: Some(4),
            ..DiffLimits::default()
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("patch bytes limit: 5 > 4"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn git_numstat_limit_stops_captured_output() {
    let test_dir = temp_test_dir("git-numstat-limit");
    let repo = test_dir.join("repo");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "changed\n").expect("worktree should change");
    let args = vec!["diff".to_owned(), "--numstat".to_owned(), "-z".to_owned()];

    let error = crate::git_io::git_numstat_stats(&repo, &args, Some(4)).unwrap_err();

    assert!(error.to_string().contains("patch bytes limit: 5 > 4"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn git_diff_limit_stops_captured_output() {
    let test_dir = temp_test_dir("git-diff-limit");
    let repo = test_dir.join("repo");
    init_repo(&repo);
    fs::write(repo.join("base.txt"), "changed\n").expect("worktree should change");
    let options = DiffOptions {
        repo: Some(repo.into()),
        local_untracked: crate::UntrackedMode::Exclude,
        ..DiffOptions::default()
    };

    let error = load_review_ref_limited(
        &options,
        DiffLimits {
            max_patch_bytes: Some(8),
            ..DiffLimits::default()
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("patch bytes limit: 9 > 8"));
    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}
