use crate::{CliResult, args::SyntaxInspectArgs, write_stdout};

pub(crate) fn inspect(args: SyntaxInspectArgs) -> CliResult<()> {
    let source = std::fs::read_to_string(&args.path)?;
    let path = args.path.to_string_lossy();
    let language = args
        .language
        .clone()
        .or_else(|| mark_syntax::detect_language_from_path(&path))
        .ok_or_else(|| {
            mark_core::MarkError::Usage(format!(
                "could not detect a syntax language for {}",
                args.path.display()
            ))
        })?;
    let theme = mark_syntax::theme::BuiltinTextMateTheme::from_name(&args.theme)
        .ok_or_else(|| {
            mark_core::MarkError::Usage(format!("unknown exact TextMate theme {:?}", args.theme))
        })?
        .get();
    let settings = mark_syntax::load_settings()?;
    let scope_overrides = mark_syntax::theme::TextMateTheme::from_syntax_rules(
        &settings.syntax_rules,
    )
    .map_err(|error| mark_core::MarkError::Usage(format!("invalid syntax_rules: {error}")))?;
    let mut highlighter = mark_syntax::SyntaxHighlighter::new();
    let highlighted = highlighter.highlight(&language, &source)?;
    let source_lines = source.split('\n').collect::<Vec<_>>();
    let inspector = Inspector {
        args: &args,
        theme,
        colors: &settings.colors,
        scope_overrides: &scope_overrides,
        transparent_background: settings.transparent_background,
    };
    for (line_index, highlighted_line) in highlighted.lines.iter().enumerate() {
        if args.line.is_some_and(|line| line != line_index + 1) {
            continue;
        }
        let text = source_lines.get(line_index).copied().unwrap_or_default();
        for segment in &highlighted_line.segments {
            inspector.print_segment(line_index, text, highlighted_line, segment)?;
        }
    }
    Ok(())
}

struct Inspector<'a> {
    args: &'a SyntaxInspectArgs,
    theme: &'a mark_syntax::theme::TextMateTheme,
    colors: &'a mark_syntax::ColorOverrides,
    scope_overrides: &'a mark_syntax::theme::TextMateTheme,
    transparent_background: bool,
}

impl Inspector<'_> {
    fn print_segment(
        &self,
        line_index: usize,
        text: &str,
        line: &mark_syntax::HighlightedLine,
        segment: &mark_syntax::SyntaxSegment,
    ) -> CliResult<()> {
        let token = text
            .get(segment.byte_start..segment.byte_end)
            .unwrap_or_default();
        let scopes = line
            .scope_table
            .stack_names(segment.scope_stack)
            .collect::<Vec<_>>();
        let matched = self
            .theme
            .resolve_with_match(&line.scope_table, segment.scope_stack);
        let modifiers = modifier_names(matched.style.modifiers);
        let scope_override = self
            .scope_overrides
            .resolve_with_match(&line.scope_table, segment.scope_stack);
        let coarse_override = segment
            .class
            .and_then(|class| coarse_override(self.colors, class));
        let (final_foreground, final_background) = final_colors(
            self.theme,
            &matched,
            &scope_override,
            coarse_override.map(String::as_str),
            self.colors,
            self.transparent_background,
        );
        let final_modifiers = if scope_override.modifiers_matched {
            modifier_names(scope_override.style.modifiers)
        } else {
            modifiers.clone()
        };
        let user_override_changed = coarse_override.is_some()
            || self.colors.fg.is_some()
            || self.colors.bg.is_some()
            || self.transparent_background
            || scope_override.foreground_matched
            || scope_override.background_matched
            || scope_override.modifiers_matched;
        let utf16_start = text[..segment.byte_start].encode_utf16().count();
        let utf16_end = text[..segment.byte_end].encode_utf16().count();
        write_stdout(format_args!(
            "{}:{} bytes={}..{} utf16={}..{} text={:?}\n  scopes={}\n  class={:?} selector={:?} score={:?} rule={:?} fg={} bg={} modifiers={:?}\n  diff_overlay=none coarse_override={:?} scope_override={:?} user_override_changed={} final_fg={} final_bg={} final_modifiers={:?}\n",
            self.args.path.display(),
            line_index + 1,
            segment.byte_start,
            segment.byte_end,
            utf16_start,
            utf16_end,
            token,
            scopes.join(" "),
            segment.class,
            matched.selector,
            matched.score,
            matched.source_order,
            color_string(matched.style.foreground),
            color_string(matched.style.background),
            modifiers,
            coarse_override,
            scope_override.selector,
            user_override_changed,
            final_foreground,
            final_background,
            final_modifiers,
        ))?;
        Ok(())
    }
}

