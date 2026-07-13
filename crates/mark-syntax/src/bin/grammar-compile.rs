use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use mark_syntax::{
    engine::{
        grammar::load_dev_grammar_from_path,
        regex::{Route, translate},
        state::GrammarId,
    },
    grammars::{
        bundle::{Bundle, CODEC_NONE, GrammarBlob, LanguageEntry, LicenseEntry, fnv1a64},
        catalog,
    },
};
use serde::Deserialize;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let bundle = build_bundle(&args.assets)?;
    let bytes = bundle.to_bytes();
    let parsed =
        Bundle::parse(&bytes).map_err(|error| format!("bundle self-parse failed: {error:?}"))?;
    if parsed.to_bytes() != bytes {
        return Err("bundle output is not deterministic after self-parse".into());
    }
    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.out, &bytes)?;
    eprintln!(
        "wrote {} languages, {} grammar blobs, {} scopes to {} ({} bytes, version {})",
        parsed.languages.len(),
        parsed.grammar_blobs.len(),
        parsed.scopes.len(),
        args.out.display(),
        bytes.len(),
        parsed.version_stamp()
    );
    Ok(())
}

#[derive(Debug)]
struct Args {
    assets: PathBuf,
    out: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut assets = PathBuf::from("assets/tm-grammars");
        let mut out = PathBuf::from("target/mark-syntax/bundle.bin");
        let mut iter = env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--assets" => {
                    assets = iter
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--assets needs a path")?
                }
                "--out" => out = iter.next().map(PathBuf::from).ok_or("--out needs a path")?,
                "--help" | "-h" => {
                    println!(
                        "usage: grammar-compile [--assets assets/tm-grammars] [--out target/mark-syntax/bundle.bin]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Self { assets, out })
    }
}

#[derive(Debug, Deserialize)]
struct CoverageManifest {
    #[serde(default)]
    kept: Vec<String>,
    #[serde(default)]
    remapped: Vec<CoverageRemap>,
}

#[derive(Debug, Deserialize)]
struct CoverageRemap {
    language: String,
    asset: String,
}

