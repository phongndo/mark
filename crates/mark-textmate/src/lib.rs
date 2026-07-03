use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    path::Path,
    sync::OnceLock,
};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxClass {
    Attribute,
    Comment,
    Constant,
    Constructor,
    Function,
    Keyword,
    Label,
    Module,
    Number,
    Operator,
    Property,
    Punctuation,
    String,
    Tag,
    Type,
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub class: Option<SyntaxClass>,
}

impl SyntaxSegment {
    pub fn new(byte_start: usize, byte_end: usize, class: Option<SyntaxClass>) -> Self {
        debug_assert!(byte_start <= byte_end);
        Self {
            byte_start,
            byte_end,
            class,
        }
    }

    pub fn len(&self) -> usize {
        self.byte_end.saturating_sub(self.byte_start)
    }

    pub fn is_empty(&self) -> bool {
        self.byte_start >= self.byte_end
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighlightedLine {
    pub fingerprint: LineTextFingerprint,
    pub segments: Vec<SyntaxSegment>,
}

impl HighlightedLine {
    pub fn new(text: &str) -> Self {
        Self {
            fingerprint: LineTextFingerprint::from_text(text),
            segments: Vec::new(),
        }
    }

    pub fn matches_text(&self, text: &str) -> bool {
        self.fingerprint.matches(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineTextFingerprint {
    byte_len: usize,
    hash: u64,
}

impl Default for LineTextFingerprint {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl LineTextFingerprint {
    pub fn from_text(text: &str) -> Self {
        Self {
            byte_len: text.len(),
            hash: stable_text_hash(text.as_bytes()),
        }
    }

    pub fn byte_len(self) -> usize {
        self.byte_len
    }

    pub fn matches(self, text: &str) -> bool {
        self.byte_len == text.len() && self.hash == stable_text_hash(text.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedText {
    pub lines: Vec<HighlightedLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct LanguageName(String);

impl LanguageName {
    fn new(value: impl Into<String>) -> Option<Self> {
        let value = normalize_language_token(&value.into());
        (!value.is_empty()).then_some(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightError {
    UnknownLanguage(String),
    Parse { language: String, message: String },
    Scope { language: String, message: String },
}

impl fmt::Display for HighlightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLanguage(language) => {
                write!(formatter, "unknown TextMate grammar '{language}'")
            }
            Self::Parse { language, message } => {
                write!(
                    formatter,
                    "failed to parse {language} with TextMate grammar: {message}"
                )
            }
            Self::Scope { language, message } => {
                write!(
                    formatter,
                    "failed to apply {language} TextMate scope stack: {message}"
                )
            }
        }
    }
}

impl std::error::Error for HighlightError {}

#[derive(Debug, Default)]
pub struct TextMateHighlighter {
    classifier: ScopeClassifier,
}

impl TextMateHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highlight(
        &mut self,
        language: &str,
        source: &str,
    ) -> Result<HighlightedText, HighlightError> {
        let language = canonical_language_name(language)
            .ok_or_else(|| HighlightError::UnknownLanguage(language.to_owned()))?;
        match FastLanguage::for_name(language.as_str()) {
            Some(FastLanguage::Rust) => return Ok(highlight_rust_fast(source)),
            Some(FastLanguage::CLike(language)) => {
                return Ok(highlight_c_like_fast(source, language));
            }
            Some(FastLanguage::CompilerIr) => return Ok(highlight_compiler_ir_fast(source)),
            Some(FastLanguage::LispLike) => return Ok(highlight_lisp_like_fast(source)),
            Some(FastLanguage::Markup) => return Ok(highlight_markup_fast(source)),
            None => {}
        }
        let syntax = syntax_for_language(language.as_str())
            .ok_or_else(|| HighlightError::UnknownLanguage(language.as_str().to_owned()))?;
        highlight_with_syntax(source, syntax, language.as_str(), &mut self.classifier)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastLanguage {
    Rust,
    CLike(CLikeLanguage),
    CompilerIr,
    LispLike,
    Markup,
}

impl FastLanguage {
    fn for_name(language: &str) -> Option<Self> {
        match language {
            "angular-html" | "astro" | "blade" | "edge" | "ejs" | "handlebars" | "liquid"
            | "marko" | "mdc" | "mdx" | "pug" | "razor" | "soy" | "twig" | "wikitext" | "xsl" => {
                Some(Self::Markup)
            }
            "angular-ts" | "glimmer-js" | "glimmer-ts" | "imba" | "ts-tags" => {
                Some(Self::CLike(CLikeLanguage::EcmaScript))
            }
            "apex" => Some(Self::CLike(CLikeLanguage::Java)),
            "ara" | "ballerina" | "berry" | "bicep" | "bsl" | "c3" | "cadence" | "cairo"
            | "chapel" | "dream-maker" | "genie" | "gleam" => Some(Self::CLike(CLikeLanguage::C)),
            "rust" => Some(Self::Rust),
            "c" | "objective-c" => Some(Self::CLike(CLikeLanguage::C)),
            "cpp" | "objective-c++" => Some(Self::CLike(CLikeLanguage::Cpp)),
            "csharp" => Some(Self::CLike(CLikeLanguage::CSharp)),
            "clarity" | "clojure" | "common-lisp" | "emacs-lisp" | "fennel" | "hy" | "lisp"
            | "racket" | "scheme" => Some(Self::LispLike),
            "cuda" | "cue" => Some(Self::CLike(CLikeLanguage::C)),
            "dart" => Some(Self::CLike(CLikeLanguage::Dart)),
            "gdshader" | "glsl" | "hlsl" | "metal" | "opencl" | "shaderlab" | "wgsl" => {
                Some(Self::CLike(CLikeLanguage::Shader))
            }
            "go" => Some(Self::CLike(CLikeLanguage::Go)),
            "hack" => Some(Self::CLike(CLikeLanguage::Php)),
            "haxe" | "java" => Some(Self::CLike(CLikeLanguage::Java)),
            "javascript" | "typescript" => Some(Self::CLike(CLikeLanguage::EcmaScript)),
            "kotlin" | "nextflow" | "nextflow-groovy" => Some(Self::CLike(CLikeLanguage::Kotlin)),
            "llvm" | "mlir" | "asm" | "arm-assembly" | "mipsasm" | "spirv" | "wasm" | "riscv"
            | "x86-64-assembly" => Some(Self::CompilerIr),
            "php" | "php-source" | "prisma" => Some(Self::CLike(CLikeLanguage::Php)),
            "abap" | "agda" | "apl" | "beancount" | "bird2" | "cobol" | "codeowners" | "codeql"
            | "coq" | "cypher" | "dax" | "dhall" | "fluent" | "forth" | "gdresource"
            | "gherkin" | "gn" | "hurl" | "hxml" | "jssm" | "just" | "kdl" | "kusto" | "logo"
            | "mermaid" | "meson" | "narrat" | "nushell" | "po" | "polar" | "powerquery"
            | "prolog" | "qmldir" | "rel" | "ron" | "rosmsg" | "sas" | "sdbl" | "smalltalk"
            | "sparql" | "splunk" | "stata" | "surrealql" | "systemd" | "talonscript" | "tasl"
            | "turtle" | "wenyan" | "wit" | "wolfram" => Some(Self::CompilerIr),
            "jison" | "luau" | "mojo" | "moonbit" | "move" | "openscad" | "pkl" | "pony"
            | "qss" | "zenscript" => Some(Self::CLike(CLikeLanguage::C)),
            "raku" => Some(Self::CompilerIr),
            "scala" => Some(Self::CLike(CLikeLanguage::Scala)),
            "solidity" | "vyper" => Some(Self::CLike(CLikeLanguage::Solidity)),
            "swift" => Some(Self::CLike(CLikeLanguage::Swift)),
            "tablegen" => Some(Self::CLike(CLikeLanguage::TableGen)),
            "templ" | "typespec" | "v" | "vala" => Some(Self::CLike(CLikeLanguage::C)),
            "zig" => Some(Self::CLike(CLikeLanguage::Zig)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustLexState {
    Normal,
    BlockComment { depth: usize },
    String,
    RawString { hashes: usize },
}

fn highlight_rust_fast(source: &str) -> HighlightedText {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut state = RustLexState::Normal;

    for chunk in LineChunks::new(source) {
        let mut line = HighlightedLine::new(chunk.text);
        let mut cursor = 0usize;
        let bytes = chunk.text.as_bytes();
        while cursor < bytes.len() {
            cursor = match state {
                RustLexState::Normal => {
                    rust_normal_token(&mut line, chunk.text, cursor, &mut state)
                }
                RustLexState::BlockComment { .. } => {
                    rust_block_comment_token(&mut line, chunk.text, cursor, false, &mut state)
                }
                RustLexState::String => {
                    rust_string_token(&mut line, chunk.text, cursor, false, &mut state)
                }
                RustLexState::RawString { hashes } => rust_raw_string_token(
                    &mut line,
                    chunk.text,
                    cursor,
                    hashes,
                    RawStringPosition::Continuation,
                    &mut state,
                ),
            };
        }
        lines.push(line);
    }

    debug_assert_eq!(lines.len(), line_count);
    HighlightedText { lines }
}

fn rust_normal_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut RustLexState,
) -> usize {
    let bytes = text.as_bytes();
    let byte = bytes[start];

    if byte.is_ascii_whitespace() {
        let end = consume_while(bytes, start, |byte| byte.is_ascii_whitespace());
        push_segment(line, start, end, text, None);
        return end;
    }

    if bytes[start..].starts_with(b"//") {
        push_segment(line, start, bytes.len(), text, Some(SyntaxClass::Comment));
        return bytes.len();
    }
    if bytes[start..].starts_with(b"/*") {
        return rust_block_comment_token(line, text, start, true, state);
    }

    if let Some(raw_string) = rust_raw_string_start(bytes, start) {
        *state = RustLexState::RawString {
            hashes: raw_string.hashes,
        };
        return rust_raw_string_token(
            line,
            text,
            start,
            raw_string.hashes,
            RawStringPosition::OpeningDelimiter {
                content_start: raw_string.content_start,
            },
            state,
        );
    }

    if byte == b'"' {
        *state = RustLexState::String;
        return rust_string_token(line, text, start, true, state);
    }

    if byte == b'\''
        && let Some(end) = rust_char_literal_end(text, start)
    {
        push_segment(line, start, end, text, Some(SyntaxClass::String));
        return end;
    }

    if byte.is_ascii_digit() {
        let end = consume_rust_number(bytes, start);
        push_segment(line, start, end, text, Some(SyntaxClass::Number));
        return end;
    }

    if is_ident_start(byte) {
        let end = consume_while(bytes, start + 1, is_ident_continue);
        let ident = &text[start..end];
        let class = if is_rust_keyword(ident) {
            Some(SyntaxClass::Keyword)
        } else if next_non_space(bytes, end) == Some(b'(') {
            Some(SyntaxClass::Function)
        } else if byte.is_ascii_uppercase() {
            Some(SyntaxClass::Type)
        } else {
            None
        };
        push_segment(line, start, end, text, class);
        return end;
    }

    if is_operator(byte) {
        let end = consume_while(bytes, start + 1, is_operator);
        push_segment(line, start, end, text, Some(SyntaxClass::Operator));
        return end;
    }
    if is_punctuation(byte) {
        let end = start + 1;
        push_segment(line, start, end, text, Some(SyntaxClass::Punctuation));
        return end;
    }

    let end = advance_char(text, start);
    push_segment(line, start, end, text, None);
    end
}

fn rust_block_comment_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    starts_with_opening_delimiter: bool,
    state: &mut RustLexState,
) -> usize {
    let bytes = text.as_bytes();
    let mut depth = if starts_with_opening_delimiter {
        debug_assert!(bytes[start..].starts_with(b"/*"));
        1usize
    } else {
        match *state {
            RustLexState::BlockComment { depth } => depth.max(1),
            _ => 1,
        }
    };
    let mut cursor = if starts_with_opening_delimiter && bytes[start..].starts_with(b"/*") {
        start + 2
    } else {
        start
    };

    while cursor + 1 < bytes.len() {
        if bytes[cursor..].starts_with(b"/*") {
            depth = depth.saturating_add(1);
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"*/") {
            depth = depth.saturating_sub(1);
            cursor += 2;
            if depth == 0 {
                push_segment(line, start, cursor, text, Some(SyntaxClass::Comment));
                *state = RustLexState::Normal;
                return cursor;
            }
        } else {
            cursor += 1;
        }
    }

    push_segment(line, start, bytes.len(), text, Some(SyntaxClass::Comment));
    *state = RustLexState::BlockComment { depth };
    bytes.len()
}

fn rust_string_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    starts_with_opening_delimiter: bool,
    state: &mut RustLexState,
) -> usize {
    debug_assert!(text.is_char_boundary(start));
    let bytes = text.as_bytes();
    let mut cursor = if starts_with_opening_delimiter {
        debug_assert_eq!(bytes.get(start), Some(&b'"'));
        start + 1
    } else {
        start
    };
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = advance_escaped_char(text, cursor),
            b'"' => {
                cursor += 1;
                *state = RustLexState::Normal;
                push_segment(line, start, cursor, text, Some(SyntaxClass::String));
                return cursor;
            }
            _ => cursor = advance_char(text, cursor),
        }
    }
    push_segment(line, start, bytes.len(), text, Some(SyntaxClass::String));
    bytes.len()
}

fn rust_raw_string_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    hashes: usize,
    position: RawStringPosition,
    state: &mut RustLexState,
) -> usize {
    let bytes = text.as_bytes();
    let terminator_len = hashes + 1;
    let search_start = match position {
        RawStringPosition::OpeningDelimiter { content_start } => content_start,
        RawStringPosition::Continuation => start,
    };
    let raw_string_end = find_raw_string_end(bytes, search_start, hashes);
    let end = raw_string_end
        .map(|end| end + terminator_len)
        .unwrap_or(bytes.len());
    push_segment(line, start, end, text, Some(SyntaxClass::String));
    if raw_string_end.is_some() {
        *state = RustLexState::Normal;
    }
    end
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawStringPosition {
    OpeningDelimiter { content_start: usize },
    Continuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawStringStart {
    hashes: usize,
    content_start: usize,
}

fn rust_raw_string_start(bytes: &[u8], start: usize) -> Option<RawStringStart> {
    let prefix_len = match bytes[start] {
        b'r' => 1,
        b'b' | b'c' if bytes.get(start + 1) == Some(&b'r') => 2,
        _ => return None,
    };

    let mut cursor = start + prefix_len;
    while cursor < bytes.len() && bytes[cursor] == b'#' {
        cursor += 1;
    }
    (cursor < bytes.len() && bytes[cursor] == b'"').then_some(RawStringStart {
        hashes: cursor - start - prefix_len,
        content_start: cursor + 1,
    })
}

fn find_raw_string_end(bytes: &[u8], start: usize, hashes: usize) -> Option<usize> {
    let mut cursor = start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && cursor + hashes < bytes.len()
            && bytes[cursor + 1..cursor + 1 + hashes]
                .iter()
                .all(|byte| *byte == b'#')
        {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn rust_char_literal_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'\''));

    let content_start = start + 1;
    let content_end = if bytes.get(content_start) == Some(&b'\\') {
        rust_escape_end(bytes, content_start)?
    } else {
        let ch = text.get(content_start..)?.chars().next()?;
        if ch == '\'' {
            return None;
        }
        content_start + ch.len_utf8()
    };

    (bytes.get(content_end) == Some(&b'\'')).then_some(content_end + 1)
}

fn rust_escape_end(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert_eq!(bytes.get(start), Some(&b'\\'));
    let escaped = start + 1;
    let byte = *bytes.get(escaped)?;
    match byte {
        b'x' => {
            let end = escaped + 3;
            (end <= bytes.len()
                && bytes[escaped + 1..end]
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit()))
            .then_some(end)
        }
        b'u' if bytes.get(escaped + 1) == Some(&b'{') => {
            let digits_start = escaped + 2;
            let mut cursor = digits_start;
            while cursor < bytes.len() && bytes[cursor].is_ascii_hexdigit() {
                cursor += 1;
            }
            (cursor > digits_start && bytes.get(cursor) == Some(&b'}')).then_some(cursor + 1)
        }
        b'\'' | b'"' | b'\\' | b'n' | b'r' | b't' | b'0' => Some(escaped + 1),
        _ => None,
    }
}

fn consume_rust_number(bytes: &[u8], start: usize) -> usize {
    consume_while(bytes, start + 1, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.')
    })
}

fn consume_while(bytes: &[u8], mut cursor: usize, mut predicate: impl FnMut(u8) -> bool) -> usize {
    while cursor < bytes.len() && predicate(bytes[cursor]) {
        cursor += 1;
    }
    cursor
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(needle.len())
        .position(|candidate| candidate == needle)
        .map(|offset| start + offset)
}

fn next_non_space(bytes: &[u8], mut cursor: usize) -> Option<u8> {
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    bytes.get(cursor).copied()
}

fn advance_char(text: &str, start: usize) -> usize {
    text[start..]
        .chars()
        .next()
        .map(|ch| start + ch.len_utf8())
        .unwrap_or(text.len())
}

fn advance_escaped_char(text: &str, start: usize) -> usize {
    let escaped = start + 1;
    if escaped >= text.len() {
        text.len()
    } else {
        advance_char(text, escaped)
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn is_operator(byte: u8) -> bool {
    matches!(
        byte,
        b'+' | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'='
            | b'!'
            | b'<'
            | b'>'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
            | b'?'
            | b':'
    )
}

fn is_punctuation(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'{' | b'}' | b'[' | b']' | b',' | b';' | b'.' | b'#' | b'@'
    )
}

fn is_rust_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CLikeLanguage {
    C,
    Cpp,
    CSharp,
    Dart,
    EcmaScript,
    Go,
    Java,
    Kotlin,
    Php,
    Scala,
    Shader,
    Solidity,
    Swift,
    TableGen,
    Zig,
}

impl CLikeLanguage {
    fn allows_backtick_string(self) -> bool {
        matches!(self, Self::EcmaScript | Self::Go)
    }

    fn escapes_backtick_string(self) -> bool {
        matches!(self, Self::EcmaScript)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CLikeLexState {
    Normal,
    BlockComment,
    String(CLikeStringDelimiter),
    TemplateString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CLikeStringDelimiter {
    Single,
    Double,
}

impl CLikeStringDelimiter {
    fn byte(self) -> u8 {
        match self {
            Self::Single => b'\'',
            Self::Double => b'"',
        }
    }
}

fn highlight_c_like_fast(source: &str, language: CLikeLanguage) -> HighlightedText {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut state = CLikeLexState::Normal;

    for chunk in LineChunks::new(source) {
        let mut line = HighlightedLine::new(chunk.text);
        let mut cursor = 0usize;
        let bytes = chunk.text.as_bytes();
        while cursor < bytes.len() {
            cursor = match state {
                CLikeLexState::Normal => {
                    c_like_normal_token(&mut line, chunk.text, cursor, language, &mut state)
                }
                CLikeLexState::BlockComment => {
                    c_like_block_comment_token(&mut line, chunk.text, cursor, &mut state)
                }
                CLikeLexState::String(delimiter) => {
                    c_like_string_token(&mut line, chunk.text, cursor, delimiter, &mut state)
                }
                CLikeLexState::TemplateString => c_like_template_string_token(
                    &mut line, chunk.text, cursor, false, language, &mut state,
                ),
            };
        }
        if !matches!(
            state,
            CLikeLexState::BlockComment | CLikeLexState::TemplateString
        ) {
            state = CLikeLexState::Normal;
        }
        lines.push(line);
    }

    debug_assert_eq!(lines.len(), line_count);
    HighlightedText { lines }
}

fn c_like_normal_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    language: CLikeLanguage,
    state: &mut CLikeLexState,
) -> usize {
    let bytes = text.as_bytes();
    let byte = bytes[start];

    if byte.is_ascii_whitespace() {
        let end = consume_while(bytes, start, |byte| byte.is_ascii_whitespace());
        push_segment(line, start, end, text, None);
        return end;
    }
    if bytes[start..].starts_with(b"//") {
        push_segment(line, start, bytes.len(), text, Some(SyntaxClass::Comment));
        return bytes.len();
    }
    if bytes[start..].starts_with(b"/*") {
        *state = CLikeLexState::BlockComment;
        return c_like_block_comment_token(line, text, start, state);
    }
    if byte == b'\'' {
        *state = CLikeLexState::String(CLikeStringDelimiter::Single);
        return c_like_string_token(line, text, start, CLikeStringDelimiter::Single, state);
    }
    if byte == b'"' {
        *state = CLikeLexState::String(CLikeStringDelimiter::Double);
        return c_like_string_token(line, text, start, CLikeStringDelimiter::Double, state);
    }
    if language.allows_backtick_string() && byte == b'`' {
        *state = CLikeLexState::TemplateString;
        return c_like_template_string_token(line, text, start, true, language, state);
    }
    if language == CLikeLanguage::EcmaScript
        && byte == b'/'
        && ecmascript_regex_literal_allowed(bytes, start)
        && let Some(end) = consume_ecmascript_regex_literal(text, start)
    {
        push_segment(line, start, end, text, Some(SyntaxClass::String));
        return end;
    }
    if byte.is_ascii_digit() {
        let end = consume_rust_number(bytes, start);
        push_segment(line, start, end, text, Some(SyntaxClass::Number));
        return end;
    }
    if is_ident_start(byte) || byte == b'$' {
        let end = consume_while(bytes, start + 1, |byte| {
            is_ident_continue(byte) || byte == b'$'
        });
        let ident = &text[start..end];
        let class = if is_c_like_keyword(ident) {
            Some(SyntaxClass::Keyword)
        } else if next_non_space(bytes, end) == Some(b'(') {
            Some(SyntaxClass::Function)
        } else if byte.is_ascii_uppercase() {
            Some(SyntaxClass::Type)
        } else {
            None
        };
        push_segment(line, start, end, text, class);
        return end;
    }
    if is_operator(byte) {
        let end = consume_while(bytes, start + 1, is_operator);
        push_segment(line, start, end, text, Some(SyntaxClass::Operator));
        return end;
    }
    if is_punctuation(byte) {
        let end = start + 1;
        push_segment(line, start, end, text, Some(SyntaxClass::Punctuation));
        return end;
    }

    let end = advance_char(text, start);
    push_segment(line, start, end, text, None);
    end
}

fn c_like_block_comment_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut CLikeLexState,
) -> usize {
    let bytes = text.as_bytes();
    let search_start = if bytes[start..].starts_with(b"/*") {
        start.saturating_add(2)
    } else {
        start
    };
    let comment_end = find_bytes(bytes, search_start, b"*/");
    let end = comment_end.map(|end| end + 2).unwrap_or(bytes.len());
    push_segment(line, start, end, text, Some(SyntaxClass::Comment));
    if comment_end.is_some() {
        *state = CLikeLexState::Normal;
    }
    end
}

fn c_like_string_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    delimiter: CLikeStringDelimiter,
    state: &mut CLikeLexState,
) -> usize {
    let bytes = text.as_bytes();
    let quote = delimiter.byte();
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = advance_escaped_char(text, cursor),
            byte if byte == quote => {
                cursor += 1;
                *state = CLikeLexState::Normal;
                push_segment(line, start, cursor, text, Some(SyntaxClass::String));
                return cursor;
            }
            _ => cursor = advance_char(text, cursor),
        }
    }
    push_segment(line, start, bytes.len(), text, Some(SyntaxClass::String));
    bytes.len()
}

fn c_like_template_string_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    starts_with_opening_delimiter: bool,
    language: CLikeLanguage,
    state: &mut CLikeLexState,
) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = if starts_with_opening_delimiter {
        debug_assert_eq!(bytes.get(start), Some(&b'`'));
        start + 1
    } else {
        start
    };
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' if language.escapes_backtick_string() => {
                cursor = advance_escaped_char(text, cursor)
            }
            b'`' => {
                cursor += 1;
                *state = CLikeLexState::Normal;
                push_segment(line, start, cursor, text, Some(SyntaxClass::String));
                return cursor;
            }
            _ => cursor = advance_char(text, cursor),
        }
    }
    push_segment(line, start, bytes.len(), text, Some(SyntaxClass::String));
    bytes.len()
}

fn ecmascript_regex_literal_allowed(bytes: &[u8], start: usize) -> bool {
    debug_assert_eq!(bytes.get(start), Some(&b'/'));
    if matches!(bytes.get(start + 1).copied(), Some(b'/' | b'*')) {
        return false;
    }

    let Some(previous) = previous_non_space(bytes, 0, start) else {
        return true;
    };

    if matches!(previous, b'(' | b'{' | b'[' | b',' | b';') {
        return true;
    }
    if is_operator(previous) {
        let operator = previous_operator_token(bytes, start);
        return operator != b"++" && operator != b"--";
    }
    if is_js_identifier_continue_byte(previous)
        && let Some(keyword) = previous_identifier_token(bytes, start)
    {
        return is_ecmascript_regex_prefix_keyword(keyword);
    }

    false
}

fn previous_operator_token(bytes: &[u8], mut cursor: usize) -> &[u8] {
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    let end = cursor;
    while cursor > 0 && is_operator(bytes[cursor - 1]) {
        cursor -= 1;
    }
    &bytes[cursor..end]
}

fn previous_identifier_token(bytes: &[u8], mut cursor: usize) -> Option<&[u8]> {
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    let end = cursor;
    while cursor > 0 && is_js_identifier_continue_byte(bytes[cursor - 1]) {
        cursor -= 1;
    }
    (cursor < end).then_some(&bytes[cursor..end])
}

fn is_js_identifier_continue_byte(byte: u8) -> bool {
    is_ident_continue(byte) || byte == b'$'
}

fn is_ecmascript_regex_prefix_keyword(keyword: &[u8]) -> bool {
    matches!(
        std::str::from_utf8(keyword),
        Ok("await"
            | "case"
            | "delete"
            | "do"
            | "else"
            | "in"
            | "instanceof"
            | "new"
            | "of"
            | "return"
            | "throw"
            | "typeof"
            | "void"
            | "yield")
    )
}

fn consume_ecmascript_regex_literal(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'/'));

