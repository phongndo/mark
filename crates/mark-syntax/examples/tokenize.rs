use std::{env, fs, path::PathBuf};

use mark_syntax::engine::{
    grammar::load_dev_grammar_from_str,
    scopes::classify_scope_stack,
    state::GrammarId,
    tokenizer::{GrammarSet, TextMateTokenizer, TokenizerState},
};
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        usage();
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || take_flag(&mut args, "--help") || take_flag(&mut args, "-h") {
        usage();
        return Ok(());
    }
    let grammar_path = take_value(&mut args, "--grammar").map(PathBuf::from);
    let assets = take_value(&mut args, "--assets").map(PathBuf::from);
    let scope = take_value(&mut args, "--scope");
    let embedded = take_values(&mut args, "--embedded")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let counters_path = take_value(&mut args, "--counters").map(PathBuf::from);
    let classes = take_flag(&mut args, "--classes");
    if args.len() != 1 {
        return Err(if args.is_empty() {
            "missing source file".into()
        } else {
            format!("unexpected arguments: {}", args.join(" ")).into()
        });
    }
    let file = PathBuf::from(&args[0]);

    let (set, root) = if let Some(assets) = assets {
        if !embedded.is_empty() {
            return Err("--embedded cannot be combined with --assets".into());
        }
        load_assets(&assets, grammar_path.as_ref(), scope.as_deref())?
    } else {
        let grammar_path = grammar_path.ok_or("--grammar is required without --assets")?;
        let contents = fs::read_to_string(grammar_path)?;
        let mut set = GrammarSet::new();
        let root = set.load_and_add(&contents)?;
        for embedded_path in embedded {
            let contents = fs::read_to_string(embedded_path)?;
            set.load_and_add(&contents)?;
        }
        (set, root)
    };

    let source = fs::read_to_string(&file)?;
    let counters_set = counters_path.as_ref().map(|_| set.clone());
    let mut tokenizer = TextMateTokenizer::new(set, root);
    let mut state = TokenizerState::default();
    for (line_number, line) in split_lines_like_engine(&source).into_iter().enumerate() {
        let tokenized = tokenizer.tokenize_line_scopes_at_line(line.parse_text, state, line_number);
        state = tokenized.state.clone();
        if classes {
            println!(
                "{}",
                json!({
                    "lineNumber": line_number,
                    "line": line.text,
                    "stateId": state.state_id().0,
                    "stateDepth": state.depth(),
                    "segments": tokenized.tokens.iter().filter_map(|token| {
                        let start = token.range.start.min(line.text.len());
                        let end = token.range.end.min(line.text.len());
                        (start < end).then(|| json!({
                            "start": start,
                            "end": end,
                            "class": classify_scope_stack(&token.scopes).map(|class| format!("{class:?}")),
                        }))
                    }).collect::<Vec<_>>(),
                })
            );
        } else {
            println!(
                "{}",
                json!({
                    "lineNumber": line_number,
                    "line": line.text,
                    "stateId": state.state_id().0,
                    "stateDepth": state.depth(),
                    "tokens": tokenized.tokens.iter().filter_map(|token| {
                        let start = token.range.start.min(line.text.len());
                        let end = token.range.end.min(line.text.len());
                        (start < end).then(|| json!({
                            "start": start,
                            "end": end,
                            "scopes": token.scopes,
                        }))
                    }).collect::<Vec<_>>(),
                })
            );
        }
    }
    if let (Some(counters_path), Some(set)) = (counters_path, counters_set) {
        let mut tokenizer = TextMateTokenizer::new(set, root);
        tokenizer.set_counters_enabled(true);
        let _ = tokenizer.tokenize_source(&source);
        fs::write(
            counters_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&tokenizer.take_counters())?
            ),
        )?;
    }
    Ok(())
}

fn load_assets(
    assets: &PathBuf,
    grammar_path: Option<&PathBuf>,
    scope: Option<&str>,
) -> Result<(GrammarSet, GrammarId), Box<dyn std::error::Error>> {
    let mut set = GrammarSet::new();
    let mut entries = fs::read_dir(assets)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        let id = GrammarId(set.grammars().len() as u16);
        match load_dev_grammar_from_str(id, &contents) {
            Ok(grammar) => {
                set.add(grammar);
            }
            Err(error) => {
                eprintln!("warning: skipped {}: {error}", path.display());
            }
        }
    }
    let root = if let Some(scope) = scope {
        set.grammar_id_by_scope(scope)
            .ok_or_else(|| format!("scope {scope:?} not found in assets"))?
    } else if let Some(grammar_path) = grammar_path {
        let contents = fs::read_to_string(grammar_path)?;
        let probe = load_dev_grammar_from_str(GrammarId(0), &contents)?;
        set.grammar_id_by_scope(&probe.scope_name)
            .ok_or_else(|| format!("scope {:?} not found in assets", probe.scope_name))?
    } else {
        return Err("--scope or --grammar is required with --assets".into());
    };
    Ok((set, root))
}

#[derive(Clone, Copy)]
struct Line<'a> {
    text: &'a str,
    parse_text: &'a str,
}

fn split_lines_like_engine(source: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    while offset < source.len() {
        let rest = &source[offset..];
        if let Some(newline) = rest.find('\n') {
            let end = offset + newline + 1;
            let parse_text = &source[offset..end];
            lines.push(Line {
                text: &parse_text[..parse_text.len() - 1],
                parse_text,
            });
            offset = end;
        } else {
            lines.push(Line {
                text: rest,
                parse_text: rest,
            });
            offset = source.len();
        }
    }
    if source.is_empty() || source.ends_with('\n') {
        lines.push(Line {
            text: "",
            parse_text: "",
        });
    }
    lines
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    if index < args.len() {
        Some(args.remove(index))
    } else {
        None
    }
}

fn take_values(args: &mut Vec<String>, flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    while let Some(value) = take_value(args, flag) {
        values.push(value);
    }
    values
}

fn usage() {
    eprintln!(
        "usage: cargo run -p mark-syntax --example tokenize -- [--classes] [--counters counters.json] [--assets assets/grammars/languages --scope source.json | --grammar path.tmLanguage.json [--embedded path.tmLanguage.json]...] <file>"
    );
}
