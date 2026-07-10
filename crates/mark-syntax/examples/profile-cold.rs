//! Cold-path profiling driver: repeatedly tokenizes a file with the line
//! cache cleared between passes so samples land on real tokenization work.
//!
//! usage: profile-cold --assets assets/tm-grammars/languages --scope source.rust <file> [iterations]

use std::{env, fs, path::PathBuf, time::Instant};

use mark_syntax::engine::{
    grammar::load_dev_grammar_from_str,
    state::GrammarId,
    tokenizer::{GrammarSet, TextMateTokenizer, TokenizerState},
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
    let iterations: usize = positional
        .get(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(10);

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

    let mut total_tokens = 0usize;
    let started = Instant::now();
    for iteration in 0..iterations {
        tokenizer.clear_line_cache();
        let iter_start = Instant::now();
        let mut state = TokenizerState::default();
        let mut lines = source.split_inclusive('\n');
        let mut tokens = 0usize;
        for line in &mut lines {
            let tokenized = tokenizer.tokenize_line_scopes(line, state);
            state = tokenized.state.clone();
            tokens += tokenized.tokens.len();
        }
        total_tokens += tokens;
        let elapsed = iter_start.elapsed();
        eprintln!(
            "iter {iteration}: {:.3}s  {:.2} MB/s  tokens={tokens}",
            elapsed.as_secs_f64(),
            source.len() as f64 / elapsed.as_secs_f64() / 1e6
        );
    }
    let elapsed = started.elapsed();
    eprintln!(
        "total: {:.3}s  avg {:.2} MB/s  tokens={total_tokens}",
        elapsed.as_secs_f64(),
        (source.len() * iterations) as f64 / elapsed.as_secs_f64() / 1e6
    );
    Ok(())
}
