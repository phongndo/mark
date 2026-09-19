use super::*;
use std::collections::BTreeMap;

#[test]
fn parsers_match_git_stats_and_source_coordinates() {
    let repo = temp_test_dir("parser-git-coordinates");
    init_repo(&repo);
    git(["config", "core.autocrlf", "false"], &repo);
    let before: BTreeMap<&str, &[u8]> = BTreeMap::from([
        ("crlf.txt", b"first\r\nold\r\nlast\r\n".as_slice()),
        ("old\t\"é.txt", b"rename me\n".as_slice()),
        ("deleted-empty.txt", b"".as_slice()),
        ("blob.bin", b"\0old".as_slice()),
        ("noeol.txt", b"old without newline".as_slice()),
    ]);
    for (path, text) in &before {
        fs::write(repo.join(path), text).unwrap();
    }
    git(["add", "."], &repo);
    git(["commit", "-qm", "before"], &repo);
    for path in before.keys() {
        fs::remove_file(repo.join(path)).unwrap();
    }
    let after: BTreeMap<&str, &[u8]> = BTreeMap::from([
        ("crlf.txt", b"first\r\nnew\r\nextra\r\nlast\r\n".as_slice()),
        ("new path.txt", b"rename me\n".as_slice()),
        ("added-empty.txt", b"".as_slice()),
        ("blob.bin", b"\0new".as_slice()),
        ("noeol.txt", b"new without newline".as_slice()),
        ("added\t界.txt", "wide 界\ncombining e\u{301}".as_bytes()),
    ]);
    for (path, text) in &after {
        fs::write(repo.join(path), text).unwrap();
    }
    git(["add", "-A"], &repo);
    let output = |format| {
        let output = Command::new("git")
            .current_dir(&repo)
            .args([
                "-c",
                "core.quotePath=true",
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-color",
                "-M",
                format,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let patch = output("--binary");
    // Numstat's filename contract is NUL-delimited (including rename pairs).
    let numstat = Command::new("git")
        .current_dir(&repo)
        .args(["diff", "--cached", "--numstat", "-z", "-M"])
        .output()
        .unwrap();
    assert!(numstat.status.success());
    let stats = parse_numstat(numstat.stdout.as_slice()).unwrap();
    let patch_text = std::str::from_utf8(&patch).unwrap();
    for files in [
        parse_patch(patch_text),
        parse_patch_bytes(Arc::from(patch.clone())),
    ] {
        assert_eq!(files.len(), stats.files.len());
        for file in files {
            let stat = stats
                .files
                .iter()
                .find(|stat| stat.display_path() == file.display_path())
                .unwrap();
            assert_eq!(
                (file.additions, file.deletions, file.is_binary()),
                (stat.additions, stat.deletions, stat.is_binary())
            );
            for line in file.hunks().iter().flat_map(|hunk| &hunk.lines) {
                if let Some(number) = line.old_line() {
                    let source = before[file.old_path().unwrap()]
                        .split(|byte| *byte == b'\n')
                        .nth(number - 1)
                        .unwrap();
                    assert_eq!(
                        line.text_bytes(),
                        source,
                        "old side {}:{number}",
                        file.display_path()
                    );
                }
                if let Some(number) = line.new_line() {
                    let source = after[file.new_path().unwrap()]
                        .split(|byte| *byte == b'\n')
                        .nth(number - 1)
                        .unwrap();
                    assert_eq!(
                        line.text_bytes(),
                        source,
                        "new side {}:{number}",
                        file.display_path()
                    );
                }
            }
        }
    }
    fs::remove_dir_all(repo).unwrap();
}
