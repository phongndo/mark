use std::{cell::RefCell, env, fs, path::PathBuf};

use mark_core::{MarkError, MarkResult};
use mark_diff::{DiffOptions, DiffSource, PatchSource};
use mark_session::{
    PROTOCOL_VERSION, Registry, SESSION_COMMAND_CHANNEL_CAPACITY, ServerHandle, SessionCommand,
    SessionRecord, current_process_identity, new_session_id, spawn_server,
};
use tokio::sync::{mpsc, oneshot};

use crate::app::DiffApp;

use super::snapshot::PatchCache;

pub(crate) struct SessionRuntime {
    pub(crate) record: SessionRecord,
    pub(crate) commands: mpsc::Receiver<SessionCommand>,
    pub(super) patch_cache: RefCell<Option<PatchCache>>,
    startup: Option<oneshot::Receiver<Result<(), String>>>,
    _server: ServerHandle,
    active: bool,
}

impl SessionRuntime {
    pub(crate) fn start(app: &DiffApp) -> MarkResult<Self> {
        let registry = Registry::discover()?;
        let session_id = new_session_id();
        let endpoint = registry.socket_path(&session_id)?;
        let working_directory = canonical_working_directory()?;
        let repository = canonical_repository(app, &working_directory);
        let record = SessionRecord {
            session_id,
            process_id: std::process::id(),
            process_identity: current_process_identity(),
            protocol: PROTOCOL_VERSION,
            repository: repository.display().to_string(),
            working_directory: working_directory.display().to_string(),
            source: source_label(&app.document.options),
            endpoint: endpoint.display().to_string(),
        };
        let (commands_tx, commands) = mpsc::channel(SESSION_COMMAND_CHANNEL_CAPACITY);
        let (_server, startup) = spawn_server(registry, record.clone(), commands_tx);
        Ok(Self {
            record,
            commands,
            patch_cache: RefCell::new(None),
            startup: Some(startup),
            _server,
            active: true,
        })
    }

    pub(crate) fn drain_startup(&mut self, app: &mut DiffApp) {
        let Some(startup) = self.startup.as_mut() else {
            return;
        };
        match startup.try_recv() {
            Ok(Ok(())) => self.startup = None,
            Ok(Err(error)) => {
                self.startup = None;
                self.active = false;
                app.set_error_log(format!("live session unavailable: {error}"));
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                self.startup = None;
                self.active = false;
                app.set_error_log("live session stopped during startup");
            }
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn mark_closed(&mut self) {
        self.active = false;
    }
}

fn canonical_working_directory() -> MarkResult<PathBuf> {
    let current = env::current_dir()?;
    fs::canonicalize(&current).map_err(|error| {
        MarkError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "could not resolve working directory {}: {error}",
                current.display()
            ),
        ))
    })
}

fn canonical_repository(app: &DiffApp, fallback: &std::path::Path) -> PathBuf {
    let repo = app.document.changeset.repo.as_path();
    if repo.as_os_str().is_empty() {
        return fallback.to_path_buf();
    }
    fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf())
}

pub(crate) fn source_label(options: &DiffOptions) -> String {
    match &options.source {
        DiffSource::Worktree => "worktree".to_owned(),
        DiffSource::Show(rev) => format!("show {rev}"),
        DiffSource::Base(rev) => format!("diff {rev}"),
        DiffSource::Branch { base, head } => format!("diff {base}...{head}"),
        DiffSource::Range { left, right } => format!("diff {left}..{right}"),
        DiffSource::Difftool { path, .. } => path
            .as_ref()
            .map_or_else(|| "difftool".to_owned(), |path| format!("difftool {path}")),
        DiffSource::Patch(PatchSource::File(path)) => format!("patch {}", path.display()),
        DiffSource::Patch(PatchSource::Stdin(_)) => "patch stdin".to_owned(),
        DiffSource::Patch(PatchSource::Text { label, .. })
        | DiffSource::Patch(PatchSource::Review { label, .. }) => label.to_string(),
    }
}
