use std::{collections::BTreeSet, time::Instant};

use mark_syntax::engine::tokenizer::{GrammarSet, TextMateTokenizer, TokenizerState};
use mark_syntax::grammars::{
    bundle::{Bundle, BundleError, BundleGrammarRegistry},
    embedded_bundle_bytes,
};

const CORE: &[&str] = &[
    "rust",
    "typescript",
    "tsx",
    "javascript",
    "json",
    "yaml",
    "toml",
    "markdown",
    "html",
    "css",
    "python",
    "go",
    "c",
    "cpp",
    "shellscript",
    "zig",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut assert_max_core_cold_us = None;
    let mut assert_max_catalog_first_line_us = None;
    let mut assert_max_bundle_bytes = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--assert-max-core-cold-us" => {
                let value = args
                    .next()
                    .ok_or("--assert-max-core-cold-us requires a value")?;
                assert_max_core_cold_us = Some(value.parse::<u128>()?);
            }
            "--assert-max-catalog-first-line-us" => {
                let value = args
                    .next()
                    .ok_or("--assert-max-catalog-first-line-us requires a value")?;
                assert_max_catalog_first_line_us = Some(value.parse::<u128>()?);
            }
            "--assert-max-bundle-bytes" => {
                let value = args
                    .next()
                    .ok_or("--assert-max-bundle-bytes requires a value")?;
                assert_max_bundle_bytes = Some(value.parse::<usize>()?);
            }
            _ => return Err(format!("unexpected argument: {arg}").into()),
        }
    }

    let bytes = embedded_bundle_bytes();
    let parse_start = Instant::now();
    let bundle = Bundle::parse(bytes).map_err(|error| format!("bundle parse failed: {error:?}"))?;
    let parse_us = parse_start.elapsed().as_micros();

    let mut registry = BundleGrammarRegistry::new(bundle.clone());
    let mut cold = Vec::new();
    for language in CORE {
        let start = Instant::now();
        registry
            .grammar(language)
            .map_err(|error| format!("decode {language} failed: {error:?}"))?;
        cold.push((*language, start.elapsed().as_micros()));
    }
    let warm_start = Instant::now();
    for language in CORE {
        registry
            .grammar(language)
            .map_err(|error| format!("warm decode {language} failed: {error:?}"))?;
    }
    let warm_us = warm_start.elapsed().as_micros();

    let markdown_start = Instant::now();
    let mut markdown_registry = BundleGrammarRegistry::new(bundle.clone());
    markdown_registry
        .grammar("markdown")
        .map_err(|error| format!("markdown decode failed: {error:?}"))?;
    let markdown_us = markdown_start.elapsed().as_micros();
    let catalog_first_line = if assert_max_catalog_first_line_us.is_some() {
        Some(measure_catalog_first_line(&bundle)?)
    } else {
        None
    };

    println!("{{");
    println!("  \"bundle_bytes\": {},", bytes.len());
    println!("  \"version\": \"{}\",", bundle.version_stamp());
    println!("  \"languages\": {},", bundle.languages.len());
    println!("  \"grammar_blobs\": {},", bundle.grammar_blobs.len());
    println!("  \"scopes\": {},", bundle.scopes.len());
    println!("  \"parse_metadata_us\": {parse_us},");
    println!("  \"markdown_cold_decode_us\": {markdown_us},");
    println!("  \"core_warm_decode_total_us\": {warm_us},");
    println!("  \"core_cold_decode_us\": {{");
    for (index, (language, us)) in cold.iter().enumerate() {
        let comma = if index + 1 == cold.len() { "" } else { "," };
        println!("    \"{language}\": {us}{comma}");
    }
    println!("  }},");
    if let Some(measurements) = &catalog_first_line {
        let (language, us) = measurements
            .iter()
            .max_by_key(|(_, us)| *us)
            .expect("public catalog is non-empty");
        println!(
            "  \"catalog_first_line_languages\": {},",
            measurements.len()
        );
        println!("  \"catalog_first_line_max_language\": \"{language}\",");
        println!("  \"catalog_first_line_max_us\": {us}");
    } else {
        println!("  \"catalog_first_line_languages\": null,");
        println!("  \"catalog_first_line_max_language\": null,");
        println!("  \"catalog_first_line_max_us\": null");
    }
    println!("}}");

    if let Some(max_bytes) = assert_max_bundle_bytes
        && bytes.len() > max_bytes
    {
        return Err(format!(
            "bundle size assertion failed: {} bytes > {max_bytes} bytes",
            bytes.len()
        )
        .into());
    }

    if let Some(max_us) = assert_max_core_cold_us
        && let Some((language, us)) = cold.iter().max_by_key(|(_, us)| *us)
        && *us > max_us
    {
        return Err(format!(
            "core cold decode assertion failed for {language}: {us}us > {max_us}us"
        )
        .into());
    }
    if let (Some(max_us), Some(measurements)) =
        (assert_max_catalog_first_line_us, catalog_first_line)
        && let Some((language, us)) = measurements.iter().max_by_key(|(_, us)| *us)
        && *us > max_us
    {
        return Err(format!(
            "catalog first-line assertion failed for {language}: {us}us > {max_us}us"
        )
        .into());
    }
    Ok(())
}

