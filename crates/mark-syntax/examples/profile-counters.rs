//! Diagnostic driver: tokenizes a file once with engine counters enabled and
//! dumps the counters (including pattern hotspots) as JSON to stdout.
//!
//! usage: profile-counters --assets assets/tm-grammars/languages --scope source.rust <file>
//!
//! Run this in a fresh process. Unlike `profile-cold`, this enables diagnostic
//! counters and must not be used for timed throughput comparisons.

use std::{env, fs, path::PathBuf};

use mark_syntax::engine::{
    grammar::load_dev_grammar_from_str,
    state::GrammarId,
    tokenizer::{GrammarSet, TextMateTokenizer},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut assets = None;
    let mut scope = None;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--assets" => {
                assets = Some(PathBuf::from(&args[index + 1]));
                index += 2;
            }
            "--scope" => {
                scope = Some(args[index + 1].clone());
                index += 2;
            }
            other => {
                positional.push(other.to_owned());
                index += 1;
            }
        }
    }
    let assets = assets.ok_or("--assets required")?;
    let scope = scope.ok_or("--scope required")?;
    let file = positional.first().ok_or("missing source file")?;

    let mut set = GrammarSet::new();
    let mut entries = fs::read_dir(&assets)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        let id = GrammarId(set.grammars().len() as u16);
        if let Ok(grammar) = load_dev_grammar_from_str(id, &contents) {
            set.add(grammar);
        }
    }
    let root = set
        .grammar_id_by_scope(&scope)
        .ok_or_else(|| format!("scope {scope:?} not found"))?;

    let source = fs::read_to_string(file)?;
    let mut tokenizer = TextMateTokenizer::new(set, root);
    tokenizer.set_counters_enabled(true);
    tokenizer.set_hot_counters_enabled(true);

    // Use the production full-source entry point so line indices, the final
    // empty line, `\A`, and the source-wide fallback budget are represented.
    let _ = tokenizer.tokenize_source(&source);
    let counters = tokenizer.take_counters();
    println!("{}", serde_json::to_string_pretty(&counters)?);
    Ok(())
}
