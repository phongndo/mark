use std::collections::BTreeSet;

use crate::{
    SyntaxAddOptions, SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorIssue,
    SyntaxDoctorReport, SyntaxGrammarState, SyntaxHighlightState, SyntaxLanguageEnablement,
    SyntaxLanguageStatus, SyntaxRemoveResult, SyntaxUpdateResult, TEXTMATE_BUNDLE_VERSION,
    core_enabled_language_set, enabled_language_set, enabled_language_set_for_mode, has_highlights,
    installed_language_set, language_vec_to_set, load_config, load_settings,
    normalize_language_names, reject_core_language_removal, save_config, upsert_extension_mappings,
    upsert_filename_mappings,
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
            let available = available.contains(&language);
            SyntaxLanguageStatus {
                enablement: if enabled.contains(&language) {
                    SyntaxLanguageEnablement::Enabled
                } else {
                    SyntaxLanguageEnablement::Disabled
                },
                grammar: if available {
                    SyntaxGrammarState::Bundled
                } else {
                    SyntaxGrammarState::Unavailable
                },
                highlighting: if available && has_highlights(&language) {
                    SyntaxHighlightState::Ready
                } else {
                    SyntaxHighlightState::Unavailable
                },
                version: available.then(|| TEXTMATE_BUNDLE_VERSION.to_owned()),
                source: available.then(|| "bundled".to_owned()),
                language,
            }
        })
        .collect())
}

pub fn add_languages(languages: &[String]) -> MarkResult<SyntaxAddResult> {
    add_languages_with_options(languages, SyntaxAddOptions::default())
}

pub fn add_languages_with_options(
    languages: &[String],
    options: SyntaxAddOptions,
) -> MarkResult<SyntaxAddResult> {
    if languages.is_empty() {
        return Err(MarkError::Usage("provide at least one language".to_owned()));
    }

    let has_custom_mappings = !options.extensions.is_empty() || !options.filenames.is_empty();
    if has_custom_mappings && languages.len() != 1 {
        return Err(MarkError::Usage(
            "use --ext or --filename with exactly one language".to_owned(),
        ));
    }

    let requested = normalize_language_names(languages);
    let available = installed_language_set();
    let mut config = load_config()?;
    let mut enabled = language_vec_to_set(&config.languages);
    let mut added = Vec::new();
    let mut already_enabled = Vec::new();
    let mut without_highlights = Vec::new();
    let mut custom_mappings = Vec::new();

    for language in requested {
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

        if !available.contains(&language) {
            without_highlights.push(language.clone());
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
        without_highlights,
        custom_mappings,
    })
}

pub fn update_languages(languages: &[String], all: bool) -> MarkResult<SyntaxUpdateResult> {
    if all && !languages.is_empty() {
        return Err(MarkError::Usage(
            "use `mark syntax update --all` without language names".to_owned(),
        ));
    }
    if !all && languages.is_empty() {
        return Err(MarkError::Usage(
            "provide at least one language or use --all".to_owned(),
        ));
    }

    let available = installed_language_set();
    let requested = if all {
        available.clone()
    } else {
        normalize_language_names(languages)
    };
    let mut result = SyntaxUpdateResult::default();

    for language in requested {
        if available.contains(&language) {
            result.bundled.push(language);
        } else {
            result.unavailable.push(language.clone());
            result.without_highlights.push(language);
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
        if !status.enablement.is_enabled() {
            continue;
        }
        if !status.grammar.is_available() {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "enabled in config, but no bundled TextMate grammar is available; run `mark syntax rm`".to_owned(),
            });
            continue;
        }
        if !status.highlighting.is_ready() {
            issues.push(SyntaxDoctorIssue {
                language: status.language.clone(),
                message: "bundled grammar is available, but highlighting failed to initialize"
                    .to_owned(),
            });
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