    let mut cursor = start + 1;
    let mut in_character_class = false;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = advance_escaped_char(text, cursor),
            b'[' if !in_character_class => {
                in_character_class = true;
                cursor += 1;
            }
            b']' if in_character_class => {
                in_character_class = false;
                cursor += 1;
            }
            b'/' if !in_character_class => {
                cursor += 1;
                while cursor < bytes.len() && is_js_identifier_continue_byte(bytes[cursor]) {
                    cursor += 1;
                }
                return Some(cursor);
            }
            _ => cursor = advance_char(text, cursor),
        }
    }

    None
}

fn is_c_like_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "abstract"
            | "alignas"
            | "alignof"
            | "anytype"
            | "as"
            | "asm"
            | "async"
            | "await"
            | "become"
            | "bool"
            | "box"
            | "break"
            | "case"
            | "catch"
            | "chan"
            | "checked"
            | "class"
            | "comptime"
            | "concept"
            | "const"
            | "constructor"
            | "continue"
            | "contract"
            | "crate"
            | "debugger"
            | "defer"
            | "delegate"
            | "delete"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "errdefer"
            | "error"
            | "event"
            | "export"
            | "extends"
            | "extern"
            | "fallthrough"
            | "false"
            | "final"
            | "finally"
            | "fn"
            | "for"
            | "friend"
            | "from"
            | "func"
            | "function"
            | "get"
            | "global"
            | "go"
            | "goto"
            | "guard"
            | "if"
            | "implements"
            | "implicit"
            | "import"
            | "in"
            | "inline"
            | "interface"
            | "internal"
            | "is"
            | "keyof"
            | "lateinit"
            | "let"
            | "library"
            | "macro"
            | "map"
            | "match"
            | "module"
            | "mutating"
            | "namespace"
            | "new"
            | "nil"
            | "noalias"
            | "noexcept"
            | "nosuspend"
            | "null"
            | "object"
            | "operator"
            | "or"
            | "orelse"
            | "out"
            | "override"
            | "package"
            | "private"
            | "protected"
            | "protocol"
            | "pub"
            | "public"
            | "readonly"
            | "receive"
            | "ref"
            | "repeat"
            | "requires"
            | "return"
            | "satisfies"
            | "sealed"
            | "select"
            | "self"
            | "Self"
            | "set"
            | "sizeof"
            | "static"
            | "struct"
            | "subscript"
            | "super"
            | "suspend"
            | "switch"
            | "synchronized"
            | "template"
            | "this"
            | "throw"
            | "throws"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typealias"
            | "typedef"
            | "typename"
            | "typeof"
            | "unchecked"
            | "undefined"
            | "union"
            | "unsafe"
            | "using"
            | "var"
            | "vec2"
            | "vec3"
            | "vec4"
            | "virtual"
            | "void"
            | "volatile"
            | "sampler"
            | "uniform"
            | "when"
            | "where"
            | "while"
            | "with"
            | "yield"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringLexState {
    Normal,
    String,
}

