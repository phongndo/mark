use std::time::Instant;

use mark_syntax::grammars::{
    bundle::{Bundle, BundleGrammarRegistry},
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
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--assert-max-core-cold-us" => {
                let value = args
                    .next()
                    .ok_or("--assert-max-core-cold-us requires a value")?;
                assert_max_core_cold_us = Some(value.parse::<u128>()?);
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
    println!("  }}");
    println!("}}");

    if let Some(max_us) = assert_max_core_cold_us
        && let Some((language, us)) = cold.iter().max_by_key(|(_, us)| *us)
        && *us > max_us
    {
        return Err(format!(
            "core cold decode assertion failed for {language}: {us}us > {max_us}us"
        )
        .into());
    }
    Ok(())
}
