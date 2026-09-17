//! Isolated compatibility tests for the production symbol implementation.
//!
//! This crate deliberately has no dependency on `vm-rust`. The modules below
//! are the production files included under the same `crate::player::symbols`
//! namespace they use in the VM. Do not replace these includes with copied
//! implementations or test-only stand-ins.

pub mod player {
    pub mod symbols {
        pub mod builtin {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../vm-rust/src/player/symbols/builtin.rs"
            ));
        }

        pub mod symbol {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../vm-rust/src/player/symbols/symbol.rs"
            ));
        }

        pub mod symbol_table {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../vm-rust/src/player/symbols/symbol_table.rs"
            ));
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use std::hash::{Hash, Hasher};

    use crate::player::symbols::{
        builtin::BuiltInSymbol,
        symbol::Symbol,
        symbol_table::{SymbolOwner, SymbolTable},
    };
    use fxhash::{FxHashMap, FxHasher};

    fn fx_hash(symbol: &Symbol) -> u64 {
        let mut hasher = FxHasher::default();
        symbol.hash(&mut hasher);
        hasher.finish()
    }

    fn observed_map(map: &FxHashMap<Symbol, u16>) -> Vec<(u16, u64)> {
        map.iter()
            .map(|(symbol, value)| (*value, fx_hash(symbol)))
            .collect()
    }

    #[test]
    fn fxhash_iteration_and_hashes_remain_stable_across_table_identities() {
        let same_metadata = SymbolOwner {
            session: 17,
            generation: 4,
        };
        let different_metadata = SymbolOwner {
            session: 18,
            generation: 4,
        };
        let mut first = SymbolTable::with_owner(same_metadata);
        let mut second = SymbolTable::with_owner(same_metadata);
        let mut third = SymbolTable::with_owner(different_metadata);

        let names = ["alpha", "beta", "gamma", "delta", "epsilon"];
        let first_symbols = names
            .iter()
            .map(|name| first.intern(name))
            .collect::<Vec<_>>();
        let second_symbols = names
            .iter()
            .map(|name| second.intern(name))
            .collect::<Vec<_>>();
        let third_symbols = names
            .iter()
            .map(|name| third.intern(name))
            .collect::<Vec<_>>();

        assert_ne!(first_symbols[0], second_symbols[0]);
        assert_ne!(first_symbols[0], third_symbols[0]);
        assert!(first.display(&second_symbols[0]).is_err());
        assert!(second.display(&first_symbols[0]).is_err());
        assert!(third.display(&first_symbols[0]).is_err());

        // FxHash uses the symbol's stable dynamic ID. Distinct owner identity
        // remains part of equality and table validation, so equal hashes do
        // not make foreign symbols interchangeable.
        for ((first_symbol, second_symbol), third_symbol) in first_symbols
            .iter()
            .zip(&second_symbols)
            .zip(&third_symbols)
        {
            assert_eq!(fx_hash(first_symbol), fx_hash(second_symbol));
            assert_eq!(fx_hash(first_symbol), fx_hash(third_symbol));
        }

        let mut first_map = FxHashMap::default();
        for (value, symbol) in first_symbols.iter().enumerate() {
            first_map.insert(symbol.clone(), value as u16);
        }
        let mut second_map = FxHashMap::default();
        for (value, symbol) in second_symbols.iter().enumerate() {
            second_map.insert(symbol.clone(), value as u16);
        }
        let mut third_map = FxHashMap::default();
        for (value, symbol) in third_symbols.iter().enumerate() {
            third_map.insert(symbol.clone(), value as u16);
        }

        assert_eq!(first_map.get(&first_symbols[0]), Some(&0));
        assert_eq!(first_map.get(&second_symbols[0]), None);
        assert_eq!(observed_map(&first_map), observed_map(&second_map));
        assert_eq!(observed_map(&first_map), observed_map(&third_map));
        assert_eq!(observed_map(&first_map).len(), names.len());
    }

    #[test]
    fn builtin_symbols_are_equal_across_tables_while_dynamic_symbols_are_foreign() {
        let mut first = SymbolTable::new();
        let mut second = SymbolTable::new();
        let first_builtin = first.intern("setAt");
        let second_builtin = second.intern("SETAT");
        let first_dynamic = first.intern("dynamicName");
        let second_dynamic = second.intern("dynamicName");

        assert_eq!(first_builtin, Symbol::from(BuiltInSymbol::SetAt));
        assert_eq!(first_builtin, second_builtin);
        assert_ne!(first_dynamic, second_dynamic);
        assert!(first.display(&second_dynamic).is_err());
        assert!(second.display(&first_dynamic).is_err());
    }

    #[test]
    fn lifecycle_dynamic_symbol_rejects_recreated_table_with_same_metadata_and_local_id() {
        let metadata = SymbolOwner {
            session: 29,
            generation: 7,
        };
        let stale = {
            let mut original = SymbolTable::with_owner(metadata);
            original.intern("staleName")
        };

        let mut recreated = SymbolTable::with_owner(metadata);
        let fresh = recreated.intern("staleName");
        assert_eq!(recreated.owner(), metadata);
        assert_ne!(stale, fresh);
        assert_eq!(fx_hash(&stale), fx_hash(&fresh));
        assert!(recreated.display(&stale).is_err());
        assert_eq!(recreated.display(&fresh), Ok("staleName"));
    }

    #[test]
    fn lifecycle_opposite_insertion_order_keeps_local_resolution_and_rejects_foreign_symbols() {
        let mut first = SymbolTable::with_owner(SymbolOwner {
            session: 31,
            generation: 1,
        });
        let mut second = SymbolTable::with_owner(SymbolOwner {
            session: 31,
            generation: 1,
        });
        let first_alpha = first.intern("alpha");
        let first_beta = first.intern("beta");
        let second_beta = second.intern("beta");
        let second_alpha = second.intern("alpha");

        assert_eq!(first.display(&first_alpha), Ok("alpha"));
        assert_eq!(first.display(&first_beta), Ok("beta"));
        assert_eq!(second.display(&second_beta), Ok("beta"));
        assert_eq!(second.display(&second_alpha), Ok("alpha"));
        assert!(first.display(&second_alpha).is_err());
        assert!(first.display(&second_beta).is_err());
        assert!(second.display(&first_alpha).is_err());
        assert!(second.display(&first_beta).is_err());
    }

    #[test]
    fn lifecycle_reset_allows_one_new_authoritative_dynamic_spelling_without_changing_identity() {
        let mut table = SymbolTable::with_owner(SymbolOwner {
            session: 37,
            generation: 2,
        });
        let original = table.intern_authoritative("OriginalSpelling");
        let same_before_reset = table.intern_authoritative("originalspelling");
        assert_eq!(original, same_before_reset);
        assert_eq!(table.display(&original), Ok("OriginalSpelling"));

        table.reset_movie_display_claims();
        let replacement = table.intern_authoritative("ORIGINALSPELLING");
        assert_eq!(original, replacement);
        assert_eq!(table.display(&original), Ok("ORIGINALSPELLING"));

        let ignored = table.intern_authoritative("oRiGiNaLsPeLlInG");
        assert_eq!(original, ignored);
        assert_eq!(table.display(&original), Ok("ORIGINALSPELLING"));
    }
}