fn highlight_compiler_ir_fast(source: &str) -> HighlightedText {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut state = StringLexState::Normal;

    for chunk in LineChunks::new(source) {
        let mut line = HighlightedLine::new(chunk.text);
        let mut cursor = 0usize;
        let bytes = chunk.text.as_bytes();
        while cursor < bytes.len() {
            cursor = match state {
                StringLexState::Normal => {
                    compiler_ir_normal_token(&mut line, chunk.text, cursor, &mut state)
                }
                StringLexState::String => {
                    quoted_string_token(&mut line, chunk.text, cursor, &mut state)
                }
            };
        }
        state = StringLexState::Normal;
        lines.push(line);
    }

    debug_assert_eq!(lines.len(), line_count);
    HighlightedText { lines }
}

fn compiler_ir_normal_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut StringLexState,
) -> usize {
    let bytes = text.as_bytes();
    let byte = bytes[start];
    if byte.is_ascii_whitespace() {
        let end = consume_while(bytes, start, |byte| byte.is_ascii_whitespace());
        push_segment(line, start, end, text, None);
        return end;
    }
    if bytes[start..].starts_with(b"//")
        || byte == b';'
        || (byte == b'#' && text[..start].bytes().all(|byte| byte.is_ascii_whitespace()))
    {
        push_segment(line, start, bytes.len(), text, Some(SyntaxClass::Comment));
        return bytes.len();
    }
    if byte == b'"' {
        *state = StringLexState::String;
        return quoted_string_token(line, text, start, state);
    }
    if byte.is_ascii_digit()
        || (byte == b'-' && bytes.get(start + 1).is_some_and(u8::is_ascii_digit))
    {
        let end = consume_while(bytes, start + 1, |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
        });
        push_segment(line, start, end, text, Some(SyntaxClass::Number));
        return end;
    }
    if is_ir_ident_start(byte) {
        let end = consume_while(bytes, start + 1, is_ir_ident_continue);
        let ident = &text[start..end];
        let class = if ident.starts_with('%') || ident.starts_with('@') || ident.starts_with('$') {
            Some(SyntaxClass::Variable)
        } else if ident.starts_with('!') || ident.starts_with('#') {
            Some(SyntaxClass::Type)
        } else if is_compiler_ir_keyword(ident.trim_start_matches('.')) {
            Some(SyntaxClass::Keyword)
        } else if next_non_space(bytes, end) == Some(b'(') {
            Some(SyntaxClass::Function)
        } else {
            None
        };
        push_segment(line, start, end, text, class);
        return end;
    }
    if is_operator(byte) {
        let end = consume_while(bytes, start + 1, is_operator);
        push_segment(line, start, end, text, Some(SyntaxClass::Operator));
        return end;
    }
    if is_punctuation(byte) {
        let end = start + 1;
        push_segment(line, start, end, text, Some(SyntaxClass::Punctuation));
        return end;
    }
    let end = advance_char(text, start);
    push_segment(line, start, end, text, None);
    end
}

