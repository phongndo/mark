use std::{env, fs, path::PathBuf};

use mark_syntax::engine::{grammar::load_dev_grammar_from_path, state::GrammarId};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = parse_args()?;
    let contents = fs::read_to_string(&path)?;
    let grammar = load_dev_grammar_from_path(GrammarId(0), Some(&path), &contents)?;
    println!("{}", grammar.debug_dump());
    Ok(())
}

fn parse_args() -> Result<PathBuf, String> {
    let mut args = env::args().skip(1);
    let Some(first) = args.next() else {
        return Err(
            "usage: cargo run -p mark-syntax --example grammar-debug -- <tmLanguage.json>"
                .to_owned(),
        );
    };
    if first == "--help" || first == "-h" {
        println!("usage: cargo run -p mark-syntax --example grammar-debug -- <tmLanguage.json>");
        std::process::exit(0);
    }
    if args.next().is_some() {
        return Err("grammar-debug accepts exactly one grammar path".to_owned());
    }
    Ok(PathBuf::from(first))
}
