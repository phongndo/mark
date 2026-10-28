//! Local MCP transport and agent-facing projections of persistent mark reviews.
//!
//! The caller owns repository access, identity, persistence, and idempotency. This
//! module owns the MCP handshake, tool schemas, framing, and feedback visibility.
//! It never takes over the terminal or submits a human's review.

use std::collections::HashSet;
use std::io::{self, BufRead, Write};

use serde_json::{Map, Value, json};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PATCH_BYTES: usize = 256 * 1024;
const PAGE_SIZE: usize = 25;
const MAX_PAGE_SIZE: usize = 100;
const PAGE_BYTES: usize = 512 * 1024;
const MAX_ERROR_CHARS: usize = 4096;
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2024-11-05"];

const INSTRUCTIONS: &str = "mark stores a review independently of the agent conversation. Use review_open to open or resume the conversation's review; use bounded review_patch pages to read its diff. Only feedback_get provides submitted human feedback. Never infer approval from an empty feedback batch, a closed viewer, or an agent comment. These tools do not embed a UI or take over the host terminal; a harness adapter must provide its supported review surface. Human submission, verdict, and review progress are not exposed as model tools. Annotations can be notes, questions, or requests. Answer in chat; add an annotation only when the human asks to annotate code.";

/// Serve newline-delimited MCP on stdin/stdout, without writing diagnostics there.
pub fn run_stdio<F>(call: F) -> io::Result<()>
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    run(io::stdin().lock(), io::stdout().lock(), call)
}

