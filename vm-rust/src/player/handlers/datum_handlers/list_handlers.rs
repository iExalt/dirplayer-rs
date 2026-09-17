use std::collections::{HashSet, VecDeque};

use log::debug;

use crate::player::datum_formatting::format_concrete_datum;
use crate::player::symbols::builtin::BuiltInSymbol;
use crate::player::symbols::symbol::Symbol;
use crate::player::symbols::symbol_table::SymbolTable;
use crate::{
    director::lingo::datum::{datum_bool, Datum},
    player::{
        allocator::DatumAllocator,
        compare::{
            datum_equals, datum_less_than, validate_direct_symbol_fields,
            validate_reachable_symbols,
        },
        handlers::types::TypeUtils,
        player_duplicate_datum,
        session::ExecutionContext,
        DatumRef, DirPlayer, ScriptError,
    },
};

pub struct ListDatumHandlers {}
pub struct ListDatumUtils {}

impl ListDatumUtils {
    fn find_index_to_add(
        list_vec: &VecDeque<DatumRef>,
        item: &DatumRef,
        allocator: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<i32, ScriptError> {
        let mut low = 0;
        let mut high = list_vec.len() as i32;
        let item = match item {
            DatumRef::Void => &Datum::Void,
            _ => allocator
                .try_get_datum(item)
                .ok_or_else(|| ScriptError::new("invalid datum reference".to_string()))?,
        };
        validate_direct_symbol_fields(item, symbols)?;

        while low < high {
            let mid = (low + high) / 2;
            let left_ref = list_vec.get(mid as usize).unwrap();
            let left = match left_ref {
                DatumRef::Void => &Datum::Void,
                _ => allocator
                    .try_get_datum(left_ref)
                    .ok_or_else(|| ScriptError::new("invalid datum reference".to_string()))?,
            };
            validate_direct_symbol_fields(left, symbols)?;
            if datum_less_than(left, item, allocator, symbols)? {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        Ok(low)
    }

    pub fn get_prop(
        list_vec: &VecDeque<DatumRef>,
        prop_name: Symbol,
        _datums: &DatumAllocator,
        symbols: &SymbolTable,
    ) -> Result<Datum, ScriptError> {
        symbols
            .display(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        // `into_builtin()` rather than `into_builtin_or_error()`: a symbol that
        // is not a builtin is simply a property this list does not have, which
        // is the VOID case below, not an error.
        match prop_name.into_builtin() {
            Some(BuiltInSymbol::Count) => Ok(Datum::Int(list_vec.len() as i32)),
            Some(BuiltInSymbol::Length) => Ok(Datum::Int(list_vec.len() as i32)),
            Some(BuiltInSymbol::Ilk) => Ok(Datum::Symbol(Symbol::builtin(BuiltInSymbol::List))),
            // `propertyList.propertyName` is a documented spelling of getaProp,
            // and "the getaProp command returns VOID when the specified value is
            // not in the list" — only BRACKET access raises: "Unlike the
            // getAProp command where VOID is returned when a property doesn't
            // exist, a script error will occur if the property doesn't exist
            // when using bracket access" (Director 11.5 Scripting Dictionary,
            // `getaProp`). Erroring here had it backwards.
            //
            // A LINEAR list reaching this arm is the same question asked of a
            // list that simply has no such property, so it answers the same way.
            // Argent Free Ride's Track Builder leans on it directly: when its
            // track XML yields no attributes, `pAttributes.skydome` is expected
            // to come back VOID, and the very next lines guard with
            // `voidp(pAttributes.findPos(#terrain_sx))`.
            _ => Ok(Datum::Void),
        }
    }
}

impl ListDatumHandlers {
    /// Validate a reference and only the symbol-bearing fields of its outer
    /// datum. Mutators use this before taking a mutable list borrow; nested
    /// values remain opaque unless the operation itself compares them.
    fn validate_direct_ref(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => return Ok(()),
            _ => player
                .allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum_ref}")))?,
        };
        validate_direct_symbol_fields(datum, symbols)
    }

    fn validate_search_ref(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
        visited: &mut HashSet<usize>,
    ) -> Result<(), ScriptError> {
        validate_reachable_symbols(datum_ref, &player.allocator, symbols, visited)
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
        prop_name: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        // Director: every datum has a `string` property returning its
        // textual representation. For lists this is the bracketed literal
        // form `[item1, item2, item3]` (with strings quoted, symbols `#sym`,
        // nested lists recursed). Used by Habbo / spineworld idioms like
        // `appearanceToString` (MovieScript 2 global events line 299) that
        // do `inList.string` then strip `[`, `]`, spaces to serialise an
        // appearance list into a flat custom-delimited string.
        Self::validate_direct_ref(player, symbols, datum_ref)?;
        let prop_name_text = symbols
            .display(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        if prop_name_text.eq_ignore_ascii_case("string") {
            let datum_clone = player.get_datum(datum_ref).clone();
            let s = format_concrete_datum(&datum_clone, symbols, player)?;
            return Ok(player.alloc_datum(Datum::String(s)));
        }
        let list_vec = player.get_datum(datum_ref).to_list()?;
        let result = ListDatumUtils::get_prop(&list_vec, prop_name, &player.allocator, symbols)?;
        Ok(player.alloc_datum(result))
    }

    pub fn get_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        Self::validate_direct_ref(player, symbols, &args[0])?;
        {
            let (list_type, list_vec, _) = player.get_datum(datum).to_list_tuple()?;
            // A VOID index (e.g. `list[uninitializedGlobal]`) yields VOID in Director
            // rather than raising — real movies rely on this (leo3d's checkbonbon does
            // `SetObjectVisible(pbonbon[weg], 0)` with `weg` VOID before any is collected).
            if matches!(player.get_datum(&args[0]), Datum::Void) {
                return Ok(DatumRef::Void);
            }
            let index_value = player.get_datum(&args[0]).int_value()?;

            // Handle different indexing schemes based on list type
            let position = match list_type {
                crate::director::lingo::datum::DatumType::XmlChildNodes => {
                    // XML childNodes use 0-based indexing (like JavaScript/DOM)
                    index_value
                }
                _ => {
                    // Regular Lingo lists use 1-based indexing
                    index_value - 1
                }
            };

            if position < 0 || position >= list_vec.len() as i32 {
                return Err(ScriptError::new(format!(
                    "List index {} out of bounds (list has {} items)",
                    position + 1,
                    list_vec.len()
                )));
            }

            let result = list_vec[position as usize].clone();
            Ok(result)
        }
    }

    pub fn set_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        {
            let position_datum = match &args[0] {
                DatumRef::Void => &Datum::Void,
                _ => player
                    .allocator
                    .try_get_datum(&args[0])
                    .ok_or_else(|| ScriptError::new("invalid datum reference".to_string()))?,
            };
            let position = position_datum.int_value()?;
            Self::validate_direct_ref(player, symbols, datum)?;
            Self::validate_direct_ref(player, symbols, &args[0])?;
            let index = position - 1;
            // Receiver validation precedes the non-positive no-op, preserving
            // the list type error while skipping the unused value.
            player.get_datum(datum).to_list_tuple()?;
            // Non-positive positions: `index < list_vec.len()` is true for a
            // negative index, so without this guard `list_vec[(-1) as usize]`
            // indexes with a huge value and VecDeque panics (crashing the whole
            // VM). Director's lists are 1-based and it tolerates setAt at
            // position <= 0 as a no-op rather than erroring — spectral-wizard's
            // generic `customHyperlink` behavior relies on this with
            // `pVisited[pHyperLinks.count] = 0` when a member has no links
            // (count 0 → `pVisited[0] = 0`). Silently ignore.
            if index < 0 {
                return Ok(DatumRef::Void);
            }

            Self::validate_direct_ref(player, symbols, &args[1])?;
            let (_, list_vec, ..) = player.get_datum_mut(datum).to_list_mut()?;
            let item_ref = &args[1];

            if index < list_vec.len() as i32 {
                list_vec[index as usize] = item_ref.clone();
            } else {
                let padding_size = index - list_vec.len() as i32;
                for _ in 0..padding_size {
                    // TODO: should this be filled with zeroes instead?
                    list_vec.push_back(DatumRef::Void);
                }
                list_vec.push_back(item_ref.clone());
            }
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            Ok(DatumRef::Void)
        }
    }

    pub fn call(
        runtime: &mut ExecutionContext,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let (player, symbols) = (&mut *runtime.player, &mut *runtime.symbols);
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::Count) => Self::count(player, symbols, datum, args),
            Some(BuiltInSymbol::GetAt) => Self::get_at(player, symbols, datum, args),
            Some(BuiltInSymbol::SetAt) => Self::set_at(player, symbols, datum, args),
            Some(BuiltInSymbol::Sort) => Self::sort(player, symbols, datum, args),
            Some(BuiltInSymbol::GetOne) => Self::get_one(player, symbols, datum, args),
            Some(BuiltInSymbol::Add) => Self::add(player, symbols, datum, args),
            Some(BuiltInSymbol::Duplicate) => Self::duplicate(player, symbols, datum, args),
            Some(BuiltInSymbol::AddAt) => Self::add_at(player, symbols, datum, args),
            Some(BuiltInSymbol::GetLast) => Self::get_last(player, symbols, datum, args),
            Some(BuiltInSymbol::Append | BuiltInSymbol::Push) => {
                Self::append(player, symbols, datum, args)
            }
            Some(BuiltInSymbol::DeleteOne) => Self::delete_one(player, symbols, datum, args),
            Some(BuiltInSymbol::DeleteAt) => Self::delete_at(player, symbols, datum, args),
            Some(BuiltInSymbol::DeleteAll) => Self::delete_all(player, symbols, datum, args),
            Some(BuiltInSymbol::FindPos) => Self::find_pos(player, symbols, datum, args),
            Some(BuiltInSymbol::GetPos) => Self::get_one(player, symbols, datum, args),
            Some(BuiltInSymbol::FindPosNear) => Self::find_pos_near(player, symbols, datum, args),
            //"getPos" => Self::find_pos(datum, args), TODO: Check which getPos is correct
            Some(BuiltInSymbol::Join) => Self::join(player, symbols, datum, args),
            Some(BuiltInSymbol::GetPropRef | BuiltInSymbol::GetProp) => {
                Self::get_prop_ref(player, symbols, datum, args)
            }
            Some(BuiltInSymbol::ToString) => {
                let s = crate::player::datum_formatting::format_datum(datum, symbols, player)?;
                Ok(player.alloc_datum(Datum::String(s)))
            }
            Some(BuiltInSymbol::GetTypeOf) => {
                // Flash objects have getTypeOf() for error checking.
                // A list is never an error type, so return empty string.
                Ok(player.alloc_datum(Datum::String("".to_string())))
            }
            Some(BuiltInSymbol::Max) => Self::max(player, symbols, datum, args),
            Some(BuiltInSymbol::Min) => Self::min(player, symbols, datum, args),
            // Director matrix methods (chapter 15): when a list-of-lists is
            // used as a matrix (the layout `newMatrix` produces), `setVal`
            // and `getVal` access cells by 1-based (row, col) indices.
            // Director's API is `matrix.setVal(row, col, value)` — extra
            // trailing arguments are silently ignored (some scripts pass a
            // 4th arg that's a leftover from copy-paste; e.g. the Batman
            // terrain script's `myMatrix.setVal(b+1, a+1, h, y)` — we drop the
            // `y` to match Director's tolerant behaviour).
            Some(BuiltInSymbol::SetVal) => Self::set_val(player, symbols, datum, args),
            Some(BuiltInSymbol::GetVal) => Self::get_val(player, symbols, datum, args),
            _ => {
                let handler_text = symbols
                    .display(&handler_name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                Err(ScriptError::new(format!(
                    "No handler {handler_text} for list datum"
                )))
            }
        }
    }

