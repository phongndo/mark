use std::{
    io::{self, IsTerminal, Read},
    path::Path,
};

use mark_session::review::ReviewStore;
use serde_json::{Value, json};

use crate::{CliResult, write_stdout};

mod anchors;
mod args;
mod source;
mod web;

pub(crate) use args::{BridgeCommand, McpArgs};

pub(crate) fn run(command: BridgeCommand) -> CliResult<()> {
    match command {
        BridgeCommand::Annotate {
            repo,
            base,
            result_file,
        } => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Err(io::Error::new(io::ErrorKind::Unsupported,
                    "Mark needs an interactive terminal. Run /mark from your agent, or run `mark diff` in a terminal.").into());
            }
            let source = base.map_or_else(
                || json!({"kind":"worktree"}),
                |base| json!({"kind":"base","base":base}),
            );
            let options = source::options(&json!(repo), &source)?;
            let markdown = mark_tui::run_annotation_review(options)?.unwrap_or_default();
            write_result(&result_file, &json!({"markdown":markdown}))?;
            Ok(())
        }
        BridgeCommand::Open {
            harness,
            conversation,
            repo,
            base,
            json: _,
        } => {
            let source = base.map_or_else(
                || json!({"kind":"worktree"}),
                |base| json!({"kind":"base","base":base}),
            );
            output(call(
                "review.open",
                json!({"harness":harness,"conversation":conversation,"repository":repo,"source":source}),
            ))
        }
        BridgeCommand::Call {
            method,
            stdin,
            json: _,
        } => {
            let params = if stdin { read_params()? } else { json!({}) };
            output(call(&method, params))
        }
        BridgeCommand::Review {
            review_id,
            result_file,
        } => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Err(io::Error::new(io::ErrorKind::Unsupported,
                    "Mark needs the harness to release its interactive terminal; use the native review panel or browser viewer on this surface").into());
            }
            let record = call("review.get", json!({"review_id":review_id}))?;
            let options = source::options(&record["binding"]["repository"], &record["source"])?;
            begin(&review_id)?;
            let result = mark_tui::run_integrated_review(options, &review_id)?;
            write_result(&result_file, &result)?;
            Ok(())
        }
        BridgeCommand::Web { review_id, json: _ } => output(web::launch(&review_id)),
        BridgeCommand::WebServe {
            review_id,
            ready_file,
        } => web::serve(&review_id, &ready_file).map_err(Into::into),
    }
}

fn begin(review_id: &str) -> io::Result<()> {
    let store = ReviewStore::from_env()?;
    let record = store.call("review.get", json!({"review_id":review_id}))?;
    store.call("review.begin",json!({"review_id":review_id,"generation":record["generation"],"client_mutation_id":format!("view-begin-{}",mark_session::new_session_id())}))?;
    Ok(())
}

pub(crate) fn mcp(args: McpArgs) -> CliResult<()> {
    mark_mcp::run_stdio(move |method, mut params| {
        if method == "review.open" {
            for (key, value) in [
                ("harness", args.harness.as_ref()),
                ("conversation", args.conversation.as_ref()),
            ] {
                if params.get(key).is_none()
                    && let Some(value) = value
                {
                    params[key] = json!(value);
                }
            }
            if params.get("repository").is_none()
                && let Some(repo) = &args.repo
            {
                params["repository"] = json!(repo);
            }
        }
        call(method, params)
    })?;
    Ok(())
}