/// Serve one connection over a supplied transport. EOF cleanly ends the server.
///
/// Backend failures become MCP tool errors; only transport failures escape.
/// Backend calls are synchronous and must finish in bounded time.
pub fn run<R, W, F>(mut input: R, mut output: W, mut call: F) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    let mut state = State::New;
    let tools = catalogue();
    let mut frame = Vec::new();
    loop {
        let response = match read_frame(&mut input, &mut frame)? {
            Frame::Eof => return Ok(()),
            Frame::Oversized => Some(rpc_error(
                Value::Null,
                -32600,
                "Request exceeds the 1 MiB frame limit",
            )),
            Frame::Message => match serde_json::from_slice::<Value>(&frame) {
                Ok(message) => dispatch(message, &mut state, &tools, &mut call),
                Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
            },
        };
        if let Some(response) = response {
            let mut encoded = serde_json::to_vec(&response)?;
            if encoded.len() > MAX_RESPONSE_BYTES {
                let id = response.get("id").cloned().unwrap_or(Value::Null);
                encoded = serde_json::to_vec(&rpc_result(
                    id,
                    tool_error(
                        "result_too_large: request a smaller page or inspect this review in mark",
                    ),
                ))?;
            }
            output.write_all(&encoded)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

enum Frame {
    Eof,
    Message,
    Oversized,
}

// Drain an oversized line without allocating beyond the configured limit, then
// recover at the next newline so a malformed client cannot desynchronize calls.
fn read_frame<R: BufRead>(input: &mut R, frame: &mut Vec<u8>) -> io::Result<Frame> {
    frame.clear();
    let mut oversized = false;
    loop {
        let bytes = input.fill_buf()?;
        if bytes.is_empty() {
            return Ok(if oversized {
                Frame::Oversized
            } else if frame.is_empty() {
                Frame::Eof
            } else {
                Frame::Message
            });
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(bytes.len(), |position| position + 1);
        let payload = newline.unwrap_or(bytes.len());
        if !oversized {
            if frame.len().saturating_add(payload) > MAX_REQUEST_BYTES {
                oversized = true;
                frame.clear();
            } else {
                frame.extend_from_slice(&bytes[..payload]);
            }
        }
        input.consume(consumed);
        if newline.is_some() {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return Ok(if oversized {
                Frame::Oversized
            } else {
                Frame::Message
            });
        }
    }
}

enum State {
    New,
    Initializing(&'static str),
    Ready(&'static str),
}

fn dispatch<F>(message: Value, state: &mut State, tools: &[Tool], call: &mut F) -> Option<Value>
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    let Some(object) = message.as_object() else {
        return Some(rpc_error(Value::Null, -32600, "Invalid request"));
    };
    let valid_id = object.get("id").filter(|id| match id {
        Value::String(value) => value.len() <= 256,
        Value::Number(value) => value.is_i64() || value.is_u64(),
        _ => false,
    });
    let id = valid_id.cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc") != Some(&json!("2.0"))
        || object.get("method").and_then(Value::as_str).is_none()
        || (object.contains_key("id") && valid_id.is_none())
    {
        return Some(rpc_error(id, -32600, "Invalid request"));
    }
    let method = object["method"].as_str().unwrap();
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if !object.contains_key("id") {
        // Request-only methods received as notifications never invoke a backend.
        if method == "notifications/initialized"
            && params.is_object()
            && let State::Initializing(version) = state
        {
            *state = State::Ready(version);
        }
        return None;
    }
    if !params.is_object() {
        return Some(rpc_error(id, -32602, "params must be an object"));
    }
    let result = if method == "ping" {
        Ok(json!({}))
    } else if method == "initialize" {
        initialize(state, &params)
    } else if method.starts_with("notifications/") {
        Err((-32600, "Notifications must not contain an id".to_owned()))
    } else if method == "server/discover" {
        // Modern clients may probe this method before falling back to initialize.
        Err((-32601, "Method not found; use initialize".to_owned()))
    } else if let State::Ready(version) = state {
        handle(method, &params, tools, call, version)
    } else {
        Err((
            -32002,
            "Complete initialize and notifications/initialized first".to_owned(),
        ))
    };
    Some(match result {
        Ok(result) => rpc_result(id, result),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

type RpcResult = Result<Value, (i32, String)>;

fn initialize(state: &mut State, params: &Value) -> RpcResult {
    if !matches!(state, State::New) {
        return Err((-32600, "Connection is already initialized".to_owned()));
    }
    let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
        return Err((-32602, "protocolVersion must be a string".to_owned()));
    };
    if !params.get("capabilities").is_some_and(Value::is_object)
        || !params.get("clientInfo").is_some_and(|info| {
            info.get("name").is_some_and(Value::is_string)
                && info.get("version").is_some_and(Value::is_string)
        })
    {
        return Err((
            -32602,
            "initialize requires capabilities and clientInfo with name and version".to_owned(),
        ));
    }
    let version = PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|version| *version == requested)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    *state = State::Initializing(version);
    Ok(json!({
        "protocolVersion": version,
        "serverInfo": {"name": "mark", "version": env!("CARGO_PKG_VERSION")},
        "capabilities": {
            "tools": {"listChanged": false},
            "prompts": {"listChanged": false},
            "experimental": {"mark.review": {
                "persistentReviews": true,
                "submittedFeedback": true,
                "embeddedUi": false,
                "terminalHandoff": false,
                "humanDecisions": "readOnly"
            }}
        },
        "instructions": INSTRUCTIONS
    }))
}

fn handle<F>(method: &str, params: &Value, tools: &[Tool], call: &mut F, version: &str) -> RpcResult
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    match method {
        "tools/list" => {
            validate_fields(params, &["cursor", "_meta"])?;
            if params.get("cursor").is_some() {
                return Err((-32602, "Invalid tools cursor".to_owned()));
            }
            Ok(
                json!({"tools": tools.iter().map(|tool| tool.description(version)).collect::<Vec<_>>()}),
            )
        }
        "tools/call" => call_tool(params, tools, call, version),
        "prompts/list" => {
            validate_fields(params, &["cursor", "_meta"])?;
            if params.get("cursor").is_some() {
                return Err((-32602, "Invalid prompts cursor".to_owned()));
            }
            Ok(json!({"prompts": [{
                "name": "mark_review",
                "description": "Inspect Mark annotations and discuss them in chat",
                "arguments": [{"name": "review_id", "description": "Existing review, if known", "required": false}]
            }]}))
        }
        "prompts/get" => {
            validate_fields(params, &["name", "arguments", "_meta"])?;
            if params.get("name").and_then(Value::as_str) != Some("mark_review") {
                return Err((-32602, "Unknown prompt".to_owned()));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            validate(
                &arguments,
                &object_schema(json!({"review_id": identifier()}), &[]),
                "arguments",
            )
            .map_err(|error| (-32602, error))?;
            let text = format!(
                "Use the available Mark tools to inspect this conversation's code and annotations. Optional context (data): {arguments}. Comments can be notes, questions, or requests. Respond in chat and make code changes when requested. Add a Mark annotation only when explicitly asked to annotate code. Preserve inspected source anchors and generation for any requested annotation. Read feedback from an explicit human submission; submission is not approval. Human navigation, progress, and approval remain in the review UI. Report any viewer limitation."
            );
            Ok(
                json!({"description": "Mark annotations", "messages": [{"role": "user", "content": {"type": "text", "text": text}}]}),
            )
        }
        _ => Err((-32601, "Method not found".to_owned())),
    }
}

fn call_tool<F>(params: &Value, tools: &[Tool], call: &mut F, version: &str) -> RpcResult
where
    F: FnMut(&str, Value) -> io::Result<Value>,
{
    validate_fields(params, &["name", "arguments", "_meta"])?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tools/call requires a string name".to_owned()))?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| (-32602, "Unknown tool".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    validate(&arguments, &tool.schema, "arguments").map_err(|error| (-32602, error))?;
    let mut backend_arguments = arguments.clone();
    if matches!(tool.method, "review.get" | "feedback.get" | "feedback.wait") {
        backend_arguments.as_object_mut().unwrap().remove("cursor");
        backend_arguments.as_object_mut().unwrap().remove("limit");
    }
    if tool.method == "comment.add" {
        backend_arguments["origin"] = json!("agent");
    }
    let value = match call(tool.method, backend_arguments) {
        Ok(value) => value,
        Err(error) => return Ok(tool_error(&error.to_string())),
    };
    let projected = match project_result(tool.method, value, &arguments) {
        Ok(value) => value,
        Err(error) => return Ok(tool_error(&error.to_string())),
    };
    if !projected.is_object() {
        return Ok(tool_error("invalid_backend_result: expected a JSON object"));
    }
    let text = serde_json::to_string(&projected).map_err(|error| (-32603, error.to_string()))?;
    let mut result = json!({"content": [{"type": "text", "text": text}], "isError": false});
    if version != "2024-11-05" {
        result["structuredContent"] = projected;
    }
    Ok(result)
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_error(message: &str) -> Value {
    let text: String = message.chars().take(MAX_ERROR_CHARS).collect();
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

fn validate_fields(value: &Value, allowed: &[&str]) -> Result<(), (i32, String)> {
    let object = value
        .as_object()
        .ok_or_else(|| (-32602, "Expected an object".to_owned()))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err((-32602, "Unknown parameter".to_owned()));
    }
    if object.get("_meta").is_some_and(|meta| !meta.is_object()) {
        return Err((-32602, "_meta must be an object".to_owned()));
    }
    Ok(())
}

struct Tool {
    name: &'static str,
    method: &'static str,
    text: &'static str,
    schema: Value,
    read_only: bool,
}

impl Tool {
    fn description(&self, version: &str) -> Value {
        let mut tool =
            json!({"name": self.name, "description": self.text, "inputSchema": self.schema});
        if version != "2024-11-05" {
            tool["annotations"] = json!({
                "readOnlyHint": self.read_only,
                "destructiveHint": false,
                "idempotentHint": self.method != "review.open",
                "openWorldHint": false
            });
        }
        tool
    }
}

fn identifier() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 256})
}

fn bounded_string(max: usize) -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": max})
}

fn integer(min: u64, max: u64) -> Value {
    json!({"type": "integer", "minimum": min, "maximum": max})
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn catalogue() -> Vec<Tool> {
    let mut tools = Vec::new();
    let mut add = |name, method, text, schema, read_only| {
        tools.push(Tool {
            name,
            method,
            text,
            schema,
            read_only,
        });
    };
    add(
        "review_open",
        "review.open",
        "Open or resume this harness conversation's persistent review with a fresh source snapshot. Identity can be supplied by the harness environment. Returns metadata and a bounded thread page; use review_patch for the snapshot. Optional browser presentation starts a local viewer and returns its URL for the host to open.",
        object_schema(
            json!({
                "harness": identifier(), "conversation": identifier(), "repository": bounded_string(4096),
                "source": {"type": "object", "description": "Git review source, for example {kind: worktree} or {kind: base, base: main}. Omit to review all local changes."},
                "presentation": {"type": "string", "enum": ["browser"]}
            }),
            &[],
        ),
        false,
    );
    add(
        "review_get",
        "review.get",
        "Read review metadata and a bounded page of threads. Agent messages and submitted human messages are visible; draft human message bodies and raw patches are excluded. Cursors expire when revision changes.",
        object_schema(
            json!({
                "review_id": identifier(), "cursor": identifier(), "limit": integer(1, MAX_PAGE_SIZE as u64)
            }),
            &["review_id"],
        ),
        true,
    );
    add(
        "review_patch",
        "review.patch",
        "Read a bounded byte page of the saved review snapshot. Follow next_offset until null; do not infer the snapshot is current without refresh.",
        object_schema(
            json!({
                "review_id": identifier(), "offset": integer(0, u64::MAX), "max_bytes": integer(1, MAX_PATCH_BYTES as u64)
            }),
            &["review_id"],
        ),
        true,
    );
    add(
        "review_refresh",
        "review.refresh",
        "Refresh this review from its existing Git source after changes. Supply the last generation and a stable mutation id. Existing comments remain associated with their original generation.",
        mutation_schema(json!({}), &[]),
        false,
    );
    add(
        "review_changes",
        "review.changes",
        "Read a bounded page of review events. Cursors report whether a full refresh is required; events are not evidence of human approval or submitted feedback.",
        object_schema(
            json!({
                "review_id": identifier(), "cursor": identifier(), "limit": integer(1, MAX_PAGE_SIZE as u64)
            }),
            &["review_id"],
        ),
        true,
    );
    let range = object_schema(
        json!({"start": integer(1, u64::MAX), "end": integer(1, u64::MAX)}),
        &["start", "end"],
    );
    let anchor = object_schema(
        json!({
            "file": bounded_string(4096), "new_line": integer(1, u64::MAX), "old_line": integer(1, u64::MAX),
            "hunk": integer(0, u64::MAX), "scope": {"type": "string", "enum": ["file"]},
            "range": object_schema(json!({"old": range, "new": range}), &[])
        }),
        &["file"],
    );
    add(
        "comment_add",
        "comment.add",
        "Add a concise code annotation when the human asks for one. Questions, explanations, and discussion belong in chat. The bridge records agent origin.",
        mutation_schema(
            json!({
                "anchor": anchor, "body": bounded_string(32768), "author": identifier()
            }),
            &["anchor", "body"],
        ),
        false,
    );
    add(
        "feedback_get",
        "feedback.get",
        "Read submitted, unacknowledged human feedback for a consumer. Draft notes and later unsubmitted replies are excluded. An empty result is not approval. Read all pages before acknowledging; cursors expire when revision changes.",
        object_schema(
            json!({
                "review_id": identifier(), "consumer": identifier(), "cursor": identifier(), "limit": integer(1, MAX_PAGE_SIZE as u64)
            }),
            &["review_id"],
        ),
        true,
    );
    add(
        "feedback_wait",
        "feedback.wait",
        "Wait at most 30 seconds for submitted feedback or a human return outcome, then read a bounded feedback page. The default timeout is 20 seconds. An empty timeout is not approval; stop when the human returns or cancels. This synchronous wait cannot process a protocol cancellation until it returns.",
        object_schema(
            json!({
                "review_id": identifier(), "consumer": identifier(), "timeout_ms": integer(0, 30000),
                "cursor": identifier(), "limit": integer(1, MAX_PAGE_SIZE as u64)
            }),
            &["review_id"],
        ),
        true,
    );
    add(
        "feedback_ack",
        "feedback.ack",
        "Acknowledge that this consumer received one submitted feedback batch. Call only after consuming the batch; this does not approve or resolve its findings.",
        mutation_schema(
            json!({
                "submission_id": identifier(), "consumer": identifier()
            }),
            &["submission_id"],
        ),
        false,
    );
    tools
}

fn mutation_schema(extra: Value, required_extra: &[&str]) -> Value {
    let mut properties = json!({
        "review_id": identifier(), "generation": integer(0, u64::MAX), "client_mutation_id": identifier()
    });
    properties
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let mut required = vec!["review_id", "generation", "client_mutation_id"];
    required.extend_from_slice(required_extra);
    object_schema(properties, &required)
}

// A deliberately small validator for the JSON Schema vocabulary used above.
// Keeping schemas and validation together prevents silently accepting authority
// fields such as origin=human that the model-facing interface does not advertise.
fn validate(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let bad = || format!("Invalid {path}");
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let object = value.as_object().ok_or_else(bad)?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        return Err(format!("Missing {path}.{key}"));
                    }
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            for (key, value) in object {
                if let Some(property) = properties.and_then(|properties| properties.get(key)) {
                    validate(value, property, &format!("{path}.{key}"))?;
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    return Err(format!("Unknown {path}.{key}"));
                }
            }
        }
        Some("string") => {
            let string = value.as_str().ok_or_else(bad)?;
            let length = string.chars().count() as u64;
            if schema
                .get("minLength")
                .and_then(Value::as_u64)
                .is_some_and(|min| length < min)
                || schema
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .is_some_and(|max| length > max)
                || schema
                    .get("enum")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.contains(value))
            {
                return Err(bad());
            }
        }
        Some("integer") => {
            let integer = value.as_u64().ok_or_else(bad)?;
            if schema
                .get("minimum")
                .and_then(Value::as_u64)
                .is_some_and(|min| integer < min)
                || schema
                    .get("maximum")
                    .and_then(Value::as_u64)
                    .is_some_and(|max| integer > max)
            {
                return Err(bad());
            }
        }
        _ => {}
    }
    Ok(())
}