fn quoted_string_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut StringLexState,
) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = advance_escaped_char(text, cursor),
            b'"' => {
                cursor += 1;
                *state = StringLexState::Normal;
                push_segment(line, start, cursor, text, Some(SyntaxClass::String));
                return cursor;
            }
            _ => cursor = advance_char(text, cursor),
        }
    }
    push_segment(line, start, bytes.len(), text, Some(SyntaxClass::String));
    bytes.len()
}

fn is_ir_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'.' | b'%' | b'@' | b'$' | b'!' | b'#')
}

fn is_ir_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'_' | b'.' | b'-' | b'%' | b'@' | b'$' | b'!' | b'#' | b':'
        )
}

fn is_compiler_ir_keyword(ident: &str) -> bool {
    if compiler_ir_keyword_lower(ident) {
        return true;
    }
    ident.bytes().any(|byte| byte.is_ascii_uppercase())
        && compiler_ir_keyword_lower(&ident.to_ascii_lowercase())
}

fn compiler_ir_keyword_lower(ident: &str) -> bool {
    matches!(
        ident,
        "add"
            | "addrspace"
            | "alloca"
            | "and"
            | "ask"
            | "attributes"
            | "background"
            | "base"
            | "bind"
            | "br"
            | "call"
            | "class"
            | "constant"
            | "construct"
            | "constructor"
            | "declare"
            | "def"
            | "define"
            | "delete"
            | "describe"
            | "else"
            | "enum"
            | "examples"
            | "export"
            | "external"
            | "false"
            | "fcmp"
            | "feature"
            | "filter"
            | "flags"
            | "for"
            | "func"
            | "function"
            | "future"
            | "given"
            | "graph"
            | "global"
            | "icmp"
            | "if"
            | "import"
            | "in"
            | "insert"
            | "interface"
            | "let"
            | "list"
            | "load"
            | "module"
            | "mul"
            | "option"
            | "optional"
            | "or"
            | "package"
            | "prefix"
            | "private"
            | "record"
            | "resource"
            | "result"
            | "ret"
            | "return"
            | "rule"
            | "scenario"
            | "select"
            | "store"
            | "stream"
            | "sub"
            | "target"
            | "then"
            | "tuple"
            | "true"
            | "type"
            | "use"
            | "variant"
            | "when"
            | "where"
            | "while"
            | "world"
            | "xor"
    )
}

fn highlight_lisp_like_fast(source: &str) -> HighlightedText {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut state = StringLexState::Normal;

    for chunk in LineChunks::new(source) {
        let mut line = HighlightedLine::new(chunk.text);
        let mut cursor = 0usize;
        let bytes = chunk.text.as_bytes();
        while cursor < bytes.len() {
            cursor = match state {
                StringLexState::Normal => {
                    lisp_normal_token(&mut line, chunk.text, cursor, &mut state)
                }
                StringLexState::String => {
                    quoted_string_token(&mut line, chunk.text, cursor, &mut state)
                }
            };
        }
        state = StringLexState::Normal;
        lines.push(line);
    }

    debug_assert_eq!(lines.len(), line_count);
    HighlightedText { lines }
}

fn lisp_normal_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut StringLexState,
) -> usize {
    let bytes = text.as_bytes();
    let byte = bytes[start];
    if byte.is_ascii_whitespace() {
        let end = consume_while(bytes, start, |byte| byte.is_ascii_whitespace());
        push_segment(line, start, end, text, None);
        return end;
    }
    if byte == b';' {
        push_segment(line, start, bytes.len(), text, Some(SyntaxClass::Comment));
        return bytes.len();
    }
    if byte == b'"' {
        *state = StringLexState::String;
        return quoted_string_token(line, text, start, state);
    }
    if byte.is_ascii_digit()
        || (byte == b'-' && bytes.get(start + 1).is_some_and(u8::is_ascii_digit))
    {
        let end = consume_while(bytes, start + 1, |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'-' | b'+')
        });
        push_segment(line, start, end, text, Some(SyntaxClass::Number));
        return end;
    }
    if matches!(byte, b'(' | b')' | b'[' | b']' | b'{' | b'}') {
        push_segment(line, start, start + 1, text, Some(SyntaxClass::Punctuation));
        return start + 1;
    }
    if matches!(byte, b'\'' | b'`' | b',' | b'@') {
        push_segment(line, start, start + 1, text, Some(SyntaxClass::Operator));
        return start + 1;
    }
    let end = consume_while(bytes, start + 1, |byte| {
        !byte.is_ascii_whitespace()
            && !matches!(byte, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b';' | b'"')
    });
    let ident = &text[start..end];
    let class = if ident.starts_with(':') {
        Some(SyntaxClass::Constant)
    } else if is_lisp_keyword(ident) {
        Some(SyntaxClass::Keyword)
    } else if ident.chars().next().is_some_and(char::is_uppercase) {
        Some(SyntaxClass::Type)
    } else {
        None
    };
    push_segment(line, start, end, text, class);
    end
}

fn is_lisp_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "def"
            | "defclass"
            | "defmacro"
            | "defmethod"
            | "defn"
            | "defparameter"
            | "defun"
            | "defvar"
            | "do"
            | "fn"
            | "if"
            | "lambda"
            | "let"
            | "let*"
            | "loop"
            | "macrolet"
            | "match"
            | "ns"
            | "progn"
            | "quote"
            | "require"
            | "set!"
            | "setq"
            | "when"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkupLexState {
    Normal,
    Comment,
}

fn highlight_markup_fast(source: &str) -> HighlightedText {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut state = MarkupLexState::Normal;

    for chunk in LineChunks::new(source) {
        let mut line = HighlightedLine::new(chunk.text);
        let mut cursor = 0usize;
        let bytes = chunk.text.as_bytes();
        while cursor < bytes.len() {
            cursor = match state {
                MarkupLexState::Normal => {
                    markup_normal_token(&mut line, chunk.text, cursor, &mut state)
                }
                MarkupLexState::Comment => {
                    markup_comment_token(&mut line, chunk.text, cursor, &mut state)
                }
            };
        }
        lines.push(line);
    }

    debug_assert_eq!(lines.len(), line_count);
    HighlightedText { lines }
}

fn markup_normal_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut MarkupLexState,
) -> usize {
    let bytes = text.as_bytes();
    if bytes[start..].starts_with(b"<!--") {
        *state = MarkupLexState::Comment;
        return markup_comment_token(line, text, start, state);
    }
    let byte = bytes[start];
    if byte.is_ascii_whitespace() {
        let end = consume_while(bytes, start, |byte| byte.is_ascii_whitespace());
        push_segment(line, start, end, text, None);
        return end;
    }
    if byte == b'<' {
        let end = find_markup_tag_end(text, start);
        push_markup_tag(line, text, start, end);
        return end;
    }
    if byte == b'&' {
        let end = find_bytes(bytes, start + 1, b";")
            .map(|end| end + 1)
            .unwrap_or(start + 1);
        push_segment(
            line,
            start,
            end.min(bytes.len()),
            text,
            Some(SyntaxClass::Constant),
        );
        return end.min(bytes.len());
    }
    let end = bytes[start..]
        .iter()
        .position(|byte| matches!(*byte, b'<' | b'&'))
        .map(|offset| start + offset)
        .unwrap_or(bytes.len());
    push_segment(line, start, end, text, None);
    end
}

fn markup_comment_token(
    line: &mut HighlightedLine,
    text: &str,
    start: usize,
    state: &mut MarkupLexState,
) -> usize {
    let bytes = text.as_bytes();
    let comment_end = find_bytes(bytes, start, b"-->");
    let end = comment_end.map(|end| end + 3).unwrap_or(bytes.len());
    push_segment(line, start, end, text, Some(SyntaxClass::Comment));
    if comment_end.is_some() {
        *state = MarkupLexState::Normal;
    }
    end
}

fn find_markup_tag_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(start), Some(&b'<'));

    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = find_quote_end(text, cursor + 1, bytes[cursor]),
            b'>' => return cursor + 1,
            _ => cursor = advance_char(text, cursor),
        }
    }
    bytes.len()
}

fn push_markup_tag(line: &mut HighlightedLine, text: &str, start: usize, end: usize) {
    let bytes = text.as_bytes();
    let mut cursor = start;
    while cursor < end {
        match bytes[cursor] {
            b'\'' | b'"' => {
                let quote = bytes[cursor];
                let string_end = find_quote_end(text, cursor + 1, quote).min(end);
                push_segment(line, cursor, string_end, text, Some(SyntaxClass::String));
                cursor = string_end;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'-' || byte == b':' => {
                let ident_start = cursor;
                let ident_end = consume_while(bytes, cursor + 1, |byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'_' | b'.')
                })
                .min(end);
                let class = if previous_non_space(bytes, start, ident_start) == Some(b'<')
                    || previous_non_space(bytes, start, ident_start) == Some(b'/')
                {
                    Some(SyntaxClass::Tag)
                } else {
                    Some(SyntaxClass::Attribute)
                };
                push_segment(line, ident_start, ident_end, text, class);
                cursor = ident_end;
            }
            _ => {
                let next = advance_char(text, cursor).min(end);
                push_segment(line, cursor, next, text, Some(SyntaxClass::Punctuation));
                cursor = next;
            }
        }
    }
}

fn find_quote_end(text: &str, start: usize, quote: u8) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = advance_escaped_char(text, cursor),
            byte if byte == quote => return cursor + 1,
            _ => cursor = advance_char(text, cursor),
        }
    }
    bytes.len()
}

