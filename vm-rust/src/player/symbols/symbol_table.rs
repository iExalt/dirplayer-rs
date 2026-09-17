use std::{collections::HashMap, rc::Rc};

use crate::player::symbols::{builtin::{BuiltInSymbol, BUILTIN_SPECS}, symbol::Symbol};

/// Diagnostic metadata only. Rc identity is the actual authority.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SymbolOwner {
    pub session: u64,
    pub generation: u64,
}

#[derive(Debug)]
pub(crate) struct SymbolOwnerInner {
    pub(crate) key: SymbolOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignSymbol;

pub struct SymbolTable {
    owner: Rc<SymbolOwnerInner>,
    dynamic_by_lower: HashMap<String, u32>,
    dynamic_lower: Vec<String>,
    dynamic_display: Vec<String>,
    movie_claimed_dynamic: Vec<bool>,
    builtin_by_lower: HashMap<String, BuiltInSymbol>,
    builtin_display: HashMap<BuiltInSymbol, String>,
    builtin_lower: HashMap<BuiltInSymbol, String>,
    builtin_display_baseline: HashMap<BuiltInSymbol, String>,
    movie_claimed_builtins: HashMap<BuiltInSymbol, bool>,
}

impl SymbolTable {
    /// A fresh Rc owner is created on every call, even for equal metadata.
    pub fn new() -> Self { Self::with_owner(SymbolOwner::default()) }

    pub fn with_owner(key: SymbolOwner) -> Self {
        let owner = Rc::new(SymbolOwnerInner { key });
        let mut builtin_by_lower = HashMap::new();
        let mut builtin_display = HashMap::new();
        for (spelling, builtin) in BUILTIN_SPECS.iter().copied() {
            builtin_by_lower.insert(spelling.to_lowercase(), builtin.canonical());
            builtin_display.entry(builtin.canonical()).or_insert_with(|| spelling.to_owned());
        }
        let builtin_lower = builtin_display.iter().map(|(b, s)| (*b, s.to_ascii_lowercase())).collect();
        let builtin_display_baseline = builtin_display.clone();
        Self {
            owner,
            dynamic_by_lower: HashMap::new(),
            dynamic_lower: Vec::new(),
            dynamic_display: Vec::new(),
            movie_claimed_dynamic: Vec::new(),
            builtin_by_lower,
            builtin_display,
            builtin_lower,
            builtin_display_baseline,
            movie_claimed_builtins: HashMap::new(),
        }
    }

    pub fn owner(&self) -> SymbolOwner { self.owner.key }

    pub(crate) fn owns(&self, symbol: &Symbol) -> bool {
        symbol.owner_identity().map_or(true, |owner| Rc::ptr_eq(owner, &self.owner))
    }

    pub fn intern(&mut self, string: &str) -> Symbol {
        let folded = string.to_lowercase();
        if let Some(builtin) = self.builtin_by_lower.get(&folded).copied() {
            return Symbol::builtin(builtin);
        }
        if let Some(id) = self.dynamic_by_lower.get(&folded).copied() {
            return Symbol::dynamic(self.owner.clone(), id);
        }
        let id = self.dynamic_lower.len() as u32;
        self.dynamic_by_lower.insert(folded.clone(), id);
        self.dynamic_lower.push(folded);
        self.dynamic_display.push(string.to_owned());
        self.movie_claimed_dynamic.push(false);
        Symbol::dynamic(self.owner.clone(), id)
    }

    /// Intern and claim the first display spelling from a movie name table.
    pub fn intern_authoritative(&mut self, string: &str) -> Symbol {
        let folded = string.to_lowercase();
        if let Some(builtin) = self.builtin_by_lower.get(&folded).copied() {
            let entry = self.movie_claimed_builtins.entry(builtin).or_insert(false);
            if !*entry {
                self.builtin_display.insert(builtin, string.to_owned());
                *entry = true;
            }
            return Symbol::builtin(builtin);
        }
        let symbol = self.intern(string);
        let id = symbol.dynamic_id().expect("intern returned dynamic symbol");
        if !self.movie_claimed_dynamic[id as usize] {
            self.dynamic_display[id as usize] = string.to_owned();
            self.movie_claimed_dynamic[id as usize] = true;
        }
        symbol
    }

