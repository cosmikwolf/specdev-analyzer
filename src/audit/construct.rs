//! Canonical shape for audit-visible source code constructs.

use crate::lang::ParsedParameter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructKind {
    Function,
    Struct,
    Enum,
    ImplBlock,
    Trait,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitMethod {
    pub name: String,
    pub parameters: Vec<ParsedParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeConstruct {
    pub kind: ConstructKind,
    pub name: String,
    pub parameters: Vec<ParsedParameter>,
    pub fields: Vec<String>,
    pub variants: Vec<String>,
    pub methods: Vec<TraitMethod>,
    pub line: usize,
}
