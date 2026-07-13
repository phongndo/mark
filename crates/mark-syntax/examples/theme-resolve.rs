use std::io::{self, BufRead};

use mark_syntax::{
    HighlightScopeTable,
    theme::{BuiltinTextMateTheme, SyntaxModifiers},
};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args()
        .nth(1)
        .ok_or("usage: theme-resolve THEME")?;
    let custom_theme;
    let theme = if let Some(theme) = BuiltinTextMateTheme::from_name(&name) {
        theme.get()
    } else {
        custom_theme =
            mark_syntax::theme::TextMateTheme::from_json(&std::fs::read_to_string(&name)?)
                .map_err(|error| format!("invalid TextMate theme {name:?}: {error}"))?;
        &custom_theme
    };
    for line in io::stdin().lock().lines() {
        let line = line?;
        let scopes: Vec<String> = serde_json::from_str(&line)?;
        let names = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        let (table, stack) = HighlightScopeTable::from_scope_names(&names);
        let style = theme.resolve(&table, stack);
        let color = |color: Option<mark_syntax::theme::RgbColor>| {
            color.map(|color| format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue))
        };
        let mut modifiers = Vec::new();
        for (modifier, name) in [
            (SyntaxModifiers::ITALIC, "italic"),
            (SyntaxModifiers::BOLD, "bold"),
            (SyntaxModifiers::UNDERLINED, "underline"),
            (SyntaxModifiers::CROSSED_OUT, "strikethrough"),
        ] {
            if style.modifiers.contains(modifier) {
                modifiers.push(name);
            }
        }
        println!(
            "{}",
            json!({
                "foreground": color(style.foreground),
                "background": color(style.background),
                "modifiers": modifiers,
            })
        );
    }
    Ok(())
}