    pub fn display<'a>(&'a self, symbol: &Symbol) -> Result<&'a str, ForeignSymbol> {
        match symbol.builtin_variant() {
            Some(builtin) => self.builtin_display.get(&builtin).map(String::as_str).ok_or(ForeignSymbol),
            None => {
                let id = symbol.dynamic_id().ok_or(ForeignSymbol)? as usize;
                if !self.owns(symbol) { return Err(ForeignSymbol); }
                self.dynamic_display.get(id).map(String::as_str).ok_or(ForeignSymbol)
            }
        }
    }

    pub fn lower<'a>(&'a self, symbol: &Symbol) -> Result<&'a str, ForeignSymbol> {
        match symbol.builtin_variant() {
            Some(builtin) => self.builtin_lower.get(&builtin).map(String::as_str).ok_or(ForeignSymbol),
            None => {
                let id = symbol.dynamic_id().ok_or(ForeignSymbol)? as usize;
                if !self.owns(symbol) { return Err(ForeignSymbol); }
                self.dynamic_lower.get(id).map(String::as_str).ok_or(ForeignSymbol)
            }
        }
    }

    pub fn reset_movie_display_claims(&mut self) {
        self.movie_claimed_dynamic.fill(false);
        self.movie_claimed_builtins.clear();
        self.builtin_display.clone_from(&self.builtin_display_baseline);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_alias_keeps_stable_ids_and_first_display() {
        let mut table = SymbolTable::with_owner(SymbolOwner { session: 7, generation: 1 });
        let short = table.intern("editShortCutsEnabled");
        let canonical = table.intern("EDITSHORTCUTSENABLED");
        assert_eq!(short, canonical);
        assert_eq!(short.into_builtin(), Some(BuiltInSymbol::EditShortcutsEnabled));
        assert_eq!(table.display(&short), Ok("editShortCutsEnabled"));
    }

    #[test]
    fn authoritative_claim_is_first_writer_and_reset_restores_baseline() {
        let mut table = SymbolTable::new();
        let dynamic = table.intern_authoritative("MiXeDName");
        table.intern_authoritative("mixedname");
        assert_eq!(table.display(&dynamic), Ok("MiXeDName"));
        table.reset_movie_display_claims();
        assert_eq!(table.display(&dynamic), Ok("MiXeDName"));
        let builtin = table.intern_authoritative("NODENAME");
        table.intern_authoritative("nodeName");
        assert_eq!(table.display(&builtin), Ok("NODENAME"));
        table.reset_movie_display_claims();
        assert_eq!(table.display(&builtin), Ok("nodeName"));
    }

    #[test]
    fn equal_numeric_owner_keys_do_not_cross_validate() {
        let key = SymbolOwner { session: 11, generation: 3 };
        let mut first = SymbolTable::with_owner(key);
        let mut second = SymbolTable::with_owner(key);
        let a = first.intern("firstOnly");
        let b = second.intern("firstOnly");
        assert_ne!(a, b);
        assert!(second.display(&a).is_err());
        assert!(first.display(&b).is_err());
    }

    #[test]
    fn interleaved_tables_keep_independent_ids_and_text() {
        let mut first = SymbolTable::new();
        let mut second = SymbolTable::new();
        let a = first.intern("one");
        let b = second.intern("two");
        let a2 = first.intern("ONE");
        assert_eq!(a, a2);
        assert_eq!(first.display(&a), Ok("one"));
        assert_eq!(second.display(&b), Ok("two"));
        assert!(first.display(&b).is_err());
    }

    #[test]
    fn parallel_tables_are_independent_without_global_state() {
        let first = std::thread::spawn(|| {
            let mut table = SymbolTable::with_owner(SymbolOwner { session: 1, generation: 0 });
            let symbol = table.intern_authoritative("First");
            table.display(&symbol).unwrap().to_owned()
        });
        let second = std::thread::spawn(|| {
            let mut table = SymbolTable::with_owner(SymbolOwner { session: 2, generation: 0 });
            let symbol = table.intern_authoritative("Second");
            table.display(&symbol).unwrap().to_owned()
        });
        assert_eq!(first.join().unwrap(), "First");
        assert_eq!(second.join().unwrap(), "Second");
    }
}

#[cfg(test)]
mod hash_tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Barrier};

    #[test]
    fn foreign_symbols_have_stable_hashes_but_are_not_equal() {
        let mut a = SymbolTable::new();
        let mut b = SymbolTable::new();
        let first = a.intern("same");
        let second = b.intern("same");
        let mut ha = DefaultHasher::new(); first.hash(&mut ha);
        let mut hb = DefaultHasher::new(); second.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
        assert_ne!(first, second);
        assert!(a.lower(&second).is_err());
        assert!(b.lower(&first).is_err());
    }

    #[test]
    fn parallel_owner_work_overlaps_without_shared_state() {
        let barrier = Arc::new(Barrier::new(2));
        let left_barrier = barrier.clone();
        let left = std::thread::spawn(move || {
            let mut table = SymbolTable::with_owner(SymbolOwner { session: 101, generation: 0 });
            let symbol = table.intern_authoritative("Left");
            left_barrier.wait();
            table.display(&symbol).unwrap().to_owned()
        });
        let right_barrier = barrier;
        let right = std::thread::spawn(move || {
            let mut table = SymbolTable::with_owner(SymbolOwner { session: 202, generation: 0 });
            let symbol = table.intern_authoritative("Right");
            right_barrier.wait();
            table.display(&symbol).unwrap().to_owned()
        });
        assert_eq!(left.join().unwrap(), "Left");
        assert_eq!(right.join().unwrap(), "Right");
    }
}
