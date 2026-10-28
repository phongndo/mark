use std::io::{self, Cursor, Write};

use mark_mcp::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, run};
use serde_json::{Value, json};

fn initialize(version: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": "initialize", "method": "initialize", "params": {
        "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}
    }})
}

fn request(id: Value, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn tool(id: Value, name: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

fn connection(mut messages: Vec<Value>) -> Vec<Value> {
    let mut setup = vec![
        initialize("2025-11-25"),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    setup.append(&mut messages);
    setup
}

fn exchange<F>(messages: &[Value], call: F) -> Vec<Value>
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    let mut input = Vec::new();
    for message in messages {
        serde_json::to_writer(&mut input, message).unwrap();
        input.push(b'\n');
    }
    raw_exchange(&input, call)
}

fn raw_exchange<F>(input: &[u8], call: F) -> Vec<Value>
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    let mut output = Vec::new();
    // A small reader buffer exercises frames that span multiple fill_buf calls.
    run(
        io::BufReader::with_capacity(37, Cursor::new(input)),
        &mut output,
        call,
    )
    .unwrap();
    assert!(output.is_empty() || output.ends_with(b"\n"));
    output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            assert!(line.len() <= MAX_RESPONSE_BYTES);
            serde_json::from_slice(line).unwrap()
        })
        .collect()
}

fn no_backend(_: &str, _: Value) -> io::Result<Value> {
    panic!("unexpected backend call")
}

#[test]
fn handshake_negotiates_supported_versions_and_unknown_counteroffers() {
    for (requested, negotiated) in [
        ("2024-11-05", "2024-11-05"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
        ("future", "2025-11-25"),
    ] {
        let responses = exchange(
            &[
                initialize(requested),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                request(json!(2), "tools/list", json!({})),
            ],
            no_backend,
        );
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["protocolVersion"], negotiated);
        assert_eq!(
            responses[0]["result"]["capabilities"]["experimental"]["mark.review"]["embeddedUi"],
            false
        );
        assert!(responses[1]["result"]["tools"].as_array().unwrap().len() >= 9);
    }
}

#[test]
fn reply_tools_are_not_exposed_or_dispatched() {
    let responses = exchange(
        &connection(vec![
            request(json!(1), "tools/list", json!({})),
            tool(
                json!(2),
                "comment_reply",
                json!({
                    "review_id":"review-1", "generation":1, "client_mutation_id":"reply",
                    "thread_id":"thread-1", "reply_to":"message-1", "body":"answer"
                }),
            ),
        ]),
        no_backend,
    );
    assert!(
        !responses[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "comment_reply")
    );
    assert_eq!(responses[2]["error"]["code"], -32602);
}

#[test]
fn lifecycle_requires_the_initialized_notification_and_allows_early_ping() {
    let responses = exchange(
        &[
            request(json!(0), "ping", json!({})),
            request(json!(1), "tools/list", json!({})),
            initialize("2025-06-18"),
            request(json!(2), "tools/list", json!({})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(json!(3), "tools/list", json!({})),
            initialize("2025-06-18"),
        ],
        no_backend,
    );
    assert_eq!(responses[0]["result"], json!({}));
    assert_eq!(responses[1]["error"]["code"], -32002);
    assert_eq!(responses[3]["error"]["code"], -32002);
    assert!(responses[4]["result"]["tools"].is_array());
    assert_eq!(responses[5]["error"]["code"], -32600);
}

#[test]
fn malformed_initialize_can_be_corrected_without_poisoning_the_connection() {
    let responses = exchange(
        &[
            request(json!(0), "initialize", json!({"protocolVersion": true})),
            request(
                json!(1),
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": []}),
            ),
            initialize("2025-06-18"),
        ],
        no_backend,
    );
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert_eq!(responses[2]["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn notifications_never_reply_or_invoke_request_only_methods() {
    let responses = exchange(
        &connection(vec![
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}),
            json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "review_open", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "method": "extension/unknown"}),
            request(json!("after"), "ping", json!({})),
        ]),
        no_backend,
    );
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[1]["id"], "after");
}