pub(crate) fn call(method: &str, mut params: Value) -> io::Result<Value> {
    if !params.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid_params: expected a JSON object",
        ));
    }
    let store = ReviewStore::from_env()?;
    if method == "comment.add" {
        let record = store.call("review.get", json!({"review_id":params["review_id"]}))?;
        // Snapshot validation is meaningful only for the claimed generation.
        // The store performs its final atomic generation and retry checks.
        if params["generation"].as_u64() == record["generation"].as_u64() {
            let patch = record["snapshot"]["patch"]
                .as_str()
                .ok_or_else(|| io::Error::other("review snapshot is missing"))?;
            anchors::validate(patch, &params["anchor"])?;
        }
    }
    if method.starts_with("feedback.") && params.get("consumer").is_none() {
        let record = store.call("review.get", json!({"review_id":params["review_id"]}))?;
        params["consumer"] = json!(format!(
            "{}:{}",
            record["binding"]["harness"].as_str().unwrap_or_default(),
            record["binding"]["conversation"]
                .as_str()
                .unwrap_or_default()
        ));
    }
    if method == "feedback.wait" {
        let timeout = params
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(20_000)
            .min(30_000);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout);
        loop {
            let mut feedback = store.call("feedback.get", params.clone())?;
            let ready = feedback
                .get("submissions")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
                || !feedback["last_return"].is_null();
            if ready || std::time::Instant::now() >= deadline {
                feedback["timed_out"] = json!(!ready);
                return Ok(feedback);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if method == "review.open" {
        fill_binding(&mut params)?;
        let repository = Path::new(params["repository"].as_str().unwrap_or("."));
        let repository = mark_git::repository_root(Some(repository))
            .map_err(|error| io::Error::other(error.to_string()))?
            .canonicalize()?;
        params["repository"] = json!(repository);
        if params.get("source").is_none() {
            params["source"] = json!({"kind":"worktree"});
        }
        // Snapshot bytes come from Mark's source loader, never agent-supplied evidence.
        let options = source::options(&params["repository"], &params["source"])?;
        params["patch"] = json!(source::patch(&options)?);
    } else if method == "review.refresh" {
        if let Some(result) = store.reconcile_refresh(&params)? {
            return Ok(result);
        }
        let record = store.call("review.get", json!({"review_id":params["review_id"]}))?;
        let options = source::options(&record["binding"]["repository"], &record["source"])?;
        params["patch"] = json!(source::patch(&options)?);
        return mark_tui::refresh_integrated_review(options, params)
            .map_err(|error| io::Error::other(error.to_string()));
    }
    let browser = params.get("presentation").and_then(Value::as_str) == Some("browser");
    let fresh_patch = if method == "review.open" {
        params.get("patch").cloned()
    } else {
        None
    };
    let mut result = store.call(method, params)?;
    if method == "review.open" {
        if fresh_patch
            .as_ref()
            .is_some_and(|patch| patch != &result["snapshot"]["patch"])
        {
            let options = source::options(&result["binding"]["repository"], &result["source"])?;
            mark_tui::refresh_integrated_review(options,json!({"review_id":result["review_id"],"generation":result["generation"],"client_mutation_id":format!("open-refresh-{}",mark_session::new_session_id()),"patch":fresh_patch})).map_err(|error|io::Error::other(error.to_string()))?;
        }
        store.call("review.begin",json!({"review_id":result["review_id"],"generation":store.call("review.get",json!({"review_id":result["review_id"]}))?["generation"],"client_mutation_id":format!("open-begin-{}",mark_session::new_session_id())}))?;
        result = store.call("review.get", json!({"review_id":result["review_id"]}))?;
    }
    if method == "review.open" && browser {
        let review_id = result["review_id"]
            .as_str()
            .ok_or_else(|| io::Error::other("missing review identity"))?;
        let viewer = web::launch(review_id)?;
        result["viewer"] = viewer;
    }
    Ok(result)
}

fn fill_binding(params: &mut Value) -> io::Result<()> {
    if params.get("harness").is_none() {
        let harness = std::env::var("MARK_HARNESS")
            .ok()
            .or_else(|| {
                [
                    ("CODEX_THREAD_ID", "codex"),
                    ("CLAUDE_SESSION_ID", "claude"),
                    ("PI_SESSION_ID", "pi"),
                ]
                .into_iter()
                .find(|(key, _)| std::env::var_os(key).is_some())
                .map(|(_, name)| name.to_owned())
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing_binding: the integration must supply its harness",
                )
            })?;
        params["harness"] = json!(harness);
    }
    if params.get("conversation").is_none() {
        let native = match params["harness"].as_str() {
            Some("codex") => "CODEX_THREAD_ID",
            Some("claude") => "CLAUDE_SESSION_ID",
            Some("pi") => "PI_SESSION_ID",
            _ => "MARK_CONVERSATION_ID",
        };
        let conversation = std::env::var(native)
            .or_else(|_| std::env::var("MARK_CONVERSATION_ID"))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing_binding: the integration must supply the originating conversation ID",
                )
            })?;
        params["conversation"] = json!(conversation);
    }
    if params.get("repository").is_none() {
        params["repository"] = json!(std::env::current_dir()?);
    }
    Ok(())
}

fn read_params() -> io::Result<Value> {
    let mut bytes = Vec::new();
    io::stdin()
        .take((mark_session::MAX_REQUEST_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > mark_session::MAX_REQUEST_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request_too_large: bridge request exceeds 1 MiB",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn output(result: io::Result<Value>) -> CliResult<()> {
    match result {
        Ok(result) => write_stdout(format_args!("{}\n", json!({"ok":true,"result":result}))),
        Err(error) => {
            write_stdout(format_args!(
                "{}\n",
                json!({"ok":false,"error":{"message":error.to_string()}})
            ))?;
            Err(error.into())
        }
    }
}

fn write_result(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