fn previous_non_space(bytes: &[u8], start: usize, mut cursor: usize) -> Option<u8> {
    while cursor > start {
        cursor -= 1;
        if !bytes[cursor].is_ascii_whitespace() {
            return Some(bytes[cursor]);
        }
    }
    None
}

pub fn available_languages() -> Vec<String> {
    grammar_catalog().languages.clone()
}

pub fn has_language(language: &str) -> bool {
    canonical_language_name(language).is_some()
}

pub fn canonical_language(language: &str) -> Option<String> {
    canonical_language_name(language).map(LanguageName::into_string)
}

pub fn detect_language_from_path(path: &str) -> Option<String> {
    let path = Path::new(path);
    let set = syntax_set();

    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if let Some(alias) = basename_alias(name) {
            return canonical_language(alias);
        }
        if let Some(syntax) = set.find_syntax_by_token(name) {
            return canonical_from_syntax(syntax).map(LanguageName::into_string);
        }
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(detect_language_from_extension)
}

pub fn detect_language_from_extension(extension: &str) -> Option<String> {
    let extension = normalize_language_token(extension);
    if extension.is_empty() {
        return None;
    }
    grammar_catalog()
        .extension_aliases
        .get(extension.as_str())
        .and_then(|language| LanguageName::new(language.clone()))
        .map(LanguageName::into_string)
}

pub fn classify_scope_name(scope: &str) -> Option<SyntaxClass> {
    classify_scope_text(scope)
}

fn highlight_with_syntax(
    source: &str,
    syntax: &SyntaxReference,
    language: &str,
    classifier: &mut ScopeClassifier,
) -> Result<HighlightedText, HighlightError> {
    let line_count = source.split('\n').count();
    let mut lines = Vec::with_capacity(line_count);
    let mut parse_state = ParseState::new(syntax);
    let mut scope_stack = ScopeStack::new();

    for chunk in LineChunks::new(source) {
        let line_source = chunk.parse_text;
        let text_len = chunk.text.len();
        let ops = parse_state
            .parse_line(line_source, syntax_set())
            .map_err(|error| HighlightError::Parse {
                language: language.to_owned(),
                message: error.to_string(),
            })?;
        let mut highlighted = HighlightedLine::new(chunk.text);
        let mut offset = 0usize;
        let mut op_index = 0usize;

        while op_index < ops.len() {
            let raw_index = ops[op_index].0.min(text_len);
            debug_assert!(raw_index <= text_len);
            push_segment(
                &mut highlighted,
                offset,
                raw_index,
                chunk.text,
                classifier.class_for_stack(&scope_stack),
            );
            offset = raw_index;

            while op_index < ops.len() && ops[op_index].0.min(text_len) == raw_index {
                scope_stack
                    .apply(&ops[op_index].1)
                    .map_err(|error| HighlightError::Scope {
                        language: language.to_owned(),
                        message: error.to_string(),
                    })?;
                op_index += 1;
            }
        }

        push_segment(
            &mut highlighted,
            offset,
            text_len,
            chunk.text,
            classifier.class_for_stack(&scope_stack),
        );
        lines.push(highlighted);
    }

    debug_assert_eq!(lines.len(), line_count);
    Ok(HighlightedText { lines })
}

fn push_segment(
    line: &mut HighlightedLine,
    start: usize,
    end: usize,
    text: &str,
    class: Option<SyntaxClass>,
) {
    if start >= end {
        return;
    }
    debug_assert!(text.is_char_boundary(start));
    debug_assert!(text.is_char_boundary(end));
    if let Some(last) = line.segments.last_mut()
        && last.class == class
        && last.byte_end == start
    {
        last.byte_end = end;
        return;
    }
    line.segments.push(SyntaxSegment::new(start, end, class));
}

fn stable_text_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[derive(Debug, Clone, Copy)]
struct LineChunk<'a> {
    text: &'a str,
    parse_text: &'a str,
}

struct LineChunks<'a> {
    source: &'a str,
    offset: usize,
    final_empty_line: FinalEmptyLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalEmptyLine {
    Pending,
    Complete,
}

impl<'a> LineChunks<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            final_empty_line: if source.is_empty() || source.ends_with('\n') {
                FinalEmptyLine::Pending
            } else {
                FinalEmptyLine::Complete
            },
        }
    }
}

impl<'a> Iterator for LineChunks<'a> {
    type Item = LineChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.source.len() {
            if self.final_empty_line == FinalEmptyLine::Pending {
                self.final_empty_line = FinalEmptyLine::Complete;
                return Some(LineChunk {
                    text: "",
                    parse_text: "",
                });
            }
            return None;
        }

        let rest = &self.source[self.offset..];
        if let Some(newline) = rest.find('\n') {
            let end = self.offset + newline + 1;
            let parse_text = &self.source[self.offset..end];
            let text = &parse_text[..parse_text.len() - 1];
            self.offset = end;
            Some(LineChunk { text, parse_text })
        } else {
            let parse_text = rest;
            self.offset = self.source.len();
            Some(LineChunk {
                text: parse_text,
                parse_text,
            })
        }
    }
}

#[derive(Debug, Default)]
struct ScopeClassifier {
    cache: HashMap<Scope, Option<SyntaxClass>>,
}

impl ScopeClassifier {
    fn class_for_stack(&mut self, stack: &ScopeStack) -> Option<SyntaxClass> {
        for preferred in [SyntaxClass::Tag, SyntaxClass::Attribute] {
            if stack
                .as_slice()
                .iter()
                .rev()
                .any(|scope| self.class_for_scope(*scope) == Some(preferred))
            {
                return Some(preferred);
            }
        }

        stack
            .as_slice()
            .iter()
            .rev()
            .find_map(|scope| self.class_for_scope(*scope))
    }

    fn class_for_scope(&mut self, scope: Scope) -> Option<SyntaxClass> {
        if let Some(class) = self.cache.get(&scope) {
            return *class;
        }
        let text = scope.build_string();
        let class = classify_scope_text(&text);
        self.cache.insert(scope, class);
        class
    }
}

fn classify_scope_text(scope: &str) -> Option<SyntaxClass> {
    let first = scope.split('.').next().unwrap_or(scope);
    match first {
        "comment" => Some(SyntaxClass::Comment),
        "string" => Some(SyntaxClass::String),
        "constant" => {
            if scope.starts_with("constant.numeric") {
                Some(SyntaxClass::Number)
            } else if scope.starts_with("constant.language.boolean") {
                Some(SyntaxClass::Keyword)
            } else {
                Some(SyntaxClass::Constant)
            }
        }
        "keyword" => {
            if scope.starts_with("keyword.operator") {
                Some(SyntaxClass::Operator)
            } else {
                Some(SyntaxClass::Keyword)
            }
        }
        "storage" => Some(SyntaxClass::Keyword),
        "variable" => Some(SyntaxClass::Variable),
        "support" => {
            if scope.starts_with("support.function") {
                Some(SyntaxClass::Function)
            } else if scope.starts_with("support.type") || scope.starts_with("support.class") {
                Some(SyntaxClass::Type)
            } else if scope.starts_with("support.constant") {
                Some(SyntaxClass::Constant)
            } else {
                None
            }
        }
        "entity" => {
            if scope.starts_with("entity.name.function") {
                Some(SyntaxClass::Function)
            } else if scope.starts_with("entity.name.type")
                || scope.starts_with("entity.name.class")
                || scope.starts_with("entity.name.struct")
                || scope.starts_with("entity.name.enum")
                || scope.starts_with("entity.name.trait")
            {
                Some(SyntaxClass::Type)
            } else if scope.starts_with("entity.name.tag") {
                Some(SyntaxClass::Tag)
            } else if scope.starts_with("entity.name.namespace") {
                Some(SyntaxClass::Module)
            } else if scope.starts_with("entity.name.label") {
                Some(SyntaxClass::Label)
            } else if scope.starts_with("entity.other.attribute-name") {
                Some(SyntaxClass::Attribute)
            } else {
                None
            }
        }
        "punctuation" => Some(SyntaxClass::Punctuation),
        "invalid" => Some(SyntaxClass::Keyword),
        _ => None,
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn syntax_for_language(language: &str) -> Option<&'static SyntaxReference> {
    let catalog = grammar_catalog();
    let syntax_name = catalog.language_to_syntax.get(language)?;
    syntax_set().find_syntax_by_name(syntax_name)
}

fn canonical_language_name(token: &str) -> Option<LanguageName> {
    let token = normalize_language_token(token);
    if token.is_empty() {
        return None;
    }
    let catalog = grammar_catalog();
    if let Some(language) = catalog.aliases.get(token.as_str()) {
        return LanguageName::new(language.clone());
    }
    if catalog.language_to_syntax.contains_key(token.as_str()) {
        return LanguageName::new(token);
    }
    syntax_set()
        .find_syntax_by_token(&token)
        .and_then(canonical_from_syntax)
}

fn canonical_from_syntax(syntax: &SyntaxReference) -> Option<LanguageName> {
    let catalog = grammar_catalog();
    if let Some(language) = catalog.syntax_to_language.get(syntax.name.as_str()) {
        return LanguageName::new(language.clone());
    }
    LanguageName::new(slug_syntax_name(&syntax.name))
}

#[derive(Debug)]
struct GrammarCatalog {
    languages: Vec<String>,
    aliases: BTreeMap<String, String>,
    extension_aliases: BTreeMap<String, String>,
    syntax_to_language: BTreeMap<String, String>,
    language_to_syntax: BTreeMap<String, String>,
}

fn grammar_catalog() -> &'static GrammarCatalog {
    static CATALOG: OnceLock<GrammarCatalog> = OnceLock::new();
    CATALOG.get_or_init(build_grammar_catalog)
}

