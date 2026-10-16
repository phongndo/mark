use super::DiffApp;
#[cfg(test)]
use super::{AppAction, AppEffect};
#[cfg(test)]
use mark_core::MarkResult;
use std::io::{self, Write};

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn osc52_clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

pub(crate) fn write_osc52_clipboard<W: Write>(writer: &mut W, text: &str) -> io::Result<()> {
    writer.write_all(osc52_clipboard_sequence(text).as_bytes())?;
    writer.flush()
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(BASE64[(first >> 2) as usize] as char);
        encoded.push(BASE64[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(BASE64[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(BASE64[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

impl DiffApp {
    /// Runs an action like `perform_app_action`, but sends clipboard effects
    /// to `clipboard` instead of the terminal so tests can inspect them.
    #[cfg(test)]
    pub(crate) fn perform_app_action_with_clipboard<W: Write>(
        &mut self,
        action: AppAction,
        clipboard: &mut W,
    ) -> MarkResult<()> {
        let outcome = self.perform_app_action_with_effects(action)?;
        for effect in outcome.into_effects() {
            match effect {
                AppEffect::CopyToClipboard {
                    text,
                    success_message,
                    error_prefix,
                } => self.copy_to_clipboard(clipboard, &text, success_message, &error_prefix),
                effect => self.run_effect(effect)?,
            }
        }
        Ok(())
    }

    pub(super) fn copy_to_clipboard<W: Write>(
        &mut self,
        writer: &mut W,
        text: &str,
        success_message: String,
        error_prefix: &str,
    ) {
        match write_osc52_clipboard(writer, text) {
            Ok(()) => self.set_success_notice(success_message),
            Err(error) => self.set_error_log(format!("{error_prefix}: {error}")),
        }
    }
}
