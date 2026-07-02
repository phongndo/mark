use std::collections::BTreeSet;

use crate::{
    SyntaxAddRequest, SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorIssue,
    SyntaxDoctorReport, SyntaxGrammarInfo, SyntaxLanguageRuntimeState, SyntaxLanguageSelection,
    SyntaxLanguageState, SyntaxLanguageStatus, SyntaxRemoveResult, SyntaxUpdateResult,
    SyntaxUpdateSelection, TEXTMATE_BUNDLE_VERSION, core_enabled_language_set,
    enabled_language_set, enabled_language_set_for_mode, has_highlights, installed_language_set,
    language_vec_to_set, load_config, load_settings, normalize_language_names,
    reject_core_language_removal, save_config, upsert_extension_mappings, upsert_filename_mappings,
};
use mark_core::{MarkError, MarkResult};

pub fn available_languages(filter: SyntaxAvailableFilter) -> MarkResult<Vec<String>> {
    match filter {
        SyntaxAvailableFilter::All | SyntaxAvailableFilter::Installed => {
            Ok(installed_language_set().into_iter().collect())
        }
        SyntaxAvailableFilter::Enabled => enabled_languages(),
    }
}

pub fn enabled_languages() -> MarkResult<Vec<String>> {
    Ok(enabled_language_set()?.into_iter().collect())
}

pub fn installed_languages() -> Vec<String> {
    installed_language_set().into_iter().collect()
}

pub fn language_statuses() -> MarkResult<Vec<SyntaxLanguageStatus>> {
    let settings = load_settings()?;
    let config = load_config()?;
    let available = installed_language_set();
    let enabled = enabled_language_set_for_mode(settings.mode, &config, &available);
    let mut languages = enabled
        .union(&available)
        .cloned()
        .collect::<BTreeSet<String>>();
    languages.extend(core_enabled_language_set());

    Ok(languages
        .into_iter()
        .map(|language| {
            let runtime = language_runtime_state(&language, &available);
            let state = if enabled.contains(&language) {
                SyntaxLanguageState::enabled(runtime)
            } else {
                let runtime = runtime
                    .into_available()
                    .expect("disabled status entries are drawn from available languages");
                SyntaxLanguageState::disabled(runtime)
            };
            SyntaxLanguageStatus { language, state }
        })
        .collect())
}

fn language_runtime_state(
    language: &str,
    available: &BTreeSet<String>,
) -> SyntaxLanguageRuntimeState {
    if !available.contains(language) {
        return SyntaxLanguageRuntimeState::MissingGrammar;
    }

    let grammar = SyntaxGrammarInfo::bundled(TEXTMATE_BUNDLE_VERSION);
    if has_highlights(language) {
        SyntaxLanguageRuntimeState::Ready(grammar)
    } else {
        SyntaxLanguageRuntimeState::MissingHighlights(grammar)
    }
}

pub fn add_languages(languages: &[String]) -> MarkResult<SyntaxAddResult> {
    let selection = SyntaxLanguageSelection::new(languages.to_vec())?;
    add_languages_with_options(SyntaxAddRequest::Languages(selection))
}

pub fn add_languages_with_options(request: SyntaxAddRequest) -> MarkResult<SyntaxAddResult> {
    let (requested, mapping_options) = match request {
        SyntaxAddRequest::Languages(languages) => {
            (normalize_language_names(languages.as_slice()), None)
        }
        SyntaxAddRequest::LanguageWithMappings { language, options } => {
            (normalize_language_names(&[language]), Some(options))
        }
    };
    let available = installed_language_set();
    let mut config = load_config()?;
    let mut enabled = language_vec_to_set(&config.languages);
    let mut added = Vec::new();
    let mut already_enabled = Vec::new();
    let mut unavailable = Vec::new();
    let mut custom_mappings = Vec::new();

    for language in requested {
        if let Some(options) = &mapping_options {
            let extension_mappings =
                upsert_extension_mappings(&mut config.extensions, &language, &options.extensions)?;
            custom_mappings.extend(
                extension_mappings
                    .into_iter()
                    .map(|extension| format!("*.{extension} -> {language}")),
            );
            let filename_mappings =
                upsert_filename_mappings(&mut config.filenames, &language, &options.filenames)?;
            custom_mappings.extend(
                filename_mappings
                    .into_iter()
                    .map(|filename| format!("{filename} -> {language}")),
            );
        }

        if !available.contains(&language) {
            unavailable.push(language.clone());
        }

        if enabled.insert(language.clone()) {
            added.push(language);
        } else {
            already_enabled.push(language);
        }
    }

    config.languages = enabled.into_iter().collect();
    save_config(&config)?;

    Ok(SyntaxAddResult {
        added,
        already_enabled,
        unavailable,
        custom_mappings,
    })
}