fn project_result(method: &str, value: Value, args: &Value) -> io::Result<Value> {
    match method {
        "review.open" | "review.get" => project_review(value, args),
        "feedback.get" | "feedback.wait" => project_feedback(value, args),
        "comment.add" | "comment.reply" => {
            let mut result = value;
            if let Some(thread) = result.get_mut("thread") {
                *thread = project_thread(thread, &HashSet::new());
            }
            Ok(result)
        }
        _ => Ok(value),
    }
}

fn selected(value: &Value, fields: &[&str]) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|key| {
            value
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect()
}

fn submitted_versions(submissions: &[Value]) -> HashSet<(String, u64)> {
    submissions
        .iter()
        .filter_map(|submission| submission.get("threads").and_then(Value::as_array))
        .flatten()
        .filter_map(|thread| thread.get("messages").and_then(Value::as_array))
        .flatten()
        .filter_map(|message| {
            Some((
                message.get("message_id")?.as_str()?.to_owned(),
                message.get("revision")?.as_u64()?,
            ))
        })
        .collect()
}

fn project_thread(thread: &Value, submitted: &HashSet<(String, u64)>) -> Value {
    let mut result = selected(
        thread,
        &["thread_id", "anchor", "generation", "stale", "disposition"],
    );
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|message| {
            message.get("origin").and_then(Value::as_str) == Some("agent")
                || message
                    .get("message_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| {
                        message
                            .get("revision")
                            .and_then(Value::as_u64)
                            .is_some_and(|revision| submitted.contains(&(id.to_owned(), revision)))
                    })
        })
        .map(|message| {
            Value::Object(selected(
                message,
                &[
                    "message_id",
                    "reply_to",
                    "body",
                    "origin",
                    "author",
                    "revision",
                    "deleted",
                ],
            ))
        })
        .collect::<Vec<_>>();
    result.insert("messages".to_owned(), json!(messages));
    Value::Object(result)
}