#[test]
fn ids_errors_and_invalid_envelopes_follow_json_rpc() {
    let mut input = b"not JSON\n[]\nnull\n".to_vec();
    for message in [
        json!({"jsonrpc": "1.0", "id": "known", "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": true, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 1.5, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": 2}),
        request(json!(-7), "ping", json!([])),
        request(json!("string-id"), "ping", json!({})),
        request(json!(u64::MAX), "ping", json!({})),
    ] {
        serde_json::to_writer(&mut input, &message).unwrap();
        input.push(b'\n');
    }
    let responses = raw_exchange(&input, no_backend);
    assert_eq!(responses[0]["error"]["code"], -32700);
    for response in &responses[1..8] {
        assert_eq!(response["error"]["code"], -32600);
    }
    assert_eq!(responses[3]["id"], "known");
    for response in &responses[4..7] {
        assert!(response["id"].is_null());
    }
    assert_eq!(responses[8]["id"], -7);
    assert_eq!(responses[8]["error"]["code"], -32602);
    assert_eq!(responses[9]["id"], "string-id");
    assert_eq!(responses[10]["id"], u64::MAX);
}

#[test]
fn oversized_and_non_utf8_frames_recover_at_the_next_newline() {
    let mut input = vec![b'x'; MAX_REQUEST_BYTES + 1024];
    input.extend_from_slice(b"\n\xff\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\r\n");
    let responses = raw_exchange(&input, no_backend);
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["error"]["code"], -32600);
    assert_eq!(responses[1]["error"]["code"], -32700);
    assert_eq!(responses[2]["result"], json!({}));
}

#[test]
fn modern_probe_unknown_methods_and_unknown_tools_are_protocol_errors() {
    let responses = exchange(
        &[
            request(json!(0), "server/discover", json!({})),
            initialize("2025-11-25"),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(json!(1), "missing/method", json!({})),
            tool(json!(2), "feedback_submit", json!({})),
            request(json!(3), "tools/list", json!({"cursor": "not-a-cursor"})),
        ],
        no_backend,
    );
    assert_eq!(responses[0]["error"]["code"], -32601);
    assert_eq!(responses[2]["error"]["code"], -32601);
    assert_eq!(responses[3]["error"]["code"], -32602);
    assert_eq!(responses[4]["error"]["code"], -32602);
}

fn mutation() -> Value {
    json!({"review_id": "review-1", "generation": 1, "client_mutation_id": "mutation-1", "anchor": {"file": "src/lib.rs", "new_line": 1}, "body": "Please handle errors"})
}

#[test]
fn schemas_reject_spoofing_invalid_types_and_oversized_pages_before_backend() {
    let mut spoof = mutation();
    spoof["origin"] = json!("human");
    let mut wrong_generation = mutation();
    wrong_generation["generation"] = json!("1");
    let mut wrong_anchor = mutation();
    wrong_anchor["anchor"]["new_line"] = json!(0);
    let responses = exchange(
        &connection(vec![
            tool(json!(0), "comment_add", spoof),
            tool(json!(1), "comment_add", wrong_generation),
            tool(json!(2), "comment_add", wrong_anchor),
            tool(json!(3), "comment_add", json!({"review_id": "r"})),
            tool(
                json!(4),
                "review_patch",
                json!({"review_id": "r", "max_bytes": 262145}),
            ),
            tool(
                json!(5),
                "review_get",
                json!({"review_id": "r", "limit": 101}),
            ),
            tool(
                json!(6),
                "feedback_wait",
                json!({"review_id": "r", "timeout_ms": 30001}),
            ),
        ]),
        no_backend,
    );
    for response in &responses[1..] {
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
}

#[test]
fn mutations_keep_retry_identity_force_agent_origin_and_redact_draft_threads() {
    let arguments = mutation();
    let mut calls = Vec::new();
    let responses = exchange(
        &connection(vec![
            tool(json!(1), "comment_add", arguments.clone()),
            tool(json!(2), "comment_add", arguments),
        ]),
        |method, args| {
            calls.push((method.to_owned(), args));
            Ok(
                json!({"review_id": "review-1", "generation": 1, "revision": 2, "thread": {
                    "thread_id": "thread-1", "messages": [
                        {"message_id": "human-1", "origin": "human", "body": "private draft", "revision": 1},
                        {"message_id": "agent-1", "origin": "agent", "body": "code annotation", "revision": 2}
                    ]
                }}),
            )
        },
    );
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(calls[0].0, "comment.add");
    assert_eq!(calls[0].1["origin"], "agent");
    assert_eq!(calls[0].1["client_mutation_id"], "mutation-1");
    assert_eq!(responses[1]["result"], responses[2]["result"]);
    let serialized = serde_json::to_string(&responses).unwrap();
    assert!(!serialized.contains("private draft"));
    assert!(serialized.contains("code annotation"));
}

#[test]
fn backend_failures_are_tool_errors_and_do_not_end_the_connection() {
    let responses = exchange(
        &connection(vec![
            tool(json!(1), "review_get", json!({"review_id": "missing"})),
            tool(json!(2), "review_get", json!({"review_id": "stale"})),
            request(json!(3), "ping", json!({})),
        ]),
        |_, args| {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("review_not_found: {}", args["review_id"]),
            ))
        },
    );
    for response in &responses[1..3] {
        assert_eq!(response["result"]["isError"], true);
        assert!(response.get("error").is_none());
    }
    assert_eq!(responses[3]["result"], json!({}));
}

