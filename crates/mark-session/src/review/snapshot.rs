//! Conservative continuity for checkpoints without a running diff renderer.
//!
//! Only byte-identical, uniquely named Git patch sections retain their anchors.
//! Changed sections and undecodable/ambiguous paths require the TUI's evidence
//! based reanchoring or explicit human inspection.

use std::collections::{HashMap, HashSet};

pub(super) fn unchanged_paths(before: &str, after: &str) -> HashSet<String> {
    let previous = sections(before);
    let current = sections(after);
    current
        .into_iter()
        .filter_map(|(path, section)| {
            (section.is_some() && previous.get(&path) == Some(&section)).then_some(path)
        })
        .collect()
}

fn sections(patch: &str) -> HashMap<String, Option<&str>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts.push(patch.len());
    let mut sections = HashMap::new();
    for window in starts.windows(2) {
        let section = &patch[window[0]..window[1]];
        let mut names = Vec::new();
        let mut valid = true;
        for line in section.lines().skip(1) {
            if line.starts_with("@@") || line == "GIT binary patch" {
                break;
            }
            if let Some(name) = line
                .strip_prefix("--- ")
                .or_else(|| line.strip_prefix("+++ "))
            {
                let name = name.split('\t').next().unwrap_or(name);
                if name == "/dev/null" {
                    continue;
                }
                if name.starts_with('"') || name.is_empty() {
                    valid = false;
                    break;
                }
                let name = name
                    .strip_prefix("a/")
                    .or_else(|| name.strip_prefix("b/"))
                    .unwrap_or(name);
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_owned());
                }
            }
        }
        // Binary/metadata-only patches lack ---/+++ headers. Accept only the
        // unquoted two-path spelling; unusual names remain conservatively stale.
        if names.is_empty() && valid {
            let header = section
                .lines()
                .next()
                .unwrap_or("")
                .strip_prefix("diff --git ")
                .unwrap_or("");
            let paths: Vec<_> = header.split_whitespace().collect();
            if paths.len() == 2 && !paths.iter().any(|path| path.starts_with('"')) {
                for path in paths {
                    if let Some(path) = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/"))
                        && !names.iter().any(|existing| existing == path)
                    {
                        names.push(path.to_owned());
                    }
                }
            }
        }
        if !valid {
            continue;
        }
        for name in names {
            sections
                .entry(name)
                .and_modify(|section| *section = None)
                .or_insert(Some(section));
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_sections_keep_only_unique_paths() {
        let a = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let b = "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let before = format!("{a}{b}");
        let after = format!("{a}{}", b.replace("+new", "+changed"));
        assert_eq!(
            unchanged_paths(&before, &after),
            HashSet::from(["a.rs".to_owned()])
        );
        assert!(unchanged_paths(a, &format!("{a}{a}")).is_empty());
        assert!(unchanged_paths(a, "").is_empty());
    }

    #[test]
    fn ambiguous_quoted_paths_never_gain_continuity() {
        let quoted = "diff --git \"a/odd\\tname\" \"b/odd\\tname\"\n--- \"a/odd\\tname\"\n+++ \"b/odd\\tname\"\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(unchanged_paths(quoted, quoted).is_empty());
    }
}
