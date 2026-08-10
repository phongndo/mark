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
