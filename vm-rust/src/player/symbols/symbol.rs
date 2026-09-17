use std::{
    hash::{Hash, Hasher},
    rc::Rc,
};

use crate::player::symbols::{
    builtin::BuiltInSymbol,
    symbol_table::{SymbolOwnerInner, SymbolTable},
};

/// An operation on a symbol failed after resolving it through its owning table.
///
/// Keeping this error in the symbols module avoids coupling the symbol table to
/// the VM's script error type. Callers at the VM boundary can convert it while
/// preserving the resolved spelling in the diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolError {
    /// The symbol was created by a different symbol table.
    Foreign,
    /// The symbol belongs to this table, but is not a builtin.
    NotBuiltin { name: String },
}

impl SymbolError {
    fn not_builtin(name: &str) -> Self {
        Self::NotBuiltin {
            name: name.to_owned(),
        }
    }
}

impl std::fmt::Display for SymbolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Foreign => formatter.write_str("symbol belongs to a different table"),
            Self::NotBuiltin { name } => {
                write!(formatter, "Symbol '{}' is not a built-in symbol", name)
            }
        }
    }
}

impl std::error::Error for SymbolError {}

/// A builtin or a dynamic name tied to one SymbolTable's immutable identity.
/// Dynamic names cannot be formatted or inspected without their owning table.
#[derive(Clone, Debug)]
pub struct Symbol {
    kind: SymbolKind,
}

#[derive(Clone, Debug)]
enum SymbolKind {
    Builtin(BuiltInSymbol),
    Dynamic {
        owner: Rc<SymbolOwnerInner>,
        id: u32,
    },
}

impl Default for Symbol {
    fn default() -> Self {
        Self::empty()
    }
}
impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (SymbolKind::Builtin(a), SymbolKind::Builtin(b)) => a.canonical() == b.canonical(),
            (
                SymbolKind::Dynamic { owner: a, id: ai },
                SymbolKind::Dynamic { owner: b, id: bi },
            ) => *ai == *bi && Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
impl Eq for Symbol {}
impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.kind {
            SymbolKind::Builtin(builtin) => {
                0u8.hash(state);
                builtin.canonical().hash(state);
            }
            SymbolKind::Dynamic { id, .. } => {
                1u8.hash(state);
                id.hash(state);
            }
        }
    }
}
impl PartialEq<BuiltInSymbol> for Symbol {
    fn eq(&self, other: &BuiltInSymbol) -> bool {
        self.eq_builtin(*other)
    }
}
impl PartialEq<Symbol> for BuiltInSymbol {
    fn eq(&self, other: &Symbol) -> bool {
        other.eq_builtin(*self)
    }
}
impl From<BuiltInSymbol> for Symbol {
    fn from(value: BuiltInSymbol) -> Self {
        Self::builtin(value)
    }
}

impl Symbol {
    pub fn builtin(builtin: BuiltInSymbol) -> Self {
        Self {
            kind: SymbolKind::Builtin(builtin.canonical()),
        }
    }
    pub(crate) fn dynamic(owner: Rc<SymbolOwnerInner>, id: u32) -> Self {
        Self {
            kind: SymbolKind::Dynamic { owner, id },
        }
    }
    pub(crate) fn owner_identity(&self) -> Option<&Rc<SymbolOwnerInner>> {
        match &self.kind {
            SymbolKind::Dynamic { owner, .. } => Some(owner),
            _ => None,
        }
    }
    pub(crate) fn dynamic_id(&self) -> Option<u32> {
        match self.kind {
            SymbolKind::Dynamic { id, .. } => Some(id),
            _ => None,
        }
    }
    pub(crate) fn builtin_variant(&self) -> Option<BuiltInSymbol> {
        match self.kind {
            SymbolKind::Builtin(builtin) => Some(builtin.canonical()),
            _ => None,
        }
    }
    pub fn into_builtin(&self) -> Option<BuiltInSymbol> {
        self.builtin_variant()
    }
    pub fn into_builtin_or_error(&self, table: &SymbolTable) -> Result<BuiltInSymbol, SymbolError> {
        if let Some(builtin) = self.into_builtin() {
            return Ok(builtin);
        }
        let name = table.display(self).map_err(|_| SymbolError::Foreign)?;
        Err(SymbolError::not_builtin(name))
    }
    pub fn eq_builtin(&self, builtin: BuiltInSymbol) -> bool {
        self.into_builtin() == Some(builtin.canonical())
    }
    pub fn empty() -> Self {
        Self::builtin(BuiltInSymbol::EmptyString)
    }
    pub fn is_empty(&self) -> bool {
        self.eq_builtin(BuiltInSymbol::EmptyString)
    }
    pub(crate) fn as_str_in<'a>(&self, table: &'a SymbolTable) -> &'a str {
        table
            .display(self)
            .expect("symbol belongs to a different table")
    }
    pub(crate) fn as_lower_str_in<'a>(&self, table: &'a SymbolTable) -> &'a str {
        table
            .lower(self)
            .expect("symbol belongs to a different table")
    }
    pub(crate) fn eq_ignore_ascii_case_in(&self, table: &SymbolTable, other: &str) -> bool {
        self.as_lower_str_in(table).eq_ignore_ascii_case(other)
    }
    pub(crate) fn to_lowercase_in(&self, table: &SymbolTable) -> String {
        self.as_lower_str_in(table).to_owned()
    }
    pub(crate) fn to_ascii_lowercase_in(&self, table: &SymbolTable) -> String {
        self.to_lowercase_in(table)
    }
    pub(crate) fn splitn_in<'a>(
        &self,
        table: &'a SymbolTable,
        n: usize,
        pat: char,
    ) -> std::str::SplitN<'a, char> {
        self.as_str_in(table).splitn(n, pat)
    }
    pub(crate) fn starts_with_in(&self, table: &SymbolTable, pat: &str) -> bool {
        self.as_lower_str_in(table)
            .starts_with(&pat.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::symbols::symbol_table::SymbolTable;

    #[test]
    fn builtin_conversion_reports_local_display_spelling() {
        let mut table = SymbolTable::new();
        let symbol = table.intern_authoritative("MovieSpecificName");
        let error = symbol.into_builtin_or_error(&table).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Symbol 'MovieSpecificName' is not a built-in symbol"
        );
    }

    #[test]
    fn builtin_conversion_rejects_foreign_symbol_without_panicking() {
        let mut owner = SymbolTable::new();
        let foreign_table = SymbolTable::new();
        let symbol = owner.intern("foreignName");
        assert_eq!(
            symbol.into_builtin_or_error(&foreign_table),
            Err(SymbolError::Foreign)
        );
    }
}

#[macro_export]
macro_rules! symbol_match {
    ($sym:expr, { $( $pat:pat => $body:expr ),+, _ => $default:expr $(,)? }) => {
        match ($sym).into_builtin() { $( Some($pat) => $body, )* _ => $default, }
    };
}