fn build_grammar_catalog() -> GrammarCatalog {
    let mut syntax_to_language = BTreeMap::new();
    let mut language_to_syntax = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    let mut extension_aliases = BTreeMap::new();

    for syntax in syntax_set().syntaxes() {
        let language = known_language_for_syntax(&syntax.name)
            .map(str::to_owned)
            .unwrap_or_else(|| slug_syntax_name(&syntax.name));
        syntax_to_language.insert(syntax.name.clone(), language.clone());
        language_to_syntax
            .entry(language.clone())
            .or_insert_with(|| syntax.name.clone());
        aliases.insert(normalize_language_token(&syntax.name), language.clone());
        aliases.insert(
            normalize_language_token(&syntax.scope.build_string()),
            language.clone(),
        );
        for extension in &syntax.file_extensions {
            let extension = normalize_language_token(extension);
            if !extension.is_empty() {
                aliases.insert(extension.clone(), language.clone());
                extension_aliases.insert(extension, language.clone());
            }
        }
    }

    for (language, syntax) in LANGUAGE_SYNTAX_NAMES {
        language_to_syntax.insert((*language).to_owned(), (*syntax).to_owned());
        aliases.insert((*language).to_owned(), (*language).to_owned());
    }
    // Fast lexers do not come with TextMate grammar metadata, so seed their
    // language names as extension matches before curated path overrides below.
    for language in FAST_ONLY_LANGUAGES {
        aliases.insert((*language).to_owned(), (*language).to_owned());
        insert_language_alias(
            &mut extension_aliases,
            &language_to_syntax,
            language,
            language,
        );
    }
    // The alias table is intentionally broader than the bundled grammars. Let
    // curated aliases override ambiguous syntax-derived extension aliases, but
    // only when their target is highlightable in this build.
    for (alias, language) in LANGUAGE_ALIASES {
        insert_language_alias(&mut aliases, &language_to_syntax, alias, language);
    }
    // Extension detection needs its own table so language-name aliases such as
    // `v` (the V language) cannot override path mappings such as `*.v`
    // (Verilog). Keep the existing code-fence/extension aliases available for
    // paths, then let path-specific overrides correct ambiguous extensions.
    for (extension, language) in LANGUAGE_ALIASES {
        insert_language_alias(
            &mut extension_aliases,
            &language_to_syntax,
            extension,
            language,
        );
    }
    // Curated extension aliases intentionally override bundled grammar
    // extensions when the grammar bundle exposes a more specific or less
    // stable syntax name.
    for (extension, language) in EXTENSION_ALIASES {
        insert_language_alias(
            &mut extension_aliases,
            &language_to_syntax,
            extension,
            language,
        );
    }

    let mut languages = language_to_syntax.keys().cloned().collect::<Vec<_>>();
    languages.extend(
        FAST_ONLY_LANGUAGES
            .iter()
            .map(|language| (*language).to_owned()),
    );
    languages.sort();
    languages.dedup();

    GrammarCatalog {
        languages,
        aliases,
        extension_aliases,
        syntax_to_language,
        language_to_syntax,
    }
}

fn insert_language_alias(
    aliases: &mut BTreeMap<String, String>,
    language_to_syntax: &BTreeMap<String, String>,
    alias: &str,
    language: &str,
) {
    let alias = normalize_language_token(alias);
    let language = normalize_language_token(language);
    if alias.is_empty() || !is_highlightable_language(&language, language_to_syntax) {
        return;
    }
    aliases.insert(alias, language);
}

fn is_highlightable_language(
    language: &str,
    language_to_syntax: &BTreeMap<String, String>,
) -> bool {
    language_to_syntax.contains_key(language) || FAST_ONLY_LANGUAGES.contains(&language)
}

fn known_language_for_syntax(name: &str) -> Option<&'static str> {
    LANGUAGE_SYNTAX_NAMES
        .iter()
        .find_map(|(language, syntax)| (*syntax == name).then_some(*language))
}

fn basename_alias(name: &str) -> Option<&'static str> {
    BASENAME_ALIASES
        .iter()
        .find_map(|(basename, language)| name.eq_ignore_ascii_case(basename).then_some(*language))
}

fn normalize_language_token(token: &str) -> String {
    token
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(*ch, '+' | '#' | '_' | '-'))
        .collect()
}

fn slug_syntax_name(name: &str) -> String {
    let mut output = String::new();
    let mut separator = SlugSeparator::None;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if separator == SlugSeparator::Pending && !output.is_empty() {
                output.push('-');
            }
            separator = SlugSeparator::None;
            output.push(ch);
        } else if ch == '+' || ch == '#' {
            output.push(ch);
            separator = SlugSeparator::None;
        } else {
            separator = SlugSeparator::Pending;
        }
    }
    output.trim_matches('-').to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlugSeparator {
    None,
    Pending,
}

const FAST_ONLY_LANGUAGES: &[&str] = &[
    "abap",
    "agda",
    "apl",
    "apex",
    "angular-html",
    "angular-ts",
    "ara",
    "astro",
    "ballerina",
    "beancount",
    "berry",
    "bicep",
    "bird2",
    "blade",
    "bsl",
    "c3",
    "cadence",
    "cairo",
    "chapel",
    "clarity",
    "codeowners",
    "codeql",
    "common-lisp",
    "coq",
    "cobol",
    "cuda",
    "cue",
    "cypher",
    "dax",
    "dhall",
    "dream-maker",
    "edge",
    "ejs",
    "emacs-lisp",
    "fennel",
    "forth",
    "fluent",
    "gdresource",
    "gdshader",
    "genie",
    "gherkin",
    "gleam",
    "glimmer-js",
    "glimmer-ts",
    "gn",
    "hack",
    "handlebars",
    "hlsl",
    "haxe",
    "hurl",
    "hxml",
    "hy",
    "imba",
    "jison",
    "jssm",
    "just",
    "kdl",
    "kusto",
    "liquid",
    "logo",
    "luau",
    "marko",
    "mdc",
    "mdx",
    "mermaid",
    "meson",
    "metal",
    "mipsasm",
    "mlir",
    "mojo",
    "moonbit",
    "move",
    "narrat",
    "nextflow",
    "nextflow-groovy",
    "nushell",
    "opencl",
    "openscad",
    "pkl",
    "po",
    "polar",
    "pony",
    "powerquery",
    "prisma",
    "prolog",
    "pug",
    "qmldir",
    "qss",
    "raku",
    "razor",
    "rel",
    "riscv",
    "ron",
    "rosmsg",
    "sas",
    "scheme",
    "sdbl",
    "shaderlab",
    "smalltalk",
    "soy",
    "sparql",
    "splunk",
    "spirv",
    "stata",
    "surrealql",
    "systemd",
    "talonscript",
    "tablegen",
    "tasl",
    "templ",
    "twig",
    "ts-tags",
    "turtle",
    "typespec",
    "v",
    "vala",
    "wasm",
    "wikitext",
    "wenyan",
    "wit",
    "wolfram",
    "xsl",
    "zenscript",
];

