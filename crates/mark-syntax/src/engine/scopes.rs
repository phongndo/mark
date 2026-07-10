use std::collections::HashMap;

use crate::SyntaxClass;

use super::state::ScopeId;

#[derive(Debug, Default)]
pub struct ScopeInterner {
    names: Vec<String>,
}

impl ScopeInterner {
    pub fn intern(&mut self, name: &str) -> ScopeId {
        if let Some(index) = self.names.iter().position(|existing| existing == name) {
            return ScopeId(index as u32);
        }
        let id = ScopeId(self.names.len() as u32);
        self.names.push(name.to_owned());
        id
    }

    pub fn get(&self, id: ScopeId) -> Option<&str> {
        self.names.get(id.0 as usize).map(String::as_str)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScopeClassifier {
    scope_cache: HashMap<String, Option<SyntaxClass>>,
    stack_cache: HashMap<Vec<String>, Option<SyntaxClass>>,
}

impl ScopeClassifier {
    pub fn class_for_stack(&mut self, stack: &[String]) -> Option<SyntaxClass> {
        if let Some(class) = self.stack_cache.get(stack) {
            return *class;
        }
        let class = classify_scope_stack(stack);
        self.stack_cache.insert(stack.to_vec(), class);
        class
    }

    pub fn class_for_scope(&mut self, scope: &str) -> Option<SyntaxClass> {
        if let Some(class) = self.scope_cache.get(scope) {
            return *class;
        }
        let class = classify_scope_name(scope);
        self.scope_cache.insert(scope.to_owned(), class);
        class
    }
}

pub fn classify_scope_stack(stack: &[String]) -> Option<SyntaxClass> {
    for preferred in [SyntaxClass::Tag, SyntaxClass::Attribute] {
        if stack
            .iter()
            .rev()
            .any(|scope| classify_scope_name(scope) == Some(preferred))
        {
            return Some(preferred);
        }
    }

    stack
        .iter()
        .rev()
        .find_map(|scope| classify_scope_name(scope))
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
    fn tag_and_attribute_have_priority() {
        let stack = vec![
            "source.test".to_owned(),
            "string.quoted".to_owned(),
            "entity.name.tag.html".to_owned(),
        ];
        assert_eq!(classify_scope_stack(&stack), Some(SyntaxClass::Tag));
    }
}
