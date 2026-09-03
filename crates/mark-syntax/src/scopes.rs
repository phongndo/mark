use crate::SyntaxClass;

pub(crate) fn classify_scope_stack<'a>(
    stack: impl IntoIterator<Item = &'a str>,
) -> Option<SyntaxClass> {
    // Walk outer-to-inner once. Tag/Attribute still win if they appear anywhere,
    // otherwise the innermost classified scope matches the previous reverse scan.
    let mut found_tag = false;
    let mut found_attribute = false;
    let mut inner_class = None;
    for scope in stack {
        match classify_scope_name(scope) {
            Some(SyntaxClass::Tag) => {
                found_tag = true;
                inner_class = Some(SyntaxClass::Tag);
            }
            Some(SyntaxClass::Attribute) => {
                found_attribute = true;
                inner_class = Some(SyntaxClass::Attribute);
            }
            other @ Some(_) => inner_class = other,
            None => {}
        }
    }
    if found_tag {
        Some(SyntaxClass::Tag)
    } else if found_attribute {
        Some(SyntaxClass::Attribute)
    } else {
        inner_class
    }
}

pub fn classify_scope_name(scope: &str) -> Option<SyntaxClass> {
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
        "variable" => {
            if scope.starts_with("variable.language")
                || scope.starts_with("variable.other.constant")
                || scope.starts_with("variable.other.enummember")
            {
                Some(SyntaxClass::Constant)
            } else if scope.starts_with("variable.other.property")
                || scope.starts_with("variable.other.member")
                || scope.starts_with("variable.other.object.property")
            {
                Some(SyntaxClass::Property)
            } else {
                Some(SyntaxClass::Variable)
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_scopes() {
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

    #[test]
    fn classifies_variable_subkinds() {
        assert_eq!(
            classify_scope_name("variable.other.rust"),
            Some(SyntaxClass::Variable)
        );
        assert_eq!(
            classify_scope_name("variable.language.self.rust"),
            Some(SyntaxClass::Constant)
        );
        assert_eq!(
            classify_scope_name("variable.other.property.ts"),
            Some(SyntaxClass::Property)
        );
    }

    #[test]
    fn tag_and_attribute_have_priority() {
        let stack = ["source.test", "string.quoted", "entity.name.tag.html"];
        assert_eq!(
            classify_scope_stack(stack.into_iter()),
            Some(SyntaxClass::Tag)
        );
    }

    #[test]
    fn attribute_has_priority_over_inner_classes() {
        let stack = [
            "source.test",
            "keyword.control",
            "entity.other.attribute-name.html",
        ];
        assert_eq!(
            classify_scope_stack(stack.into_iter()),
            Some(SyntaxClass::Attribute)
        );
    }

    #[test]
    fn innermost_classified_scope_wins_without_tag_or_attribute() {
        let stack = ["source.test", "keyword.control", "variable.other.rust"];
        assert_eq!(
            classify_scope_stack(stack.into_iter()),
            Some(SyntaxClass::Variable)
        );
    }

    #[test]
    fn empty_scope_stack_is_unclassified() {
        assert_eq!(classify_scope_stack(std::iter::empty()), None);
    }
}