const LANGUAGE_SYNTAX_NAMES: &[(&str, &str)] = &[
    ("asm", "Assembly (x86_64)"),
    ("bash", "Bourne Again Shell (bash)"),
    ("c", "C"),
    ("cmake", "CMake"),
    ("cpp", "C++"),
    ("csharp", "C#"),
    ("css", "CSS"),
    ("dockerfile", "Dockerfile"),
    ("elixir", "Elixir"),
    ("go", "Go"),
    ("html", "HTML"),
    ("java", "Java"),
    ("javascript", "JavaScript"),
    ("jsx", "JavaScript (Babel)"),
    ("json", "JSON"),
    ("kotlin", "Kotlin"),
    ("llvm", "LLVM"),
    ("lua", "Lua"),
    ("make", "Makefile"),
    ("markdown", "Markdown"),
    ("nix", "Nix"),
    ("python", "Python"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("starlark", "Python"),
    ("toml", "TOML"),
    ("tsx", "TypeScriptReact"),
    ("typescript", "TypeScript"),
    ("yaml", "YAML"),
    ("zig", "Zig"),
];

const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("1c", "bsl"),
    ("actionscript-3", "actionscript"),
    ("adoc", "asciidoc-asciidoctor"),
    ("apache", "apache-conf"),
    ("apl", "apl"),
    ("asciidoc", "asciidoc-asciidoctor"),
    ("bzl", "starlark"),
    ("bazel", "starlark"),
    ("bat", "batch-file"),
    ("batch", "batch-file"),
    ("be", "berry"),
    ("bib", "bibtex"),
    ("bird", "bird2"),
    ("c++", "cpp"),
    ("cbl", "cobol"),
    ("cc", "cpp"),
    ("c#", "csharp"),
    ("clar", "clarity"),
    ("clj", "clojure"),
    ("cljs", "clojure"),
    ("cmd", "batch-file"),
    ("cob", "cobol"),
    ("coffee", "coffeescript"),
    ("cjs", "javascript"),
    ("cl", "common-lisp"),
    ("code-owner", "codeowners"),
    ("codeowner", "codeowners"),
    ("common-lisp", "common-lisp"),
    ("commonlisp", "common-lisp"),
    ("coq", "coq"),
    ("csv", "comma-separated-values"),
    ("cs", "csharp"),
    ("cu", "cuda"),
    ("cuh", "cuda"),
    ("cxx", "cpp"),
    ("cls", "apex"),
    ("dm", "dream-maker"),
    ("dme", "dream-maker"),
    ("dmm", "dream-maker"),
    ("docker", "dockerfile"),
    ("dockerignore", "git-ignore"),
    ("edn", "clojure"),
    ("el", "emacs-lisp"),
    ("elisp", "emacs-lisp"),
    ("emacs-lisp", "emacs-lisp"),
    ("env", "dotenv"),
    ("ejs", "ejs"),
    ("erb", "html-rails"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("f77", "fortran-fixed-form"),
    ("f90", "fortran-modern"),
    ("f95", "fortran-modern"),
    ("fortran", "fortran-modern"),
    ("fortran-free-form", "fortran-modern"),
    ("fs", "f#"),
    ("fsharp", "f#"),
    ("fnl", "fennel"),
    ("feature", "gherkin"),
    ("ftl", "fluent"),
    ("gjs", "glimmer-js"),
    ("gdscript", "gdscript-godot-engine"),
    ("gts", "glimmer-ts"),
    ("git-rebase", "git-rebase-todo"),
    ("gql", "graphql"),
    ("graphqls", "graphql"),
    ("gradle", "groovy"),
    ("haml", "ruby-haml"),
    ("hcl", "terraform"),
    ("hjson", "json"),
    ("hlsl", "hlsl"),
    ("hs", "haskell"),
    ("hx", "haxe"),
    ("hy", "hy"),
    ("html-derivative", "html"),
    ("jinja", "jinja2"),
    ("jinja-html", "html-jinja2"),
    ("jl", "julia"),
    ("ipynb", "json"),
    ("kql", "kusto"),
    ("json5", "json"),
    ("jsonc", "json"),
    ("jsonl", "json"),
    ("jisonlex", "jison"),
    ("automount", "systemd"),
    ("just", "just"),
    ("justfile", "just"),
    ("lean", "lean-4"),
    ("lhs", "literate-haskell"),
    ("liquid", "liquid"),
    ("ll", "llvm"),
    ("lsp", "common-lisp"),
    ("md", "markdown"),
    ("mdx", "mdx"),
    ("ml", "ocaml"),
    ("mli", "ocaml"),
    ("mjs", "javascript"),
    ("mlir", "mlir"),
    ("mips", "mipsasm"),
    ("mount", "systemd"),
    ("msg", "rosmsg"),
    ("nb", "wolfram"),
    ("ndjson", "json"),
    ("ignorefile", "git-ignore"),
    ("js", "javascript"),
    ("javascript-babel", "jsx"),
    ("jsx", "jsx"),
    ("objective-cpp", "objective-c++"),
    ("node", "javascript"),
    ("objc", "objective-c"),
    ("objc++", "objective-c++"),
    ("pb", "protocol-buffer"),
    ("pbt", "protocol-buffer-text"),
    ("pot", "po"),
    ("pro", "prolog"),
    ("prolog", "prolog"),
    ("plsql", "sql"),
    ("postgres", "sql"),
    ("postgresql", "sql"),
    ("postcss", "css"),
    ("ql", "codeql"),
    ("properties", "java-properties"),
    ("proto", "protocol-buffer"),
    ("protobuf", "protocol-buffer"),
    ("ps1", "powershell"),
    ("ps", "powershell"),
    ("pwsh", "powershell"),
    ("python3", "python"),
    ("regex", "regular-expression"),
    ("regexp", "regular-expression"),
    ("risc-v", "riscv"),
    ("rest", "restructuredtext"),
    ("rst", "restructuredtext"),
    ("scad", "openscad"),
    ("s", "asm"),
    ("scm", "scheme"),
    ("scheme", "scheme"),
    ("makefile", "make"),
    ("shell", "bash"),
    ("shellscript", "bash"),
    ("shell-session", "shell-unix-generic"),
    ("shellsession", "shell-unix-generic"),
    ("sh", "bash"),
    ("shader", "shaderlab"),
    ("slim", "ruby-slim"),
    ("sol", "solidity"),
    ("spl", "splunk"),
    ("spir-v", "spirv"),
    ("spirv-asm", "spirv"),
    ("sv", "systemverilog"),
    ("service", "systemd"),
    ("socket", "systemd"),
    ("scope", "systemd"),
    ("slice", "systemd"),
    ("srv", "rosmsg"),
    ("swap", "systemd"),
    ("system-verilog", "systemverilog"),
    ("target", "systemd"),
    ("td", "tablegen"),
    ("tfstate", "json"),
    ("tf", "terraform"),
    ("tfvars", "terraform"),
    ("ts", "typescript"),
    ("trigger", "apex"),
    ("tres", "gdresource"),
    ("tscn", "gdresource"),
    ("tsv", "tab-separated-values"),
    ("ttl", "turtle"),
    ("timer", "systemd"),
    ("twig", "twig"),
    ("typescriptreact", "tsx"),
    ("vim", "viml"),
    ("vimscript", "viml"),
    ("vue", "vue-component"),
    ("vue-html", "vue-component"),
    ("vue-vine", "vue-component"),
    ("wast", "wasm"),
    ("wat", "wasm"),
    ("wy", "wenyan"),
    ("wl", "wolfram"),
    ("wls", "wolfram"),
    ("x86asm", "x86-64-assembly"),
    ("xslt", "xsl"),
    ("yml", "yaml"),
    ("zs", "zenscript"),
    ("zsh", "bash"),
];

const EXTENSION_ALIASES: &[(&str, &str)] = &[
    ("bazel", "starlark"),
    ("bzl", "starlark"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cls", "apex"),
    ("cs", "csharp"),
    ("cu", "cuda"),
    ("cuh", "cuda"),
    ("cxx", "cpp"),
    ("fs", "f#"),
    ("h", "c"),
    ("h++", "cpp"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("js", "javascript"),
    ("jsx", "jsx"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("sv", "systemverilog"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("v", "verilog"),
];

const BASENAME_ALIASES: &[(&str, &str)] = &[
    ("BUILD", "starlark"),
    ("BUILD.bazel", "starlark"),
    ("WORKSPACE", "starlark"),
    ("WORKSPACE.bazel", "starlark"),
    ("MODULE.bazel", "starlark"),
    ("CODEOWNERS", "codeowners"),
    ("Dockerfile", "dockerfile"),
    ("Justfile", "just"),
    ("qmldir", "qmldir"),
    ("Makefile", "make"),
    ("GNUmakefile", "make"),
    ("BSDmakefile", "make"),
    ("CMakeLists.txt", "cmake"),
    (".bazelrc", "starlark"),
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".dockerignore", "git-ignore"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust() {
        let mut highlighter = TextMateHighlighter::new();
        let highlighted = highlighter
            .highlight("rust", "fn main() {\n    let value = 1;\n}")
            .unwrap();
        assert_eq!(highlighted.lines.len(), 3);
        assert!(
            highlighted.lines[0]
                .segments
                .iter()
                .any(|segment| segment.class.is_some())
        );
    }

    #[test]
    fn preserves_empty_and_trailing_lines() {
        let mut highlighter = TextMateHighlighter::new();
        assert_eq!(highlighter.highlight("rust", "").unwrap().lines.len(), 1);
        assert_eq!(
            highlighter
                .highlight("rust", "fn main() {}\n")
                .unwrap()
                .lines
                .len(),
            2
        );
    }

    #[test]
    fn fast_path_highlights_typescript() {
        let mut highlighter = TextMateHighlighter::new();
        let ts = highlighter
            .highlight("typescript", "export function value() { return 1; }")
            .unwrap();
        assert!(
            ts.lines[0]
                .segments
                .iter()
                .any(|segment| segment.class == Some(SyntaxClass::Keyword))
        );
    }

    #[test]
    fn jsx_and_tsx_use_react_grammars_for_tags_and_attributes() {
        let source = "const view = <Button data-id=\"x\" disabled />;";
        let mut highlighter = TextMateHighlighter::new();

        for language in ["jsx", "tsx"] {
            let highlighted = highlighter.highlight(language, source).unwrap();
            assert!(
                span_has_class(source, &highlighted.lines[0], "Button", SyntaxClass::Tag),
                "{language} should classify JSX tag names as tags"
            );
            assert!(
                span_has_class(
                    source,
                    &highlighted.lines[0],
                    "data-id",
                    SyntaxClass::Attribute
                ),
                "{language} should classify JSX attribute names as attributes"
            );
            assert!(
                span_has_class(
                    source,
                    &highlighted.lines[0],
                    "disabled",
                    SyntaxClass::Attribute
                ),
                "{language} should classify boolean JSX attribute names as attributes"
            );
        }
    }

    #[test]
    fn rust_fast_path_does_not_treat_lifetimes_as_char_literals() {
        let source = "struct Ref<'a> { value: &'a str }\nfn main() { let c = 'x'; }";
        let highlighted = highlight_rust_fast(source);

        assert!(!has_class(&highlighted.lines[0], SyntaxClass::String));
        assert!(span_has_class(
            "fn main() { let c = 'x'; }",
            &highlighted.lines[1],
            "'x'",
            SyntaxClass::String,
        ));
    }

    #[test]
    fn rust_fast_path_handles_raw_string_delimiters() {
        let source = "let value = r\"abc\";\nlet next = 1;";
        let highlighted = highlight_rust_fast(source);

        assert!(span_has_class(
            "let value = r\"abc\";",
            &highlighted.lines[0],
            "r\"abc\"",
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[1], SyntaxClass::String));

        let source = "let value = r#\"abc\n\"#;\nlet next = 1;";
        let highlighted = highlight_rust_fast(source);

        assert!(span_has_class(
            "\"#;",
            &highlighted.lines[1],
            "\"#",
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[2], SyntaxClass::String));

        let source = concat!(r##"let value = br#"\"#;"##, "\nlet next = 1;");
        let highlighted = highlight_rust_fast(source);

        assert!(span_has_class(
            r##"let value = br#"\"#;"##,
            &highlighted.lines[0],
            r##"br#"\"#"##,
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[1], SyntaxClass::String));

        let source = concat!(r##"let value = cr#"\"#;"##, "\nlet next = 1;");
        let highlighted = highlight_rust_fast(source);

        assert!(span_has_class(
            r##"let value = cr#"\"#;"##,
            &highlighted.lines[0],
            r##"cr#"\"#"##,
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[1], SyntaxClass::String));
    }

    #[test]
    fn rust_fast_path_closes_multiline_string_at_column_zero() {
        let highlighted = highlight_rust_fast("let s = \"first\n\";\nlet n = 1;");

        assert_eq!(
            highlighted.lines[1].segments,
            vec![
                SyntaxSegment::new(0, 1, Some(SyntaxClass::String)),
                SyntaxSegment::new(1, 2, Some(SyntaxClass::Punctuation)),
            ]
        );
        assert_eq!(
            highlighted.lines[2]
                .segments
                .first()
                .map(|segment| segment.class),
            Some(Some(SyntaxClass::Keyword))
        );
    }

    #[test]
    fn rust_fast_path_scans_multiline_string_continuation_from_utf8_boundary() {
        let highlighted = highlight_rust_fast("let s = \"first\né\";");

        assert_eq!(
            highlighted.lines[1].segments,
            vec![
                SyntaxSegment::new(0, 3, Some(SyntaxClass::String)),
                SyntaxSegment::new(3, 4, Some(SyntaxClass::Punctuation)),
            ]
        );
    }

    #[test]
    fn rust_fast_path_tracks_nested_block_comments() {
        let line_text = "/* outer /* inner */ still commented */ let value = 1;";
        let highlighted = highlight_rust_fast(line_text);

        assert!(span_has_class(
            line_text,
            &highlighted.lines[0],
            "still commented */",
            SyntaxClass::Comment,
        ));
        assert!(span_has_class(
            line_text,
            &highlighted.lines[0],
            "let",
            SyntaxClass::Keyword,
        ));

        let highlighted =
            highlight_rust_fast("/* outer\n/* inner */\nstill commented */\nlet value = 1;");
        assert!(span_has_class(
            "still commented */",
            &highlighted.lines[2],
            "still commented */",
            SyntaxClass::Comment,
        ));
        assert!(!has_class(&highlighted.lines[3], SyntaxClass::Comment));
        assert!(span_has_class(
            "let value = 1;",
            &highlighted.lines[3],
            "let",
            SyntaxClass::Keyword,
        ));
    }

    #[test]
    fn go_fast_path_does_not_escape_raw_string_backticks() {
        let source = "var s = `abc\\`;\nvar next = 1;";
        let highlighted = highlight_c_like_fast(source, CLikeLanguage::Go);

        assert!(span_has_class(
            "var s = `abc\\`;",
            &highlighted.lines[0],
            "`abc\\`",
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[1], SyntaxClass::String));
        assert!(span_has_class(
            "var next = 1;",
            &highlighted.lines[1],
            "var",
            SyntaxClass::Keyword,
        ));

        let javascript = highlight_c_like_fast(source, CLikeLanguage::EcmaScript);
        assert!(has_class(&javascript.lines[1], SyntaxClass::String));
    }

    #[test]
    fn ecmascript_fast_path_handles_regex_literals_with_slashes() {
        let source = "const re = /https?:\\/\\//; const ok = true;";
        let highlighted = highlight_c_like_fast(source, CLikeLanguage::EcmaScript);

        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "/https?:\\/\\//",
            SyntaxClass::String,
        ));
        assert!(!has_class(&highlighted.lines[0], SyntaxClass::Comment));

        let mut highlighter = TextMateHighlighter::new();
        let typescript = highlighter.highlight("typescript", source).unwrap();
        assert!(span_has_class(
            source,
            &typescript.lines[0],
            "/https?:\\/\\//",
            SyntaxClass::String,
        ));
        assert!(!has_class(&typescript.lines[0], SyntaxClass::Comment));

        let source = "return /[/:]\\/\\//i.test(value); // trailing";
        let highlighted = highlight_c_like_fast(source, CLikeLanguage::EcmaScript);

        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "/[/:]\\/\\//i",
            SyntaxClass::String,
        ));
        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "// trailing",
            SyntaxClass::Comment,
        ));
    }

    #[test]
    fn ecmascript_fast_path_keeps_division_before_line_comment() {
        let source = "const ratio = total / count; // trailing";
        let highlighted = highlight_c_like_fast(source, CLikeLanguage::EcmaScript);

        assert!(!span_has_class(
            source,
            &highlighted.lines[0],
            "/ count",
            SyntaxClass::String,
        ));
        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "// trailing",
            SyntaxClass::Comment,
        ));
    }

    #[test]
    fn fast_paths_reset_comments_closed_at_line_end() {
        let rust = highlight_rust_fast("/* ok */\nlet value = 1;");
        assert!(has_class(&rust.lines[0], SyntaxClass::Comment));
        assert!(!has_class(&rust.lines[1], SyntaxClass::Comment));
        assert!(has_class(&rust.lines[1], SyntaxClass::Keyword));

        let c_like = highlight_c_like_fast("/* ok */\nreturn 1;", CLikeLanguage::C);
        assert!(has_class(&c_like.lines[0], SyntaxClass::Comment));
        assert!(!has_class(&c_like.lines[1], SyntaxClass::Comment));
        assert!(has_class(&c_like.lines[1], SyntaxClass::Keyword));

        let markup = highlight_markup_fast("<!-- ok -->\n<div></div>");
        assert!(has_class(&markup.lines[0], SyntaxClass::Comment));
        assert!(!has_class(&markup.lines[1], SyntaxClass::Comment));
        assert!(has_class(&markup.lines[1], SyntaxClass::Tag));
    }

    #[test]
    fn markup_fast_path_ignores_gt_inside_quoted_attributes() {
        let source = "<a title=\"1 > 0\" href='#'>link</a>";
        let highlighted = highlight_markup_fast(source);

        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "\"1 > 0\"",
            SyntaxClass::String,
        ));
        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "href",
            SyntaxClass::Attribute,
        ));
        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "'#'",
            SyntaxClass::String,
        ));

        let source = "<Component data-test='a > b' disabled />";
        let highlighted = highlight_markup_fast(source);

        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "'a > b'",
            SyntaxClass::String,
        ));
        assert!(span_has_class(
            source,
            &highlighted.lines[0],
            "disabled",
            SyntaxClass::Attribute,
        ));
    }

    #[test]
    fn fast_string_scanners_advance_escapes_on_utf8_boundaries() {
        let rust = highlight_rust_fast("let value = \"\\é\";\nlet next = 1;");
        assert!(span_has_class(
            "let value = \"\\é\";",
            &rust.lines[0],
            "\"\\é\"",
            SyntaxClass::String,
        ));
        assert!(!has_class(&rust.lines[1], SyntaxClass::String));

        let c_like = highlight_c_like_fast(
            "const value = \"\\é\";\nconst next = 1;",
            CLikeLanguage::EcmaScript,
        );
        assert!(span_has_class(
            "const value = \"\\é\";",
            &c_like.lines[0],
            "\"\\é\"",
            SyntaxClass::String,
        ));
        assert!(!has_class(&c_like.lines[1], SyntaxClass::String));

        let template = highlight_c_like_fast(
            "const value = `\\é`;\nconst next = 1;",
            CLikeLanguage::EcmaScript,
        );
        assert!(span_has_class(
            "const value = `\\é`;",
            &template.lines[0],
            "`\\é`",
            SyntaxClass::String,
        ));
        assert!(!has_class(&template.lines[1], SyntaxClass::String));

        let quoted = highlight_compiler_ir_fast("$value = \"\\é\"\n$next = 1");
        assert!(span_has_class(
            "$value = \"\\é\"",
            &quoted.lines[0],
            "\"\\é\"",
            SyntaxClass::String,
        ));
        assert!(!has_class(&quoted.lines[1], SyntaxClass::String));

        let markup = highlight_markup_fast("<tag title=\"\\é\"></tag>");
        assert!(span_has_class(
            "<tag title=\"\\é\"></tag>",
            &markup.lines[0],
            "\"\\é\"",
            SyntaxClass::String,
        ));
    }

    #[test]
    fn language_aliases_resolve_to_highlightable_languages() {
        for (alias, _) in LANGUAGE_ALIASES {
            let Some(language) = canonical_language(alias) else {
                continue;
            };
            assert!(
                syntax_for_language(&language).is_some()
                    || FastLanguage::for_name(&language).is_some(),
                "alias {alias:?} resolved to unhighlightable language {language:?}"
            );
        }
    }

    #[test]
    fn all_available_languages_highlight_smoke() {
        let mut highlighter = TextMateHighlighter::new();
        let mut failures = Vec::new();
        for language in available_languages() {
            match highlighter.highlight(
                &language,
                "function value() { return 1; }\n# comment\n\"string\"\n",
            ) {
                Ok(highlighted) if highlighted.lines.len() == 4 => {}
                Ok(highlighted) => failures.push(format!(
                    "{language} preserved {} lines instead of 4",
                    highlighted.lines.len()
                )),
                Err(error) => failures.push(format!("{language}: {error}")),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn detects_common_paths() {
        assert_eq!(
            detect_language_from_path("src/lib.rs").as_deref(),
            Some("rust")
        );
        assert_eq!(
            detect_language_from_path("Makefile").as_deref(),
            Some("make")
        );
        assert_eq!(
            detect_language_from_path("CMakeLists.txt").as_deref(),
            Some("cmake")
        );
        assert_eq!(
            detect_language_from_path(".bazelrc").as_deref(),
            Some("starlark")
        );
        assert_eq!(
            detect_language_from_path(".dockerignore").as_deref(),
            Some("git-ignore")
        );
        assert_eq!(
            detect_language_from_path("foo.js").as_deref(),
            Some("javascript")
        );
        assert_eq!(detect_language_from_path("foo.jsx").as_deref(), Some("jsx"));
        assert_eq!(detect_language_from_path("foo.tsx").as_deref(), Some("tsx"));
        assert_eq!(
            detect_language_from_path("foo.mjs").as_deref(),
            Some("javascript")
        );
        assert_eq!(
            detect_language_from_path("foo.cjs").as_deref(),
            Some("javascript")
        );
        assert_eq!(
            detect_language_from_path("foo.bzl").as_deref(),
            Some("starlark")
        );
        assert_eq!(
            detect_language_from_path("foo.cls").as_deref(),
            Some("apex")
        );
        assert_eq!(detect_language_from_path("foo.fs").as_deref(), Some("f#"));
        assert_eq!(detect_language_from_path("foo.h").as_deref(), Some("c"));
        assert_eq!(detect_language_from_path("foo.hpp").as_deref(), Some("cpp"));
        assert_eq!(detect_language_from_path("foo.cu").as_deref(), Some("cuda"));
        assert_eq!(
            detect_language_from_path("foo.cuh").as_deref(),
            Some("cuda")
        );
        assert_eq!(
            detect_language_from_path("foo.v").as_deref(),
            Some("verilog")
        );
        assert_eq!(canonical_language("v").as_deref(), Some("v"));
    }

    #[test]
    fn detects_fast_only_language_extensions_from_paths() {
        for (path, language) in [
            ("main.bicep", "bicep"),
            ("foo.cue", "cue"),
            ("foo.gleam", "gleam"),
            ("foo.kdl", "kdl"),
            ("foo.pkl", "pkl"),
            ("schema.prisma", "prisma"),
            ("foo.ron", "ron"),
            ("build.meson", "meson"),
        ] {
            assert_eq!(detect_language_from_path(path).as_deref(), Some(language));
        }

        assert_eq!(
            detect_language_from_path("foo.v").as_deref(),
            Some("verilog")
        );
        assert_eq!(canonical_language("v").as_deref(), Some("v"));
    }

    #[test]
    fn classifies_scopes() {
        assert_eq!(
            classify_scope_name("keyword.control"),
            Some(SyntaxClass::Keyword)
        );
        assert_eq!(
            classify_scope_name("entity.name.function"),
            Some(SyntaxClass::Function)
        );
        assert_eq!(classify_scope_name("typewriter"), None);
    }

    fn has_class(line: &HighlightedLine, class: SyntaxClass) -> bool {
        line.segments
            .iter()
            .any(|segment| segment.class == Some(class))
    }

    fn span_has_class(
        line_text: &str,
        line: &HighlightedLine,
        needle: &str,
        class: SyntaxClass,
    ) -> bool {
        let start = line_text.find(needle).expect("test span should exist");
        let end = start + needle.len();
        line.segments.iter().any(|segment| {
            segment.class == Some(class) && segment.byte_start <= start && segment.byte_end >= end
        })
    }
}
