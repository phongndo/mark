use std::sync::{Arc, Mutex, OnceLock};

use crate::theme::ResolvedThemeStyle;

/// A reference within one Mark highlighting result, not a Syntaxmate stack ID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ScopeStackRef(u32);

/// Exact scopes shared by a result's lines, independent of the current theme.
///
/// Syntaxmate owns scope storage. Keeping one representative token per stack
/// lets Mark borrow its scopes without copying names or retaining every token.
#[derive(Debug)]
pub struct HighlightScopeTable {
    tokens: Vec<Option<syntaxmate::Token>>,
    scope_content_bytes: usize,
    styles: Mutex<StyleCache>,
}

#[derive(Debug, Default)]
struct StyleCache {
    // Base theme and user overrides must remain warm together. Replacing a
    // theme drops its slot, so cycling themes cannot grow the result forever.
    slots: [ThemeStyles; 2],
    next_slot: usize,
    hits: u64,
    misses: u64,
}

#[derive(Debug)]
struct ThemeStyles {
    theme: u64,
    styles: Vec<Option<ResolvedThemeStyle>>,
}

impl Default for ThemeStyles {
    fn default() -> Self {
        Self {
            theme: 0,
            styles: vec![None],
        }
    }
}

impl Default for HighlightScopeTable {
    fn default() -> Self {
        Self {
            tokens: vec![None],
            scope_content_bytes: 0,
            styles: Mutex::default(),
        }
    }
}

impl PartialEq for HighlightScopeTable {
    fn eq(&self, other: &Self) -> bool {
        self.tokens.len() == other.tokens.len()
            && (0..self.tokens.len()).all(|index| {
                self.stack_names(ScopeStackRef(index as u32))
                    .eq(other.stack_names(ScopeStackRef(index as u32)))
            })
    }
}

impl Eq for HighlightScopeTable {}

impl HighlightScopeTable {
    pub(crate) fn empty_shared() -> Arc<Self> {
        static EMPTY: OnceLock<Arc<HighlightScopeTable>> = OnceLock::new();
        Arc::clone(EMPTY.get_or_init(|| Arc::new(Self::default())))
    }

    pub(crate) fn push(&mut self, token: syntaxmate::Token) -> ScopeStackRef {
        let stack =
            ScopeStackRef(u32::try_from(self.tokens.len()).expect("scope stack count fits u32"));
        // Charge scope content per distinct stack (including repeated names).
        // Syntaxmate's private allocation capacities and internal style cache
        // are not observable; this is an estimate, not allocator accounting.
        self.scope_content_bytes += token
            .scopes()
            .map(|scope| scope.len() + std::mem::size_of::<Arc<str>>())
            .sum::<usize>();
        self.tokens.push(Some(token));
        // Reserve both bounded theme slots before the result enters the
        // weighted syntax cache, so later rendering cannot increase its weight.
        for slot in &mut self
            .styles
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .slots
        {
            slot.styles.push(None);
        }
        stack
    }

    /// Builds a standalone scope view for theme diagnostics and palette probes.
    /// Syntaxmate's public API creates owned scopes through tokenization.
    pub fn from_scope_names(scopes: &[&str]) -> (Self, ScopeStackRef) {
        let mut table = Self::default();
        let Some(root) = scopes.first() else {
            return (table, ScopeStackRef::default());
        };
        let grammar = serde_json::json!({
            "scopeName": root,
            "patterns": [{"match": ".", "name": scopes[1..].join(" ")}],
        });
        let mut registry = syntaxmate::GrammarRegistry::new();
        let root = registry
            .add_json(&grammar.to_string())
            .expect("scope probe grammar");
        let mut tokenizer =
            syntaxmate::Tokenizer::new(&registry, root, syntaxmate::TokenizerOptions::default())
                .expect("scope probe tokenizer");
        let document = tokenizer.tokenize("x");
        let stack = table.push(document.lines()[0].tokens()[0].clone());
        (table, stack)
    }

    pub(crate) fn scopes(&self, stack: ScopeStackRef) -> Option<syntaxmate::Scopes<'_>> {
        self.tokens
            .get(stack.0 as usize)?
            .as_ref()
            .map(syntaxmate::Token::scopes)
    }

    pub fn stack_names(&self, stack: ScopeStackRef) -> impl Iterator<Item = &str> {
        self.scopes(stack).into_iter().flatten()
    }

    pub fn stack_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn memory_bytes(&self) -> usize {
        let styles = self
            .styles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        std::mem::size_of::<Self>()
            + self.tokens.capacity() * std::mem::size_of::<Option<syntaxmate::Token>>()
            + self.scope_content_bytes
            + styles
                .slots
                .iter()
                .map(|slot| {
                    slot.styles.capacity() * std::mem::size_of::<Option<ResolvedThemeStyle>>()
                })
                .sum::<usize>()
    }

    pub fn style_cache_stats(&self) -> (u64, u64) {
        let styles = self
            .styles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (styles.hits, styles.misses)
    }

    pub(crate) fn resolve_style(
        &self,
        theme: u64,
        stack: ScopeStackRef,
        resolve: impl FnOnce() -> ResolvedThemeStyle,
    ) -> ResolvedThemeStyle {
        let mut cache = self
            .styles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let slot = cache
            .slots
            .iter()
            .position(|slot| slot.theme == theme)
            .unwrap_or_else(|| {
                let slot = cache.next_slot;
                cache.next_slot = (slot + 1) % cache.slots.len();
                cache.slots[slot].theme = theme;
                cache.slots[slot].styles.fill(None);
                slot
            });
        if let Some(style) = cache.slots[slot]
            .styles
            .get(stack.0 as usize)
            .copied()
            .flatten()
        {
            if cfg!(feature = "diagnostics") {
                cache.hits += 1;
            }
            return style;
        }
        if cfg!(feature = "diagnostics") {
            cache.misses += 1;
        }
        let style = resolve();
        if let Some(entry) = cache.slots[slot].styles.get_mut(stack.0 as usize) {
            *entry = Some(style);
        }
        style
    }
}