fn project_review(value: Value, args: &Value) -> io::Result<Value> {
    let mut result = selected(
        &value,
        &[
            "review_id",
            "binding",
            "source",
            "generation",
            "revision",
            "viewer",
        ],
    );
    let patch = value
        .pointer("/snapshot/patch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    result.insert(
        "snapshot".to_owned(),
        json!({"total_bytes": patch.len(), "tool": "review_patch"}),
    );
    let submissions = value
        .get("submissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let submitted = submitted_versions(&submissions);
    let threads = value
        .get("threads")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let (page, next_cursor) = page(&threads, args, revision, |thread| {
        project_thread(thread, &submitted)
    })?;
    result.insert("threads".to_owned(), json!(page));
    result.insert("thread_count".to_owned(), json!(threads.len()));
    result.insert("next_cursor".to_owned(), json!(next_cursor));
    result.insert("submission_count".to_owned(), json!(submissions.len()));
    Ok(Value::Object(result))
}

fn project_feedback(value: Value, args: &Value) -> io::Result<Value> {
    let mut result = selected(
        &value,
        &[
            "review_id",
            "generation",
            "revision",
            "last_return",
            "timed_out",
        ],
    );
    let submissions = value
        .get("submissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let revision = value
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let (submissions, next_cursor) = page(&submissions, args, revision, Clone::clone)?;
    let submitted = submitted_versions(&submissions);
    let thread_ids: HashSet<&str> = submissions
        .iter()
        .filter_map(|submission| submission.get("thread_ids").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let threads = value
        .get("threads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|thread| {
            thread
                .get("thread_id")
                .and_then(Value::as_str)
                .is_some_and(|id| thread_ids.contains(id))
        })
        .map(|thread| project_thread(thread, &submitted))
        .collect::<Vec<_>>();
    result.insert("submissions".to_owned(), json!(submissions));
    result.insert("threads".to_owned(), json!(threads));
    result.insert("next_cursor".to_owned(), json!(next_cursor));
    Ok(Value::Object(result))
}

fn page<F>(
    items: &[Value],
    args: &Value,
    revision: u64,
    project: F,
) -> io::Result<(Vec<Value>, Option<String>)>
where
    F: Fn(&Value) -> Value,
{
    let start = if let Some(cursor) = args.get("cursor").and_then(Value::as_str) {
        let (cursor_revision, offset) = cursor.split_once(':').ok_or_else(invalid_cursor)?;
        if cursor_revision.parse::<u64>().ok() != Some(revision) {
            return Err(invalid_cursor());
        }
        offset.parse::<usize>().map_err(|_| invalid_cursor())?
    } else {
        0
    };
    if start > items.len() {
        return Err(invalid_cursor());
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(PAGE_SIZE as u64) as usize;
    let mut bytes = 0usize;
    let mut results = Vec::new();
    for item in items.iter().skip(start).take(limit) {
        let item = project(item);
        let length = serde_json::to_vec(&item)?.len();
        if !results.is_empty() && bytes.saturating_add(length) > PAGE_BYTES {
            break;
        }
        bytes = bytes.saturating_add(length);
        results.push(item);
    }
    let next = start + results.len();
    Ok((
        results,
        (next < items.len()).then(|| format!("{revision}:{next}")),
    ))
}

fn invalid_cursor() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid_cursor: review changed or cursor is invalid; restart from the first page",
    )
}
