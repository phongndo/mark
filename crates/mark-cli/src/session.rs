use std::{
    borrow::Cow,
    fs,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use mark_core::MarkError;
use mark_session::{
    Client, CommentAddParams, CommentApplyParams, CommentClearParams, CommentDispositionParams,
    CommentInput, CommentListParams, CommentOrigin, CommentRemoveParams, ContextParams,
    EmptyParams, FindingDisposition, GenerationParams, METHOD_COMMENT_ADD, METHOD_COMMENT_APPLY,
    METHOD_COMMENT_CLEAR, METHOD_COMMENT_DISPOSITION, METHOD_COMMENT_LIST, METHOD_COMMENT_REMOVE,
    METHOD_CONTEXT_GET, METHOD_NAVIGATE, METHOD_PATCH_GET, METHOD_PROGRESS_SET, METHOD_RELOAD,
    METHOD_REVIEW_GET, METHOD_SESSION_GET, METHOD_VERDICT_CLEAR, METHOD_VERDICT_GET,
    METHOD_VERDICT_SET, NavigateParams, NavigateTarget, PatchParams, ProgressSetParams,
    RangeTarget, Registry, ReloadParams, ReloadRequest, Request, Response, ReviewAnchor,
    ReviewAnchorScope, ReviewParams, SelectionError, SessionListing, SourceRange,
    VerdictDestination, VerdictKind, VerdictSetParams,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    CliResult,
    args::{
        CommentOriginArg, CommentTargetArgs, FindingDispositionArg, SessionCommentAddArgs,
        SessionCommentApplyArgs, SessionCommentClearArgs, SessionCommentCommand,
        SessionCommentDispositionArgs, SessionCommentListArgs, SessionCommentRemoveArgs,
        SessionContextArgs, SessionGetArgs, SessionListArgs, SessionNavigateArgs, SessionPatchArgs,
        SessionProgressArgs, SessionReloadArgs, SessionReviewArgs, SessionSelectorArgs,
        SessionSubcommand, SessionVerdictCommand, SessionVerdictSetArgs, VerdictDestinationArg,
        VerdictKindArg,
    },
    write_stdout, write_stdout_bytes,
};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn session(command: SessionSubcommand) -> CliResult<()> {
    match command {
        SessionSubcommand::List(args) => list(args),
        SessionSubcommand::Get(args) => request_empty(args, METHOD_SESSION_GET),
        SessionSubcommand::Context(args) => context(args),
        SessionSubcommand::Review(args) => review(args),
        SessionSubcommand::Patch(args) => patch(args),
        SessionSubcommand::Navigate(args) => navigate(args),
        SessionSubcommand::Comment { command } => comment(command),
        SessionSubcommand::Progress(args) => progress(args),
        SessionSubcommand::Verdict { command } => verdict(command),
        SessionSubcommand::Reload(args) => reload(args),
    }
}

fn list(args: SessionListArgs) -> CliResult<()> {
    let registry = Registry::discover()?;
    let repository = args.repo.as_deref().map(canonical_repository).transpose()?;
    let sessions = registry
        .list()?
        .into_iter()
        .filter(|session| {
            repository
                .as_ref()
                .is_none_or(|repository| Path::new(&session.record.repository) == repository)
        })
        .collect::<Vec<_>>();
    if args.json {
        let sessions = sessions
            .iter()
            .map(|session| {
                json!({
                    "session_id": session.record.session_id,
                    "process_id": session.record.process_id,
                    "protocol": session.record.protocol,
                    "repository": session.record.repository,
                    "working_directory": session.record.working_directory,
                    "source": session.record.source,
                    "responsive": session.responsive,
                })
            })
            .collect::<Vec<_>>();
        return print_json(&Response::success("cli", json!({ "sessions": sessions })));
    }
    if sessions.is_empty() {
        write_stdout(format_args!("no live sessions\n"))?;
    } else {
        for session in sessions {
            let repository = terminal_safe_field(&session.record.repository);
            let source = terminal_safe_field(&session.record.source);
            write_stdout(format_args!(
                "{}\t{}\t{}\n",
                session.record.session_id, repository, source
            ))?;
        }
    }
    Ok(())
}

fn request_empty(args: SessionGetArgs, method: &str) -> CliResult<()> {
    let response = send(&args.selector, method, &EmptyParams {})?;
    output_response(response, args.json, HumanOutput::Json)
}

fn context(args: SessionContextArgs) -> CliResult<()> {
    let response = send(
        &args.selector,
        METHOD_CONTEXT_GET,
        &ContextParams {
            changed_files_cursor: args.changed_files_cursor,
            changed_files_limit: args.changed_files_limit,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn review(args: SessionReviewArgs) -> CliResult<()> {
    let response = send(
        &args.selector,
        METHOD_REVIEW_GET,
        &ReviewParams {
            cursor: args.cursor,
            limit: args.limit,
            include_comments: args.include_comments,
            comments_cursor: args.comments_cursor,
            comments_limit: args.comments_limit,
            changed_only: args.changed_only,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn patch(args: SessionPatchArgs) -> CliResult<()> {
    let response = send(
        &args.selector,
        METHOD_PATCH_GET,
        &PatchParams {
            file: args.file,
            hunk: args.hunk,
            old_line: args.old_line,
            new_line: args.new_line,
            context: args.context,
            max_bytes: args.max_bytes,
            cursor: args.cursor,
        },
    )?;
    output_response(response, args.json, HumanOutput::Patch)
}

fn navigate(args: SessionNavigateArgs) -> CliResult<()> {
    let target = if args.next_comment {
        require_absent_file(&args.file)?;
        NavigateTarget::NextComment
    } else if args.previous_comment {
        require_absent_file(&args.file)?;
        NavigateTarget::PreviousComment
    } else {
        let file = args
            .file
            .ok_or_else(|| MarkError::Usage("navigation to code requires --file".to_owned()))?;
        let file_scope = args.hunk.is_none() && args.old_line.is_none() && args.new_line.is_none();
        NavigateTarget::Anchor {
            anchor: ReviewAnchor {
                file,
                scope: file_scope.then_some(ReviewAnchorScope::File),
                hunk: args.hunk,
                old_line: args.old_line,
                new_line: args.new_line,
                range: None,
            },
        }
    };
    let response = send(&args.selector, METHOD_NAVIGATE, &NavigateParams { target })?;
    output_response(response, args.json, HumanOutput::Json)
}

fn comment(command: SessionCommentCommand) -> CliResult<()> {
    match command {
        SessionCommentCommand::Add(args) => comment_add(args),
        SessionCommentCommand::Apply(args) => comment_apply(args),
        SessionCommentCommand::List(args) => comment_list(args),
        SessionCommentCommand::Rm(args) => comment_remove(args),
        SessionCommentCommand::Clear(args) => comment_clear(args),
        SessionCommentCommand::Disposition(args) => comment_disposition(args),
    }
}

fn comment_add(args: SessionCommentAddArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = match args.generation {
        Some(generation) => generation,
        None => current_generation(&selected)?,
    };
    let anchor = comment_anchor(args.file, args.target)?;
    let response = send_to(
        &selected,
        METHOD_COMMENT_ADD,
        &CommentAddParams {
            generation,
            comment: CommentInput {
                anchor,
                summary: args.summary,
                rationale: args.rationale,
                author: args.author,
            },
            focus: args.focus,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

#[derive(Debug, Deserialize)]
struct StdinCommentBatch {
    #[serde(default)]
    generation: Option<u64>,
    comments: Vec<CommentInput>,
}

fn comment_apply(args: SessionCommentApplyArgs) -> CliResult<()> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take((mark_session::MAX_REQUEST_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > mark_session::MAX_REQUEST_FRAME_BYTES {
        return Err(MarkError::Usage("comment batch exceeds input limit".to_owned()).into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| MarkError::Usage(format!("invalid comment batch JSON: {error}")))?;
    let batch = if value.is_array() {
        StdinCommentBatch {
            generation: None,
            comments: serde_json::from_value(value).map_err(|error| {
                MarkError::Usage(format!("invalid comment batch JSON: {error}"))
            })?,
        }
    } else {
        serde_json::from_value(value)
            .map_err(|error| MarkError::Usage(format!("invalid comment batch JSON: {error}")))?
    };
    let selected = select_session(&args.selector)?;
    let generation = match batch.generation {
        Some(generation) => generation,
        None => current_generation(&selected)?,
    };
    let response = send_to(
        &selected,
        METHOD_COMMENT_APPLY,
        &CommentApplyParams {
            generation,
            comments: batch.comments,
            focus: args.focus,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn comment_list(args: SessionCommentListArgs) -> CliResult<()> {
    let origin = match args.origin {
        CommentOriginArg::Agent => CommentOrigin::Agent,
        CommentOriginArg::Human => CommentOrigin::Human,
        CommentOriginArg::All => CommentOrigin::All,
    };
    let response = send(
        &args.selector,
        METHOD_COMMENT_LIST,
        &CommentListParams {
            file: args.file,
            origin,
            cursor: args.cursor,
            limit: args.limit,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn comment_remove(args: SessionCommentRemoveArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = current_generation(&selected)?;
    let response = send_to(
        &selected,
        METHOD_COMMENT_REMOVE,
        &CommentRemoveParams {
            generation,
            id: args.comment_id,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn comment_clear(args: SessionCommentClearArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = current_generation(&selected)?;
    let response = send_to(
        &selected,
        METHOD_COMMENT_CLEAR,
        &CommentClearParams {
            generation,
            file: args.file,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn comment_disposition(args: SessionCommentDispositionArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = current_generation(&selected)?;
    let disposition = match args.disposition {
        FindingDispositionArg::Open => FindingDisposition::Open,
        FindingDispositionArg::Accepted => FindingDisposition::Accepted,
        FindingDispositionArg::Dismissed => FindingDisposition::Dismissed,
        FindingDispositionArg::Blocking => FindingDisposition::Blocking,
        FindingDispositionArg::NonBlocking => FindingDisposition::NonBlocking,
        FindingDispositionArg::Fixed => FindingDisposition::Fixed,
    };
    let response = send_to(
        &selected,
        METHOD_COMMENT_DISPOSITION,
        &CommentDispositionParams {
            generation,
            id: args.comment_id,
            disposition,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn progress(args: SessionProgressArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = current_generation(&selected)?;
    let response = send_to(
        &selected,
        METHOD_PROGRESS_SET,
        &ProgressSetParams {
            generation,
            file: args.file,
            hunk: args.hunk,
            reviewed: args.reviewed || !args.unreviewed,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn verdict(command: SessionVerdictCommand) -> CliResult<()> {
    match command {
        SessionVerdictCommand::Get(args) => request_empty(args, METHOD_VERDICT_GET),
        SessionVerdictCommand::Set(args) => verdict_set(args),
        SessionVerdictCommand::Clear(args) => {
            let selected = select_session(&args.selector)?;
            let generation = current_generation(&selected)?;
            let response = send_to(
                &selected,
                METHOD_VERDICT_CLEAR,
                &GenerationParams { generation },
            )?;
            output_response(response, args.json, HumanOutput::Json)
        }
    }
}

fn verdict_set(args: SessionVerdictSetArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = current_generation(&selected)?;
    let kind = match args.kind {
        VerdictKindArg::Approve => VerdictKind::Approve,
        VerdictKindArg::RequestChanges => VerdictKind::RequestChanges,
        VerdictKindArg::Comment => VerdictKind::Comment,
    };
    let destination = match args.destination {
        VerdictDestinationArg::Local => VerdictDestination::Local,
        VerdictDestinationArg::Stdout => VerdictDestination::Stdout,
    };
    let response = send_to(
        &selected,
        METHOD_VERDICT_SET,
        &VerdictSetParams {
            generation,
            kind,
            summary: args.summary,
            destination,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn reload(args: SessionReloadArgs) -> CliResult<()> {
    let selected = select_session(&args.selector)?;
    let generation = match args.generation {
        Some(generation) => generation,
        None => current_generation(&selected)?,
    };
    let request = parse_reload_request(&args.request)?;
    let response = send_to(
        &selected,
        METHOD_RELOAD,
        &ReloadParams {
            generation,
            request,
        },
    )?;
    output_response(response, args.json, HumanOutput::Json)
}

fn parse_reload_request(arguments: &[String]) -> CliResult<ReloadRequest> {
    let Some((kind, rest)) = arguments.split_first() else {
        return Err(MarkError::Usage("reload requires diff or show".to_owned()).into());
    };
    if !matches!(kind.as_str(), "diff" | "show") {
        return Err(MarkError::Usage("reload accepts only diff or show".to_owned()).into());
    }
    let separator = rest.iter().position(|argument| argument == "--");
    let (before_paths, pathspecs) = match separator {
        Some(index) => (&rest[..index], rest[index + 1..].to_vec()),
        None => (rest, Vec::new()),
    };
    if before_paths.len() > 1 {
        return Err(
            MarkError::Usage("reload accepts at most one revision before --".to_owned()).into(),
        );
    }
    let rev = before_paths.first().cloned();
    Ok(if kind == "diff" {
        ReloadRequest::Diff { rev, pathspecs }
    } else {
        ReloadRequest::Show { rev, pathspecs }
    })
}

fn comment_anchor(file: String, target: CommentTargetArgs) -> CliResult<ReviewAnchor> {
    let has_range = target.old_start.is_some() || target.new_start.is_some();
    let target_count = usize::from(target.hunk.is_some())
        + usize::from(target.old_line.is_some())
        + usize::from(target.new_line.is_some())
        + usize::from(has_range);
    if target_count > 1 {
        return Err(MarkError::Usage(
            "comment targets are mutually exclusive: hunk, old line, new line, or range".to_owned(),
        )
        .into());
    }
    let range = if has_range {
        Some(RangeTarget {
            old: source_range(target.old_start, target.old_end, "old")?,
            new: source_range(target.new_start, target.new_end, "new")?,
        })
    } else {
        None
    };
    Ok(ReviewAnchor {
        file,
        scope: (target_count == 0).then_some(ReviewAnchorScope::File),
        hunk: target.hunk,
        old_line: target.old_line,
        new_line: target.new_line,
        range,
    })
}

fn source_range(
    start: Option<usize>,
    end: Option<usize>,
    side: &str,
) -> CliResult<Option<SourceRange>> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if start > 0 && end >= start => {
            Ok(Some(SourceRange { start, end }))
        }
        (Some(_), Some(_)) => Err(MarkError::Usage(format!(
            "{side} range must be positive and end at or after start"
        ))
        .into()),
        _ => Err(MarkError::Usage(format!("{side} range requires both start and end")).into()),
    }
}

fn current_generation(selected: &SessionListing) -> CliResult<u64> {
    let response = send_to(selected, METHOD_CONTEXT_GET, &ContextParams::default())?;
    successful_result(&response)?["generation"]
        .as_u64()
        .ok_or_else(|| MarkError::Usage("session context omitted generation".to_owned()).into())
}

fn send(
    selector: &SessionSelectorArgs,
    method: &str,
    params: &impl serde::Serialize,
) -> CliResult<Response> {
    let selected = select_session(selector)?;
    send_to(&selected, method, params)
}

fn send_to(
    selected: &SessionListing,
    method: &str,
    params: &impl serde::Serialize,
) -> CliResult<Response> {
    let params = serde_json::to_value(params)
        .map_err(|error| MarkError::Usage(format!("could not encode session request: {error}")))?;
    let request = Request::new(next_request_id(), method, params);
    Ok(Client::new(&selected.record.endpoint).request(&request)?)
}

fn select_session(selector: &SessionSelectorArgs) -> CliResult<SessionListing> {
    let registry = Registry::discover()?;
    let repository = selector
        .repo
        .as_deref()
        .map(canonical_repository)
        .transpose()?;
    registry
        .select(selector.session_id.as_deref(), repository.as_deref())
        .map_err(selection_error)
}

fn canonical_repository(path: &Path) -> CliResult<PathBuf> {
    let path = fs::canonicalize(path)?;
    match mark_git::repository_root(Some(&path)) {
        Ok(root) => Ok(fs::canonicalize(root)?),
        Err(MarkError::Usage(_)) => Ok(path),
        Err(error) => Err(error.into()),
    }
}

fn selection_error(error: SelectionError) -> crate::CliError {
    let message = match error {
        SelectionError::Ambiguous(ids) => format!(
            "ambiguous_session: select a session explicitly ({})",
            ids.join(", ")
        ),
        SelectionError::NotFound => "session_not_found: no live Mark session matched".to_owned(),
        SelectionError::InvalidRepository(path) => {
            format!("invalid_repository: repository does not exist: {path}")
        }
        SelectionError::Io(error) => format!("session_registry_error: {error}"),
    };
    MarkError::Usage(message).into()
}

fn next_request_id() -> String {
    format!(
        "cli-{}-{}",
        std::process::id(),
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn successful_result(response: &Response) -> CliResult<&Value> {
    if response.ok {
        return response
            .result
            .as_ref()
            .ok_or_else(|| MarkError::Usage("session returned no result".to_owned()).into());
    }
    let error = response
        .error
        .as_ref()
        .ok_or_else(|| MarkError::Usage("session returned an invalid error response".to_owned()))?;
    let code = terminal_safe_field(&error.code);
    let message = terminal_safe_field(&error.message);
    Err(MarkError::Usage(format!("{code}: {message}")).into())
}

#[derive(Debug, Clone, Copy)]
enum HumanOutput {
    Json,
    Patch,
}

fn output_response(response: Response, json_output: bool, human: HumanOutput) -> CliResult<()> {
    if json_output {
        print_json(&response)?;
        return successful_result(&response).map(|_| ());
    }
    let result = successful_result(&response)?;
    match human {
        HumanOutput::Patch => {
            let patch = result["patch"].as_str().unwrap_or_default();
            if io::stdout().is_terminal() {
                write_stdout_bytes(&crate::pager::sanitized_terminal_bytes(patch.as_bytes()))?;
            } else {
                write_stdout(format_args!("{patch}"))?;
            }
            if !patch.ends_with('\n') {
                write_stdout(format_args!("\n"))?;
            }
        }
        HumanOutput::Json => {
            let text = serde_json::to_string_pretty(result).map_err(|error| {
                MarkError::Usage(format!("could not render session response: {error}"))
            })?;
            write_stdout(format_args!("{text}\n"))?;
        }
    }
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> CliResult<()> {
    let text = serde_json::to_string(value)
        .map_err(|error| MarkError::Usage(format!("could not render JSON output: {error}")))?;
    write_stdout(format_args!("{text}\n"))
}

fn terminal_safe_field(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    Cow::Owned(escaped)
}

fn require_absent_file(file: &Option<String>) -> CliResult<()> {
    if file.is_some() {
        return Err(MarkError::Usage(
            "--file cannot be combined with comment navigation".to_owned(),
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_table_fields_escape_row_and_terminal_controls() {
        assert_eq!(
            terminal_safe_field("repo\tname\n\x1b]52;c;payload\x07"),
            "repo\\tname\\n\\u{1b}]52;c;payload\\u{7}"
        );
    }

    #[test]
    fn empty_comment_target_selects_the_file_scope() {
        let anchor = comment_anchor("src/lib.rs".to_owned(), CommentTargetArgs::default()).unwrap();

        assert_eq!(anchor.scope, Some(ReviewAnchorScope::File));
    }

    #[test]
    fn canonical_repository_accepts_a_non_git_directory() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        let expected = fs::canonicalize(temp.path()).unwrap();

        assert_eq!(canonical_repository(temp.path()).unwrap(), expected);
    }

    #[test]
    fn protocol_errors_escape_terminal_controls() {
        let response = Response::failure(
            "cli",
            mark_session::ProtocolError::new(
                "path_not_found",
                "missing: unsafe\u{1b}]52;c;payload\u{7}",
            ),
        );

        let error = successful_result(&response).unwrap_err().to_string();

        assert!(!error.contains('\u{1b}'));
        assert!(!error.contains('\u{7}'));
        assert!(error.contains("unsafe\\u{1b}]52;c;payload\\u{7}"));
    }

    #[test]
    fn reload_parser_accepts_closed_diff_and_show_forms() {
        assert_eq!(
            parse_reload_request(&[
                "diff".to_owned(),
                "main".to_owned(),
                "--".to_owned(),
                "src".to_owned(),
            ])
            .unwrap(),
            ReloadRequest::Diff {
                rev: Some("main".to_owned()),
                pathspecs: vec!["src".to_owned()],
            }
        );
        assert_eq!(
            parse_reload_request(&["show".to_owned()]).unwrap(),
            ReloadRequest::Show {
                rev: None,
                pathspecs: Vec::new(),
            }
        );
    }

    #[test]
    fn reload_parser_rejects_shell_text_and_unknown_commands() {
        assert!(parse_reload_request(&["status".to_owned()]).is_err());
        assert!(
            parse_reload_request(&["diff".to_owned(), "HEAD".to_owned(), "&&".to_owned()]).is_err()
        );
    }
}
