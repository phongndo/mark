use std::path::{Component, Path, PathBuf};

use mark_diff::{DiffOptions, DiffOutput, DiffSource, RevSpec};
use mark_session::{
    ProtocolError, ReloadParams, ReloadRequest, ReloadResult, Response, SessionCommand,
};

use crate::{app::DiffApp, runtime};

pub(crate) fn handle(app: &mut DiffApp, command: SessionCommand) {
    let id = command.request.id.clone();
    let params: ReloadParams = match serde_json::from_value(command.request.params) {
        Ok(params) => params,
        Err(error) => {
            let _ = command.reply.send(Response::failure(
                id,
                ProtocolError::new(
                    "invalid_params",
                    format!("invalid reload parameters: {error}"),
                ),
            ));
            return;
        }
    };
    if params.generation != app.document.generation {
        let _ = command.reply.send(Response::failure(
            id,
            ProtocolError::new(
                "stale_generation",
                format!(
                    "request generation {} does not match current generation {}",
                    params.generation, app.document.generation
                ),
            ),
        ));
        return;
    }
    let (source, pathspecs) = match reload_options(params.request) {
        Ok(options) => options,
        Err(error) => {
            let _ = command.reply.send(Response::failure(id, error));
            return;
        }
    };
    let options = DiffOptions {
        repo: Some(app.document.changeset.repo.as_path().to_path_buf().into()),
        source,
        local_untracked: app.document.options.local_untracked,
        output: DiffOutput::Patch,
    };
    let Some(completion) = app.start_session_diff_load(options, pathspecs) else {
        let _ = command.reply.send(Response::failure(
            id,
            ProtocolError::new(
                "reload_in_progress",
                "another diff load is already in progress",
            ),
        ));
        return;
    };
    runtime::spawn(async move {
        let response = match completion.await {
            Ok(Ok(generation)) => success_response(id, ReloadResult { generation }),
            Ok(Err(message)) => Response::failure(id, ProtocolError::new("reload_failed", message)),
            Err(_) => Response::failure(
                id,
                ProtocolError::new("session_unavailable", "reload stopped before completion"),
            ),
        };
        let _ = command.reply.send(response);
    });
}

fn success_response(id: String, result: impl serde::Serialize) -> Response {
    match serde_json::to_value(result) {
        Ok(result) => Response::success(id, result),
        Err(error) => Response::failure(
            id,
            ProtocolError::new(
                "internal_error",
                format!("could not encode reload response: {error}"),
            ),
        ),
    }
}

fn reload_options(request: ReloadRequest) -> Result<(DiffSource, Vec<PathBuf>), ProtocolError> {
    let (source, pathspecs) = match request {
        ReloadRequest::Diff { rev, pathspecs } => {
            validate_revision(rev.as_deref())?;
            let source = rev.map_or(DiffSource::Worktree, |rev| {
                DiffSource::Base(RevSpec::new(rev))
            });
            (source, pathspecs)
        }
        ReloadRequest::Show { rev, pathspecs } => {
            validate_revision(rev.as_deref())?;
            (
                DiffSource::Show(RevSpec::new(rev.unwrap_or_else(|| "HEAD".to_owned()))),
                pathspecs,
            )
        }
    };
    let pathspecs = pathspecs
        .into_iter()
        .map(validate_pathspec)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((source, pathspecs))
}

fn validate_revision(revision: Option<&str>) -> Result<(), ProtocolError> {
    if revision.is_some_and(|revision| {
        revision.is_empty()
            || revision.len() > mark_session::MAX_PATH_BYTES
            || revision.contains(['\0', '\n', '\r'])
    }) {
        return Err(ProtocolError::new(
            "invalid_reload",
            "reload revision is empty or exceeds safe text limits",
        ));
    }
    Ok(())
}

fn validate_pathspec(pathspec: String) -> Result<PathBuf, ProtocolError> {
    if pathspec.is_empty()
        || pathspec.len() > mark_session::MAX_PATH_BYTES
        || pathspec.contains(['\0', '\n', '\r'])
    {
        return Err(ProtocolError::new(
            "invalid_reload",
            "reload pathspec is empty or exceeds safe text limits",
        ));
    }
    if pathspec.starts_with(':') || pathspec.contains(['*', '?', '[', ']']) {
        return Err(ProtocolError::new(
            "invalid_reload",
            "reload pathspec patterns are not supported for scoped reloads",
        ));
    }
    let path = Path::new(&pathspec);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(ProtocolError::new(
            "invalid_reload",
            "reload pathspec must be a normalized path within the repository",
        ));
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingResult;

    impl serde::Serialize for FailingResult {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("serialization failed"))
        }
    }

    #[test]
    fn reload_serialization_failures_return_an_internal_error() {
        let response = success_response("request-1".to_owned(), FailingResult);

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "internal_error");
    }

    #[test]
    fn reload_request_is_closed_and_rejects_escaping_paths() {
        assert!(
            reload_options(ReloadRequest::Diff {
                rev: None,
                pathspecs: vec!["../secret".to_owned()],
            })
            .is_err()
        );
        assert!(
            reload_options(ReloadRequest::Show {
                rev: Some("HEAD".to_owned()),
                pathspecs: vec!["src/lib.rs".to_owned()],
            })
            .is_ok()
        );
        assert!(
            reload_options(ReloadRequest::Diff {
                rev: None,
                pathspecs: vec!["src/*.rs".to_owned()],
            })
            .is_err()
        );
    }
}
