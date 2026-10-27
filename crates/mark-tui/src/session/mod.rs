use mark_diff::DiffFile;
use mark_session::ProtocolError;

use crate::{annotation::diff_file_has_path, app::DiffApp};

pub(crate) mod anchors;
mod comments;
mod controller;
mod navigation;
mod reload;
mod runtime;
mod snapshot;

const RESPONSE_RESULT_BUDGET_BYTES: usize = mark_session::MAX_RESPONSE_FRAME_BYTES - 64 * 1024;
const COMMENT_PAGE_RESERVE_BYTES: usize = 4 * 1024;

pub(crate) use controller::handle;
pub(crate) use runtime::SessionRuntime;

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ProtocolError> {
    cursor
        .map(|cursor| {
            cursor.parse::<usize>().map_err(|_| {
                ProtocolError::new("invalid_cursor", "cursor is not a valid continuation token")
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn require_generation(app: &DiffApp, generation: u64) -> Result<(), ProtocolError> {
    if generation == app.document.generation {
        return Ok(());
    }
    Err(ProtocolError::new(
        "stale_generation",
        format!(
            "request generation {generation} does not match current generation {}",
            app.document.generation
        ),
    ))
}

fn value(value: impl serde::Serialize) -> Result<serde_json::Value, ProtocolError> {
    serde_json::to_value(value).map_err(|error| {
        ProtocolError::new(
            "internal_error",
            format!("could not encode session response: {error}"),
        )
    })
}

/// The one loaded file that `path` names, rejecting unknown and ambiguous paths.
fn find_file<'a>(app: &'a DiffApp, path: &str) -> Result<&'a DiffFile, ProtocolError> {
    let mut matches = app
        .document
        .changeset
        .files
        .iter()
        .filter(|file| diff_file_has_path(file, path));
    let file = matches.next().ok_or_else(|| {
        ProtocolError::new(
            "path_not_found",
            format!("file is not in the loaded changeset: {path}"),
        )
    })?;
    if matches.next().is_some() {
        return Err(ProtocolError::new(
            "ambiguous_path",
            format!("path matches multiple files in the changeset: {path}"),
        ));
    }
    Ok(file)
}
