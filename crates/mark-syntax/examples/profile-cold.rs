//! Cold-path profiling driver with explicit cache semantics.
//!
//! usage: profile-cold --mode line-cold --assets assets/tm-grammars/languages --scope source.rust <file> [iterations]

#[cfg(feature = "alloc-trial")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
    time::Instant,
};

use mark_syntax::SyntaxLimits;
use mark_syntax::engine::{
    grammar::load_dev_grammar_from_str,
    state::GrammarId,
    tokenizer::{GrammarSet, TextMateTokenizer},
};

#[derive(Clone, Copy)]
enum Mode {
    ProcessCold,
    LineCold,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "process-cold" => Ok(Self::ProcessCold),
            "line-cold" => Ok(Self::LineCold),
            _ => Err(format!(
                "invalid --mode {value:?}; expected process-cold or line-cold"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ProcessCold => "process-cold",
            Self::LineCold => "line-cold",
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut assets = None;
    let mut scope = None;
    let mut mode = Mode::LineCold;
    let mut assert_min_mb_s = None;
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
            "--mode" => {
                mode = Mode::parse(&args[index + 1])?;
                index += 2;
            }
            "--assert-min-mb-s" => {
                assert_min_mb_s = Some(args[index + 1].parse::<f64>()?);
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

    let mut sources = BTreeMap::new();
    let mut entries = fs::read_dir(&assets)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        let parsed: serde_json::Value = serde_json::from_str(&contents)?;
        if let Some(scope_name) = parsed.get("scopeName").and_then(|value| value.as_str()) {
            sources.insert(scope_name.to_owned(), (contents, parsed));
        }
    }

    // Clone only the external-include closure of the requested root into each
    // fresh tokenizer. Loading every private Markdown dependency made an
    // unrelated Rust process-cold sample pay for the expanded asset catalog.
    let mut selected = BTreeSet::new();
    let mut pending = vec![scope.clone()];
    while let Some(requested) = pending.pop() {
        if !selected.insert(requested.clone()) {
            continue;
        }
        if let Some((_, grammar)) = sources.get(&requested) {
            collect_external_scopes(grammar, &sources, &mut pending);
        }
    }

    let mut set = GrammarSet::new();
    for requested in selected {
        let Some((contents, _)) = sources.get(&requested) else {
            continue;
        };
        let id = GrammarId(set.grammars().len() as u16);
        if let Ok(grammar) = load_dev_grammar_from_str(id, contents) {
            set.add(grammar);
        }
    }
    let root = set
        .grammar_id_by_scope(&scope)
        .ok_or_else(|| format!("scope {scope:?} not found"))?;

    let source = fs::read_to_string(file)?;
    let mut reusable_tokenizer =
        matches!(mode, Mode::LineCold).then(|| TextMateTokenizer::new(set.clone(), root));

    let mut total_tokens = 0usize;
    eprintln!(
        "mode={} iterations={iterations} bytes={} tokenizer_caches={}",
        mode.name(),
        source.len(),
        match mode {
            Mode::ProcessCold => "fresh-per-iteration",
            Mode::LineCold => "matcher-and-candidate-warm,line-cache-cleared",
        }
    );
    let started = Instant::now();
    for iteration in 0..iterations {
        let mut tokenizer = reusable_tokenizer
            .take()
            .unwrap_or_else(|| TextMateTokenizer::new(set.clone(), root));
        tokenizer.configure_limits(SyntaxLimits::default());
        if let Some(capacity) = env::var("MARK_TEXTMATE_BENCH_LINE_CACHE")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            tokenizer.set_line_cache_capacity(capacity);
        }
        if matches!(mode, Mode::LineCold) {
            tokenizer.clear_line_cache();
        }
        let iter_start = Instant::now();
        // Match production full-file semantics: correct line indices/final
        // empty line and a source-wide fallback budget.
        let highlighted = tokenizer.tokenize_source(&source);
        let tokens = highlighted
            .lines
            .iter()
            .map(|line| line.segments.len())
            .sum::<usize>();
        total_tokens += tokens;
        let elapsed = iter_start.elapsed();
        eprintln!(
            "iter {iteration} mode={}: {:.3}s  {:.2} MB/s  tokens={tokens}",
            mode.name(),
            elapsed.as_secs_f64(),
            source.len() as f64 / elapsed.as_secs_f64() / 1e6
        );
        if matches!(mode, Mode::LineCold) {
            reusable_tokenizer = Some(tokenizer);
        }
    }
    let elapsed = started.elapsed();
    let avg_mb_s = (source.len() * iterations) as f64 / elapsed.as_secs_f64() / 1e6;
    eprintln!(
        "total mode={}: {:.3}s  avg {:.2} MB/s  tokens={total_tokens}",
        mode.name(),
        elapsed.as_secs_f64(),
        avg_mb_s
    );
    if let Some(min_mb_s) = assert_min_mb_s
        && avg_mb_s < min_mb_s
    {
        return Err(format!(
            "throughput assertion failed: {avg_mb_s:.2} MB/s < {min_mb_s:.2} MB/s"
        )
        .into());
    }
    Ok(())
}

fn collect_external_scopes(
    value: &serde_json::Value,
    sources: &BTreeMap<String, (String, serde_json::Value)>,
    pending: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(include) = object.get("include").and_then(|value| value.as_str())
                && !include.starts_with('#')
                && !matches!(include, "$self" | "$base")
            {
                let scope = include.split('#').next().unwrap_or(include);
                if sources.contains_key(scope) {
                    pending.push(scope.to_owned());
                }
            }
            for child in object.values() {
                collect_external_scopes(child, sources, pending);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                collect_external_scopes(child, sources, pending);
            }
        }
        _ => {}
    }
}