fn thread(id: &str, messages: Vec<Value>) -> Value {
    json!({"thread_id": id, "anchor": {"file": "src/lib.rs"}, "messages": messages, "generation": 1, "stale": false, "disposition": "open"})
}

fn message(id: &str, origin: &str, body: &str, revision: u64) -> Value {
    json!({"message_id": id, "origin": origin, "body": body, "revision": revision, "deleted": false})
}

fn review_fixture() -> Value {
    let submitted_thread = thread(
        "thread-1",
        vec![
            message("agent-1", "agent", "agent finding", 1),
            message("human-1", "human", "submitted feedback", 2),
        ],
    );
    json!({
        "review_id": "review-1", "generation": 1, "revision": 5,
        "binding": {"harness": "codex", "conversation": "chat-1", "repository": "/repo"},
        "source": {"kind": "diff"}, "snapshot": {"patch": "raw snapshot must be paged"},
        "threads": [thread("thread-1", vec![
            message("agent-1", "agent", "agent finding", 1),
            message("human-1", "human", "unsent edit of submitted message", 4),
            message("human-2", "human", "private unsent followup", 5)
        ])],
        "submissions": [{"submission_id": "batch-1", "thread_ids": ["thread-1"], "message_ids": ["agent-1", "human-1"], "generation": 1, "revision": 3, "acknowledged_by": [], "threads": [submitted_thread]}],
        "mutations": [{"result": "receipt must not escape"}],
        "last_return": {"outcome": "submitted", "submission_ids": ["batch-1"], "revision": 3}
    })
}

#[test]
fn get_and_open_exclude_unsent_edits_draft_messages_snapshots_and_receipts() {
    let responses = exchange(
        &connection(vec![
            tool(json!(1), "review_get", json!({"review_id": "review-1"})),
            tool(json!(2), "review_open", json!({})),
        ]),
        |_, _| Ok(review_fixture()),
    );
    for response in &responses[1..] {
        let result = &response["result"]["structuredContent"];
        assert_eq!(result["snapshot"]["total_bytes"], 26);
        assert_eq!(
            result["threads"][0]["messages"].as_array().unwrap().len(),
            1
        );
        assert_eq!(result["threads"][0]["messages"][0]["origin"], "agent");
        assert_eq!(result["submission_count"], 1);
        let encoded = result.to_string();
        for private in ["unsent", "raw snapshot", "receipt must"] {
            assert!(!encoded.contains(private), "{encoded}");
        }
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), *result);
    }
}

#[test]
fn browser_presentation_is_forwarded_and_viewer_url_survives_projection() {
    let responses = exchange(
        &connection(vec![tool(
            json!(1),
            "review_open",
            json!({"presentation": "browser"}),
        )]),
        |method, args| {
            assert_eq!(method, "review.open");
            assert_eq!(args["presentation"], "browser");
            let mut result = review_fixture();
            result["viewer"] = json!({"review_id": "review-1", "url": "http://127.0.0.1:42/token/", "process_id": 1});
            Ok(result)
        },
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["viewer"]["url"],
        "http://127.0.0.1:42/token/"
    );
}

#[test]
fn feedback_uses_immutable_batches_and_forwards_wait_controls() {
    let mut calls = Vec::new();
    let responses = exchange(
        &connection(vec![
            tool(
                json!(1),
                "feedback_get",
                json!({"review_id": "review-1", "consumer": "codex", "limit": 1}),
            ),
            tool(
                json!(2),
                "feedback_wait",
                json!({"review_id": "review-1", "timeout_ms": 20000}),
            ),
        ]),
        |method, args| {
            calls.push((method.to_owned(), args));
            let fixture = review_fixture();
            Ok(
                json!({"review_id": "review-1", "generation": 1, "revision": 5,
                    "submissions": fixture["submissions"], "threads": fixture["submissions"][0]["threads"],
                    "last_return": fixture["last_return"], "timed_out": false
                }),
            )
        },
    );
    assert_eq!(calls[0].0, "feedback.get");
    assert_eq!(calls[0].1["consumer"], "codex");
    assert!(calls[0].1.get("limit").is_none());
    assert_eq!(calls[1].0, "feedback.wait");
    assert_eq!(calls[1].1["timeout_ms"], 20000);
    for response in &responses[1..] {
        let result = &response["result"]["structuredContent"];
        assert_eq!(result["last_return"]["outcome"], "submitted");
        assert!(result.to_string().contains("submitted feedback"));
        assert!(!result.to_string().contains("unsent"));
    }
}