    /// `matrix.setVal(row, col, value [, ignored…])` — 1-based, row-major.
    pub fn set_val(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        if args.len() >= 3 {
            Self::validate_direct_ref(player, symbols, &args[0])?;
            Self::validate_direct_ref(player, symbols, &args[1])?;
            Self::validate_direct_ref(player, symbols, &args[2])?;
        }
        {
            if args.len() < 3 {
                return Err(ScriptError::new(
                    "setVal requires (row, col, value)".to_string(),
                ));
            }
            let row = player.get_datum(&args[0]).int_value()? - 1;
            let col = player.get_datum(&args[1]).int_value()? - 1;
            let value_ref = args[2].clone();
            if row < 0 || col < 0 {
                return Err(ScriptError::new(format!(
                    "setVal: row/col must be 1-based positive (got {}, {})",
                    row + 1,
                    col + 1
                )));
            }
            // Look up the row reference.
            let row_ref = {
                let (_, list_vec, _) = player.get_datum(datum).to_list_tuple()?;
                if (row as usize) >= list_vec.len() {
                    return Err(ScriptError::new(format!(
                        "setVal: row {} out of range (matrix has {} rows)",
                        row + 1,
                        list_vec.len()
                    )));
                }
                list_vec[row as usize].clone()
            };
            Self::validate_direct_ref(player, symbols, &row_ref)?;
            // Mutate the inner row.
            let (_, row_vec, _) = player.get_datum_mut(&row_ref).to_list_mut()?;
            if (col as usize) >= row_vec.len() {
                return Err(ScriptError::new(format!(
                    "setVal: col {} out of range (row has {} cols)",
                    col + 1,
                    row_vec.len()
                )));
            }
            row_vec[col as usize] = value_ref;
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            Ok(DatumRef::Void)
        }
    }

