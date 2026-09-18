use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError,
    },
};

pub struct RectDatumHandlers {}
pub struct RectUtils {}

impl RectUtils {
    pub fn union(rect1: (i32, i32, i32, i32), rect2: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        let left = rect1.0.min(rect2.0);
        let top = rect1.1.min(rect2.1);
        let right = rect1.2.max(rect2.2);
        let bottom = rect1.3.max(rect2.3);
        (left, top, right, bottom)
    }

    pub fn intersect(
        rect1: (i32, i32, i32, i32),
        rect2: (i32, i32, i32, i32),
    ) -> (i32, i32, i32, i32) {
        let left = rect1.0.max(rect2.0);
        let top = rect1.1.max(rect2.1);
        let right = rect1.2.min(rect2.2);
        let bottom = rect1.3.min(rect2.3);
        // If rectangles don't overlap, return empty rect (0,0,0,0)
        if left >= right || top >= bottom {
            return (0, 0, 0, 0);
        }
        (left, top, right, bottom)
    }
}

impl RectDatumHandlers {
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
                "intersect" => Self::intersect(player, symbols, &datum, args),
                "duplicate" => Self::duplicate(player, symbols, &datum),
                "offset" => Self::offset(player, symbols, &datum, args),
                "inflate" => Self::inflate(player, symbols, &datum, args),
                // Same list-like addressing as a point (see PointDatumHandlers),
                // four elements here: left, top, right, bottom.
                "count" => {
                    checked_datum(player, &datum, symbols)?.to_rect_inline()?;
                    Ok(player.alloc_datum(Datum::Int(4)))
                }
                _ => Err(ScriptError::new(format!(
                    "no handler {handler_name_display} for rect"
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
        let (vals, flags) = value.to_rect_inline()?;
        Ok(player.alloc_datum(Datum::Rect(vals, flags)))
    }

    pub fn offset(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new("offset requires 2 arguments".to_string()));
        }
        let (vals, _flags) = checked_datum(player, datum, symbols)?.to_rect_inline()?;
        let dx = checked_datum(player, &args[0], symbols)?.int_value()?;
        let dy = checked_datum(player, &args[1], symbols)?.int_value()?;

        // offset always produces int results
        Ok(player.alloc_datum(Datum::Rect(
            [
                vals[0] + dx as f64,
                vals[1] + dy as f64,
                vals[2] + dx as f64,
                vals[3] + dy as f64,
            ],
            0,
        )))
    }

    /// `rect.inflate(widthChange, heightChange)` / `inflate(rect, w, h)`.
    ///
    /// Expands (or contracts, for negative arguments) the rectangle about its
    /// centre: the change is applied to *both* opposing sides, so the result is
    /// 2*w wider and 2*h taller than the original and keeps the same centre
    /// point. Dropped from the 11.5 dictionary but still a live Lingo function
    /// — semantics are the Director 8 Lingo Dictionary's:
    ///   put inflate(rect(100, 150, 200, 250), 10, 20)
    ///   -- rect(90, 130, 210, 270)
    pub fn inflate(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new("inflate requires 2 arguments".to_string()));
        }
        let (vals, _flags) = checked_datum(player, datum, symbols)?.to_rect_inline()?;
        let dw = checked_datum(player, &args[0], symbols)?.to_float()? as f64;
        let dh = checked_datum(player, &args[1], symbols)?.to_float()? as f64;

        let result = Datum::build_rect(
            &Datum::from_f64(vals[0] - dw),
            &Datum::from_f64(vals[1] - dh),
            &Datum::from_f64(vals[2] + dw),
            &Datum::from_f64(vals[3] + dh),
        )?;

        Ok(player.alloc_datum(result))
    }

    pub fn intersect(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "intersect requires a rectangle".to_string(),
            ));
        }
        let (r1, _f1) = checked_datum(player, datum, symbols)?.to_rect_inline()?;
        let (r2, _f2) = checked_datum(player, &args[0], symbols)?.to_rect_inline()?;

        // intersect uses min/max
        let l = r1[0].min(r2[0]);
        let t = r1[1].min(r2[1]);
        let r = r1[2].max(r2[2]);
        let b = r1[3].max(r2[3]);

        // Use from_f64 logic for flags
        let result = Datum::build_rect(
            &Datum::from_f64(l),
            &Datum::from_f64(t),
            &Datum::from_f64(r),
            &Datum::from_f64(b),
        )?;

        Ok(player.alloc_datum(result))
    }

    pub fn get_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new("getAt requires an index".to_string()));
        }
        let (vals, flags) = checked_datum(player, datum, symbols)?.to_rect_inline()?;

        let index = checked_datum(player, &args[0], symbols)?.int_value()?; // 1..4
        if !(1..=4).contains(&index) {
            return Err(ScriptError::new("Invalid index for rect".to_string()));
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
            return Err(ScriptError::new(
                "setAt requires an index and value".to_string(),
            ));
        }
        checked_datum(player, datum, symbols)?.to_rect_inline()?;
        let index = checked_datum(player, &args[0], symbols)?.int_value()?;
        let new_val = checked_datum(player, &args[1], symbols)?.clone();

        if !(1..=4).contains(&index) {
            return Err(ScriptError::new("Invalid index for rect".to_string()));
        }

        let (val, is_float) = Datum::datum_to_inline_component(&new_val)?;

        let i = (index - 1) as usize;
        let (vals, flags) = player
            .allocator
            .try_get_datum_mut(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?
            .to_rect_inline_mut()?;
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
        let (vals, flags) = datum.to_rect_inline()?;
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;

        let left = vals[0];
        let top = vals[1];
        let right = vals[2];
        let bottom = vals[3];

        match prop_lower {
            "ilk" => Ok(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Rect))),
            "width" => Ok(Datum::from_f64(right - left)),
            "height" => Ok(Datum::from_f64(bottom - top)),
            "left" => Ok(Datum::inline_component_to_datum(
                left,
                Datum::inline_is_float(flags, 0),
            )),
            "top" => Ok(Datum::inline_component_to_datum(
                top,
                Datum::inline_is_float(flags, 1),
            )),
            "right" => Ok(Datum::inline_component_to_datum(
                right,
                Datum::inline_is_float(flags, 2),
            )),
            "bottom" => Ok(Datum::inline_component_to_datum(
                bottom,
                Datum::inline_is_float(flags, 3),
            )),
            _ => Err(ScriptError::new(format!(
                "Cannot get rect property {}",
                prop_name
            ))),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let idx = match symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
        {
            "left" => 0usize,
            "top" => 1usize,
            "right" => 2usize,
            "bottom" => 3usize,
            _ => {
                return Err(ScriptError::new(format!(
                    "Cannot set rect property {}",
                    prop_name
                )))
            }
        };

        let new_val = checked_datum(player, value_ref, symbols)?.clone();
        let (val, is_float) = Datum::datum_to_inline_component(&new_val)?;

        let (vals, flags) = player
            .allocator
            .try_get_datum_mut(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?
            .to_rect_inline_mut()?;
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
