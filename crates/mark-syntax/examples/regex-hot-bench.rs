use std::time::Instant;

use mark_syntax::engine::regex::{AnchorContext, FallbackMatcher};

struct Case {
    name: &'static str,
    pattern: &'static str,
    line: &'static str,
    iterations: usize,
}

fn main() {
    let cases = [
        Case {
            name: "typescript-lookbehind-function-name",
            pattern: r"(?<=\bfunction\s+)[$_[:alpha:]][$_[:alnum:]]*",
            line: "export function parseResponse<T>(input: T) { return input }",
            iterations: 2_000,
        },
        Case {
            name: "typescript-arrow-after-paren",
            pattern: r"(?<=\))\s*=>",
            line: "const f = (value: number) => value + 1",
            iterations: 2_000,
        },
        Case {
            name: "cpp-raw-string-delimiter-backref",
            pattern: r#"R\"([A-Za-z_][A-Za-z0-9_]*)\(.*\)\1\""#,
            line: r#"auto s = R"tag(hello world)tag";"#,
            iterations: 1_000,
        },
        Case {
            name: "ruby-heredoc-backref",
            pattern: r"<<[-~]?([A-Z_][A-Z0-9_]*)",
            line: "  SQL = <<~QUERY",
            iterations: 2_000,
        },
    ];

    println!("{{\"version\":1,\"cases\":[");
    for (index, case) in cases.iter().enumerate() {
        let matcher = FallbackMatcher::with_budget(case.pattern, 200_000);
        let ctx = AnchorContext::start_of_file();
        let started = Instant::now();
        let mut last_steps = 0usize;
        let mut matches = 0usize;
        for _ in 0..case.iterations {
            let report = matcher.try_find(case.line, 0, ctx).expect("fallback run");
            last_steps = report.steps;
            if report.result.is_some() {
                matches += 1;
            }
        }
        let elapsed = started.elapsed();
        let nanos = elapsed.as_nanos() as f64 / case.iterations as f64;
        println!(
            "  {{\"name\":\"{}\",\"pattern\":\"{}\",\"iterations\":{},\"matches\":{},\"avg_nanos\":{:.1},\"last_steps\":{}}}{}",
            escape(case.name),
            escape(case.pattern),
            case.iterations,
            matches,
            nanos,
            last_steps,
            if index + 1 == cases.len() { "" } else { "," }
        );
    }
    println!("]}}");
}

fn escape(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