    /// `matrix.getVal(row, col)` — 1-based, row-major.
    pub fn get_val(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        if args.len() >= 2 {
            Self::validate_direct_ref(player, symbols, &args[0])?;
            Self::validate_direct_ref(player, symbols, &args[1])?;
        }
        {
            if args.len() < 2 {
                return Err(ScriptError::new("getVal requires (row, col)".to_string()));
            }
            let row = player.get_datum(&args[0]).int_value()? - 1;
            let col = player.get_datum(&args[1]).int_value()? - 1;
            let (_, list_vec, _) = player.get_datum(datum).to_list_tuple()?;
            if row < 0 || (row as usize) >= list_vec.len() {
                return Err(ScriptError::new(format!(
                    "getVal: row {} out of range (matrix has {} rows)",
                    row + 1,
                    list_vec.len()
                )));
            }
            let row_ref = list_vec[row as usize].clone();
            Self::validate_direct_ref(player, symbols, &row_ref)?;
            let (_, row_vec, _) = player.get_datum(&row_ref).to_list_tuple()?;
            if col < 0 || (col as usize) >= row_vec.len() {
                return Err(ScriptError::new(format!(
                    "getVal: col {} out of range (row has {} cols)",
                    col + 1,
                    row_vec.len()
                )));
            }
            Ok(row_vec[col as usize].clone())
        }
    }

    pub fn max(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        {
            let list_vec = player.get_datum(datum).to_list()?;
            if list_vec.is_empty() {
                return Ok(DatumRef::Void);
            }
            let mut max_item = list_vec[0].clone();
            Self::validate_direct_ref(player, symbols, &max_item)?;
            for item_ref in list_vec.iter().skip(1) {
                Self::validate_direct_ref(player, symbols, item_ref)?;
                let item = player.get_datum(item_ref);
                let current_max = player.get_datum(&max_item);
                if datum_less_than(current_max, item, &player.allocator, symbols)? {
                    max_item = item_ref.clone();
                }
            }
            Ok(max_item)
        }
    }

    pub fn min(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        {
            let list_vec = player.get_datum(datum).to_list()?;
            if list_vec.is_empty() {
                return Ok(DatumRef::Void);
            }
            let mut min_item = list_vec[0].clone();
            Self::validate_direct_ref(player, symbols, &min_item)?;
            for item_ref in list_vec.iter().skip(1) {
                Self::validate_direct_ref(player, symbols, item_ref)?;
                let item = player.get_datum(item_ref);
                let current_min = player.get_datum(&min_item);
                if datum_less_than(item, current_min, &player.allocator, symbols)? {
                    min_item = item_ref.clone();
                }
            }
            Ok(min_item)
        }
    }

