use std::collections::BTreeSet;

use crate::{
    BUNDLED_GRAMMAR_VERSION, SyntaxAddOptions, SyntaxAddRequest, SyntaxAddResult,
    SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorIssue, SyntaxDoctorReport,
    SyntaxGrammarInfo, SyntaxLanguageRuntimeState, SyntaxLanguageSelection, SyntaxLanguageState,
    SyntaxLanguageStatus, SyntaxRemoveResult, SyntaxUpdateResult, SyntaxUpdateSelection,
    core_enabled_language_set, core_language_set, enabled_language_set,
    enabled_language_set_for_mode, has_highlights, installed_language_set, language_vec_to_set,
    load_config, load_settings, normalize_custom_extension, normalize_custom_filename,
    normalize_language_name, normalize_language_names, reject_core_language_removal, save_config,
    upsert_extension_mappings, upsert_filename_mappings,
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

    let grammar = SyntaxGrammarInfo::bundled(BUNDLED_GRAMMAR_VERSION);
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
    let available = installed_language_set();
    let mut config = load_config()?;
    let original_config = config.clone();
    let result = add_languages_to_config(&mut config, request, &available)?;
    if config != original_config {
        save_config(&config)?;
    }

    Ok(result)
}

pub(crate) fn add_languages_to_config(
    config: &mut crate::StoredSyntaxConfig,
    request: SyntaxAddRequest,
    available: &BTreeSet<String>,
) -> MarkResult<SyntaxAddResult> {
    let (requested, mapping_options) = match request {
        SyntaxAddRequest::Languages(languages) => {
            (normalize_language_names(languages.as_slice()), None)
        }
        SyntaxAddRequest::LanguageWithMappings { language, options } => {
            (normalize_language_names(&[language]), Some(options))
        }
    };
    if let Some(options) = &mapping_options {
        validate_custom_mapping_options(options)?;
    }

    let mut enabled = language_vec_to_set(&config.languages);
    let mut added = Vec::new();
    let mut already_enabled = Vec::new();
    let mut unavailable = Vec::new();
    let mut custom_mappings = Vec::new();
    let mut language_config_changed = false;

    for language in requested {
        let is_available = available.contains(&language);
        if !is_available {
            if mapping_options
                .as_ref()
                .is_some_and(SyntaxAddOptions::has_mappings)
            {
                return Err(custom_mapping_target_error(&language, false));
            }
            unavailable.push(language.clone());
            continue;
        }

        let mapping_target_ready = has_highlights(&language);
        if let Some(options) = mapping_options.as_ref() {
            if options.has_mappings() && !mapping_target_ready {
                return Err(custom_mapping_target_error(&language, is_available));
            }

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

        if enabled.insert(language.clone()) {
            language_config_changed = true;
            added.push(language);
        } else {
            already_enabled.push(language);
        }
    }

    if language_config_changed {
        config.languages = enabled.into_iter().collect();
    }

    Ok(SyntaxAddResult {
        added,
        already_enabled,
        unavailable,
        custom_mappings,
    })
}

fn validate_custom_mapping_options(options: &SyntaxAddOptions) -> MarkResult<()> {
    for extension in &options.extensions {
        normalize_custom_extension(extension)?;
    }
    for filename in &options.filenames {
        normalize_custom_filename(filename)?;
    }
    Ok(())
}

fn custom_mapping_target_error(language: &str, is_available: bool) -> MarkError {
    let reason = if is_available {
        "bundled grammar is available, but highlighting failed to initialize"
    } else {
        "no bundled grammar is available"
    };
    MarkError::Usage(format!(
        "cannot add custom mappings for `{language}`: {reason}; custom mappings require a bundled highlight-ready language"
    ))
}

pub fn update_languages(selection: SyntaxUpdateSelection) -> MarkResult<SyntaxUpdateResult> {
    let available = installed_language_set();
    let requested = match selection {
        SyntaxUpdateSelection::All => {
            let config = load_config()?;
            update_all_language_set(&config, &available)
        }
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

    let mut config = load_config()?;
    let requested = normalize_language_names(languages);
    reject_unconfigured_core_language_removal(&config, &requested)?;
    let result = remove_languages_from_config(&mut config, &requested);
    save_config(&config)?;

    Ok(result)
}

pub(crate) fn reject_unconfigured_core_language_removal(
    config: &crate::StoredSyntaxConfig,
    requested: &BTreeSet<String>,
) -> MarkResult<()> {
    let core = core_language_set();
    let blocked = requested
        .intersection(&core)
        .filter(|language| !has_user_config_for_language(config, language))
        .cloned()
        .collect::<BTreeSet<String>>();

    reject_core_language_removal(&blocked)
}

fn has_user_config_for_language(config: &crate::StoredSyntaxConfig, language: &str) -> bool {
    language_vec_to_set(&config.languages).contains(language)
        || mappings_include_language(&config.extensions, language)
        || mappings_include_language(&config.filenames, language)
}

fn mappings_include_language(mappings: &[crate::StoredLanguageMapping], language: &str) -> bool {
    mappings
        .iter()
        .any(|mapping| normalize_language_name(mapping.language.clone()) == language)
}

pub(crate) fn remove_languages_from_config(
    config: &mut crate::StoredSyntaxConfig,
    requested: &BTreeSet<String>,
) -> SyntaxRemoveResult {
    let core = core_language_set();
    let mut enabled = language_vec_to_set(&config.languages);
    let mut removed = Vec::new();
    let mut missing = Vec::new();
    let mut kept_core = BTreeSet::new();

    for language in requested {
        if enabled.remove(language.as_str()) {
            if core.contains(language) {
                kept_core.insert(language.clone());
            } else {
                removed.push(language.clone());
            }
        } else if core.contains(language) {
            kept_core.insert(language.clone());
        } else {
            missing.push(language.clone());
        }
    }

    let mut removed_custom_mappings = Vec::new();
    for mapping in remove_language_mappings(&mut config.extensions, requested) {
        if core.contains(&mapping.language) {
            kept_core.insert(mapping.language.clone());
        }
        removed_custom_mappings.push(format!("*.{} -> {}", mapping.pattern, mapping.language));
    }
    for mapping in remove_language_mappings(&mut config.filenames, requested) {
        if core.contains(&mapping.language) {
            kept_core.insert(mapping.language.clone());
        }
        removed_custom_mappings.push(format!("{} -> {}", mapping.pattern, mapping.language));
    }

    config.languages = enabled.into_iter().collect();

    SyntaxRemoveResult {
        removed,
        missing,
        kept_core: kept_core.into_iter().collect(),
        removed_custom_mappings,
    }
}

pub fn clean_cache() -> MarkResult<SyntaxCleanResult> {
    if !crate::engine::SyntaxEngine::is_available() {
        return Err(MarkError::Usage(
            "cannot clean syntax config while no syntax highlighting backend is available"
                .to_owned(),
        ));
    }

    let mut config = load_config()?;
    let available = installed_language_set();
    let result = clean_language_config(&mut config, &available);
    save_config(&config)?;

    Ok(result)
}

pub(crate) fn clean_language_config(
    config: &mut crate::StoredSyntaxConfig,
    available: &BTreeSet<String>,
) -> SyntaxCleanResult {
    let core = core_language_set();
    let keep_language = |language: &str| available.contains(language) || core.contains(language);
    let stale_language_records_removed = config
        .languages
        .iter()
        .filter(|language| {
            let language = normalize_language_name((*language).clone());
            language.is_empty() || !keep_language(&language)
        })
        .count();
    let mut languages = language_vec_to_set(&config.languages);
    languages.retain(|language| keep_language(language));
    let enabled_languages_kept = languages.len();

    config.languages = languages.into_iter().collect();

    let stale_extension_mappings_removed = {
        let before = config.extensions.len();
        config.extensions.retain_mut(|mapping| {
            normalize_mapping_language(mapping);
            !mapping.language.is_empty() && keep_language(&mapping.language)
        });
        before - config.extensions.len()
    };
    let stale_filename_mappings_removed = {
        let before = config.filenames.len();
        config.filenames.retain_mut(|mapping| {
            normalize_mapping_language(mapping);
            !mapping.language.is_empty() && keep_language(&mapping.language)
        });
        before - config.filenames.len()
    };

    SyntaxCleanResult {
        stale_records_removed: stale_language_records_removed
            + stale_extension_mappings_removed
            + stale_filename_mappings_removed,
        enabled_languages_kept,
    }
}

fn remove_language_mappings(
    mappings: &mut Vec<crate::StoredLanguageMapping>,
    requested: &BTreeSet<String>,
) -> Vec<crate::StoredLanguageMapping> {
    let mut removed = Vec::new();
    mappings.retain_mut(|mapping| {
        normalize_mapping_language(mapping);
        if requested.contains(&mapping.language) {
            removed.push(mapping.clone());
            false
        } else {
            true
        }
    });
    removed
}

fn normalize_mapping_language(mapping: &mut crate::StoredLanguageMapping) {
    mapping.language = normalize_language_name(std::mem::take(&mut mapping.language));
}

pub fn doctor() -> MarkResult<SyntaxDoctorReport> {
    let statuses = language_statuses()?;
    let issues = if crate::engine::SyntaxEngine::is_available() {
        doctor_issues(&statuses)
    } else {
        vec![SyntaxDoctorIssue {
            language: "backend".to_owned(),
            message:
                "no syntax highlighting backend is available; Mark will render plain diff text"
                    .to_owned(),
        }]
    };

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
                message: "enabled in config, but no bundled syntax grammar is available; run `mark syntax rm`".to_owned(),
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

pub(crate) fn update_all_language_set(
    config: &crate::StoredSyntaxConfig,
    installed: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut languages = language_vec_to_set(&config.languages);
    languages.extend(installed.iter().cloned());
    languages
}
