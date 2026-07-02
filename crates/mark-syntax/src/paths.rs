use std::{fs, path::PathBuf};

use crate::{
    COLORSCHEME_DIR, CONFIG_DIR, CONFIG_FILE, SETTINGS_FILE, SyntaxSettings, config_home,
    parse_settings,
};
use mark_core::{MarkError, MarkResult};

pub fn config_path() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE))
}

pub fn settings_path() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(SETTINGS_FILE))
}

pub fn settings_write_path() -> MarkResult<PathBuf> {
    settings_path()
}

pub fn colorscheme_dir() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(COLORSCHEME_DIR))
}

pub fn load_settings() -> MarkResult<SyntaxSettings> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(SyntaxSettings::default());
    }

    let contents = fs::read_to_string(&path)?;
    parse_settings(&contents)
        .map_err(|error| MarkError::Usage(format!("failed to parse {}: {error}", path.display())))
}

pub fn cache_dir() -> MarkResult<String> {
    config_home().map(|path| path.join(CONFIG_DIR).join("cache").display().to_string())
}
