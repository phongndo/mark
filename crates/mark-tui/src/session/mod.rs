use mark_diff::DiffFile;
use mark_session::ProtocolError;

use crate::{annotation::diff_file_has_path, app::DiffApp};

mod anchors;
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