#[derive(Debug, Deserialize)]
struct LicensesManifest {
    source: LicenseSource,
    #[serde(default)]
    assets: Vec<LicenseAsset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LicenseSource {
    #[serde(default)]
    repository: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    license: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LicenseAsset {
    language: String,
    path: String,
    #[serde(default)]
    license: String,
}

#[derive(Debug)]
struct AssetGrammar {
    language: String,
    path: String,
    scope_name: String,
    first_line_pattern: Option<String>,
    bytes: Vec<u8>,
    pattern_count: u32,
    dfa_count: u32,
    fallback_count: u32,
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LanguageMetadataManifest {
    languages: Vec<LanguageMetadata>,
}

#[derive(Debug, Deserialize)]
struct LanguageMetadata {
    id: String,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    basenames: Vec<String>,
}

fn build_bundle(assets: &Path) -> Result<Bundle, Box<dyn std::error::Error>> {
    let coverage_text = fs::read_to_string(assets.join("coverage.toml"))?;
    let source_text = fs::read_to_string(assets.join("SOURCE.toml"))?;
    let licenses_text = fs::read_to_string(assets.join("licenses.json"))?;
    let metadata_text = fs::read_to_string(assets.join("language-metadata.json"))?;
    let coverage: CoverageManifest = toml::from_str(&coverage_text)?;
    let licenses: LicensesManifest = serde_json::from_str(&licenses_text)?;
    let metadata: LanguageMetadataManifest = serde_json::from_str(&metadata_text)?;
    let metadata_by_language = metadata
        .languages
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let license_by_language = licenses
        .assets
        .iter()
        .map(|asset| (asset.language.as_str(), asset))
        .collect::<BTreeMap<_, _>>();

    let grammars = collect_grammars(assets)?;
    let grammar_by_language = grammars
        .iter()
        .enumerate()
        .map(|(index, grammar)| (grammar.language.as_str(), (index as u32, grammar)))
        .collect::<BTreeMap<_, _>>();

    let mut scope_set = BTreeSet::new();
    for grammar in &grammars {
        scope_set.insert(grammar.scope_name.clone());
        scope_set.extend(grammar.scopes.iter().cloned());
    }

    let mut licenses_out = Vec::with_capacity(grammars.len());
    for grammar in &grammars {
        let asset_license = license_by_language.get(grammar.language.as_str());
        licenses_out.push(LicenseEntry {
            language: grammar.language.clone(),
            source_path: asset_license
                .map(|asset| asset.path.clone())
                .unwrap_or_else(|| grammar.path.clone()),
            upstream_url: licenses.source.repository.clone(),
            spdx_id: asset_license
                .and_then(|asset| (!asset.license.is_empty()).then_some(asset.license.clone()))
                .unwrap_or_else(|| licenses.source.license.clone()),
            license_text: String::new(),
            source_revision: licenses.source.version.clone(),
        });
    }

    let grammar_blobs = grammars
        .iter()
        .map(|grammar| GrammarBlob {
            language: grammar.language.clone(),
            scope_name: grammar.scope_name.clone(),
            codec: CODEC_NONE,
            flags: grammar_flags(grammar),
            raw_len: grammar.bytes.len() as u32,
            bytes: grammar.bytes.clone(),
            pattern_count: grammar.pattern_count,
            dfa_count: grammar.dfa_count,
            fallback_count: grammar.fallback_count,
        })
        .collect::<Vec<_>>();

    let mut language_entries = Vec::new();
    let mut seen_languages = BTreeSet::new();
    for language in coverage.kept {
        if grammar_by_language.contains_key(language.as_str()) {
            language_entries.push(language_entry(
                &language,
                &language,
                &grammar_by_language,
                &metadata_by_language,
                &mut seen_languages,
            )?);
        }
    }
    for remap in coverage.remapped {
        // Only package remaps whose recovered asset is present on disk.
        if !grammar_by_language.contains_key(remap.asset.as_str()) {
            continue;
        }
        language_entries.push(language_entry(
            &remap.language,
            &remap.asset,
            &grammar_by_language,
            &metadata_by_language,
            &mut seen_languages,
        )?);
    }
    language_entries.sort_by(|left, right| left.canonical.cmp(&right.canonical));

    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(source_text.as_bytes());
    hash_input.extend_from_slice(coverage_text.as_bytes());
    hash_input.extend_from_slice(licenses_text.as_bytes());
    hash_input.extend_from_slice(metadata_text.as_bytes());

    Ok(Bundle {
        source_hash: fnv1a64(&hash_input),
        bundle_hash: 0,
        strings: Vec::new(),
        scopes: scope_set.into_iter().collect(),
        languages: language_entries,
        grammar_blobs,
        licenses: licenses_out,
    })
}

fn collect_grammars(assets: &Path) -> Result<Vec<AssetGrammar>, Box<dyn std::error::Error>> {
    let languages_dir = assets.join("languages");
    let mut entries = fs::read_dir(&languages_dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut grammars = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path)?;
        let contents = std::str::from_utf8(&bytes)?;
        let id = GrammarId(grammars.len() as u16);
        let compiled = load_dev_grammar_from_path(id, Some(&path), contents)?;
        let mut dfa_count = 0u32;
        let mut fallback_count = 0u32;
        for pattern in &compiled.patterns {
            match translate(pattern).route {
                Route::Dfa => dfa_count += 1,
                Route::Fallback { .. } => fallback_count += 1,
            }
        }
        let language = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .trim_end_matches(".tmLanguage.json")
            .to_owned();
        grammars.push(AssetGrammar {
            language,
            path: normalize_path(&path),
            scope_name: compiled.scope_name.clone(),
            first_line_pattern: compiled.metadata.first_line_match.clone(),
            bytes,
            pattern_count: compiled.patterns.len() as u32,
            dfa_count,
            fallback_count,
            scopes: compiled.scope_names,
        });
    }
    Ok(grammars)
}

fn language_entry(
    language: &str,
    asset: &str,
    grammar_by_language: &BTreeMap<&str, (u32, &AssetGrammar)>,
    metadata_by_language: &BTreeMap<&str, &LanguageMetadata>,
    seen_languages: &mut BTreeSet<String>,
) -> Result<LanguageEntry, Box<dyn std::error::Error>> {
    if !seen_languages.insert(language.to_owned()) {
        return Err(format!("duplicate language in coverage: {language}").into());
    }
    let (grammar_blob, grammar) = grammar_by_language
        .get(asset)
        .ok_or_else(|| format!("coverage maps {language} to missing grammar asset {asset}"))?;
    let metadata = metadata_by_language
        .get(language)
        .ok_or_else(|| format!("language metadata is missing public id {language}"))?;
    if metadata.asset.as_deref().unwrap_or(language) != asset {
        return Err(format!("language metadata maps {language} to the wrong asset").into());
    }
    let mut aliases = catalog::aliases_for_language(language)
        .into_iter()
        .filter(|alias| alias != language)
        .collect::<BTreeSet<_>>();
    aliases.extend(
        metadata
            .aliases
            .iter()
            .map(|alias| catalog::normalize_language_token(alias))
            .filter(|alias| !alias.is_empty() && alias != language),
    );
    if asset != language {
        aliases.insert(catalog::normalize_language_token(asset));
    }
    let mut extensions = catalog::extensions_for_language(language)
        .into_iter()
        .filter(|extension| !extension.is_empty())
        .collect::<BTreeSet<_>>();
    let mut basenames = catalog::basenames_for_language(language)
        .into_iter()
        .filter(|basename| !basename.is_empty())
        .collect::<BTreeSet<_>>();
    extensions.extend(
        metadata
            .extensions
            .iter()
            .filter(|extension| catalog::extension_is_allowed(extension, language))
            .cloned(),
    );
    basenames.extend(metadata.basenames.iter().cloned());
    if language == "tsx" {
        aliases.insert("typescriptreact".to_owned());
        extensions.insert("tsx".to_owned());
    }
    if language == "jsx" {
        aliases.insert("javascript-babel".to_owned());
        extensions.insert("jsx".to_owned());
    }
    if language == "c" {
        extensions.insert("h".to_owned());
    }
    if language == "cpp" {
        extensions.insert("hpp".to_owned());
        extensions.insert("hh".to_owned());
        extensions.insert("hxx".to_owned());
    }
    if language == "dockerfile" {
        basenames.insert("Dockerfile".to_owned());
        aliases.insert("docker".to_owned());
    }
    if language == "make" {
        basenames.insert("Makefile".to_owned());
        basenames.insert("GNUmakefile".to_owned());
        basenames.insert("BSDmakefile".to_owned());
    }
    if language == "bash" {
        aliases.insert("shellscript".to_owned());
        aliases.insert("shell".to_owned());
        aliases.insert("sh".to_owned());
        aliases.insert("zsh".to_owned());
    }
    Ok(LanguageEntry {
        canonical: language.to_owned(),
        scope_name: grammar.scope_name.clone(),
        aliases: aliases.into_iter().collect(),
        extensions: extensions.into_iter().collect(),
        basenames: basenames.into_iter().collect(),
        first_line_pattern: grammar.first_line_pattern.clone(),
        grammar_blob: *grammar_blob,
        license: *grammar_blob,
    })
}

fn grammar_flags(grammar: &AssetGrammar) -> u32 {
    let mut flags = 0u32;
    if grammar.fallback_count > 0 {
        flags |= 1;
    }
    if grammar.pattern_count > grammar.dfa_count + grammar.fallback_count {
        flags |= 2;
    }
    flags
}

fn normalize_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