fn final_colors(
    theme: &mark_syntax::theme::TextMateTheme,
    matched: &mark_syntax::theme::ThemeMatch<'_>,
    scope_override: &mark_syntax::theme::ThemeMatch<'_>,
    coarse_override: Option<&str>,
    colors: &mark_syntax::ColorOverrides,
    transparent_background: bool,
) -> (String, String) {
    // Exact rendering starts with the configured line style and only replaces
    // properties for which a token rule actually matched. The resolved
    // TextMate style also contains theme defaults, so using it unconditionally
    // would hide configured fg/bg values on otherwise unmatched properties.
    let base_foreground = colors
        .fg
        .clone()
        .unwrap_or_else(|| color_string(theme.default_style().foreground));
    let themed_foreground = matched
        .foreground_matched
        .then_some(matched.style.foreground)
        .flatten()
        .map(|color| color_string(Some(color)));
    let final_foreground = scope_override
        .foreground_matched
        .then_some(scope_override.style.foreground)
        .flatten()
        .map(|color| color_string(Some(color)))
        .or_else(|| coarse_override.map(str::to_owned))
        .or(themed_foreground)
        .unwrap_or(base_foreground);

    let final_background = if transparent_background {
        "none".to_owned()
    } else if scope_override.background_matched {
        color_string(scope_override.style.background)
    } else if matched.background_matched {
        color_string(matched.style.background)
    } else {
        colors
            .bg
            .clone()
            .unwrap_or_else(|| color_string(theme.default_style().background))
    };

    (final_foreground, final_background)
}

fn color_string(color: Option<mark_syntax::theme::RgbColor>) -> String {
    color.map_or_else(
        || "none".to_owned(),
        |color| format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
    )
}

fn coarse_override(
    colors: &mark_syntax::ColorOverrides,
    class: mark_syntax::SyntaxClass,
) -> Option<&String> {
    match class {
        mark_syntax::SyntaxClass::Attribute => colors.attribute.as_ref(),
        mark_syntax::SyntaxClass::Comment => colors.comment.as_ref(),
        mark_syntax::SyntaxClass::Constant => colors.constant.as_ref(),
        mark_syntax::SyntaxClass::Constructor => colors.constructor.as_ref(),
        mark_syntax::SyntaxClass::Function => colors.function.as_ref(),
        mark_syntax::SyntaxClass::Keyword => colors.keyword.as_ref(),
        mark_syntax::SyntaxClass::Label => colors.label.as_ref(),
        mark_syntax::SyntaxClass::Module => colors.module.as_ref(),
        mark_syntax::SyntaxClass::Number => colors.number.as_ref(),
        mark_syntax::SyntaxClass::Operator => colors.operator.as_ref(),
        mark_syntax::SyntaxClass::Property => colors.property.as_ref(),
        mark_syntax::SyntaxClass::Punctuation => colors.punctuation.as_ref(),
        mark_syntax::SyntaxClass::String => colors.string.as_ref(),
        mark_syntax::SyntaxClass::Tag => colors.tag.as_ref(),
        mark_syntax::SyntaxClass::Type => colors.r#type.as_ref(),
        mark_syntax::SyntaxClass::Variable => colors.variable.as_ref(),
    }
}

fn modifier_names(modifiers: mark_syntax::theme::SyntaxModifiers) -> String {
    [
        (mark_syntax::theme::SyntaxModifiers::BOLD, "bold"),
        (mark_syntax::theme::SyntaxModifiers::ITALIC, "italic"),
        (mark_syntax::theme::SyntaxModifiers::UNDERLINED, "underline"),
        (
            mark_syntax::theme::SyntaxModifiers::CROSSED_OUT,
            "strikethrough",
        ),
    ]
    .into_iter()
    .filter(|(modifier, _)| modifiers.contains(*modifier))
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(test)]
mod tests {
    use super::final_colors;

    #[test]
    fn final_colors_use_configured_base_for_unmatched_properties() {
        let theme = mark_syntax::theme::TextMateTheme::from_json(
            r##"{
                "colors": {
                    "editor.foreground": "#aaaaaa",
                    "editor.background": "#bbbbbb"
                },
                "tokenColors": [{
                    "scope": "keyword",
                    "settings": { "foreground": "#cccccc" }
                }]
            }"##,
        )
        .unwrap();
        let overrides = mark_syntax::theme::TextMateTheme::from_syntax_rules(&[]).unwrap();
        let (table, stack) = mark_syntax::HighlightScopeTable::from_scope_names(&["source.test"]);
        let matched = theme.resolve_with_match(&table, stack);
        let scope_override = overrides.resolve_with_match(&table, stack);
        let colors = mark_syntax::ColorOverrides {
            fg: Some("#123456".to_owned()),
            bg: Some("#654321".to_owned()),
            ..Default::default()
        };

        assert_eq!(
            final_colors(&theme, &matched, &scope_override, None, &colors, false,),
            ("#123456".to_owned(), "#654321".to_owned())
        );
    }

    #[test]
    fn transparent_background_suppresses_token_and_user_rule_backgrounds() {
        let theme = mark_syntax::theme::TextMateTheme::from_json(
            r##"{
                "colors": { "editor.background": "#bbbbbb" },
                "tokenColors": [{
                    "scope": "keyword",
                    "settings": { "background": "#cccccc" }
                }]
            }"##,
        )
        .unwrap();
        let overrides = mark_syntax::theme::TextMateTheme::from_syntax_rules(&[
            mark_syntax::SyntaxRuleOverride {
                scope: "keyword".to_owned(),
                background: Some("#dddddd".to_owned()),
                ..Default::default()
            },
        ])
        .unwrap();
        let (table, stack) =
            mark_syntax::HighlightScopeTable::from_scope_names(&["keyword.control"]);
        let matched = theme.resolve_with_match(&table, stack);
        let scope_override = overrides.resolve_with_match(&table, stack);

        let (_, background) = final_colors(
            &theme,
            &matched,
            &scope_override,
            None,
            &mark_syntax::ColorOverrides::default(),
            true,
        );

        assert_eq!(background, "none");
    }
}
