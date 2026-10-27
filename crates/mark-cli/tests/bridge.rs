use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repository = root.join("repo");
        fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "--quiet"]);
        fs::write(repository.join("change.txt"), "before\n").unwrap();
        git(&repository, &["add", "change.txt"]);
        git(
            &repository,
            &[
                "-c",
                "user.name=Mark tests",
                "-c",
                "user.email=mark-tests@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "base",
            ],
        );
        fs::write(repository.join("change.txt"), "after\n").unwrap();
        Self {
            _temp: temp,
            root,
            repository,
        }
    }

    fn command(&self) -> Command {
        self.command_at(&self.repository)
    }

    fn command_at(&self, repository: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mark"));
        command
            .current_dir(repository)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        for key in [
            "MARK_HARNESS",
            "MARK_CONVERSATION_ID",
            "CODEX_THREAD_ID",
            "CLAUDE_SESSION_ID",
            "PI_SESSION_ID",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
        ] {
            command.env_remove(key);
        }
        command
    }

    fn call_output(&self, method: &str, params: Value) -> Output {
        let mut child = self
            .command()
            .args(["bridge", "call", method, "--stdin", "--json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        serde_json::to_writer(&mut stdin, &params).unwrap();
        drop(stdin);
        child.wait_with_output().unwrap()
    }

    fn call(&self, method: &str, params: Value) -> Value {
        result(self.call_output(method, params))
    }

    fn open(&self, conversation: &str) -> Value {
        self.call(
            "review.open",
            json!({
                "harness": "codex", "conversation": conversation, "repository": self.repository,
                "source": {"kind": "worktree"}
            }),
        )
    }
}

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repository)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn result(output: Output) -> Value {
    assert!(
        output.status.success(),
        "mark failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["ok"], true, "{envelope}");
    envelope["result"].clone()
}

fn mutation(review: &Value, mutation_id: &str) -> Value {
    json!({"review_id": review["review_id"], "generation": review["generation"], "client_mutation_id": mutation_id})
}

#[test]
fn bridge_resumes_identity_and_refreshes_from_git_without_accepting_forged_patch() {
    let fixture = Fixture::new();
    fs::write(fixture.repository.join("new.txt"), "untracked contents\n").unwrap();
    let opened = fixture.open("conversation-1");
    let patch = opened["snapshot"]["patch"].as_str().unwrap();
    assert!(patch.contains("+after"), "{patch}");
    assert!(patch.contains("untracked contents"), "{patch}");
    fs::write(fixture.repository.join("change.txt"), "next pass\n").unwrap();
    let saved = fixture.call("review.get", json!({"review_id": opened["review_id"]}));
    assert_eq!(saved["snapshot"], opened["snapshot"]);
    let mut refresh = mutation(&opened, "refresh-1");
    refresh["patch"] = json!("forged snapshot evidence");
    let refreshed = fixture.call("review.refresh", refresh);
    assert_eq!(
        refreshed["generation"],
        opened["generation"].as_u64().unwrap() + 1
    );
    let current = fixture.call("review.get", json!({"review_id": opened["review_id"]}));
    assert!(
        current["snapshot"]["patch"]
            .as_str()
            .unwrap()
            .contains("+next pass")
    );
    assert!(
        !current["snapshot"]["patch"]
            .as_str()
            .unwrap()
            .contains("forged snapshot")
    );
    let resumed = fixture.open("conversation-1");
    assert_eq!(resumed["review_id"], opened["review_id"]);
    assert_eq!(resumed["generation"], current["generation"]);
    assert_eq!(resumed["snapshot"], current["snapshot"]);
    assert!(resumed["last_return"].is_null());
    fs::write(fixture.repository.join("change.txt"), "another pass\n").unwrap();
    let reopened = fixture.open("conversation-1");
    assert_eq!(reopened["review_id"], opened["review_id"]);
    assert_eq!(
        reopened["generation"],
        current["generation"].as_u64().unwrap() + 1
    );
    assert!(
        reopened["snapshot"]["patch"]
            .as_str()
            .unwrap()
            .contains("+another pass")
    );
}

#[test]
fn refresh_and_reopen_reanchor_threads_preserve_unchanged_progress_and_reconcile_retries() {
    let fixture = Fixture::new();
    fs::write(fixture.repository.join("moving.txt"), "top\nold\nbottom\n").unwrap();
    fs::write(fixture.repository.join("stable.txt"), "old stable\n").unwrap();
    git(&fixture.repository, &["add", "moving.txt", "stable.txt"]);
    git(
        &fixture.repository,
        &[
            "-c",
            "user.name=Mark tests",
            "-c",
            "user.email=mark-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "review fixtures",
        ],
    );
    fs::write(
        fixture.repository.join("moving.txt"),
        "top\ntarget\nbottom\n",
    )
    .unwrap();
    fs::write(fixture.repository.join("stable.txt"), "stable changed\n").unwrap();
    let opened = fixture.open("reanchoring");
    let mut moving = mutation(&opened, "moving-note");
    moving["anchor"] = json!({"file": "moving.txt", "new_line": 2});
    moving["body"] = json!("Verify the target");
    moving["origin"] = json!("human");
    let moving = fixture.call("comment.add", moving);
    let mut stable = mutation(&opened, "stable-note");
    stable["anchor"] = json!({"file": "stable.txt", "new_line": 1});
    stable["body"] = json!("Keep the stable finding");
    let stable = fixture.call("comment.add", stable);
    let mut progress = mutation(&opened, "reviewed-progress");
    progress["reviewed_files"] = json!(["stable.txt", "moving.txt"]);
    progress["reviewed_hunks"] = json!([["stable.txt", 1]]);
    progress["verdict"] = json!({"kind": "approve"});
    fixture.call("review.progress", progress);

    fs::write(
        fixture.repository.join("moving.txt"),
        "inserted\ntop\ntarget\nbottom\n",
    )
    .unwrap();
    let refresh = mutation(&opened, "reanchor-refresh");
    let responses = mcp(
        &fixture,
        &mcp_connection(vec![tool(1, "review_refresh", refresh.clone())]),
    );
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    let refreshed = responses[1]["result"]["structuredContent"].clone();
    assert_eq!(refreshed["changed"], true);
    let current = fixture.call("review.get", json!({"review_id": opened["review_id"]}));
    let find = |record: &Value, id: &Value| {
        record["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| &thread["thread_id"] == id)
            .cloned()
            .unwrap()
    };
    let moved = find(&current, &moving["thread"]["thread_id"]);
    assert_eq!(moved["anchor"]["new_line"], 3, "{moved}");
    assert_eq!(moved["stale"], false);
    assert_eq!(
        moved["messages"][0]["message_id"],
        moving["message"]["message_id"]
    );
    assert_eq!(moved["messages"][0]["body"], "Verify the target");
    let untouched = find(&current, &stable["thread"]["thread_id"]);
    assert_eq!(untouched["anchor"]["new_line"], 1);
    assert_eq!(untouched["stale"], false);
    assert_eq!(untouched["messages"][0]["body"], "Keep the stable finding");
    assert_eq!(current["progress"]["reviewed_files"], json!(["stable.txt"]));
    assert_eq!(
        current["progress"]["reviewed_hunks"],
        json!([["stable.txt", 1]])
    );
    assert!(current["progress"]["verdict"].is_null());
    // A tool's retry input is unchanged even if the agent has edited the
    // worktree since the first response was lost. Reconcile that first action
    // before explicitly opening another pass over the newer source bytes.
    fs::write(
        fixture.repository.join("moving.txt"),
        "another insertion\ninserted\ntop\ntarget\nbottom\n",
    )
    .unwrap();
    let retried = fixture.call("review.refresh", refresh);
    assert_eq!(retried, refreshed);
    let after_retry = fixture.call("review.get", json!({"review_id": opened["review_id"]}));
    assert_eq!(
        after_retry, current,
        "an uncertain retry must not mutate the reconciled review again"
    );

    let reopened = fixture.open("reanchoring");
    assert_eq!(reopened["review_id"], opened["review_id"]);
    assert_eq!(
        reopened["generation"],
        current["generation"].as_u64().unwrap() + 1
    );
    let moved = find(&reopened, &moving["thread"]["thread_id"]);
    assert_eq!(moved["anchor"]["new_line"], 4, "{moved}");
    assert_eq!(moved["stale"], false);
    assert_eq!(moved["messages"][0]["body"], "Verify the target");
    assert_eq!(
        find(&reopened, &stable["thread"]["thread_id"])["messages"],
        untouched["messages"]
    );
    assert_eq!(reopened["progress"], current["progress"]);
    let repeated = fixture.open("reanchoring");
    assert_eq!(repeated["generation"], reopened["generation"]);
    assert_eq!(repeated["threads"], reopened["threads"]);
    assert_eq!(repeated["progress"], reopened["progress"]);
}

#[test]
fn explicit_sources_conversations_and_git_worktrees_have_distinct_reviews() {
    let fixture = Fixture::new();
    let first = fixture.open("one");
    let other_conversation = fixture.open("two");
    assert_ne!(first["review_id"], other_conversation["review_id"]);
    let base = fixture.call(
        "review.open",
        json!({
            "harness": "codex", "conversation": "one", "repository": fixture.repository,
            "source": {"kind": "base", "base": "HEAD"}
        }),
    );
    assert_ne!(first["review_id"], base["review_id"]);
    assert!(
        base["snapshot"]["patch"]
            .as_str()
            .unwrap()
            .contains("+after")
    );
    let worktree = fixture.root.join("other-worktree");
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "other",
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("change.txt"), "worktree-specific\n").unwrap();
    let separate = fixture.call(
        "review.open",
        json!({
            "harness": "codex", "conversation": "one", "repository": worktree,
            "source": {"kind": "worktree"}
        }),
    );
    assert_ne!(first["review_id"], separate["review_id"]);
    assert_eq!(
        separate["binding"]["repository"],
        json!(worktree.canonicalize().unwrap())
    );
    assert!(
        separate["snapshot"]["patch"]
            .as_str()
            .unwrap()
            .contains("worktree-specific")
    );
}

#[test]
fn show_range_patch_and_difftool_sources_load_their_own_evidence() {
    let fixture = Fixture::new();
    git(&fixture.repository, &["add", "change.txt"]);
    git(
        &fixture.repository,
        &[
            "-c",
            "user.name=Mark tests",
            "-c",
            "user.email=mark-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "second",
        ],
    );
    fs::write(fixture.repository.join("change.txt"), "uncommitted\n").unwrap();
    for source in [
        json!({"kind": "show", "rev": "HEAD"}),
        json!({"kind": "range", "left": "HEAD~1", "right": "HEAD"}),
    ] {
        let review = fixture.call("review.open", json!({"harness": "codex", "conversation": "source-specific", "repository": fixture.repository, "source": source}));
        let patch = review["snapshot"]["patch"].as_str().unwrap();
        assert!(
            patch.contains("-before") && patch.contains("+after"),
            "{patch}"
        );
        assert!(!patch.contains("+uncommitted"), "{patch}");
    }
    let saved_patch = git(&fixture.repository, &["diff", "--binary", "HEAD"]);
    let patch_file = fixture.root.join("saved.patch");
    fs::write(&patch_file, saved_patch).unwrap();
    fs::write(fixture.repository.join("change.txt"), "newer worktree\n").unwrap();
    let review = fixture.call("review.open", json!({"harness": "codex", "conversation": "source-specific", "repository": fixture.repository, "source": {"kind": "patch", "path": patch_file}}));
    let patch = review["snapshot"]["patch"].as_str().unwrap();
    assert!(patch.contains("+uncommitted"), "{patch}");
    assert!(!patch.contains("+newer worktree"), "{patch}");
    let left = fixture.root.join("left.txt");
    let right = fixture.root.join("right.txt");
    fs::write(&left, "left side\n").unwrap();
    fs::write(&right, "right side\n").unwrap();
    let review = fixture.call("review.open", json!({"harness": "codex", "conversation": "source-specific", "repository": fixture.repository, "source": {"kind": "difftool", "left": left, "right": right, "path": "comparison.txt"}}));
    let patch = review["snapshot"]["patch"].as_str().unwrap();
    assert!(
        patch.contains("-left side") && patch.contains("+right side"),
        "{patch}"
    );
}

#[test]
fn bridge_uses_native_conversation_environment_and_refuses_missing_identity() {
    let fixture = Fixture::new();
    let mut command = fixture.command();
    let mut child = command
        .args(["bridge", "call", "review.open", "--stdin", "--json"])
        .env("CODEX_THREAD_ID", "native-thread")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{}").unwrap();
    let review = result(child.wait_with_output().unwrap());
    assert_eq!(review["binding"]["harness"], "codex");
    assert_eq!(review["binding"]["conversation"], "native-thread");
    let missing = fixture.call_output("review.open", json!({}));
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stdout).contains("missing_binding"));
}

