use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError,
    },
};

use std::f64::consts::PI;

pub struct MathObject {
    pub id: u32,
}

impl MathObject {
    pub fn new(id: u32) -> Self {
        MathObject { id }
    }
}

pub struct MathDatumHandlers;

impl MathDatumHandlers {
    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let handler_name_display = symbols
                .display(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let datum_value = checked_datum(player, &datum, symbols)?;
            let math_id = datum_value.to_math_ref()?;
            let _math_obj = player
                .math_objects
                .get(&math_id)
                .ok_or_else(|| ScriptError::new(format!("Math object {} not found", math_id)))?;

            let arg_values: Vec<f64> = args
                .iter()
                .map(|arg| checked_datum(player, arg, symbols))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter_map(|arg| arg.float_value().ok().map(|v| v as f64))
                .collect();

            let arg0 = || arg_values.get(0).copied().unwrap_or(0.0);
            let arg1 = || arg_values.get(1).copied().unwrap_or(0.0);

            let result: f64 = match handler_name.into_builtin() {
                Some(BuiltInSymbol::Abs) => arg0().abs(),
                Some(BuiltInSymbol::Ceil) => arg0().ceil(),
                Some(BuiltInSymbol::Floor) => arg0().floor(),
                Some(BuiltInSymbol::Round) => arg0().round(),
                Some(BuiltInSymbol::Sin) => arg0().sin(),
                Some(BuiltInSymbol::Cos) => arg0().cos(),
                Some(BuiltInSymbol::Tan) => arg0().tan(),
                Some(BuiltInSymbol::Asin) => arg0().asin(),
                Some(BuiltInSymbol::Acos) => arg0().acos(),
                Some(BuiltInSymbol::Atan) => arg0().atan(),
                Some(BuiltInSymbol::Atan2) => arg0().atan2(arg1()),
                Some(BuiltInSymbol::Sqrt) => arg0().sqrt(),
                Some(BuiltInSymbol::Exp) => arg0().exp(),
                Some(BuiltInSymbol::Log) => arg0().ln(),
                Some(BuiltInSymbol::Pow) => arg0().powf(arg1()),
                Some(BuiltInSymbol::Min) => {
                    arg_values.iter().copied().fold(f64::INFINITY, f64::min)
                }
                Some(BuiltInSymbol::Max) => {
                    arg_values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                }
                _ => {
                    return Err(ScriptError::new(format!(
                        "Unknown math function '{handler_name_display}'"
                    )))
                }
            };

            Ok(player.alloc_datum(Datum::Float(result)))
        })
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        _datum: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop.into_builtin() {
            Some(BuiltInSymbol::Ilk) => {
                Ok(player.alloc_datum(Datum::Symbol(BuiltInSymbol::Math.into())))
            }
            Some(BuiltInSymbol::Pi) => Ok(player.alloc_datum(Datum::Float(PI))),
            _ => Err(ScriptError::new(format!(
                "Unknown math property '{prop_name}'"
            ))),
        }
    }

    pub fn set_prop(
        _player: &mut DirPlayer,
        symbols: &SymbolTable,
        _datum: &DatumRef,
        prop: Symbol,
        _value: &DatumRef,
    ) -> Result<(), ScriptError> {
        Err(ScriptError::new(format!(
            "Cannot set math property '{}'",
            symbols.display(&prop).unwrap_or("<foreign symbol>")
        )))
    }
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    let value = match datum {
        DatumRef::Void => &Datum::Void,
        _ => player
            .allocator
            .try_get_datum(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
    };
    validate_direct_symbol_fields(value, symbols)?;
    Ok(value)
}
