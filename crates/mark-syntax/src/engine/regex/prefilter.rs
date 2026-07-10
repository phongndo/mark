use super::ast::{Ast, CharClass, ClassAtom, LookKind, ParsedRegex, has_case_insensitive_scope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredLiterals {
    None,
    One(String),
    Any(Vec<String>),
}

impl RequiredLiterals {
    fn is_empty(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Fast rejection prefilter. False positives are allowed; false negatives are not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prefilter {
    None,
    Byte(u8),
    Literal(String),
    Any {
        literals: Vec<String>,
        ascii_case_insensitive: bool,
    },
}

impl Prefilter {
    pub fn from_regex(parsed: &ParsedRegex) -> Self {
        if parsed.flags.case_insensitive {
            Self::from_case_insensitive_pattern(&parsed.ast)
        } else if has_case_insensitive_scope(&parsed.ast) {
            // A single prefilter cannot safely apply one case-folding policy
            // to a pattern containing scoped flags. False negatives would
            // change TextMate rule selection, so leave filtering disabled.
            Self::None
        } else {
            Self::from_pattern(&parsed.ast)
        }
    }

    pub fn from_pattern(ast: &Ast) -> Self {
        match required_literals(ast) {
            RequiredLiterals::None => Self::None,
            RequiredLiterals::One(literal) => prefilter_one(literal),
            RequiredLiterals::Any(literals) if literals.is_empty() => Self::None,
            RequiredLiterals::Any(literals) if literals.len() == 1 => {
                prefilter_one(literals.into_iter().next().expect("one literal"))
            }
            RequiredLiterals::Any(literals) => Self::Any {
                literals,
                ascii_case_insensitive: false,
            },
        }
    }

    fn from_case_insensitive_pattern(ast: &Ast) -> Self {
        let literals = match required_literals(ast) {
            RequiredLiterals::None => return Self::None,
            RequiredLiterals::One(literal) => vec![literal],
            RequiredLiterals::Any(literals) => literals,
        };
        if literals.is_empty()
            || literals
                .iter()
                .any(|literal| literal.is_empty() || !literal.is_ascii())
        {
            return Self::None;
        }
        Self::Any {
            literals,
            ascii_case_insensitive: true,
        }
    }

    pub fn may_match(&self, haystack: &str, from: usize) -> bool {
        if !haystack.is_char_boundary(from) {
            return false;
        }
        let Some(slice) = haystack.get(from..) else {
            return false;
        };
        match self {
            Self::None => true,
            Self::Byte(byte) => find_byte(slice.as_bytes(), *byte).is_some(),
            Self::Literal(literal) => slice.contains(literal.as_str()),
            Self::Any {
                literals,
                ascii_case_insensitive: false,
            } => literals
                .iter()
                .any(|literal| slice.contains(literal.as_str())),
            Self::Any {
                literals,
                ascii_case_insensitive: true,
            } => literals
                .iter()
                .any(|literal| contains_ignore_ascii_case(slice, literal)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn literals(&self) -> &[String] {
        match self {
            Self::Any { literals, .. } => literals,
            _ => &[],
        }
    }
}

fn prefilter_one(literal: String) -> Prefilter {
    if literal.len() == 1 && literal.is_ascii() {
        Prefilter::Byte(literal.as_bytes()[0])
    } else {
        Prefilter::Literal(literal)
    }
}

/// Native single-byte search (memchr substitute).
fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

pub fn required_literal(pattern: &str) -> Option<String> {
    let parsed = super::ast::parse(pattern);
    match required_literals(&parsed.ast) {
        RequiredLiterals::One(literal) => Some(literal),
        RequiredLiterals::Any(literals) => literals.into_iter().max_by_key(|literal| literal.len()),
        RequiredLiterals::None => literal_prefix(pattern),
    }
}

pub fn required_literals(ast: &Ast) -> RequiredLiterals {
    if let Some(literal) = exact_literal(ast).filter(|literal| !literal.is_empty()) {
        return RequiredLiterals::One(literal);
    }
    match ast {
        Ast::Literal(literal) if !literal.is_empty() => RequiredLiterals::One(literal.clone()),
        Ast::Concat(nodes) => sequence_required_literals(nodes),
        Ast::Alternation(branches) => alternation_required_literals(branches),
        Ast::Group { child, .. } | Ast::Flags { child, .. } => required_literals(child),
        Ast::Look {
            kind: LookKind::Ahead,
            child,
        } => required_literals(child),
        Ast::Repeat { node, min, .. } if *min > 0 => required_literals(node),
        Ast::Class(class) => class_required_literals(class),
        _ => RequiredLiterals::None,
    }
}

fn sequence_required_literals(nodes: &[Ast]) -> RequiredLiterals {
    let mut best = RequiredLiterals::None;
    let mut run = String::new();
    for node in nodes {
        if let Some(literal) = exact_literal(node) {
            run.push_str(&literal);
            continue;
        }
        if !run.is_empty() {
            best = choose_more_selective(best, RequiredLiterals::One(std::mem::take(&mut run)));
        }
        let candidate = required_literals(node);
        best = choose_more_selective(best, candidate);
    }
    if !run.is_empty() {
        best = choose_more_selective(best, RequiredLiterals::One(run));
    }
    best
}

fn exact_literal(ast: &Ast) -> Option<String> {
    match ast {
        Ast::Empty => Some(String::new()),
        Ast::Literal(literal) => Some(literal.clone()),
        Ast::Concat(nodes) => {
            let mut out = String::new();
            for node in nodes {
                out.push_str(&exact_literal(node)?);
            }
            Some(out)
        }
        Ast::Group { child, .. } | Ast::Flags { child, .. } => exact_literal(child),
        _ => None,
    }
}

fn alternation_required_literals(branches: &[Ast]) -> RequiredLiterals {
    let mut literals = Vec::new();
    for branch in branches {
        match required_literals(branch) {
            RequiredLiterals::One(literal) => literals.push(literal),
            RequiredLiterals::Any(mut branch_literals) => literals.append(&mut branch_literals),
            RequiredLiterals::None => return RequiredLiterals::None,
        }
    }
    literals.sort();
    literals.dedup();
    RequiredLiterals::Any(literals)
}

fn choose_more_selective(left: RequiredLiterals, right: RequiredLiterals) -> RequiredLiterals {
    if left.is_empty() {
        return right;
    }
    if right.is_empty() {
        return left;
    }
    let left_len = max_literal_len(&left);
    let right_len = max_literal_len(&right);
    if right_len > left_len { right } else { left }
}

fn max_literal_len(literals: &RequiredLiterals) -> usize {
    match literals {
        RequiredLiterals::None => 0,
        RequiredLiterals::One(literal) => literal.len(),
        RequiredLiterals::Any(literals) => literals.iter().map(String::len).max().unwrap_or(0),
    }
}

fn class_required_literals(class: &CharClass) -> RequiredLiterals {
    if class.negated || class.atoms.is_empty() {
        return RequiredLiterals::None;
    }
    let mut literals = Vec::new();
    for atom in &class.atoms {
        match atom {
            ClassAtom::Char(ch) => literals.push(ch.to_string()),
            ClassAtom::Range(..)
            | ClassAtom::Perl(_)
            | ClassAtom::Posix { .. }
            | ClassAtom::Unicode { .. }
            | ClassAtom::Nested(_) => return RequiredLiterals::None,
        }
    }
    literals.sort();
    literals.dedup();
    match literals.len() {
        0 => RequiredLiterals::None,
        1 => RequiredLiterals::One(literals.remove(0)),
        _ => RequiredLiterals::Any(literals),
    }
}

fn literal_prefix(pattern: &str) -> Option<String> {
    let mut literal = String::new();
    let mut escaped = false;
    for ch in pattern.chars() {
        if escaped {
            if ch.is_ascii_alphanumeric() {
                return None;
            }
            literal.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '(' | ')' | '[' | ']' | '{' | '}' | '|' | '?' | '*' | '+' | '.' | '^' | '$' => break,
            ch => literal.push(ch),
        }
    }
    (!literal.is_empty()).then_some(literal)
}

/// Native multi-literal leftmost search used by pattern sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralSet {
    literals: Vec<String>,
}

impl LiteralSet {
    pub fn new(literals: Vec<String>) -> Self {
        Self { literals }
    }

    pub fn literals(&self) -> &[String] {
        &self.literals
    }

    /// Leftmost match; on equal start offset the lowest pattern index wins.
    pub fn find(&self, haystack: &str, from: usize) -> Option<(usize, usize, usize)> {
        if !haystack.is_char_boundary(from) {
            return None;
        }
        let slice = haystack.get(from..)?;
        let mut best: Option<(usize, usize, usize)> = None; // start, end, pattern_idx
        for (idx, literal) in self.literals.iter().enumerate() {
            if literal.is_empty() {
                let start = from;
                let end = from;
                if is_better_match(best, start, idx) {
                    best = Some((start, end, idx));
                }
                continue;
            }
            if let Some(rel) = slice.find(literal.as_str()) {
                let start = from + rel;
                let end = start + literal.len();
                if is_better_match(best, start, idx) {
                    best = Some((start, end, idx));
                    // Nothing can start earlier than `from`. First hit at `from`
                    // is optimal because we walk in index order.
                    if start == from {
                        return Some((idx, start, end));
                    }
                }
            }
        }
        best.map(|(start, end, idx)| (idx, start, end))
    }
}

fn is_better_match(best: Option<(usize, usize, usize)>, start: usize, idx: usize) -> bool {
    match best {
        None => true,
        Some((best_start, _, best_idx)) => {
            start < best_start || (start == best_start && idx < best_idx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::regex::ast::parse;

    #[test]
    fn extracts_safe_literal_prefix() {
        assert_eq!(required_literal("foo|bar"), Some("foo".to_owned()));
        assert_eq!(required_literal(r"\w+"), None);
    }

    #[test]
    fn extracts_alternation_literals() {
        let parsed = parse("foo|bar");
        assert_eq!(
            required_literals(&parsed.ast),
            RequiredLiterals::Any(vec!["bar".to_owned(), "foo".to_owned()])
        );
    }

    #[test]
    fn extracts_positive_lookahead_literals() {
        let parsed = parse(r"(?<=return)\s*(?=(<)\s*([A-Za-z]+))");
        assert_eq!(
            required_literals(&parsed.ast),
            RequiredLiterals::One("<".to_owned())
        );

        let parsed = parse(r"(?<!\\)(?=;)");
        assert_eq!(
            required_literals(&parsed.ast),
            RequiredLiterals::One(";".to_owned())
        );

        let parsed = parse(r"(?<=return)");
        assert_eq!(required_literals(&parsed.ast), RequiredLiterals::None);
    }

    #[test]
    fn extracts_positive_class_literals() {
        let parsed = parse(r"(?=[;)])(?<!\\)");
        assert_eq!(
            required_literals(&parsed.ast),
            RequiredLiterals::Any(vec![")".to_owned(), ";".to_owned()])
        );

        let parsed = parse(r"(?=[A-Z])");
        assert_eq!(required_literals(&parsed.ast), RequiredLiterals::None);
    }

    #[test]
    fn enables_ascii_literal_prefilter_for_case_insensitive_patterns() {
        let parsed = parse(r"(?i)foo");
        let prefilter = Prefilter::from_regex(&parsed);
        assert!(prefilter.is_enabled());
        assert!(prefilter.may_match("xxFOO", 0));
        assert!(!prefilter.may_match("xxbar", 0));

        let parsed = parse(r"(?i)café");
        assert!(!Prefilter::from_regex(&parsed).is_enabled());

        let parsed = parse(r"foo");
        assert!(Prefilter::from_regex(&parsed).is_enabled());
    }

    #[test]
    fn prefilter_uses_byte_scan_for_single_byte() {
        let parsed = parse("x+");
        let prefilter = Prefilter::from_pattern(&parsed.ast);
        assert!(prefilter.may_match("abcx", 0));
        assert!(!prefilter.may_match("abc", 0));
    }

    #[test]
    fn literal_set_leftmost_lowest_index() {
        let set = LiteralSet::new(vec!["bb".into(), "b".into(), "a".into()]);
        assert_eq!(set.find("abb", 0), Some((2, 0, 1)));
        assert_eq!(set.find("abb", 1), Some((0, 1, 3)));
    }
}
