//! Benchmark driver copied into the pre-TextMate worktree by the companion script.

use std::{env, fs, time::Instant};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "label",
    "module",
    "namespace",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let language = args.first().ok_or("language required")?;
    let file = args.get(1).ok_or("file required")?;
    let iterations = args
        .get(2)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1);
    let source = fs::read_to_string(file)?;
    let setup_started = Instant::now();
    // Parser installation is persistent setup, just like vendored grammar
    // availability. Download only when this historical language pack lacks a
    // local parser artifact.
    let _ = tree_sitter_language_pack::download(&[language.as_str()]);
    let parser = tree_sitter_language_pack::get_language(language)?;
    let query_language = if matches!(language.as_str(), "typescript" | "tsx") {
        "javascript"
    } else {
        language
    };
    let query = tree_sitter_language_pack::get_highlights_query(query_language)
        .ok_or("highlight query unavailable")?;
    let mut config = HighlightConfiguration::new(parser, language, query, "", "")?;
    config.configure(HIGHLIGHT_NAMES);
    let mut highlighter = Highlighter::new();
    let setup = setup_started.elapsed();

    let started = Instant::now();
    let mut segments = 0usize;
    for _ in 0..iterations {
        let events = highlighter.highlight(&config, source.as_bytes(), None, |_| None)?;
        for event in events {
            if let HighlightEvent::Source { start, end } = event? {
                // Materialize source text as the legacy Mark adapter did.
                let copied = String::from_utf8_lossy(&source.as_bytes()[start..end]).into_owned();
                segments += copied.split('\n').filter(|part| !part.is_empty()).count();
            }
        }
    }
    let elapsed = started.elapsed();
    let bytes = source.len().saturating_mul(iterations);
    println!(
        "{{\"engine\":\"legacy-tree-sitter-highlight\",\"commit\":\"692e78d^\",\"language\":{language:?},\"file\":{file:?},\"iterations\":{iterations},\"sourceBytes\":{},\"bytes\":{bytes},\"setupMicros\":{},\"highlightMicros\":{},\"bytesPerSecond\":{},\"megabytesPerSecond\":{},\"segments\":{segments}}}",
        source.len(),
        setup.as_micros(),
        elapsed.as_micros(),
        (bytes as f64 / elapsed.as_secs_f64()).round() as u64,
        bytes as f64 / elapsed.as_secs_f64() / 1e6,
    );
    Ok(())
}