#[test]
fn thread_pages_are_bounded_and_revision_cursors_prevent_silent_skips() {
    let fixture = json!({"review_id": "r", "revision": 7, "generation": 1, "threads": [
        thread("one", vec![]), thread("two", vec![]), thread("three", vec![])
    ]});
    let mut calls = 0;
    let responses = exchange(
        &connection(vec![
            tool(
                json!(1),
                "review_get",
                json!({"review_id": "r", "limit": 2}),
            ),
            tool(
                json!(2),
                "review_get",
                json!({"review_id": "r", "cursor": "7:2", "limit": 2}),
            ),
            tool(
                json!(3),
                "review_get",
                json!({"review_id": "r", "cursor": "6:2"}),
            ),
        ]),
        |method, args| {
            calls += 1;
            assert_eq!(method, "review.get");
            assert_eq!(args, json!({"review_id": "r"}));
            Ok(fixture.clone())
        },
    );
    assert_eq!(calls, 3);
    assert_eq!(
        responses[1]["result"]["structuredContent"]["threads"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["next_cursor"],
        "7:2"
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["threads"][0]["thread_id"],
        "three"
    );
    assert!(responses[2]["result"]["structuredContent"]["next_cursor"].is_null());
    assert_eq!(responses[3]["result"]["isError"], true);
}

#[test]
fn older_negotiated_protocol_returns_text_without_new_result_fields() {
    let responses = exchange(
        &[
            initialize("2024-11-05"),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            tool(
                json!(1),
                "review_patch",
                json!({"review_id": "r", "max_bytes": 10}),
            ),
            request(json!(2), "tools/list", json!({})),
        ],
        |method, _| {
            assert_eq!(method, "review.patch");
            Ok(json!({"review_id": "r", "generation": 1, "patch": "diff", "next_offset": null}))
        },
    );
    assert!(responses[1]["result"].get("structuredContent").is_none());
    assert!(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("diff")
    );
    for tool in responses[2]["result"]["tools"].as_array().unwrap() {
        assert!(tool.get("annotations").is_none());
    }
}

#[test]
fn prompts_are_available_without_backend_or_claiming_human_approval() {
    let responses = exchange(
        &connection(vec![
            request(json!(1), "prompts/list", json!({})),
            request(
                json!(2),
                "prompts/get",
                json!({"name": "mark_review", "arguments": {"review_id": "r"}}),
            ),
            request(json!(3), "prompts/get", json!({"name": "unknown"})),
        ]),
        no_backend,
    );
    assert_eq!(responses[1]["result"]["prompts"][0]["name"], "mark_review");
    let text = responses[2]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("explicit human submission"));
    assert!(text.contains("limitation"));
    assert_eq!(responses[3]["error"]["code"], -32602);
}

#[test]
fn oversized_backend_results_and_errors_are_bounded_without_breaking_transport() {
    let responses = exchange(
        &connection(vec![
            tool(json!(1), "review_patch", json!({"review_id": "r"})),
            tool(json!(2), "review_patch", json!({"review_id": "r"})),
            request(json!(3), "ping", json!({})),
        ]),
        {
            let mut first = true;
            move |_, _| {
                if first {
                    first = false;
                    Ok(json!({"patch": "x".repeat(MAX_RESPONSE_BYTES)}))
                } else {
                    Err(io::Error::other("界".repeat(MAX_RESPONSE_BYTES)))
                }
            }
        },
    );
    assert_eq!(responses[1]["result"]["isError"], true);
    assert!(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("result_too_large")
    );
    assert!(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 4096
    );
    assert_eq!(responses[3]["result"], json!({}));
}

#[test]
fn transport_write_failures_are_reported_to_the_caller() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let input = serde_json::to_vec(&request(json!(1), "ping", json!({}))).unwrap();
    let error = run(Cursor::new(input), Broken, no_backend).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}