fn measure_catalog_first_line(bundle: &Bundle) -> Result<Vec<(String, u128)>, String> {
    let mut measurements = Vec::with_capacity(bundle.languages.len());
    for language in &bundle.languages {
        let started = Instant::now();
        let closure = grammar_blob_closure(bundle, &language.scope_name)?;
        let mut set = GrammarSet::new();
        let mut root = None;
        for index in closure {
            let blob = &bundle.grammar_blobs[index];
            let bytes = blob.decoded_bytes().map_err(bundle_error)?;
            let source = std::str::from_utf8(&bytes)
                .map_err(|_| format!("{} grammar is not UTF-8", blob.language))?;
            let id = set
                .load_and_add(source)
                .map_err(|error| format!("{} grammar failed to load: {error}", blob.language))?;
            if blob.scope_name == language.scope_name {
                root = Some(id);
            }
        }
        let root = root.ok_or_else(|| format!("{} root grammar is missing", language.canonical))?;
        let mut tokenizer = TextMateTokenizer::new(set, root);
        let _ = tokenizer.tokenize_line_scopes("x\n", TokenizerState::default());
        measurements.push((language.canonical.clone(), started.elapsed().as_micros()));
    }
    Ok(measurements)
}

fn grammar_blob_closure(bundle: &Bundle, root_scope: &str) -> Result<Vec<usize>, String> {
    let mut pending = vec![root_scope.to_owned()];
    let mut selected = BTreeSet::new();
    while let Some(scope) = pending.pop() {
        let Some((index, blob)) = bundle
            .grammar_blobs
            .iter()
            .enumerate()
            .find(|(_, blob)| blob.scope_name == scope)
        else {
            continue;
        };
        if !selected.insert(index) {
            continue;
        }
        let bytes = blob.decoded_bytes().map_err(bundle_error)?;
        let json = serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|error| format!("{} grammar JSON is invalid: {error}", blob.language))?;
        collect_external_scopes(&json, bundle, &mut pending);
    }
    Ok(selected.into_iter().collect())
}

fn collect_external_scopes(value: &serde_json::Value, bundle: &Bundle, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(include) = object.get("include").and_then(|value| value.as_str())
                && !include.starts_with('#')
                && !matches!(include, "$self" | "$base")
            {
                let scope = include.split('#').next().unwrap_or(include);
                if bundle.grammar_blob_for_scope(scope).is_some() {
                    out.push(scope.to_owned());
                }
            }
            for child in object.values() {
                collect_external_scopes(child, bundle, out);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_external_scopes(child, bundle, out);
            }
        }
        _ => {}
    }
}

fn bundle_error(error: BundleError) -> String {
    format!("bundle decode failed: {error:?}")
}