    pub fn get_prop_ref(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "getPropRef requires at least one argument".to_string(),
            ));
        }

        let key = args[0].clone();
        Self::validate_direct_ref(player, &*symbols, datum)?;
        Self::validate_direct_ref(player, &*symbols, &key)?;
        if args.len() >= 2 {
            Self::validate_direct_ref(player, &*symbols, &args[1])?;
        }
        let items = player.get_datum(datum).to_list()?;
        let index = player.get_datum(&key).int_value()?;
        let actual_index = if index == 0 {
            0
        } else if index >= 1 {
            (index - 1) as usize
        } else {
            return Err(ScriptError::new(format!("Index out of bounds: {}", index)));
        };
        if actual_index >= items.len() {
            return Err(ScriptError::new(format!("Index out of bounds: {}", index)));
        }
        let result = items[actual_index].clone();
        if args.len() >= 2 && player.get_datum(&args[0]).is_int() {
            TypeUtils::get_sub_prop(&result, &args[1], player, symbols)
        } else {
            Ok(result)
        }
    }

    fn count(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        let len = player.get_datum(datum).to_list()?.len();
        Ok(player.alloc_datum(Datum::Int(len as i32)))
    }

    fn get_last(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        Ok(player
            .get_datum(datum)
            .to_list()?
            .back()
            .cloned()
            .unwrap_or(DatumRef::Void))
    }

    fn find_equal_index(
        player: &DirPlayer,
        symbols: &SymbolTable,
        list_vec: &VecDeque<DatumRef>,
        find_ref: &DatumRef,
        visited: &mut HashSet<usize>,
    ) -> Result<Option<usize>, ScriptError> {
        Self::validate_search_ref(player, symbols, find_ref, visited)?;
        let find = player.get_datum(find_ref);
        for (index, item_ref) in list_vec.iter().enumerate() {
            Self::validate_search_ref(player, symbols, item_ref, visited)?;
            if datum_equals(player.get_datum(item_ref), find, &player.allocator, symbols).unwrap() {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub fn get_one(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        let list_vec = player.get_datum(datum).to_list()?;
        let index =
            Self::find_equal_index(player, symbols, &list_vec, &args[0], &mut HashSet::new())?;
        let result = index.map(|i| i as i32).unwrap_or(-1) + 1;
        Ok(player.alloc_datum(Datum::Int(result)))
    }

    pub fn find_pos(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::get_one(player, symbols, datum, args)
    }

    pub fn find_pos_near(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        let (_, list_vec, is_sorted) = player.get_datum(datum).to_list_tuple()?;
        let mut visited = HashSet::new();
        if is_sorted {
            Self::validate_direct_ref(player, symbols, &args[0])?;
            let pos =
                ListDatumUtils::find_index_to_add(&list_vec, &args[0], &player.allocator, symbols)?;
            Ok(player.alloc_datum(Datum::Int(pos + 1)))
        } else {
            let index = Self::find_equal_index(player, symbols, &list_vec, &args[0], &mut visited)?;
            Ok(player.alloc_datum(Datum::Int(index.map(|i| i as i32 + 1).unwrap_or(0))))
        }
    }

    pub fn add(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, &*symbols, datum)?;
        if player.get_datum(datum).is_void() {
            return Ok(DatumRef::Void);
        }
        if args.is_empty() {
            let tex_layer_context: Option<(i32, i32, Symbol, usize)> = {
                let mut found = None;
                for cast in &player.movie.cast_manager.casts {
                    for (member_num, member) in &cast.members {
                        if let Some(w3d) = member.member_type.as_shockwave3d() {
                            for (model_name, md) in &w3d.runtime_state.mesh_deform {
                                for (mesh_idx, mesh) in md.meshes.iter().enumerate() {
                                    if mesh.texture_layer_datum_ref.as_ref() == Some(datum) {
                                        found = Some((
                                            cast.number as i32,
                                            *member_num as i32,
                                            model_name.clone(),
                                            mesh_idx,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                found
            };
            debug!(
                "[W3D-ADD] add() no-args on list datum_id={} context={:?}",
                datum.unwrap(),
                tex_layer_context
            );
            let new_ref =
                if let Some((cast_lib, cast_member, model_name, mesh_idx)) = tex_layer_context {
                    use crate::director::lingo::datum::Shockwave3dObjectRef;
                    let new_layer_idx = player.get_datum(datum).to_list()?.len();
                    let model_name = symbols
                        .display(&model_name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                        .to_owned();
                    let name =
                        symbols.intern(&format!("{}:{}:{}", model_name, mesh_idx, new_layer_idx));
                    player.alloc_datum(Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
                        cast_lib,
                        cast_member,
                        object_type: BuiltInSymbol::MeshDeformTexLayer,
                        name,
                    }))
                } else {
                    let key = player.alloc_datum(Datum::Symbol(Symbol::builtin(
                        BuiltInSymbol::TextureCoordinateList,
                    )));
                    let val = player.alloc_datum(Datum::List(
                        crate::director::lingo::datum::DatumType::List,
                        VecDeque::new(),
                        false,
                    ));
                    player.alloc_datum(Datum::PropList(VecDeque::from(vec![(key, val)]), false))
                };
            player
                .get_datum_mut(datum)
                .to_list_mut()?
                .1
                .push_back(new_ref);
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            return Ok(DatumRef::Void);
        }
        Self::validate_direct_ref(player, &*symbols, &args[0])?;
        let (_, list_vec, is_sorted) = player.get_datum(datum).to_list_tuple()?;
        let index = if is_sorted {
            ListDatumUtils::find_index_to_add(list_vec, &args[0], &player.allocator, symbols)?
        } else {
            list_vec.len() as i32
        };
        let values = player.get_datum_mut(datum).to_list_mut()?.1;
        if is_sorted {
            values.insert(index as usize, args[0].clone());
        } else {
            values.push_back(args[0].clone());
        }
        player.note_actor_list_mutation(datum);
        player.note_script_instance_list_mutation(datum);
        Ok(DatumRef::Void)
    }

    pub fn delete_one(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        Self::validate_direct_ref(player, symbols, &args[0])?;
        let mut visited = HashSet::new();
        Self::validate_search_ref(player, symbols, &args[0], &mut visited)?;
        let index = {
            let search_ref = &args[0];
            let item = player.get_datum(search_ref);
            let list_vec = player.get_datum(datum).to_list()?;
            let mut index = None;
            for (i, list_item_ref) in list_vec.iter().enumerate() {
                Self::validate_search_ref(player, symbols, list_item_ref, &mut visited)?;
                // For script instances and other reference types, check reference equality first
                // Direct reference comparison (important for deleteOne(me) in scripts)
                if list_item_ref == search_ref {
                    index = Some(i);
                    break;
                }

                // Fallback to value equality for other types
                let list_item = player.get_datum(list_item_ref);
                if datum_equals(list_item, item, &player.allocator, symbols).unwrap_or(false) {
                    index = Some(i);
                    break;
                }
            }
            index
        };

        {
            let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
            if let Some(index) = index {
                if index == 0 {
                    list_vec.pop_front();
                } else if index == list_vec.len() - 1 {
                    list_vec.pop_back();
                } else {
                    list_vec.remove(index);
                }
                player.note_actor_list_mutation(datum);
                player.note_script_instance_list_mutation(datum);
            }
            Ok(player.alloc_datum(datum_bool(index.is_some())))
        }
    }

    pub fn delete_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        Self::validate_direct_ref(player, symbols, &args[0])?;
        {
            let position = player.get_datum(&args[0]).int_value()?;
            let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
            if position <= list_vec.len() as i32 {
                let index = (position - 1) as usize;
                // Use pop_front/pop_back for endpoints — O(1) on VecDeque
                // vs O(n) for remove() which shifts elements.
                if index == 0 {
                    list_vec.pop_front();
                } else if index == list_vec.len() - 1 {
                    list_vec.pop_back();
                } else {
                    list_vec.remove(index);
                }
                player.note_actor_list_mutation(datum);
                player.note_script_instance_list_mutation(datum);
                Ok(DatumRef::Void)
            } else {
                Err(ScriptError::new("Index out of bounds".to_string()))
            }
        }
    }

    pub fn delete_all(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        {
            let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
            list_vec.clear();
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            Ok(DatumRef::Void)
        }
    }

    pub fn add_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        {
            let datum_value = player.get_datum(datum);
            // Return void gracefully if datum is void or not a list
            // Director typically silently fails rather than erroring on wrong types
            if datum_value.is_void() {
                return Ok(DatumRef::Void);
            }

            // Check if it's actually a list before trying to mutate
            if !matches!(datum_value, Datum::List(..)) {
                return Ok(DatumRef::Void);
            }

            // Director 11.5 spec (p.255) documents addAt(position, value) with
            // both args required, but the shipped runtime silently accepts the
            // one-arg form `list.addAt(value)` and treats it as append, for any
            // value type (int / string / list / etc.) and even on []. Verified
            // in Director's message window. Habbo's spineworld_dcr Docs script
            // `getXMLItem` relies on this: `tList.addAt(tAdd)` in a repeat loop.
            let (position, item_ref) = if args.len() == 1 {
                Self::validate_direct_ref(player, symbols, &args[0])?;
                let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
                (list_vec.len(), args[0].clone())
            } else {
                Self::validate_direct_ref(player, symbols, &args[0])?;
                Self::validate_direct_ref(player, symbols, &args[1])?;
                let pos = player.get_datum(&args[0]).int_value()? - 1;
                (pos.max(0) as usize, args[1].clone())
            };

            let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
            let clamped = position.min(list_vec.len());
            list_vec.insert(clamped, item_ref);
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            Ok(DatumRef::Void)
        }
    }

    pub fn append(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        {
            let datum_value = player.get_datum(datum);
            if datum_value.is_void() {
                return Ok(DatumRef::Void);
            }

            let item = &args[0];
            Self::validate_direct_ref(player, symbols, item)?;
            let (_, list_vec, _) = player.get_datum_mut(datum).to_list_mut()?;
            list_vec.push_back(item.clone());
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);
            Ok(DatumRef::Void)
        }
    }

    pub fn sort(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        let mut visited = HashSet::new();
        let sorted_list = {
            let list_vec = player.get_datum(datum).to_list()?;
            for item_ref in list_vec {
                Self::validate_search_ref(player, symbols, item_ref, &mut visited)?;
            }
            let mut sorted_list = list_vec.clone();
            sorted_list.make_contiguous().sort_by(|a, b| {
                let left = player.get_datum(a);
                let right = player.get_datum(b);

                if datum_equals(left, right, &player.allocator, symbols).unwrap() {
                    return std::cmp::Ordering::Equal;
                } else if datum_less_than(left, right, &player.allocator, symbols).unwrap() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });

            sorted_list
        };

        {
            let (_, list_vec, is_sorted) = player.get_datum_mut(datum).to_list_mut()?;
            list_vec.clear();
            list_vec.extend(sorted_list);
            *is_sorted = true;
            player.note_actor_list_mutation(datum);
            player.note_script_instance_list_mutation(datum);

            Ok(DatumRef::Void)
        }
    }

    pub fn duplicate(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        player_duplicate_datum(player, symbols, datum)
    }

    pub fn join(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Self::validate_direct_ref(player, symbols, datum)?;
        if let Some(delimiter) = args.first() {
            Self::validate_direct_ref(player, symbols, delimiter)?;
        }
        {
            let list_len = player.get_datum(datum).to_list()?.len();

            // Optional delimiter argument
            // TODO: verify default delimiter
            let delimiter = if args.len() >= 1 {
                match player.get_datum(&args[0]) {
                    Datum::String(s) => s.clone(),
                    _ => "&".to_string(),
                }
            } else {
                "&".to_string()
            };

            // Convert each element to string safely without extra quotes
            let mut pieces = Vec::with_capacity(list_len);
            for index in 0..list_len {
                let item_ref = player
                    .get_datum(datum)
                    .to_list()?
                    .get(index)
                    .cloned()
                    .unwrap();
                Self::validate_direct_ref(player, symbols, &item_ref)?;
                let datum = player.get_datum(&item_ref).clone();
                let piece = match &datum {
                    Datum::String(s) => s.clone(),
                    Datum::Symbol(sym) => symbols
                        .display(sym)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                        .to_owned(),
                    Datum::Int(n) => n.to_string(),
                    _ => format!("{:?}", format_concrete_datum(&datum, symbols, player)?),
                };
                pieces.push(piece);
            }

            let joined = pieces.join(&delimiter);
            Ok(player.alloc_datum(Datum::String(joined)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;
    use async_std::channel;

    fn session_with_two_players() -> RuntimeSession {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 700,
            generation: 1,
        });
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.add_player(2, channel::unbounded().0));
        session
    }

    #[test]
    fn search_stops_before_later_foreign_candidate() {
        let mut session = session_with_two_players();
        let foreign_symbol = {
            let mut foreign_symbols = SymbolTable::with_owner(SymbolOwner {
                session: 701,
                generation: 1,
            });
            foreign_symbols.intern("foreignCandidate")
        };
        let (list_ref, first) = session
            .with_player(1, |context| {
                let first = context.player.alloc_datum(Datum::Int(7));
                let foreign = context
                    .player
                    .alloc_datum(Datum::Symbol(foreign_symbol.clone()));
                let list = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([first.clone(), foreign]),
                    false,
                ));
                (list, first)
            })
            .unwrap();

        let found = session
            .with_player(1, |mut context| {
                let probe = context.player.alloc_datum(Datum::Int(7));
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::GetOne),
                    &vec![probe],
                )
            })
            .unwrap()
            .unwrap();
        assert!(session
            .with_player(1, |context| matches!(
                context.player.get_datum(&found),
                Datum::Int(1)
            ))
            .unwrap());

        let rejected = session
            .with_player(1, |mut context| {
                let probe = context.player.alloc_datum(Datum::Int(8));
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::GetOne),
                    &vec![probe],
                )
            })
            .unwrap();
        assert!(rejected.is_err());
        assert!(session
            .with_player(1, |context| matches!(
                context.player.get_datum(&first),
                Datum::Int(7)
            ))
            .unwrap());
    }

    #[test]
    fn rejected_foreign_mutation_preserves_cache_generation() {
        let mut session = session_with_two_players();
        let foreign = session
            .with_player(2, |context| context.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        let (list_ref, owned) = session
            .with_player(1, |context| {
                let owned = context.player.alloc_datum(Datum::Int(3));
                let list = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([owned.clone()]),
                    false,
                ));
                context
                    .player
                    .cache_script_instance_list(4, list.clone(), Vec::new());
                (list, owned)
            })
            .unwrap();

        let before = session
            .with_player(1, |context| {
                (
                    context
                        .player
                        .script_instance_list_generation
                        .get(&4)
                        .copied()
                        .unwrap(),
                    context.player.get_datum(&list_ref).to_list().unwrap().len(),
                )
            })
            .unwrap();
        let rejected = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Append),
                    &vec![foreign.clone()],
                )
            })
            .unwrap();
        assert!(rejected.is_err());
        let after_reject = session
            .with_player(1, |context| {
                (
                    context
                        .player
                        .script_instance_list_generation
                        .get(&4)
                        .copied()
                        .unwrap(),
                    context.player.get_datum(&list_ref).to_list().unwrap().len(),
                )
            })
            .unwrap();
        assert_eq!(before, after_reject);

        session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Append),
                    &vec![owned.clone()],
                )
                .unwrap();
            })
            .unwrap();
        assert_eq!(
            session
                .with_player(1, |context| context.player.script_instance_list_generation
                    [&4])
                .unwrap(),
            before.0 + 1
        );
    }

    #[test]
    fn set_at_nonpositive_validates_receiver_but_not_unused_value() {
        let mut session = session_with_two_players();
        let foreign = session
            .with_player(2, |context| context.player.alloc_datum(Datum::Int(11)))
            .unwrap();
        let list_ref = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        let position = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(0)))
            .unwrap();
        let result = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::SetAt),
                    &vec![position.clone(), foreign.clone()],
                )
            })
            .unwrap();
        assert!(result.is_ok());
        assert_eq!(
            session
                .with_player(1, |context| context
                    .player
                    .get_datum(&list_ref)
                    .to_list()
                    .unwrap()
                    .len())
                .unwrap(),
            0
        );

        let non_list = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let result = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &non_list,
                    Symbol::builtin(BuiltInSymbol::SetAt),
                    &vec![position, foreign],
                )
            })
            .unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn add_without_args_uses_generic_texture_coordinate_fallback() {
        let mut session = session_with_two_players();
        let list_ref = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Add),
                    &vec![],
                )
                .unwrap();
            })
            .unwrap();
        session
            .with_player(1, |context| {
                let child = context.player.get_datum(&list_ref).to_list().unwrap().back().unwrap().clone();
                let (pairs, _) = context.player.get_datum(&child).to_map_tuple().unwrap();
                assert_eq!(pairs.len(), 1);
                assert!(matches!(context.player.get_datum(&pairs[0].0), Datum::Symbol(symbol) if *symbol == Symbol::builtin(BuiltInSymbol::TextureCoordinateList)));
                assert!(matches!(context.player.get_datum(&pairs[0].1), Datum::List(..)));
            })
            .unwrap();
    }

    #[test]
    fn join_uses_displayed_symbols_and_debug_quotes_fallback() {
        let mut session = session_with_two_players();
        let list_ref = session
            .with_player(1, |context| {
                let symbol = context.symbols.intern("MiXeD");
                let symbol_ref = context.player.alloc_datum(Datum::Symbol(symbol));
                let number_ref = context.player.alloc_datum(Datum::Int(2));
                let nested_item = context.player.alloc_datum(Datum::Int(3));
                let nested = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([nested_item]),
                    false,
                ));
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([symbol_ref, number_ref, nested]),
                    false,
                ))
            })
            .unwrap();
        let delimiter = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::String(",".into()))
            })
            .unwrap();
        let joined = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Join),
                    &vec![delimiter],
                )
            })
            .unwrap()
            .unwrap();
        assert!(session
            .with_player(1, |context| matches!(context.player.get_datum(&joined), Datum::String(value) if value == "MiXeD,2,\"[3]\""))
            .unwrap());
    }

    #[test]
    fn duplicate_dispatch_returns_an_independent_list_reference() {
        let mut session = session_with_two_players();
        let list_ref = session
            .with_player(1, |context| {
                let value = context.player.alloc_datum(Datum::Int(4));
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([value]),
                    false,
                ))
            })
            .unwrap();
        let duplicate = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Duplicate),
                    &vec![],
                )
            })
            .unwrap()
            .unwrap();
        assert_ne!(duplicate.unwrap(), list_ref.unwrap());
        assert!(session
            .with_player(1, |context| matches!(context.player.get_datum(&duplicate), Datum::List(_, values, _) if values.len() == 1))
            .unwrap());
    }

    #[test]
    fn ordinary_and_xml_lists_keep_their_indexing_conventions() {
        let mut session = session_with_two_players();
        let (ordinary, xml) = session
            .with_player(1, |context| {
                let first = context.player.alloc_datum(Datum::Int(10));
                let second = context.player.alloc_datum(Datum::Int(20));
                let ordinary = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([first.clone(), second.clone()]),
                    false,
                ));
                let xml = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::XmlChildNodes,
                    VecDeque::from([first, second]),
                    false,
                ));
                (ordinary, xml)
            })
            .unwrap();
        let ordinary_position = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let xml_position = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(0)))
            .unwrap();
        let ordinary_first = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &ordinary,
                    Symbol::builtin(BuiltInSymbol::GetAt),
                    &vec![ordinary_position],
                )
            })
            .unwrap()
            .unwrap();
        let xml_first = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &xml,
                    Symbol::builtin(BuiltInSymbol::GetAt),
                    &vec![xml_position],
                )
            })
            .unwrap()
            .unwrap();
        assert!(session
            .with_player(1, |context| matches!(
                context.player.get_datum(&ordinary_first),
                Datum::Int(10)
            ))
            .unwrap());
        assert!(session
            .with_player(1, |context| matches!(
                context.player.get_datum(&xml_first),
                Datum::Int(10)
            ))
            .unwrap());
    }

    #[test]
    fn unknown_foreign_list_property_is_rejected_before_void_fallback() {
        let mut session = session_with_two_players();
        let mut foreign_symbols = SymbolTable::with_owner(SymbolOwner {
            session: 702,
            generation: 1,
        });
        let foreign_name = foreign_symbols.intern("unknownLocalName");
        let list_ref = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        let result = session
            .with_player(1, |context| {
                let list_vec = context.player.get_datum(&list_ref).to_list().unwrap();
                ListDatumUtils::get_prop(
                    list_vec,
                    foreign_name,
                    &context.player.allocator,
                    context.symbols,
                )
            })
            .unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn sorted_add_and_stable_sort_preserve_order_and_void_comparisons() {
        let mut session = session_with_two_players();
        let (sorted, first_equal, second_equal) = session
            .with_player(1, |context| {
                let one = context.player.alloc_datum(Datum::Int(1));
                let three = context.player.alloc_datum(Datum::Int(3));
                let sorted = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([one, three]),
                    true,
                ));
                let first_equal = context.player.alloc_datum(Datum::String("same".into()));
                let second_equal = context.player.alloc_datum(Datum::String("SAME".into()));
                (sorted, first_equal, second_equal)
            })
            .unwrap();
        let two = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(2)))
            .unwrap();
        session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &sorted,
                    Symbol::builtin(BuiltInSymbol::Add),
                    &vec![two],
                )
                .unwrap()
            })
            .unwrap();
        let values = session
            .with_player(1, |context| {
                context
                    .player
                    .get_datum(&sorted)
                    .to_list()
                    .unwrap()
                    .iter()
                    .map(|r| context.player.get_datum(r).int_value().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(values, vec![1, 2, 3]);

        let stable = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([first_equal.clone(), second_equal.clone()]),
                    false,
                ))
            })
            .unwrap();
        session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &stable,
                    Symbol::builtin(BuiltInSymbol::Sort),
                    &vec![],
                )
                .unwrap()
            })
            .unwrap();
        let stable_values = session
            .with_player(1, |context| {
                context
                    .player
                    .get_datum(&stable)
                    .to_list()
                    .unwrap()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(stable_values[0].unwrap(), first_equal.unwrap());
        assert_eq!(stable_values[1].unwrap(), second_equal.unwrap());

        let void_probe = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Void))
            .unwrap();
        let near = session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &sorted,
                    Symbol::builtin(BuiltInSymbol::FindPosNear),
                    &vec![void_probe.clone()],
                )
                .unwrap()
            })
            .unwrap();
        assert!(session
            .with_player(1, |context| matches!(
                context.player.get_datum(&near),
                Datum::Int(1)
            ))
            .unwrap());
        session
            .with_player(1, |mut context| {
                ListDatumHandlers::call(
                    &mut context,
                    &sorted,
                    Symbol::builtin(BuiltInSymbol::Add),
                    &vec![void_probe],
                )
                .unwrap()
            })
            .unwrap();
        assert!(session
            .with_player(1, |context| matches!(
                context
                    .player
                    .get_datum(&sorted)
                    .to_list()
                    .unwrap()
                    .front()
                    .map(|r| context.player.get_datum(r)),
                Some(Datum::Void)
            ))
            .unwrap());
    }

    #[test]
    fn foreign_sort_and_matrix_row_rejections_preserve_state() {
        let mut session = session_with_two_players();
        let foreign_row = session
            .with_player(2, |context| {
                let value = context.player.alloc_datum(Datum::Int(8));
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([value]),
                    false,
                ))
            })
            .unwrap();
        let foreign_symbol = {
            let mut symbols = SymbolTable::with_owner(SymbolOwner {
                session: 703,
                generation: 1,
            });
            symbols.intern("foreignSort")
        };
        let (matrix, sortable) = session
            .with_player(1, |context| {
                let row_value = context.player.alloc_datum(Datum::Int(1));
                let row = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([row_value]),
                    false,
                ));
                let matrix = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([foreign_row.clone(), row]),
                    false,
                ));
                let sortable_value = context.player.alloc_datum(Datum::Int(2));
                let foreign_sort_value = context
                    .player
                    .alloc_datum(Datum::Symbol(foreign_symbol.clone()));
                let sortable = context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([sortable_value, foreign_sort_value]),
                    false,
                ));
                context
                    .player
                    .cache_script_instance_list(5, sortable.clone(), Vec::new());
                (matrix, sortable)
            })
            .unwrap();
        let row_index = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let col_index = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let value = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        let before = session
            .with_player(1, |context| {
                (
                    context.player.script_instance_list_generation[&5],
                    context
                        .player
                        .get_datum(&sortable)
                        .to_list_tuple()
                        .unwrap()
                        .2,
                    context.player.get_datum(&matrix).to_list().unwrap().len(),
                    context
                        .player
                        .get_datum(&sortable)
                        .to_list()
                        .unwrap()
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap();
        assert!(session
            .with_player(1, |mut context| ListDatumHandlers::call(
                &mut context,
                &matrix,
                Symbol::builtin(BuiltInSymbol::GetVal),
                &vec![row_index.clone(), col_index.clone()],
            ))
            .unwrap()
            .is_err());
        assert!(session
            .with_player(1, |mut context| ListDatumHandlers::call(
                &mut context,
                &matrix,
                Symbol::builtin(BuiltInSymbol::SetVal),
                &vec![row_index, col_index, value],
            ))
            .unwrap()
            .is_err());
        assert!(session
            .with_player(1, |mut context| ListDatumHandlers::call(
                &mut context,
                &sortable,
                Symbol::builtin(BuiltInSymbol::Sort),
                &vec![],
            ))
            .unwrap()
            .is_err());
        let after = session
            .with_player(1, |context| {
                (
                    context.player.script_instance_list_generation[&5],
                    context
                        .player
                        .get_datum(&sortable)
                        .to_list_tuple()
                        .unwrap()
                        .2,
                    context.player.get_datum(&matrix).to_list().unwrap().len(),
                    context
                        .player
                        .get_datum(&sortable)
                        .to_list()
                        .unwrap()
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn w3d_texture_layer_add_uses_first_cast_context_and_interned_composite_name() {
        let mut session = session_with_two_players();
        let list_ref = session
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        session
            .with_player(1, |mut context| {
                use crate::director::enums::Shockwave3dInfo;
                use crate::player::cast_lib::CastLib;
                use crate::player::cast_member::{
                    CastMember, CastMemberType, MeshDeformMesh, MeshDeformState, Shockwave3dMember,
                };
                use crate::player::sprite::ColorRef;
                // W3D model names come from authored movie data.  Claim the
                // display spelling before the built-in `model` symbol's
                // canonical spelling can replace it.
                let model_name = context.symbols.intern_authoritative("Model");
                let mut runtime_state =
                    crate::player::cast_member::Shockwave3dRuntimeState::default();
                runtime_state.mesh_deform.insert(
                    model_name.clone(),
                    MeshDeformState {
                        meshes: vec![MeshDeformMesh {
                            texture_layers: Vec::new(),
                            texture_layer_datum_ref: Some(list_ref.clone()),
                        }],
                    },
                );
                let member = CastMember {
                    number: 1,
                    name: "w3d".into(),
                    comments: String::new(),
                    member_type: CastMemberType::Shockwave3d(Shockwave3dMember {
                        info: Shockwave3dInfo {
                            loops: false,
                            duration: 0,
                            direct_to_stage: false,
                            animation_enabled: false,
                            preload: false,
                            reg_point: (0, 0),
                            default_rect: (0, 0, 320, 240),
                            camera_position: None,
                            camera_rotation: None,
                            bg_color: None,
                            ambient_color: None,
                        },
                        w3d_data: Vec::new(),
                        source_scene: None,
                        parsed_scene: None,
                        runtime_state,
                        converted_from_text: false,
                        text3d_state: None,
                        text3d_source: None,
                    }),
                    color: ColorRef::PaletteIndex(255),
                    bg_color: ColorRef::PaletteIndex(0),
                    reg_point: (0, 0),
                };
                context
                    .player
                    .movie
                    .cast_manager
                    .casts
                    .push(CastLib::test_external(1, 0));
                context.player.movie.cast_manager.casts[0]
                    .members
                    .insert(1, member);
                ListDatumHandlers::call(
                    &mut context,
                    &list_ref,
                    Symbol::builtin(BuiltInSymbol::Add),
                    &vec![],
                )
                .unwrap();
            })
            .unwrap();
        session
            .with_player(1, |context| {
                let child = context
                    .player
                    .get_datum(&list_ref)
                    .to_list()
                    .unwrap()
                    .front()
                    .unwrap()
                    .clone();
                match context.player.get_datum(&child) {
                    Datum::Shockwave3dObjectRef(object) => {
                        assert_eq!(object.object_type, BuiltInSymbol::MeshDeformTexLayer);
                        assert_eq!(context.symbols.display(&object.name).unwrap(), "Model:0:0");
                    }
                    _ => panic!("W3D add did not create a texture-layer object"),
                }
            })
            .unwrap();
    }
}