#[test]
fn threaded_human_feedback_is_submitted_once_and_acknowledged_per_consumer() {
    let fixture = Fixture::new();
    let opened = fixture.open("feedback");
    let mut add = mutation(&opened, "agent-finding");
    add["anchor"] = json!({"file": "change.txt", "new_line": 1});
    add["body"] = json!("Agent question");
    let finding = fixture.call("comment.add", add);
    let mut reply = mutation(&opened, "human-reply");
    reply["thread_id"] = finding["thread"]["thread_id"].clone();
    reply["reply_to"] = finding["message"]["message_id"].clone();
    reply["body"] = json!("Please preserve errors");
    reply["origin"] = json!("human");
    let human = fixture.call("comment.reply", reply.clone());
    let retried = fixture.call("comment.reply", reply);
    assert_eq!(
        retried["message"]["message_id"],
        human["message"]["message_id"]
    );
    let before_submit = fixture.call("feedback.get", json!({"review_id": opened["review_id"]}));
    assert!(before_submit["submissions"].as_array().unwrap().is_empty());
    let submitted = fixture.call("feedback.submit", mutation(&opened, "submit"));
    let submission_id = submitted["submission"]["submission_id"].clone();
    let feedback = fixture.call(
        "feedback.get",
        json!({"review_id": opened["review_id"], "consumer": "codex"}),
    );
    assert_eq!(feedback["submissions"][0]["submission_id"], submission_id);
    let mut ack = mutation(&opened, "ack");
    ack["submission_id"] = submission_id;
    ack["consumer"] = json!("codex");
    let first_ack = fixture.call("feedback.ack", ack.clone());
    let retry_ack = fixture.call("feedback.ack", ack);
    assert_eq!(first_ack, retry_ack);
    let acknowledged = fixture.call(
        "feedback.get",
        json!({"review_id": opened["review_id"], "consumer": "codex"}),
    );
    assert!(acknowledged["submissions"].as_array().unwrap().is_empty());
    let other_consumer = fixture.call(
        "feedback.get",
        json!({"review_id": opened["review_id"], "consumer": "pi"}),
    );
    assert_eq!(other_consumer["submissions"].as_array().unwrap().len(), 1);
}

