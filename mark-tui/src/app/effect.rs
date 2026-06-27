use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppEffect {
    Quit,
    Reload,
    OpenFocusedHunkInEditor,
    Toast(ToastLevel, String),
    CopyToClipboard {
        text: String,
        success_message: String,
        error_prefix: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ActionOutcome {
    pub(crate) consumed: bool,
    pub(crate) effects: Vec<AppEffect>,
}

impl ActionOutcome {
    pub(crate) fn ignored() -> Self {
        Self::default()
    }

    pub(crate) fn consumed() -> Self {
        Self {
            consumed: true,
            effects: Vec::new(),
        }
    }

    pub(crate) fn effect(effect: AppEffect) -> Self {
        Self {
            consumed: true,
            effects: vec![effect],
        }
    }

    pub(crate) fn into_legacy_quit(self) -> Option<bool> {
        if self
            .effects
            .iter()
            .any(|effect| matches!(effect, AppEffect::Quit))
        {
            Some(true)
        } else if self.consumed {
            Some(false)
        } else {
            None
        }
    }
}

impl DiffApp {
    pub(crate) fn run_effects(&mut self, effects: Vec<AppEffect>) -> MarkResult<()> {
        for effect in effects {
            self.run_effect(effect)?;
        }
        Ok(())
    }

    pub(crate) fn run_effect(&mut self, effect: AppEffect) -> MarkResult<()> {
        match effect {
            AppEffect::Quit => Ok(()),
            AppEffect::Reload => self.reload(),
            AppEffect::OpenFocusedHunkInEditor => {
                self.open_focused_hunk_in_editor();
                Ok(())
            }
            AppEffect::Toast(level, text) => {
                if self.notifications.toasts.push(level, text) {
                    self.runtime.dirty = true;
                }
                Ok(())
            }
            AppEffect::CopyToClipboard {
                text,
                success_message,
                error_prefix,
            } => {
                let mut stdout = io::stdout().lock();
                match write_osc52_clipboard(&mut stdout, &text) {
                    Ok(()) => self.set_success_notice(success_message),
                    Err(error) => self.set_error_log(format!("{error_prefix}: {error}")),
                }
                Ok(())
            }
        }
    }
}
