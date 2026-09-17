use crate::{
    director::lingo::datum::{Datum, datum_bool},
    player::{
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        DatumRef, DirPlayer, ScriptError,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
    },
};

pub struct PointDatumHandlers {}

impl PointDatumHandlers {
    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let handler_name_lower = symbols
                .lower(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let handler_name_display = symbols
                .display(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;

            match handler_name_lower {
            "getat" => Self::get_at(player, symbols, &datum, args),
            "setat" => Self::set_at(player, symbols, &datum, args),
            "inside" => Self::inside(player, symbols, &datum, args),
            "duplicate" => Self::duplicate(player, symbols, &datum),
            // A point is addressable like a two-element linear list —
            // `point[1]` / `point[2]` read and write it, and `duplicate()`
            // already works — so `count` answers 2. Generic list helpers get
            // handed points all the time: Merlin's Revenge routes a sprite
            // location through `ListInteger(alist)`, which does
            // `alist.duplicate().count()` before rounding each element, and
            // erroring here killed the handler that positions map tiles.
            "count" => {
                let value = checked_datum(player, &datum, symbols)?;
                value.to_point_inline()?;
                Ok(player.alloc_datum(Datum::Int(2)))
            }
            _ => Err(ScriptError::new(format!(
                "no handler {handler_name_display} for point"
            ))),
            }
        })
    }

    pub fn duplicate(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        let value = checked_datum(player, datum, symbols)?;
        let (vals, flags) = value.to_point_inline()?;
        Ok(player.alloc_datum(Datum::Point(vals, flags)))
    }

    pub fn inside(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 1 {
            return Err(ScriptError::new("inside requires a rectangle".to_string()));
        }
        let (point, _pf) = {
            let value = checked_datum(player, datum, symbols)?;
            value.to_point_inline()?
        };
        let (rect, _rf) = {
            let value = checked_datum(player, &args[0], symbols)?;
            value.to_rect_inline()?
        };

        let px = point[0] as i32;
        let py = point[1] as i32;
        let x1 = rect[0] as i32;
        let y1 = rect[1] as i32;
        let x2 = rect[2] as i32;
        let y2 = rect[3] as i32;

        let inside = x1 <= px && px < x2 && y1 <= py && py < y2;

        Ok(player.alloc_datum(datum_bool(inside)))
    }

    pub fn get_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 1 {
            return Err(ScriptError::new("getAt requires an index".to_string()));
        }
        let (vals, flags) = {
            let value = checked_datum(player, datum, symbols)?;
            value.to_point_inline()?
        };

        let index = {
            let value = checked_datum(player, &args[0], symbols)?;
            value.int_value()?
        }; // 1 or 2
        if !(1..=2).contains(&index) {
            return Err(ScriptError::new("Invalid index for point".to_string()));
        }

        let i = (index - 1) as usize;
        let component = Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(flags, i));
        Ok(player.alloc_datum(component))
    }

    pub fn set_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new("setAt requires an index and value".to_string()));
        }
        {
            let value = checked_datum(player, datum, symbols)?;
            value.to_point_inline()?;
        }
        let index = {
            let value = checked_datum(player, &args[0], symbols)?;
            value.int_value()?
        };

        if !(1..=2).contains(&index) {
            return Err(ScriptError::new("Invalid index for point".to_string()));
        }

        let new_val = {
            let value = checked_datum(player, &args[1], symbols)?;
            value.clone()
        };
        let (val, is_float) = Datum::datum_to_inline_component(&new_val)?;

        let i = (index - 1) as usize;
        let (vals, flags) = player
            .allocator
            .try_get_datum_mut(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?
            .to_point_inline_mut()?;
        vals[i] = val;
        Datum::inline_set_float(flags, i, is_float);

        Ok(DatumRef::Void)
    }

    pub fn get_prop(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
    ) -> Result<Datum, ScriptError> {
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
        };
        let (vals, flags) = datum.to_point_inline()?;
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;

        match prop_lower {
            "loch" => Ok(Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(flags, 0))),
            "locv" => Ok(Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(flags, 1))),
            "ilk"  => Ok(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Point))),
            // `(expr).float` is the property form of `float(expr)` (Director 11.5
            // Scripting Dictionary, "float()": Usage `(expression).float`).
            // Director returns a point unchanged from float() — verified:
            //   p = point(-342, 159); put float(p * p) -- point(116964, 25281)
            // so both surfaces must agree. See TypeHandlers::float.
            "float" => Ok(datum.clone()),
            _ => Err(ScriptError::new(format!("Cannot get point property {}", prop_name))),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let new_val = checked_datum(player, value_ref, symbols)?.clone();
        let (val, is_float) = Datum::datum_to_inline_component(&new_val)?;

        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let idx = match symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
        {
            "loch" => 0usize,
            "locv" => 1usize,
            _ => return Err(ScriptError::new(format!("Cannot set point property {}", prop_name))),
        };

        let (vals, flags) = player
            .allocator
            .try_get_datum_mut(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?
            .to_point_inline_mut()?;
        vals[idx] = val;
        Datum::inline_set_float(flags, idx, is_float);

        Ok(())
    }
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    match datum {
        DatumRef::Void => {
            validate_direct_symbol_fields(&Datum::Void, symbols)?;
            Ok(&Datum::Void)
        }
        _ => {
            let value = player
                .allocator
                .try_get_datum(datum)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?;
            validate_direct_symbol_fields(value, symbols)?;
            Ok(value)
        }
    }
}