pub fn update_languages(selection: SyntaxUpdateSelection) -> MarkResult<SyntaxUpdateResult> {
    let available = installed_language_set();
    let requested = match selection {
        SyntaxUpdateSelection::All => available.clone(),
        SyntaxUpdateSelection::Languages(languages) => {
            normalize_language_names(languages.as_slice())
        }
    };
    let mut result = SyntaxUpdateResult::default();

    for language in requested {
        if available.contains(&language) {
            result.bundled.push(language);
        } else {
            result.unavailable.push(language);
        }
    }

    Ok(result)
}

pub fn remove_languages(languages: &[String]) -> MarkResult<SyntaxRemoveResult> {
    if languages.is_empty() {
        return Err(MarkError::Usage("provide at least one language".to_owned()));
    }

    let requested = normalize_language_names(languages);
    reject_core_language_removal(&requested)?;
    let mut config = load_config()?;
    let mut enabled = language_vec_to_set(&config.languages);
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for language in &requested {
        if enabled.remove(language.as_str()) {
            removed.push(language.clone());
        } else {
            missing.push(language.clone());
        }
    }
    config
        .extensions
        .retain(|mapping| !requested.contains(&mapping.language));
    config
        .filenames
        .retain(|mapping| !requested.contains(&mapping.language));

    config.languages = enabled.into_iter().collect();
    save_config(&config)?;

    Ok(SyntaxRemoveResult { removed, missing })
}

pub fn clean_cache() -> MarkResult<SyntaxCleanResult> {
    let mut config = load_config()?;
    let available = installed_language_set();
    let before = config.languages.len();
    config.languages.retain(|language| {
        available.contains(language) || core_enabled_language_set().contains(language)
    });
    let stale_records_removed = before.saturating_sub(config.languages.len());
    let enabled_languages_kept = language_vec_to_set(&config.languages).len();
    save_config(&config)?;

    Ok(SyntaxCleanResult {
        stale_records_removed,
        enabled_languages_kept,
    })
}

pub fn doctor() -> MarkResult<SyntaxDoctorReport> {
    let statuses = language_statuses()?;
    let issues = doctor_issues(&statuses);

    Ok(SyntaxDoctorReport { statuses, issues })
}

pub(crate) fn doctor_issues(statuses: &[SyntaxLanguageStatus]) -> Vec<SyntaxDoctorIssue> {
    let mut issues = Vec::new();

    for status in statuses {
        let SyntaxLanguageState::Enabled(runtime) = &status.state else {
            continue;
        };
        match runtime {
            SyntaxLanguageRuntimeState::Ready(_) => {}
            SyntaxLanguageRuntimeState::MissingGrammar => issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "enabled in config, but no bundled TextMate grammar is available; run `mark syntax rm`".to_owned(),
            }),
            SyntaxLanguageRuntimeState::MissingHighlights(_) => issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "bundled grammar is available, but highlighting failed to initialize"
                    .to_owned(),
            }),
        }
    }

    issues
}

#[cfg(test)]
pub(crate) fn update_all_language_set(
    config: &crate::StoredSyntaxConfig,
    installed: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut languages = language_vec_to_set(&config.languages);
    languages.extend(installed.iter().cloned());
    languages
}
