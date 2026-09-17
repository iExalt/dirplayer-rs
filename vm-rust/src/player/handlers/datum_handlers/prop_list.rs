use std::{collections::VecDeque, collections::HashSet};

use log::debug;

use crate::player::symbols::builtin::BuiltInSymbol;
use crate::player::symbols::symbol::Symbol;
use crate::{
    director::lingo::datum::{datum_bool, Datum, PropListPair},
    player::{
        allocator::{DatumAllocator, DatumAllocatorTrait},
        compare::{datum_equals, datum_less_than},
        datum_formatting::format_concrete_datum,
        handlers::types::TypeUtils,
        player_duplicate_datum, DatumRef, DirPlayer, ScriptError,
        symbols::symbol_table::SymbolTable,
    },
};
use crate::player::session::ExecutionContext;

pub struct PropListDatumHandlers {}

pub struct PropListUtils {}

impl PropListUtils {
    fn validate_direct_ref(
        datum_ref: &DatumRef,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<(), ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => return Ok(()),
            _ => allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum_ref}")))?,
        };
        crate::player::compare::validate_direct_symbol_fields(datum, symbols)
    }

    fn find_index_to_add(
        prop_list: &VecDeque<PropListPair>,
        item: (&DatumRef, &DatumRef),
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<i32, ScriptError> {
        Self::validate_direct_ref(item.0, allocator, symbols)?;
        Self::validate_direct_ref(item.1, allocator, symbols)?;
        let mut low = 0;
        let mut high = prop_list.len() as i32;
        let key = allocator.get_datum(item.0);
        crate::player::compare::validate_direct_symbol_fields(key, symbols)?;
        while low < high {
            let mid = (low + high) / 2;
            let left_key_ref = &prop_list.get(mid as usize).unwrap().0;
            Self::validate_direct_ref(left_key_ref, allocator, symbols)?;
            let left_key = allocator.get_datum(left_key_ref);
            crate::player::compare::validate_direct_symbol_fields(left_key, symbols)?;
            if datum_less_than(left_key, key, allocator, symbols)? {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        Ok(low)
    }

    /// `is_sorted` is `Datum::PropList`'s own sorted flag — the one maintained
    /// by `sort`/`addProp`/`setaProp` via `find_index_to_add`.
    ///
    /// It matters because the binary search below is only valid on a sorted
    /// list, and on an UNSORTED one it is pure waste: log2(n) `datum_less_than`
    /// probes plus a `datum_equals_for_lookup` hit check that cannot succeed,
    /// and then the full linear scan runs anyway. Prop lists are unsorted
    /// unless a script sorts them, so that was the normal path for every list
    /// of 8 or more entries. The flag was available at the call sites and
    /// simply being dropped on the way in.
    ///
    /// Passing `false` is never wrong, only slower on a genuinely sorted list —
    /// the linear scan is the authoritative answer either way (and returns the
    /// FIRST match, where the binary search could land on a later duplicate).
    fn get_key_index(
        prop_list: &VecDeque<PropListPair>,
        key: &Datum,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
        is_sorted: bool,
    ) -> Result<i32, ScriptError> {
        crate::player::compare::validate_direct_symbol_fields(key, symbols)?;
        if is_sorted && prop_list.len() >= 8 {
            // Try binary search: find insertion point, then check if the key matches
            let mut low = 0i32;
            let mut high = prop_list.len() as i32;
            while low < high {
                let mid = (low + high) / 2;
                let mid_key_ref = &prop_list[mid as usize].0;
                Self::validate_direct_ref(mid_key_ref, allocator, symbols)?;
                let mid_key = allocator.get_datum(mid_key_ref);
                crate::player::compare::validate_direct_symbol_fields(mid_key, symbols)?;
                if datum_less_than(mid_key, key, allocator, symbols)? {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            // Binary search found insertion point `low`. Check if key at `low` matches.
            if (low as usize) < prop_list.len() {
                let found_key_ref = &prop_list[low as usize].0;
                Self::validate_direct_ref(found_key_ref, allocator, symbols)?;
                let found_key = allocator.get_datum(found_key_ref);
                crate::player::compare::validate_direct_symbol_fields(found_key, symbols)?;
                if Self::datum_equals_for_lookup(found_key, key, allocator, symbols)? {
                    return Ok(low);
                }
            }
            // Binary search may miss due to mixed key types — fall through to linear scan
        }
        // A Symbol key — `obj.foo`, `getaProp(#foo)` — is the overwhelmingly
        // common lookup, and it reduces to comparing interned ids. Take it
        // through an inlined scan rather than the generic one below, which
        // makes an out-of-line `Result`-returning call per entry.
        if let Datum::Symbol(want) = key {
            return Self::symbol_key_index(prop_list, want, allocator, symbols);
        }
        for (i, (k, _)) in prop_list.iter().enumerate() {
            Self::validate_direct_ref(k, allocator, symbols)?;
            let k_datum = allocator.get_datum(k);
            crate::player::compare::validate_direct_symbol_fields(k_datum, symbols)?;
            if Self::datum_equals_for_lookup(k_datum, key, allocator, symbols)? {
                return Ok(i as i32);
            }
        }
        Ok(-1)
    }

    /// `get_key_index`'s linear scan, specialised for a Symbol key.
    ///
    /// Exactly mirrors `datum_equals_for_lookup(entry_key, Datum::Symbol(want))`:
    /// symbol keys compare by interned id, string keys by text, and an integer
    /// key matches a numeric symbol (`#2` finds key `2`). Every other key type
    /// falls through `datum_equals`' symbol arm to `false`.
    fn symbol_key_index(
        prop_list: &VecDeque<PropListPair>,
        want: &Symbol,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<i32, ScriptError> {
        let want_str = symbols.display(want).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        // Hoisted: the integer arm re-parsed the symbol name for every entry.
        let want_as_int = want_str.parse::<i32>().ok();
        for (i, (k, _)) in prop_list.iter().enumerate() {
            Self::validate_direct_ref(k, allocator, symbols)?;
            let hit = match allocator.get_datum(k) {
                Datum::Symbol(got) => {
                    symbols
                        .display(got)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    got == want
                }
                Datum::String(got) => got == want_str,
                Datum::Int(got) => want_as_int == Some(*got),
                _ => false,
            };
            if hit {
                return Ok(i as i32);
            }
        }
        Ok(-1)
    }

    /// Format a prop-list key for an error message WITHOUT a `&DirPlayer`.
    ///
    /// The `formatted_key` these lookups used to take was produced eagerly by
    /// `format_datum` at every call site — a heap allocation and a full format
    /// on every `getaProp`/`setaProp`, for a string that is only ever read on
    /// the not-found error path. Keys are symbols, strings or ints in
    /// practically all real Lingo, none of which need player context; anything
    /// else gets its type name, which is all the message was worth anyway.
    pub fn format_key_for_error(key: &Datum, symbols: &SymbolTable) -> Result<String, ScriptError> {
        match key {
            Datum::Symbol(s) => Ok(format!("#{}", symbols.display(s).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?)),
            Datum::String(s) => Ok(format!("\"{s}\"")),
            Datum::Int(i) => Ok(i.to_string()),
            other => Ok(format!("<{}>", other.type_enum().type_str())),
        }
    }

    fn format_for_debug(datum: &Datum, symbols: &SymbolTable, player: &DirPlayer) -> String {
        match format_concrete_datum(datum, symbols, player) {
            Ok(formatted) => formatted,
            Err(error) => format!("<format error: {error}>"),
        }
    }

    fn format_ref_for_debug(
        datum_ref: &DatumRef,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) -> String {
        match crate::player::datum_formatting::format_datum(datum_ref, symbols, player) {
            Ok(formatted) => formatted,
            Err(error) => format!("<format error: {error}>"),
        }
    }

    pub fn datum_equals_for_lookup(
        left: &Datum,
        right: &Datum,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<bool, ScriptError> {
        let result = match (left, right) {
            // Prop-list mixed String/Symbol keys compare the active table's
            // authoritative display spelling exactly. Symbol/Symbol equality
            // remains interned identity equality in `datum_equals` below.
            (Datum::String(l), Datum::String(r)) => l == r,
            (Datum::String(l), Datum::Symbol(r)) => l == symbols.display(r).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?,
            (Datum::Symbol(l), Datum::String(r)) => symbols.display(l).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)? == r,

            // Handle symbol-to-int comparison (e.g., #2 should match key 2)
            (Datum::Symbol(s), Datum::Int(i)) | (Datum::Int(i), Datum::Symbol(s)) => {
                symbols.display(s).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?.parse::<i32>().ok() == Some(*i)
            }

            _ => datum_equals(left, right, allocator, symbols)?,
        };

        Ok(result)
    }

    pub fn get_prop_or_built_in(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        prop_list: &VecDeque<PropListPair>,
        key: Symbol,
        is_sorted: bool,
    ) -> Result<DatumRef, ScriptError> {
        // ONE pass with the Symbol form. This used to run the whole lookup
        // twice — first with `Datum::String(key.to_string())` (a heap
        // allocation per property read), then again with `Datum::Symbol`. The
        // String pass was pure waste on a symbol-keyed list, which is nearly
        // all of them: `datum_less_than` orders symbols by their name, so a
        // String probe key never matches a Symbol entry in the binary search
        // and every read fell through to the full O(n) linear scan before the
        // Symbol pass even started. `datum_equals_for_lookup` already treats
        // String and Symbol keys as interchangeable (both arms below), so the
        // single Symbol pass finds string-keyed entries too.
        let key_index = Self::get_key_index(
            prop_list,
            &Datum::Symbol(key.to_owned()),
            &player.allocator,
            symbols,
            is_sorted,
        )?;
        if key_index >= 0 {
            return Ok(prop_list[key_index as usize].1.clone());
        }
        // Director: `propList.string` returns the bracketed `[#key: val, …]`
        // representation. Handled here (vs. get_built_in_prop) because
        // formatting needs the full player context for nested datum lookup.
        if symbols
            .lower(&key)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            .eq_ignore_ascii_case("string")
        {
            let datum_clone = Datum::PropList(prop_list.clone(), false);
            let s = crate::player::datum_formatting::format_concrete_datum(&datum_clone, symbols, player)?;
            return Ok(player.alloc_datum(Datum::String(s)));
        }
        // Try built-in properties, but return VOID if not found instead of error
        match Self::get_built_in_prop(prop_list, key, symbols) {
            Ok(datum) => Ok(player.alloc_datum(datum)),
            Err(_) => Ok(DatumRef::Void), // Return VOID for non-existent properties
        }
    }

    pub fn get_built_in_prop(
        prop_list: &VecDeque<PropListPair>,
        prop: Symbol,
        symbols: &SymbolTable,
    ) -> Result<Datum, ScriptError> {
        symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop.into_builtin() {
            Some(BuiltInSymbol::Count) => Ok(Datum::Int(prop_list.len() as i32)),
            Some(BuiltInSymbol::Ilk) => Ok(Datum::Symbol(Symbol::builtin(BuiltInSymbol::PropList))),
            _ => {
                let prop_name = symbols
                    .display(&prop)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                return Err(ScriptError::new(format!(
                    "Invalid prop list built-in property {}",
                    prop_name
                )))
            }
        }
    }

    pub fn get_prop(
        prop_list: &VecDeque<PropListPair>,
        key_ref: &DatumRef,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
        is_required: bool,
        is_sorted: bool,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(key_ref, allocator, symbols)?;
        let key = allocator.get_datum(&key_ref);
        // First try key-based lookup (works for all types including Int)
        let key_index = Self::get_key_index(prop_list, key, allocator, symbols, is_sorted)?;
        if key_index >= 0 {
            return Ok(prop_list[key_index as usize].1.clone());
        }

        // If not found and key is an Int, try as positional index
        if let Datum::Int(position) = key {
            let index = *position - 1;
            if index >= 0 && index < prop_list.len() as i32 {
                return Ok(prop_list[index as usize].1.clone());
            } else {
                return Err(ScriptError::new(format!("Index out of range: {}", index)));
            }
        }
        if is_required {
            return Err(ScriptError::new(format!(
                "Prop not found: {}",
                Self::format_key_for_error(key, symbols)?
            )));
        }
        Ok(DatumRef::Void)
    }

    pub fn set_prop(
        prop_list_ref: &DatumRef,
        key_ref: &DatumRef,
        value_ref: &DatumRef,
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        is_required: bool,
    ) -> Result<(), ScriptError> {
        Self::validate_direct_ref(key_ref, &player.allocator, symbols)?;
        Self::validate_direct_ref(value_ref, &player.allocator, symbols)?;
        let key = player.get_datum(key_ref);
        let (prop_list, is_sorted) = player.get_datum(prop_list_ref).to_map_tuple()?;
        let key_index = Self::get_key_index(&prop_list, key, &player.allocator, symbols, is_sorted)?;
        if is_required && key_index < 0 {
            return Err(ScriptError::new(format!(
                "Prop not found: {}",
                Self::format_key_for_error(key, symbols)?
            )));
        }
        let index_to_add =
            PropListUtils::find_index_to_add(&prop_list, (key_ref, value_ref), &player.allocator, symbols)?;
        let (prop_list, ..) = player.get_datum_mut(prop_list_ref).to_map_tuple_mut()?;
        if key_index >= 0 {
            prop_list[key_index as usize].1 = value_ref.clone();
        } else if is_sorted {
            prop_list.insert(index_to_add as usize, (key_ref.clone(), value_ref.clone()));
        } else {
            prop_list.push_back((key_ref.clone(), value_ref.clone()));
        }
        Ok(())
    }

    pub fn get_at(
        prop_list: &VecDeque<PropListPair>,
        key_ref: &DatumRef,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
        is_sorted: bool,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(key_ref, allocator, symbols)?;
        let key = allocator.get_datum(key_ref);
        match key {
            // TODO do same for float
            Datum::Int(index) => {
                let index = (*index as usize) - 1;
                if index < prop_list.len() {
                    Ok(prop_list[index].1.clone())
                } else {
                    Err(ScriptError::new(format!("Index out of range: {}", index)))
                }
            }
            _ => Self::get_by_key(prop_list, key_ref, allocator, symbols, is_sorted),
        }
    }

    pub fn get_by_key(
        prop_list: &VecDeque<PropListPair>,
        key_ref: &DatumRef,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
        is_sorted: bool,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(key_ref, allocator, symbols)?;
        let key = allocator.get_datum(key_ref);
        Self::get_by_concrete_key(prop_list, key, allocator, symbols, is_sorted)
    }

    /// `is_sorted` must be the owning `Datum::PropList`'s flag. An earlier cut
    /// of this hard-coded `false` here on the assumption that every caller was
    /// a small option list; `getPropRef` and `getAt` also come through here on
    /// large sorted lists, and dropping their binary search measurably
    /// regressed `get_key_index`. Pass the real flag.
    pub fn get_by_concrete_key(
        prop_list: &VecDeque<PropListPair>,
        key: &Datum,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
        is_sorted: bool,
    ) -> Result<DatumRef, ScriptError> {
        let key_index = Self::get_key_index(prop_list, key, allocator, symbols, is_sorted)?;
        if key_index < 0 {
            return Ok(DatumRef::Void);
        }
        Ok(prop_list[key_index as usize].1.clone())
    }

    pub fn set_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        prop_list_ref: &DatumRef,
        key_ref: &DatumRef,
        value_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        Self::validate_direct_ref(key_ref, &player.allocator, symbols)?;
        Self::validate_direct_ref(value_ref, &player.allocator, symbols)?;
        let key = &player.get_datum(key_ref);
        match key {
            // TODO do same for float
            Datum::Int(index) => {
                let index = (*index as usize) - 1;
                let prop_list = player.get_datum_mut(prop_list_ref).to_map_mut()?;
                if index < prop_list.len() {
                    prop_list[index].1 = value_ref.clone();
                } else if index <= prop_list.len() {
                    prop_list.push_back((key_ref.clone(), value_ref.clone()));
                } else {
                    return Err(ScriptError::new(format!("Index out of range: {}", index)));
                }
            }
            _ => Self::set_prop(prop_list_ref, key_ref, value_ref, player, symbols, false)?,
        }
        Ok(())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::director::lingo::datum::DatumType;
    use crate::player::ownership::OwnerToken;
    use crate::player::script::ScriptInstance;
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use async_std::channel;
    use fxhash::FxHashMap;

    fn test_player() -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn lookup_preserves_exact_spelling_first_match_and_numeric_symbols() -> Result<(), ScriptError> {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let alpha = symbols.intern("alpha");
        let numeric = symbols.intern("2");
        let first = player.alloc_datum(Datum::Symbol(alpha.clone()));
        let duplicate = player.alloc_datum(Datum::String("alpha".to_owned()));
        let numeric_key = player.alloc_datum(Datum::Symbol(numeric));
        let first_value = player.alloc_datum(Datum::Int(1));
        let duplicate_value = player.alloc_datum(Datum::Int(2));
        let numeric_value = player.alloc_datum(Datum::Int(3));
        let entries = VecDeque::from([
            (first, first_value.clone()),
            (duplicate, duplicate_value),
            (numeric_key, numeric_value),
        ]);

        assert_eq!(
            PropListUtils::get_key_index(
                &entries,
                &Datum::Symbol(alpha.clone()),
                &player.allocator,
                &symbols,
                false,
            )?,
            0,
        );
        assert_eq!(
            PropListUtils::get_key_index(
                &entries,
                &Datum::String("ALPHA".to_owned()),
                &player.allocator,
                &symbols,
                false,
            )?,
            -1,
        );
        assert_eq!(
            PropListUtils::get_key_index(
                &entries,
                &Datum::Int(2),
                &player.allocator,
                &symbols,
                false,
            )?,
            2,
        );
        Ok(())
    }

    #[test]
    fn builtin_properties_are_available_and_explicit_properties_shadow_them() -> Result<(), ScriptError> {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let count_key = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Count)));
        let explicit_count = player.alloc_datum(Datum::Int(99));
        let entries = VecDeque::from([(count_key, explicit_count.clone())]);

        let shadowed = PropListUtils::get_prop_or_built_in(
            &mut player,
            &symbols,
            &entries,
            Symbol::builtin(BuiltInSymbol::Count),
            false,
        )?;
        assert_eq!(shadowed, explicit_count);

        let empty = VecDeque::new();
        let builtin = PropListUtils::get_prop_or_built_in(
            &mut player,
            &symbols,
            &empty,
            Symbol::builtin(BuiltInSymbol::Count),
            false,
        )?;
        assert!(matches!(player.get_datum(&builtin), Datum::Int(0)));
        Ok(())
    }

    #[test]
    fn debug_ref_format_distinguishes_void_and_foreign() -> Result<(), ScriptError> {
        let player = test_player();
        let symbols = SymbolTable::new();
        assert_eq!(
            PropListUtils::format_ref_for_debug(&DatumRef::Void, &symbols, &player),
            "Void"
        );

        let mut foreign_player = test_player();
        let foreign_ref = foreign_player.alloc_datum(Datum::String("foreign".to_owned()));
        let formatted = PropListUtils::format_ref_for_debug(&foreign_ref, &symbols, &player);
        assert!(formatted.starts_with("<format error: invalid datum reference"));
        Ok(())
    }

    #[test]
    fn foreign_mutation_operand_is_rejected_before_prop_list_changes() -> Result<(), ScriptError> {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let local_key = player.alloc_datum(Datum::String("key".to_owned()));
        let local_value = player.alloc_datum(Datum::Int(1));
        let prop_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(local_key.clone(), local_value.clone())]),
            false,
        ));
        let mut foreign_player = test_player();
        let foreign_key = foreign_player.alloc_datum(Datum::String("foreign-key".to_owned()));
        let foreign_value = foreign_player.alloc_datum(Datum::Int(2));

        assert!(PropListUtils::set_prop(
            &prop_list,
            &local_key,
            &foreign_value,
            &mut player,
            &symbols,
            false,
        )
        .is_err());
        match player.get_datum(&prop_list) {
            Datum::PropList(entries, false) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, local_key);
                assert_eq!(entries[0].1, local_value);
            }
            _ => panic!("prop list changed shape"),
        }
        assert!(PropListUtils::set_prop(
            &prop_list,
            &foreign_key,
            &local_value,
            &mut player,
            &symbols,
            false,
        )
        .is_err());
        match player.get_datum(&prop_list) {
            Datum::PropList(entries, false) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, local_key);
                assert_eq!(entries[0].1, local_value);
            }
            _ => panic!("prop list changed shape"),
        }
        Ok(())
    }

    #[test]
    fn real_context_dispatch_preserves_fallback_prefix_and_foreign_probe_errors() -> Result<(), ScriptError> {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let key = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ToString)));
        let value = player.alloc_datum(Datum::String("value".to_owned()));
        let prop_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key, value.clone())]),
            false,
        ));
        let empty_prop_list = player.alloc_datum(Datum::PropList(VecDeque::new(), false));
        let getter = symbols.intern("getAnything");
        let uppercase_getter = symbols.intern("GetDifferent");
        let mut context = ExecutionContext {
            player_id: 1,
            symbols: &mut symbols,
            player: &mut player,
        };
        let args = Vec::new();
        let result = PropListDatumHandlers::call(&mut context, &prop_list, getter, &args)?;
        assert_eq!(result, value);
        assert!(PropListDatumHandlers::call(
            &mut context,
            &prop_list,
            uppercase_getter,
            &args,
        )
        .is_err());

        let mut foreign_player = test_player();
        let foreign_probe = foreign_player.alloc_datum(Datum::String("probe".to_owned()));
        let probe_args = vec![foreign_probe];
        assert!(PropListDatumHandlers::call(
            &mut context,
            &empty_prop_list,
            Symbol::builtin(BuiltInSymbol::GetaProp),
            &probe_args,
        )
        .is_err());
        let mut foreign_symbols = SymbolTable::new();
        let local_foreign_symbol = context
            .player
            .alloc_datum(Datum::Symbol(foreign_symbols.intern("foreign-probe")));
        let symbol_probe_args = vec![local_foreign_symbol];
        assert!(PropListDatumHandlers::call(
            &mut context,
            &empty_prop_list,
            Symbol::builtin(BuiltInSymbol::GetaProp),
            &symbol_probe_args,
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn sort_is_stable_sets_owned_flag_and_lookup_stops_before_later_foreign_value() -> Result<(), ScriptError> {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let first_key = player.alloc_datum(Datum::String("a".to_owned()));
        let equal_key = player.alloc_datum(Datum::String("A".to_owned()));
        let last_key = player.alloc_datum(Datum::String("b".to_owned()));
        let first_value = player.alloc_datum(Datum::Int(1));
        let equal_value = player.alloc_datum(Datum::Int(2));
        let last_value = player.alloc_datum(Datum::Int(3));
        let prop_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([
                (last_key, last_value),
                (first_key.clone(), first_value.clone()),
                (equal_key, equal_value.clone()),
            ]),
            false,
        ));
        PropListDatumHandlers::sort(&mut player, &symbols, &prop_list, &Vec::new())?;
        let Datum::PropList(entries, is_sorted) = player.get_datum(&prop_list) else {
            panic!("expected prop list")
        };
        assert!(is_sorted);
        assert_eq!(entries[0].1, first_value);
        assert_eq!(entries[1].1, equal_value);

        let mut foreign_player = test_player();
        let foreign_value = foreign_player.alloc_datum(Datum::Symbol(
            SymbolTable::new().intern("foreign-value"),
        ));
        let later_key = player.alloc_datum(Datum::String("later".to_owned()));
        let lookup_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([
                (first_key, first_value),
                (later_key.clone(), foreign_value.clone()),
            ]),
            false,
        ));
        let lookup_args = vec![player.alloc_datum(Datum::String("a".to_owned()))];
        let found = PropListDatumHandlers::get_a_prop(&mut player, &symbols, &lookup_list, &lookup_args)?;
        assert!(matches!(player.get_datum(&found), Datum::Int(1)));
        let get_one_key = player.alloc_datum(Datum::String("first-value".to_owned()));
        let get_one_value = player.alloc_datum(Datum::String("needle".to_owned()));
        let get_one_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(get_one_key.clone(), get_one_value.clone()), (later_key, foreign_value)]),
            false,
        ));
        let get_one_args = vec![get_one_value];
        let found_key = PropListDatumHandlers::get_one(&mut player, &symbols, &get_one_list, &get_one_args)?;
        assert_eq!(found_key, get_one_key);
        let missing_probe = player.alloc_datum(Datum::String("missing".to_owned()));
        assert!(PropListDatumHandlers::get_one(
            &mut player,
            &symbols,
            &get_one_list,
            &vec![missing_probe],
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn duplicate_is_independent_and_rejects_nested_foreign_symbols() -> Result<(), ScriptError> {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let key = player.alloc_datum(Datum::String("nested".to_owned()));
        let original_item = player.alloc_datum(Datum::Int(1));
        let nested = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([original_item.clone()]),
            false,
        ));
        let original = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key, nested.clone())]),
            true,
        ));
        let duplicate = super::player_duplicate_datum(&mut player, &symbols, &original)?;
        let duplicate_nested = match player.get_datum(&duplicate) {
            Datum::PropList(entries, _) => entries[0].1.clone(),
            _ => panic!("expected duplicated prop list"),
        };
        assert_ne!(duplicate_nested, nested);
        let replacement = player.alloc_datum(Datum::Int(9));
        if let Datum::List(_, items, _) = player.get_datum_mut(&duplicate_nested) {
            items[0] = replacement;
        } else {
            panic!("expected duplicated nested list");
        }
        assert!(matches!(player.get_datum(&original_item), Datum::Int(1)));
        let original_nested_item = match player.get_datum(&nested) {
            Datum::List(_, items, _) => items[0].clone(),
            _ => panic!("expected original nested list"),
        };
        assert_eq!(original_nested_item, original_item);

        let mut foreign_symbols = SymbolTable::new();
        let foreign_ref = player.alloc_datum(Datum::Symbol(foreign_symbols.intern("nested-foreign")));
        let nested_foreign = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([foreign_ref]),
            false,
        ));
        let foreign_key = player.alloc_datum(Datum::String("nested".to_owned()));
        let foreign_root = player.alloc_datum(Datum::PropList(
            VecDeque::from([(
                foreign_key,
                nested_foreign,
            )]),
            false,
        ));
        assert!(super::player_duplicate_datum(&mut player, &symbols, &foreign_root).is_err());
        Ok(())
    }

    #[test]
    fn instance_string_sub_prop_uses_table_interning() -> Result<(), ScriptError> {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let property = symbols.intern("DisplayName");
        let property_value = player.alloc_datum(Datum::String("Luna".to_owned()));
        let instance_ref = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 1,
            script: CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: None,
            properties: FxHashMap::from_iter([(property.clone(), property_value.clone())]),
            begin_sprite_called: false,
        });
        let instance = player.alloc_datum(Datum::ScriptInstanceRef(instance_ref));
        let string_key = player.alloc_datum(Datum::String("displayname".to_owned()));
        let result = TypeUtils::get_sub_prop(&instance, &string_key, &mut player, &mut symbols)?;
        assert_eq!(result, property_value);

        let symbol_key = player.alloc_datum(Datum::Symbol(property.clone()));
        let symbol_result = TypeUtils::get_sub_prop(&instance, &symbol_key, &mut player, &mut symbols)?;
        assert_eq!(symbol_result, property_value);

        let numeric_one = player.alloc_datum(Datum::Int(1));
        let numeric_result = TypeUtils::get_sub_prop(&instance, &numeric_one, &mut player, &mut symbols)?;
        assert_eq!(numeric_result, property_value);

        for index in [0, i32::MIN] {
            let index_ref = player.alloc_datum(Datum::Int(index));
            let result = TypeUtils::get_sub_prop(&instance, &index_ref, &mut player, &mut symbols)?;
            assert!(matches!(result, DatumRef::Void));
        }

        let unsupported_key = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::new(),
            false,
        ));
        let unsupported = TypeUtils::get_sub_prop(
            &instance,
            &unsupported_key,
            &mut player,
            &mut symbols,
        )?;
        assert!(matches!(unsupported, DatumRef::Void));
        Ok(())
    }
}

