use std::env;

use mark_syntax::engine::regex::{
    AnchorContext, AutomataMatcher, FallbackMatcher, Matcher, RegexMatcher, parse, translate,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        usage();
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        usage();
        return Ok(());
    }

    let match_mode = take_flag(&mut args, "--match");
    let engine = take_value(&mut args, "--engine").unwrap_or_else(|| "auto".to_owned());
    let from = take_value(&mut args, "--from")
        .map(|value| value.parse::<usize>().map_err(|_| "--from must be a usize"))
        .transpose()?
        .unwrap_or(0);
    let budget = take_value(&mut args, "--budget")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--budget must be a usize")
        })
        .transpose()?
        .unwrap_or(100_000);
    let allow_a = take_flag(&mut args, "--allow-a");
    let allow_g = take_value(&mut args, "--allow-g")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--allow-g must be a usize")
        })
        .transpose()?;

    let pattern = args.first().ok_or("missing pattern")?.clone();
    let parsed = parse(&pattern);
    let translation = translate(&pattern);
    println!("{parsed}");
    println!("translated_pattern: {}", translation.pattern);
    println!("anchor_strategy: {:?}", translation.anchor_strategy);
    println!("route: {:?}", translation.route);

    if !match_mode {
        return Ok(());
    }
    let line = args.get(1).ok_or("--match needs a line argument")?;
    let ctx = AnchorContext {
        allow_a,
        allow_g: allow_g.is_some(),
        g_pos: allow_g.unwrap_or(0),
    };
    match engine.as_str() {
        "auto" => {
            let matcher = RegexMatcher::new(&pattern);
            let (result, steps) = matcher
                .find_report(line, from, ctx)
                .map_err(|error| format!("fallback error: {error:?}"))?;
            println!("engine: {}", matcher.engine_name());
            if let Some(steps) = steps {
                println!("steps: {steps}");
            }
            print_result(result);
        }
        "dfa" | "automata" => {
            let matcher = AutomataMatcher::new(&pattern)
                .map_err(|error| format!("regex-automata compile failed: {error}"))?;
            println!("engine: dfa");
            print_result(matcher.find(line, from, ctx));
        }
        "fallback" => {
            let matcher = FallbackMatcher::with_budget(&pattern, budget);
            let report = matcher
                .try_find(line, from, ctx)
                .map_err(|error| format!("fallback error: {error:?}"))?;
            println!("engine: fallback");
            println!("steps: {}", report.steps);
            print_result(report.result);
        }
        other => return Err(format!("unknown --engine {other:?}")),
    }

    Ok(())
}

fn print_result(result: Option<mark_syntax::engine::regex::MatchResult>) {
    match result {
        Some(result) => {
            println!("match: {}..{}", result.start, result.end);
            for (index, capture) in result.captures.iter().enumerate() {
                println!("capture[{index}]: {capture:?}");
            }
        }
        None => println!("match: <none>"),
    }
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

fn usage() {
    eprintln!(
        "usage: cargo run -p mark-syntax --example regex-parse -- [--match] [--engine auto|dfa|fallback] [--from N] [--allow-a] [--allow-g N] [--budget N] <pattern> [line]"
    );
}
