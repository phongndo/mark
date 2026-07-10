use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use mark_syntax::engine::regex::{Route, translate};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let assets = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/tm-grammars/languages"));
    let mut files = fs::read_dir(&assets)?.collect::<Result<Vec<_>, _>>()?;
    files.sort_by_key(|entry| entry.file_name());

    let mut grammar_count = 0usize;
    let mut pattern_count = 0usize;
    let mut dfa_count = 0usize;
    let mut fallback_count = 0usize;
    let mut reasons = BTreeMap::<String, usize>::new();
    for entry in files {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        grammar_count += 1;
        let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
        let patterns = collect_patterns(&value);
        for pattern in patterns {
            pattern_count += 1;
            match translate(&pattern).route {
                Route::Dfa => dfa_count += 1,
                Route::Fallback {
                    reasons: route_reasons,
                } => {
                    fallback_count += 1;
                    if route_reasons.is_empty() {
                        *reasons.entry("unknown".to_owned()).or_default() += 1;
                    }
                    for reason in route_reasons {
                        *reasons.entry(reason.to_owned()).or_default() += 1;
                    }
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "version": 1,
            "assets": normalize_path(&assets),
            "grammar_count": grammar_count,
            "pattern_count": pattern_count,
            "dfa_count": dfa_count,
            "fallback_count": fallback_count,
            "fallback_percent": if pattern_count == 0 { 0.0 } else { fallback_count as f64 / pattern_count as f64 },
            "fallback_reasons": reasons,
            "anchor_strategy": "leading \\A/^/\\G rewritten to guarded anchored searches; non-leading ambiguous anchors route to fallback"
        })
    );
    Ok(())
}

fn collect_patterns(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_patterns_into(value, &mut out);
    out
}

fn collect_patterns_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in ["match", "begin", "end", "while"] {
                if let Some(pattern) = map.get(key).and_then(Value::as_str) {
                    out.push(pattern.to_owned());
                }
            }
            for value in map.values() {
                collect_patterns_into(value, out);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_patterns_into(value, out);
            }
        }
        _ => {}
    }
}

fn normalize_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