impl PropListDatumHandlers {
    pub fn call(
        runtime: &mut ExecutionContext,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let (player, symbols) = (&mut *runtime.player, &mut *runtime.symbols);
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::GetAt) => Self::get_at(player, symbols, datum, args),
            Some(BuiltInSymbol::SetAt) => Self::set_at(player, symbols, datum, args),
            Some(BuiltInSymbol::Sort) => Self::sort(player, symbols, datum, args),
            Some(BuiltInSymbol::GetPropAt) => Self::get_prop_at(player, symbols, datum, args),
            Some(BuiltInSymbol::AddProp) => Self::add_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::SetaProp) => Self::set_opt_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::SetProp) => {
                if args.len() == 3 {
                    {
                        let prop_key_ref = &args[0];
                        let index_ref = &args[1];
                        let value_ref = &args[2];

                        PropListUtils::validate_direct_ref(prop_key_ref, &player.allocator, symbols)?;
                        PropListUtils::validate_direct_ref(index_ref, &player.allocator, symbols)?;
                        PropListUtils::validate_direct_ref(value_ref, &player.allocator, symbols)?;

                        let (prop_list, is_sorted) =
                            player.get_datum(datum).to_map_tuple()?;
                        let list_ref =
                            PropListUtils::get_by_key(prop_list, prop_key_ref, &player.allocator, symbols, is_sorted)?;

                        let index = player.get_datum(index_ref).int_value()? as usize;
                        let adjusted_index = if index == 0 { 0 } else { index - 1 };

                        let list_datum = player.get_datum(&list_ref);
                        match list_datum {
                            Datum::List(_, list, _) => {
                                // Calculate how many VOID values we need
                                let current_len = list.len();
                                let voids_needed = if adjusted_index >= current_len {
                                    adjusted_index - current_len + 1
                                } else {
                                    0
                                };


                                // Allocate all VOID values first
                                let void_refs: Vec<DatumRef> = (0..voids_needed)
                                    .map(|_| player.alloc_datum(Datum::Void))
                                    .collect();


                                // Now get mutable reference and extend the list
                                let (_, list_vec, _) =
                                    player.get_datum_mut(&list_ref).to_list_mut()?;
                                list_vec.extend(void_refs);


                                // Set the value
                                list_vec[adjusted_index] = value_ref.clone();
                                Ok(DatumRef::Void)
                            }
                            Datum::PropList(..) => {
                                // Property is a PropList. An INTEGER subscript is
                                // POSITIONAL in Director (`setAt`), exactly as it
                                // is for the plain-List arm above — only a
                                // non-integer subscript is a property key. Going
                                // straight to set_prop treated the index as a key
                                // and APPENDED a new integer-keyed pair instead of
                                // updating the slot:
                                //   g.missionCompleted[g.levelID - 1] = 1
                                //     → [.. #CHAR_WILDMUTT: 0, 3: 1, 1: 1]  (count 12)
                                // which then double-counted on read, because
                                // PropListUtils::get_prop tries the key first and
                                // falls back to positional — so each completed
                                // mission scored once via its bogus key and once
                                // via its position (2 missions → a HUD "4 / 10").
                                // set_at does the Int/positional split and still
                                // routes non-integer keys to set_prop.
                                PropListUtils::set_at(player, symbols, &list_ref, index_ref, value_ref)?;
                                Ok(DatumRef::Void)
                            }
                            Datum::Point(..) => {
                                // Point: index 1 = locH (x), index 2 = locV (y)
                                if adjusted_index < 2 {
                                    let new_val = player.get_datum(value_ref).clone();
                                    let (component_val, is_float) = Datum::datum_to_inline_component(&new_val)?;
                                    let (vals, flags) = player.get_datum_mut(&list_ref).to_point_inline_mut()?;
                                    vals[adjusted_index] = component_val;
                                    Datum::inline_set_float(flags, adjusted_index, is_float);
                                    Ok(DatumRef::Void)
                                } else {
                                    Err(ScriptError::new(format!(
                                        "Point index out of bounds: {} (must be 1 or 2)",
                                        index
                                    )))
                                }
                            }
                            Datum::Rect(..) => {
                                // Rect: index 1-4
                                if adjusted_index < 4 {
                                    let new_val = player.get_datum(value_ref).clone();
                                    let (component_val, is_float) = Datum::datum_to_inline_component(&new_val)?;
                                    let (vals, flags) = player.get_datum_mut(&list_ref).to_rect_inline_mut()?;
                                    vals[adjusted_index] = component_val;
                                    Datum::inline_set_float(flags, adjusted_index, is_float);
                                    Ok(DatumRef::Void)
                                } else {
                                    Err(ScriptError::new(format!(
                                        "Rect index out of bounds: {} (must be 1-4)",
                                        index
                                    )))
                                }
                            }
                            _ => {
                                Err(ScriptError::new(format!(
                                    "Property is not a list or propList, it's: {}",
                                    list_datum.type_str()
                                )))
                            }
                        }
                    }
                } else if args.len() == 2 {
                    Self::set_required_prop(player, symbols, datum, args)
                } else {
                    Err(ScriptError::new(format!(
                        "Invalid number of arguments for setProp: {}",
                        args.len()
                    )))
                }
            }
            Some(BuiltInSymbol::GetProp) => Self::get_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::GetaProp) => Self::get_a_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::DeleteProp) => Self::delete_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::DeleteAt) => Self::delete_at(player, symbols, datum, args),
            Some(BuiltInSymbol::DeleteOne) => Self::delete_one(player, symbols, datum, args),
            Some(BuiltInSymbol::GetOne) => Self::get_one(player, symbols, datum, args),
            Some(BuiltInSymbol::FindPos) => Self::find_pos(player, symbols, datum, args),
            Some(BuiltInSymbol::FindPosNear) => Self::find_pos_near(player, symbols, datum, args),
            Some(BuiltInSymbol::GetPos) => Self::get_pos(player, symbols, datum, args),
            Some(BuiltInSymbol::Duplicate) => Self::duplicate(player, symbols, datum, args),
            Some(BuiltInSymbol::GetLast) => Self::get_last(player, symbols, datum, args),
            Some(BuiltInSymbol::Count) => Self::count(player, symbols, datum, args),
            Some(BuiltInSymbol::GetPropRef) => Self::get_prop_ref(player, symbols, datum, args),
            Some(BuiltInSymbol::ToString) => {
                // Support toString() on PropLists that have a #toString property (e.g. Flash Date objects)
                {
                    let prop_list = player.get_datum(datum).to_map()?;
                    for (key_ref, val_ref) in prop_list {
                        let key = player.get_datum(&key_ref);
                        if let Datum::Symbol(s) = key {
                            symbols
                                .display(s)
                                .map_err(|_| ScriptError::from(crate::player::symbols::symbol::SymbolError::Foreign))?;
                            if *s == Symbol::builtin(BuiltInSymbol::ToString) {
                                return Ok(val_ref.clone());
                            }
                        }
                    }
                    // Fallback: return string representation
                    let formatted = crate::player::datum_formatting::format_datum(datum, symbols, player)?;
                    Ok(player.alloc_datum(Datum::String(formatted)))
                }
            }
            _ => {
                // Flash object prop lists from callbacks: handle getter/setter methods
                // by mapping to properties. e.g. getScreenName() -> #screenName,
                // getTypeOf() -> #typeOf, setScreenName(v) -> #screenName = v
                let name_str = symbols
                    .display(&handler_name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                if name_str.starts_with("get") && name_str.len() > 3 {
                    let prop_name = &name_str[3..];
                    let prop_name_camel = {
                        let mut chars = prop_name.chars();
                        match chars.next() {
                            Some(c) => format!("{}{}", c.to_lowercase(), chars.as_str()),
                            None => String::new(),
                        }
                    };
                    let prop_list = player.get_datum(datum).to_map()?.clone();
                    for (key_ref, val_ref) in &prop_list {
                        if let Datum::Symbol(s) = player.get_datum(key_ref) {
                            symbols
                                .display(s)
                                .map_err(|_| ScriptError::from(crate::player::symbols::symbol::SymbolError::Foreign))?;
                            if *s == Symbol::builtin(BuiltInSymbol::ToString) {
                                return Ok(val_ref.clone());
                            }
                        }
                    }
                    return Ok(player.alloc_datum(Datum::Void));
                }
                if name_str.starts_with("set") && name_str.len() > 3 {
                    // Silently ignore setters on prop lists
                    return Ok(DatumRef::Void);
                }
                Err(ScriptError::new(format!(
                    "No handler {} for prop list datum",
                    name_str
                )))
            }
        }
    }

    fn count(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
            let count = if args.is_empty() {
                prop_list.len()
            } else if args.len() == 1 {
                let prop_name = &args[0];
                let prop_value =
                    PropListUtils::get_by_key(prop_list, prop_name, &player.allocator, symbols, is_sorted)?;
                let prop_value = player.get_datum(&prop_value);
                match prop_value {
                    Datum::List(_, list, _) => list.len(),
                    Datum::PropList(list, ..) => list.len(),
                    _ => return Err(ScriptError::new("Cannot get count of non-list".to_string())),
                }
            } else {
                return Err(ScriptError::new(
                    "Invalid number of arguments for count".to_string(),
                ));
            };
            Ok(player.alloc_datum(Datum::Int(count as i32)))


    }

    pub fn get_one(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            PropListUtils::validate_direct_ref(&args[0], &player.allocator, symbols)?;
            let find = player.get_datum(&args[0]);
            let prop_list = player.get_datum(datum);
            let prop_list = match prop_list {
                Datum::PropList(list, ..) => list,
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let mut visited = HashSet::new();
            crate::player::compare::validate_reachable_symbols(
                &args[0], &player.allocator, symbols, &mut visited,
            )?;
            // For prop lists, getOne returns the PROPERTY (key) for a given value.
            let mut found_key = None;
            for (key_ref, value_ref) in prop_list {
                crate::player::compare::validate_reachable_symbols(
                    value_ref, &player.allocator, symbols, &mut visited,
                )?;
                if datum_equals(player.get_datum(value_ref), find, &player.allocator, symbols)
                    .unwrap_or(false)
                {
                    found_key = Some(key_ref.clone());
                    break;
                }
            }

            match found_key {
                Some(key_ref) => Ok(key_ref),
                None => Ok(player.alloc_datum(Datum::Int(0))),
            }


    }

    pub fn find_pos(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            PropListUtils::validate_direct_ref(&args[0], &player.allocator, symbols)?;
            let find = player.get_datum(&args[0]);
            let prop_list = player.get_datum(datum);
            let prop_list = match prop_list {
                Datum::PropList(list, ..) => list,
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let mut visited = HashSet::new();
            crate::player::compare::validate_reachable_symbols(
                &args[0], &player.allocator, symbols, &mut visited,
            )?;
            let mut position = None;
            for (index, (key_ref, _)) in prop_list.iter().enumerate() {
                crate::player::compare::validate_reachable_symbols(
                    key_ref, &player.allocator, symbols, &mut visited,
                )?;
                    // Use the lookup-aware equality so a STRING argument matches a
                    // SYMBOL key (and vice-versa), matching Director: e.g.
                    // `[#dozer: "none"].findPos("dozer")` -> 6, and the doc's
                    // `foodList.findPos("breakfast")` -> 1 for key #breakfast.
                    // Plain datum_equals is type-strict (#dozer != "dozer") and
                    // returned VOID, breaking gSoundControl.adjustVolume.
                if PropListUtils::datum_equals_for_lookup(
                    player.get_datum(key_ref), find, &player.allocator, symbols,
                )
                .unwrap_or(false)
                {
                    position = Some(index as i32);
                    break;
                }
            }
            if let Some(position) = position {
                return Ok(player.alloc_datum(Datum::Int(position as i32 + 1)));
            } else {
                return Ok(DatumRef::Void);
            }


    }

    /// `sortedPropList.findPosNear(property)` — Director 11.5 Scripting
    /// Dictionary, `findPosNear`. For a *sorted* property list, returns the
    /// 1-based position of the entry whose property (key) is the nearest
    /// match to `property`. Unlike `findPos` (which is VOID when the property
    /// is absent), `findPosNear` always reports the position of the closest
    /// key. The doc's "most similar alphanumeric name" maps to the list's
    /// sort order: we locate the key's sorted insertion point (lower bound)
    /// and clamp it into `[1, count]` so the result is always a valid
    /// position. On Run's DriveHuman behavior calls this on `sndGearSpeed`
    /// (integer speed keys -> gear symbols) to pick the gear sound nearest the
    /// current speed.
    pub fn find_pos_near(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            PropListUtils::validate_direct_ref(&args[0], &player.allocator, symbols)?;
            let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
            let count = prop_list.len() as i32;
            if count == 0 {
                return Ok(player.alloc_datum(Datum::Int(0)));
            }
            let result = if is_sorted {
                let pos = PropListUtils::find_index_to_add(
                    prop_list,
                    (&args[0], &args[0]),
                    &player.allocator,
                    symbols,
                )?;
                (pos + 1).clamp(1, count)
            } else {
                // Unsorted list: fall back to an exact key match (mirrors the
                // linear-list `findPosNear` path), returning 0 when absent.
                let find = player.get_datum(&args[0]);
                let mut visited = HashSet::new();
                crate::player::compare::validate_reachable_symbols(
                    &args[0], &player.allocator, symbols, &mut visited,
                )?;
                let mut position = None;
                for (index, (key_ref, _)) in prop_list.iter().enumerate() {
                    crate::player::compare::validate_reachable_symbols(
                        key_ref, &player.allocator, symbols, &mut visited,
                    )?;
                        // Lookup-aware equality so a string arg matches a symbol
                        // key (parity with find_pos above).
                    if PropListUtils::datum_equals_for_lookup(
                        player.get_datum(key_ref), find, &player.allocator, symbols,
                    )
                    .unwrap_or(false)
                    {
                        position = Some(index as i32 + 1);
                        break;
                    }
                }
                position.unwrap_or(0)
            };
            Ok(player.alloc_datum(Datum::Int(result)))


    }

    // Finds position of value
    pub fn get_pos(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            PropListUtils::validate_direct_ref(&args[0], &player.allocator, symbols)?;
            let find = player.get_datum(&args[0]);
            let prop_list = player.get_datum(datum).to_map()?;
            let mut visited = HashSet::new();
            crate::player::compare::validate_reachable_symbols(
                &args[0], &player.allocator, symbols, &mut visited,
            )?;
            let mut position = None;
            for (index, (_, value_ref)) in prop_list.iter().enumerate() {
                crate::player::compare::validate_reachable_symbols(
                    value_ref, &player.allocator, symbols, &mut visited,
                )?;
                if datum_equals(player.get_datum(value_ref), find, &player.allocator, symbols)
                    .unwrap_or(false)
                {
                    position = Some(index as i32);
                    break;
                }
            }
            let position = position.unwrap_or(-1);
            return Ok(player.alloc_datum(Datum::Int(position + 1)));


    }

    pub fn get_last(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, _: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            let prop_list = match prop_list {
                Datum::PropList(list, ..) => list,
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let last = prop_list.back().map(|(_, v)| v).unwrap();
            Ok(last.clone())


    }

    pub fn duplicate(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, _: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Ok(player_duplicate_datum(player, symbols, datum)?)
    }

    pub fn get_a_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            PropListUtils::validate_direct_ref(&args[0], &player.allocator, symbols)?;
            let key = player.get_datum(&args[0]);
            let prop_list = player.get_datum(datum);

            debug!(
                "PropList getaProp: looking for key={} in proplist={}", 
                PropListUtils::format_for_debug(key, symbols, player),
                PropListUtils::format_for_debug(prop_list, symbols, player)
            );

            match prop_list {
                Datum::PropList(entries, is_sorted) => {
                    debug!(
                        "PropList has {} entries", 
                        entries.len()
                    );


                    for (i, (k, v)) in entries.iter().enumerate() {
                        debug!(
                            "   [{}] key={}, value={}", 
                            i,
                            PropListUtils::format_ref_for_debug(k, symbols, player),
                            PropListUtils::format_ref_for_debug(v, symbols, player)
                        );
                    }


                    let key_index =
                        PropListUtils::get_key_index(entries, key, &player.allocator, symbols, *is_sorted)?;
                    if key_index >= 0 {
                        let result = entries[key_index as usize].1.clone();
                        debug!(
                            "Found at index {}: {}", 
                            key_index,
                            PropListUtils::format_ref_for_debug(&result, symbols, player)
                        );
                        Ok(result)
                    } else {
                        debug!("get_a_prop: Key not found, returning void");
                        Ok(DatumRef::Void)
                    }
                }
                _ => Err(ScriptError::new(
                    "Cannot get a prop of non-prop list".to_string(),
                )),
            }


    }

    pub fn get_prop(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let base_prop_ref = {
            let key = player.get_datum(&args[0]);
            if matches!(key, Datum::Void) {
                return Ok(DatumRef::Void);
            }
            let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
            let key_index = PropListUtils::get_key_index(
                prop_list,
                key,
                &player.allocator,
                symbols,
                is_sorted,
            )?;
            if key_index >= 0 {
                prop_list[key_index as usize].1.clone()
            } else {
                let formatted_key = format_concrete_datum(key, symbols, player)?;
                return Err(ScriptError::new(format!("Unknown prop {} in prop list", formatted_key)));
            }
        };

        if args.len() == 1 {
            return Ok(base_prop_ref);
        } else if args.len() == 2 {
            return TypeUtils::get_sub_prop(&base_prop_ref, &args[1], player, symbols);
        } else {
            return Err(ScriptError::new(
                "Invalid number of arguments for getProp".to_string(),
            ));
        }
    }

    pub fn set_opt_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            match prop_list {
                Datum::PropList(..) => {}
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let prop_name_ref = &args[0];
            let value_ref = &args[1];

            PropListUtils::set_prop(datum, &prop_name_ref, &value_ref, player, symbols, false)?;
            Ok(DatumRef::Void)


    }

    pub fn add_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_name_ref = &args[0];
            let value_ref = &args[1];

            PropListUtils::validate_direct_ref(prop_name_ref, &player.allocator, symbols)?;
            PropListUtils::validate_direct_ref(value_ref, &player.allocator, symbols)?;

            let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
            let index_to_add = if is_sorted {
                PropListUtils::find_index_to_add(
                    &prop_list,
                    (&prop_name_ref, &value_ref),
                    &player.allocator,
                    symbols,
                )?
            } else {
                prop_list.len() as i32
            };

            let (prop_list, ..) = player.get_datum_mut(datum).to_map_tuple_mut()?;
            if is_sorted {
                prop_list.insert(
                    index_to_add as usize,
                    (prop_name_ref.clone(), value_ref.clone()),
                );
            } else {
                prop_list.push_back((prop_name_ref.clone(), value_ref.clone()));
            }

            Ok(DatumRef::Void)


    }

    fn set_required_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            match prop_list {
                Datum::PropList(..) => {}
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let prop_name_ref = &args[0];
            let value_ref = &args[1];

            PropListUtils::set_prop(datum, &prop_name_ref, &value_ref, player, symbols, true)?;
            Ok(DatumRef::Void)


    }

    pub fn set_at(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            match prop_list {
                Datum::PropList(..) => {}
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set prop list at non-prop list".to_string(),
                    ))
                }
            };
            let prop_name_ref = &args[0];
            let value_ref = &args[1];

            PropListUtils::set_at(player, symbols, datum, &prop_name_ref, &value_ref)?;
            Ok(DatumRef::Void)


    }

    pub fn get_at(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            let (prop_list, is_sorted) = match prop_list {
                Datum::PropList(prop_list, is_sorted) => (prop_list, *is_sorted),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot get prop list at non-prop list".to_string(),
                    ))
                }
            };
            let prop_name_ref = &args[0];
            PropListUtils::get_at(&prop_list, &prop_name_ref, &player.allocator, symbols, is_sorted)


    }

    pub fn delete_at(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let position = player.get_datum(&args[0]).int_value()?;
        let prop_list = player.get_datum_mut(datum);
        match prop_list {
            Datum::PropList(prop_list, ..) => {
                prop_list.remove((position - 1) as usize);
            }
            _ => {
                return Err(ScriptError::new(
                    "Cannot get prop list at non-prop list".to_string(),
                ))
            }
        }
        Ok(DatumRef::Void)
    }

    pub fn delete_one(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let search_ref = &args[0];
        PropListUtils::validate_direct_ref(search_ref, &player.allocator, symbols)?;
        let mut visited = HashSet::new();
        crate::player::compare::validate_reachable_symbols(
            search_ref,
            &player.allocator,
            symbols,
            &mut visited,
        )?;
        let search_val = player.get_datum(search_ref);
        let prop_list = player.get_datum(datum);
        let prop_list = match prop_list {
            Datum::PropList(list, ..) => list,
            _ => {
                return Err(ScriptError::new(
                    "Cannot deleteOne on non-prop list".to_string(),
                ))
            }
        };
        let mut index = None;
        for (i, (_, value_ref)) in prop_list.iter().enumerate() {
            crate::player::compare::validate_reachable_symbols(
                value_ref,
                &player.allocator,
                symbols,
                &mut visited,
            )?;
            if value_ref == search_ref
                || datum_equals(
                    player.get_datum(value_ref),
                    search_val,
                    &player.allocator,
                    symbols,
                )
                .unwrap_or(false)
            {
                index = Some(i);
                break;
            }
        }

        if let Some(i) = index {
            let prop_list = player.get_datum_mut(datum);
            match prop_list {
                Datum::PropList(list, ..) => {
                    list.remove(i);
                }
                _ => {}
            }
        }
        Ok(DatumRef::Void)
    }

    pub fn get_prop_at(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_list = player.get_datum(datum);
            let prop_list = match prop_list {
                Datum::PropList(prop_list, ..) => prop_list,
                _ => {
                    return Err(ScriptError::new(
                        "Cannot get prop list at non-prop list".to_string(),
                    ))
                }
            };
            let position = player.get_datum(&args[0]).int_value()?;
            Ok(prop_list.get((position - 1) as usize).unwrap().0.clone())


    }

    pub fn sort(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, _: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let sorted_prop_list = {
            let prop_list = player.get_datum(datum).to_map()?;
            let mut visited = HashSet::new();
            for (key_ref, _value_ref) in prop_list {
                crate::player::compare::validate_reachable_symbols(
                    key_ref,
                    &player.allocator,
                    symbols,
                    &mut visited,
                )?;
            }
            let mut sorted_prop_list = player.get_datum(datum).to_map()?.clone();
            sorted_prop_list.make_contiguous().sort_by(|a, b| {
                let (left_key_ref, _) = a;
                let (right_key_ref, _) = b;

                let left = player.get_datum(left_key_ref);
                let right = player.get_datum(right_key_ref);

                if datum_equals(left, right, &player.allocator, symbols).unwrap() {
                    return std::cmp::Ordering::Equal;
                } else if datum_less_than(left, right, &player.allocator, symbols).unwrap() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });
            Ok::<_, ScriptError>(sorted_prop_list)
        }?;

        let (list_vec, is_sorted) = player.get_datum_mut(datum).to_map_tuple_mut()?;
        list_vec.clear();
        list_vec.extend(sorted_prop_list);
        *is_sorted = true;

        Ok(DatumRef::Void)
    }

    pub fn delete_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {


            let prop_name = player.get_datum(&args[0]);


            if prop_name.is_void() {
                return Ok(player.alloc_datum(datum_bool(false)));
            }


            if prop_name.is_string() || prop_name.is_symbol() {
                // Key-based lookup for strings and symbols
                let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
                let index =
                    PropListUtils::get_key_index(prop_list, prop_name, &player.allocator, symbols, is_sorted)?;
                if index >= 0 {
                    let prop_list = player.get_datum_mut(datum).to_map_mut()?;
                    prop_list.remove(index as usize);
                    Ok(player.alloc_datum(datum_bool(true)))
                } else {
                    Ok(player.alloc_datum(datum_bool(false)))
                }
            } else if prop_name.is_int() {
                // For ints: try key lookup first, then fall back to positional
                let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
                let key_index =
                    PropListUtils::get_key_index(prop_list, prop_name, &player.allocator, symbols, is_sorted)?;


                if key_index >= 0 {
                    // Found as key - delete by key
                    let prop_list = player.get_datum_mut(datum).to_map_mut()?;
                    prop_list.remove(key_index as usize);
                    Ok(player.alloc_datum(datum_bool(true)))
                } else {
                    // Not found as key - try positional (1-based)
                    let position = prop_name.int_value()?;
                    let prop_list = player.get_datum_mut(datum).to_map_mut()?;


                    if position >= 1 && position <= prop_list.len() as i32 {
                        prop_list.remove((position - 1) as usize);
                        Ok(player.alloc_datum(datum_bool(true)))
                    } else {
                        Ok(player.alloc_datum(datum_bool(false)))
                    }
                }
            } else {
                // Other types (list/point, etc.) — key-based lookup
                let (prop_list, is_sorted) = player.get_datum(datum).to_map_tuple()?;
                let index =
                    PropListUtils::get_key_index(prop_list, prop_name, &player.allocator, symbols, is_sorted)?;

                if index >= 0 {
                    let prop_list = player.get_datum_mut(datum).to_map_mut()?;
                    prop_list.remove(index as usize);
                    Ok(player.alloc_datum(datum_bool(true)))
                } else {
                    Ok(player.alloc_datum(datum_bool(false)))
                }
            }


    }

    pub fn get_prop_ref(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "getPropRef requires at least one argument".to_string(),
            ));
        }

        let key = args[0].clone();
        let base = player.get_datum(datum);

        let result = match base {
            Datum::PropList(prop_list, _is_sorted) => {
                // Get the property from the prop list
                let prop_value =
                    PropListUtils::get_by_key(&prop_list, &key, &player.allocator, symbols, *_is_sorted)?;

                // If there's a second argument and the property is a list, index into it
                if args.len() >= 2 {
                    let index_ref = &args[1];
                    let prop_datum = player.get_datum(&prop_value);

                    match prop_datum {
                        Datum::List(_, items, _) => {
                            let index = player.get_datum(index_ref).int_value()?;
                            // Support both 0-based and 1-based indexing
                            let actual_index = if index == 0 {
                                0
                            } else if index >= 1 {
                                (index - 1) as usize
                            } else {
                                return Err(ScriptError::new(format!(
                                    "Index out of bounds: {}",
                                    index
                                )));
                            };

                            if actual_index >= items.len() {
                                return Err(ScriptError::new(format!(
                                    "Index out of bounds: {}",
                                    index
                                )));
                            }

                            items[actual_index].clone()
                        }
                        // Nested PropList: an INTEGER subscript is POSITIONAL,
                        // like the plain-List arm above and like the matching
                        // write path (`setProp` → `PropListUtils::set_at`).
                        // Only a non-integer subscript is a property key, which
                        // is what `get_at` splits on.
                        //
                        // Reading it as a key made `p.list[i]` VOID for every
                        // integer i, since the keys are symbols. Merlin's
                        // Revenge expands its animation frame ranges with
                        //   repeat while an <= numanims
                        //     nextan = p.anims[an].sequence
                        //     if ilk(nextan, #list) and nextan.count = 3 then
                        //       ListInterpret(nextan)     -- [2, #til, 9] -> [2..9]
                        // so `nextan` was VOID, `ilk` failed, and no sequence was
                        // ever expanded. The player then animated frame 2, then
                        // `#til` — `member(#til, N)` is not a member, so the
                        // sprite blanked for a frame every walk cycle: the
                        // blinking wizard.
                        Datum::PropList(inner_pairs, inner_sorted) => {
                            PropListUtils::get_at(
                                &inner_pairs,
                                index_ref,
                                &player.allocator,
                                symbols,
                                *inner_sorted,
                            )?
                        }
                        // Point: index 1 = locH (x), index 2 = locV (y)
                        Datum::Point(vals, flags) => {
                            let index = player.get_datum(index_ref).int_value()?;
                            let actual_index = if index >= 1 { (index - 1) as usize } else { 0 };
                            if actual_index < 2 {
                                player.alloc_datum(Datum::inline_component_to_datum(vals[actual_index], Datum::inline_is_float(*flags, actual_index)))
                            } else {
                                return Err(ScriptError::new(format!(
                                    "Point index out of bounds: {}", index
                                )));
                            }
                        }
                        // Rect: index 1-4
                        Datum::Rect(vals, flags) => {
                            let index = player.get_datum(index_ref).int_value()?;
                            let actual_index = if index >= 1 { (index - 1) as usize } else { 0 };
                            if actual_index < 4 {
                                player.alloc_datum(Datum::inline_component_to_datum(vals[actual_index], Datum::inline_is_float(*flags, actual_index)))
                            } else {
                                return Err(ScriptError::new(format!(
                                    "Rect index out of bounds: {}", index
                                )));
                            }
                        }
                        // Sprite: `p.spr[#foo]` is the bracket form of
                        // `sprite(N).foo`, and Director resolves an unknown
                        // sprite property against the properties of the
                        // sprite's behaviours. Merlin's Revenge's shared
                        // movement code does `p.spr[p.spr.pIam].w.runspeed`,
                        // where every character behaviour sets `pIam` to a
                        // symbol naming its own state property.
                        Datum::SpriteRef(sprite_number) => {
                            let sprite_number = *sprite_number;
                            let prop_name = match player.get_datum(index_ref) {
                                Datum::Symbol(name) => {
                                    symbols
                                        .display(name)
                                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                                    name.clone()
                                }
                                Datum::String(name) => symbols.intern(name),
                                other => {
                                    return Err(ScriptError::new(format!(
                                        "Cannot index sprite {} with {}",
                                        sprite_number,
                                        other.type_str()
                                    )))
                                }
                            };
                            let result = crate::player::score::sprite_get_prop(
                                player,
                                symbols,
                                sprite_number,
                                prop_name,
                            )?;
                            // Keep the behaviour's own DatumRef when there is
                            // one, so in-place mutation still lands on the
                            // instance's storage rather than on a clone.
                            player
                                .last_sprite_prop_ref
                                .take()
                                .unwrap_or_else(|| player.alloc_datum(result))
                        }
                        _ => return Err(ScriptError::new(
                            "Second argument to getPropRef requires first property to be a list or propList"
                                .to_string(),
                        )),
                    }
                } else {
                    prop_value
                }
            }
            _ => {
                return Err(ScriptError::new(
                    "getPropRef: datum is not a propList".to_string(),
                ))
            }
        };

        // If there are more keys, recursively resolve
        if args.len() > 2 {
            TypeUtils::get_sub_prop(&result, &args[2], player, symbols)
        } else {
            Ok(result)
        }
    }
}