#[test]
fn native_acknowledgement_matches_the_default_mcp_consumer() {
    let fixture = Fixture::new();
    let opened = fixture.open("mcp-thread");
    let mut add = mutation(&opened, "human-note");
    add["anchor"] = json!({"file": "change.txt", "new_line": 1});
    add["body"] = json!("Native submitted feedback");
    add["origin"] = json!("human");
    fixture.call("comment.add", add);
    let submitted = fixture.call("feedback.submit", mutation(&opened, "native-submit"));
    let initial = mcp(
        &fixture,
        &mcp_connection(vec![tool(
            1,
            "feedback_get",
            json!({"review_id": opened["review_id"]}),
        )]),
    );
    assert_eq!(
        initial[1]["result"]["structuredContent"]["submissions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut acknowledgement = mutation(&opened, "native-ack");
    acknowledgement["submission_id"] = submitted["submission"]["submission_id"].clone();
    acknowledgement["consumer"] = json!("codex:mcp-thread");
    fixture.call("feedback.ack", acknowledgement);
    let responses = mcp(
        &fixture,
        &mcp_connection(vec![
            tool(1, "feedback_get", json!({"review_id": opened["review_id"]})),
            tool(
                2,
                "feedback_get",
                json!({"review_id": opened["review_id"], "consumer": "another-consumer"}),
            ),
        ]),
    );
    assert!(
        responses[1]["result"]["structuredContent"]["submissions"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the same conversation must not receive an acknowledged batch again"
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["submissions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn comment_add_rejects_targets_absent_from_the_saved_snapshot() {
    let fixture = Fixture::new();
    let opened = fixture.open("invalid-anchors");
    let anchors = vec![
        json!({"file": "missing.txt"}),
        json!({"file": "change.txt", "new_line": 999}),
        json!({"file": "change.txt", "old_line": 999}),
        json!({"file": "change.txt", "hunk": 2}),
        json!({"file": "change.txt", "hunk": 0}),
        json!({"file": "change.txt", "new_line": 0}),
        json!({"file": "change.txt", "scope": "file", "new_line": 1}),
        json!({"file": "change.txt", "old_line": 1, "new_line": 1}),
        json!({"file": "change.txt", "range": {}}),
        json!({"file": "change.txt", "range": {"new": {"start": 1, "end": 2}}}),
        json!({"file": "change.txt", "range": {"new": {"start": 2, "end": 1}}}),
        json!({"file": "change.txt", "new_line": null}),
        json!({"file": "change.txt", "new_line": "1"}),
        json!({"file": "change.txt", "line": 1}),
    ];
    for (index, anchor) in anchors.into_iter().enumerate() {
        let mut params = mutation(&opened, &format!("invalid-{index}"));
        params["anchor"] = anchor.clone();
        params["body"] = json!("Do not publish this finding");
        let output = fixture.call_output("comment.add", params);
        assert!(!output.status.success(), "accepted invalid anchor {anchor}");
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        let message = envelope["error"]["message"].as_str().unwrap();
        assert!(
            message.starts_with("anchor_not_found:") || message.starts_with("invalid_anchor:"),
            "{message}"
        );
    }
    let mut params = mutation(&opened, "mcp-out-of-snapshot");
    params["anchor"] = json!({"file": "change.txt", "new_line": 999});
    params["body"] = json!("MCP finding on an absent line");
    let responses = mcp(
        &fixture,
        &mcp_connection(vec![tool(1, "comment_add", params)]),
    );
    assert_eq!(responses[1]["result"]["isError"], true);
    assert!(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("anchor_not_found:")
    );
    assert!(
        fixture.call("review.get", json!({"review_id": opened["review_id"]}))["threads"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn comment_add_preserves_valid_renamed_deleted_unicode_hunk_and_range_anchors() {
    let fixture = Fixture::new();
    let patch = "diff --git a/old.rs b/new.rs\nsimilarity index 50%\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,3 +1,4 @@\n+inserted\n context\n-old\n+target\n tail\ndiff --git a/gone-é.txt b/gone-é.txt\ndeleted file mode 100644\n--- a/gone-é.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-before one\n-before two\ndiff --git \"a/é unicode.txt\" \"b/é unicode.txt\"\n--- \"a/é unicode.txt\"\n+++ \"b/é unicode.txt\"\n@@ -1 +1 @@\n-before unicode\n+after unicode\ndiff --git a/binary.dat b/binary.dat\nBinary files a/binary.dat and b/binary.dat differ\n";
    let patch_path = fixture.root.join("anchors.patch");
    fs::write(&patch_path, patch).unwrap();
    let opened = fixture.call("review.open", json!({"harness": "codex", "conversation": "positive-anchors", "repository": fixture.repository, "source": {"kind": "patch", "path": patch_path}}));
    for (index, anchor) in [
        json!({"file": "old.rs", "old_line": 2}),
        json!({"file": "new.rs", "new_line": 3}),
        json!({"file": "old.rs", "hunk": 1}),
        json!({"file": "new.rs", "range": {"old": {"start": 1, "end": 3}, "new": {"start": 1, "end": 4}}}),
        json!({"file": "gone-é.txt", "old_line": 2}),
        json!({"file": "é unicode.txt", "new_line": 1}),
        json!({"file": "new.rs"}),
        json!({"file": "gone-é.txt", "scope": "file"}),
        json!({"file": "binary.dat", "scope": "file"}),
    ].into_iter().enumerate() {
        let mut params = mutation(&opened, &format!("valid-{index}"));
        params["anchor"] = anchor.clone();
        params["body"] = json!(format!("Valid finding {index}"));
        let comment = fixture.call("comment.add", params);
        assert_eq!(comment["thread"]["anchor"], anchor, "valid anchor must remain complete");
    }
    let mut invalid_side = mutation(&opened, "deleted-new-line");
    invalid_side["anchor"] = json!({"file": "gone-é.txt", "new_line": 1});
    invalid_side["body"] = json!("Deleted files have no new-side source lines");
    let output = fixture.call_output("comment.add", invalid_side);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("anchor_not_found"));
}

#[test]
fn stale_comment_generation_is_rejected_before_current_snapshot_anchor_validation() {
    let fixture = Fixture::new();
    let opened = fixture.open("stale-anchor");
    fs::write(fixture.repository.join("change.txt"), "new generation\n").unwrap();
    fixture.call("review.refresh", mutation(&opened, "refresh"));
    let mut stale = mutation(&opened, "stale-comment");
    stale["anchor"] = json!({"file": "not-in-current-snapshot.txt", "new_line": 999});
    stale["body"] = json!("Wrong generation");
    let output = fixture.call_output("comment.add", stale);
    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("stale_generation:")
    );
}

fn mcp(fixture: &Fixture, messages: &[Value]) -> Vec<Value> {
    let mut child = fixture
        .command()
        .args([
            "mcp",
            "--harness",
            "codex",
            "--conversation",
            "mcp-thread",
            "--repo",
        ])
        .arg(&fixture.repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in messages {
        serde_json::to_writer(&mut stdin, message).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("stdout must contain only MCP frames"))
        .collect()
}

fn tool(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn mcp_connection(messages: Vec<Value>) -> Vec<Value> {
    let mut setup = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "integration-test", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    setup.extend(messages);
    setup
}

#[test]
fn mcp_binary_binds_the_real_review_and_rejects_human_authority() {
    let fixture = Fixture::new();
    let opened = fixture.open("mcp-thread");
    let responses = mcp(
        &fixture,
        &mcp_connection(vec![
            tool(1, "review_open", json!({})),
            tool(
                2,
                "review_patch",
                json!({"review_id": opened["review_id"], "max_bytes": 32}),
            ),
            tool(
                3,
                "comment_add",
                json!({"review_id": opened["review_id"], "generation": opened["generation"], "client_mutation_id": "mcp-comment", "anchor": {"file": "change.txt", "new_line": 1}, "body": "Agent finding"}),
            ),
            tool(
                4,
                "comment_add",
                json!({"review_id": opened["review_id"], "generation": opened["generation"], "client_mutation_id": "spoof", "anchor": {"file": "change.txt", "new_line": 1}, "body": "Spoof", "origin": "human"}),
            ),
            tool(
                5,
                "feedback_submit",
                json!({"review_id": opened["review_id"]}),
            ),
        ]),
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["review_id"],
        opened["review_id"]
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["binding"]["conversation"],
        "mcp-thread"
    );
    assert!(
        responses[1]["result"]["structuredContent"]["snapshot"]
            .get("patch")
            .is_none()
    );
    assert!(
        responses[2]["result"]["structuredContent"]["patch"]
            .as_str()
            .unwrap()
            .len()
            <= 32
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["message"]["origin"],
        "agent"
    );
    assert_eq!(responses[4]["error"]["code"], -32602);
    assert_eq!(responses[5]["error"]["code"], -32602);
    let stored = fixture.call("review.get", json!({"review_id": opened["review_id"]}));
    assert_eq!(stored["threads"].as_array().unwrap().len(), 1);
}

#[test]
fn mcp_reads_submitted_snapshot_without_exposing_later_unsent_edits() {
    let fixture = Fixture::new();
    let opened = fixture.open("privacy");
    let mut add = mutation(&opened, "human-add");
    add["anchor"] = json!({"file": "change.txt", "new_line": 1});
    add["body"] = json!("submitted-content");
    add["origin"] = json!("human");
    let human = fixture.call("comment.add", add);
    fixture.call("feedback.submit", mutation(&opened, "submit"));
    let mut edit = mutation(&opened, "edit");
    edit["thread_id"] = human["thread"]["thread_id"].clone();
    edit["message_id"] = human["message"]["message_id"].clone();
    edit["body"] = json!("private-unsent-edit");
    fixture.call("comment.edit", edit);
    let responses = mcp(
        &fixture,
        &mcp_connection(vec![
            tool(1, "review_get", json!({"review_id": opened["review_id"]})),
            tool(2, "feedback_get", json!({"review_id": opened["review_id"]})),
        ]),
    );
    let encoded = serde_json::to_string(&responses).unwrap();
    assert!(!encoded.contains("private-unsent-edit"));
    assert!(
        responses[2]["result"]["structuredContent"]
            .to_string()
            .contains("submitted-content")
    );
}

struct WebServer {
    child: Option<Child>,
    detached_process: Option<u32>,
    address: String,
    prefix: String,
}

impl WebServer {
    fn start(fixture: &Fixture, review: &Value) -> Self {
        let ready = fixture.root.join("viewer-ready.json");
        let mut child = fixture
            .command()
            .args(["bridge", "web-serve"])
            .arg(review["review_id"].as_str().unwrap())
            .arg("--ready-file")
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(bytes) = fs::read(&ready)
                && let Ok(metadata) = serde_json::from_slice::<Value>(&bytes)
            {
                assert!(metadata["error"].is_null(), "{metadata}");
                let url = metadata["url"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("http://")
                    .unwrap();
                let (address, token) = url.split_once('/').unwrap();
                assert_eq!(token.trim_end_matches('/').len(), 64);
                assert_eq!(metadata["review_id"], review["review_id"]);
                return Self {
                    child: Some(child),
                    detached_process: None,
                    address: address.to_owned(),
                    prefix: format!("/{token}"),
                };
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("viewer exited before startup: {status}");
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("viewer did not start");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn launch(fixture: &Fixture, review: &Value) -> Self {
        let metadata = result(
            fixture
                .command()
                .args(["bridge", "web"])
                .arg(review["review_id"].as_str().unwrap())
                .arg("--json")
                .output()
                .unwrap(),
        );
        let url = metadata["url"]
            .as_str()
            .unwrap()
            .strip_prefix("http://")
            .unwrap();
        let (address, token) = url.split_once('/').unwrap();
        Self {
            child: None,
            detached_process: Some(metadata["process_id"].as_u64().unwrap().try_into().unwrap()),
            address: address.to_owned(),
            prefix: format!("/{token}"),
        }
    }

    fn request(
        &self,
        method: &str,
        suffix: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "{method} {}{suffix} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n",
            self.prefix,
            self.address,
            body.len()
        )
        .unwrap();
        for (key, value) in headers {
            write!(stream, "{key}: {value}\r\n").unwrap();
        }
        stream.write_all(b"\r\n").unwrap();
        stream.write_all(body).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        response(&mut stream)
    }

    fn api(&self, method: &str, params: Value) -> Value {
        let body = serde_json::to_vec(&json!({"method": method, "params": params})).unwrap();
        let (status, body) = self.request(
            "POST",
            "api",
            &[("Content-Type", "application/json")],
            &body,
        );
        let envelope: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, 200, "{envelope}");
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(process_id) = self.detached_process {
            // `bridge web` deliberately detaches its own child. Terminate only
            // the specific viewer this test launched; do not leave a one-hour
            // background server behind after the fixture is removed.
            let _ = Command::new("kill")
                .args(["-TERM", &process_id.to_string()])
                .output();
        }
    }
}

fn response(stream: &mut TcpStream) -> (u16, Vec<u8>) {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let header_end = bytes
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap()
        + 4;
    let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, bytes[header_end..].to_vec())
}

#[test]
fn local_viewer_records_human_feedback_and_returns_it_to_same_review() {
    let fixture = Fixture::new();
    let opened = fixture.open("browser");
    let other = fixture.open("other-browser");
    let viewer = WebServer::start(&fixture, &opened);
    let (status, html) = viewer.request("GET", "", &[], &[]);
    assert_eq!(status, 200);
    assert!(String::from_utf8(html).unwrap().contains("Send feedback"));
    let mut add = mutation(&opened, "browser-note");
    // The capability URL pins the original review even if a caller supplies a different ID.
    add["review_id"] = other["review_id"].clone();
    add["anchor"] = json!({"file": "change.txt", "new_line": 1});
    add["body"] = json!("Human browser request");
    let note = viewer.api("comment.add", add);
    assert_eq!(note["review_id"], opened["review_id"]);
    assert_eq!(note["message"]["origin"], "human");
    let untouched = fixture.call("review.get", json!({"review_id": other["review_id"]}));
    assert!(untouched["threads"].as_array().unwrap().is_empty());
    let submitted = viewer.api("feedback.submit", mutation(&opened, "browser-submit"));
    let mut finish = mutation(&opened, "browser-finish");
    finish["outcome"] = json!("submitted");
    finish["submission_ids"] = json!([submitted["submission"]["submission_id"]]);
    viewer.api("review.finish", finish);
    let feedback = fixture.call(
        "feedback.wait",
        json!({"review_id": opened["review_id"], "timeout_ms": 10}),
    );
    assert_eq!(feedback["timed_out"], false);
    assert_eq!(feedback["last_return"]["outcome"], "submitted");
    assert_eq!(
        feedback["last_return"]["submission_ids"][0],
        submitted["submission"]["submission_id"]
    );
    assert!(feedback.to_string().contains("Human browser request"));
}

#[test]
fn web_launcher_returns_a_reachable_viewer_and_starts_a_fresh_return_cycle() {
    let fixture = Fixture::new();
    let opened = fixture.open("launched-browser");
    let mut finish = mutation(&opened, "prior-back");
    finish["outcome"] = json!("back");
    fixture.call("review.finish", finish);
    let viewer = WebServer::launch(&fixture, &opened);
    let (status, body) = viewer.request("GET", "api/review", &[], &[]);
    assert_eq!(status, 200);
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["result"]["review_id"], opened["review_id"]);
    assert!(
        envelope["result"]["last_return"].is_null(),
        "a previous Back must not immediately finish the new browser pass"
    );
}

#[test]
fn browser_back_cancel_and_empty_wait_are_distinct_from_submitted_feedback() {
    let fixture = Fixture::new();
    let opened = fixture.open("browser-back");
    let empty = fixture.call(
        "feedback.wait",
        json!({"review_id": opened["review_id"], "timeout_ms": 0}),
    );
    assert_eq!(empty["timed_out"], true);
    assert!(empty["last_return"].is_null());
    assert!(empty["submissions"].as_array().unwrap().is_empty());
    let viewer = WebServer::start(&fixture, &opened);
    for (index, outcome) in ["back", "cancelled"].into_iter().enumerate() {
        let mut finish = mutation(&opened, &format!("finish-{index}"));
        finish["outcome"] = json!(outcome);
        viewer.api("review.finish", finish);
        let returned = fixture.call(
            "feedback.wait",
            json!({"review_id": opened["review_id"], "timeout_ms": 0}),
        );
        assert_eq!(returned["timed_out"], false);
        assert_eq!(returned["last_return"]["outcome"], outcome);
        assert!(returned["submissions"].as_array().unwrap().is_empty());
    }
    let resumed = fixture.open("browser-back");
    assert_eq!(resumed["review_id"], opened["review_id"]);
    assert!(resumed["last_return"].is_null());
}

#[test]
fn local_viewer_rejects_foreign_origins_missing_capabilities_and_agent_operations() {
    let fixture = Fixture::new();
    let opened = fixture.open("browser-security");
    let viewer = WebServer::start(&fixture, &opened);
    assert_eq!(
        viewer
            .request(
                "GET",
                "api/review",
                &[("Origin", "https://foreign.example")],
                &[]
            )
            .0,
        403
    );
    assert_eq!(
        viewer
            .request("POST", "api", &[("Content-Type", "text/plain")], b"{}")
            .0,
        415
    );
    let unknown = serde_json::to_vec(&json!({"method": "review.open", "params": {}})).unwrap();
    assert_eq!(
        viewer
            .request(
                "POST",
                "api",
                &[("Content-Type", "application/json")],
                &unknown
            )
            .0,
        403
    );
    let mut stream = TcpStream::connect(&viewer.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {}\r\n\r\n", viewer.address).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eq!(response(&mut stream).0, 404);
    let mut stream = TcpStream::connect(&viewer.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "POST {}api HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 1048577\r\n\r\n", viewer.prefix, viewer.address).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    assert_eq!(response(&mut stream).0, 400);
}
