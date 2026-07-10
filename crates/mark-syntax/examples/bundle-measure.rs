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
    "bash",
    "zig",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}
