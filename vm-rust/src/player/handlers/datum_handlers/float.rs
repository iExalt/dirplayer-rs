use crate::{
    director::lingo::datum::Datum,
    player::{
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError,
    },
};

pub struct FloatDatumHandlers {}

impl FloatDatumHandlers {
    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum_ref}")))?,
        };
        crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
        let float_value = datum.float_value()?;
        match prop_lower {
            "abs" => Ok(player.alloc_datum(Datum::Float(float_value.abs()))),
            "ilk" => Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Float)))),
            "integer" => Ok(player.alloc_datum(Datum::Int(float_value.round() as i32))),
            "float" => Ok(datum_ref.clone()),
            "char" => {
                let int_value = float_value.round() as i32;
                if int_value >= 0 && int_value <= 255 {
                    let ch = char::from_u32(int_value as u32).unwrap_or('?');
                    Ok(player.alloc_datum(Datum::String(ch.to_string())))
                } else {
                    Err(ScriptError::new(format!(
                        "Float {} out of range for char (must be 0-255)",
                        float_value
                    )))
                }
            }
            "string" => Ok(player.alloc_datum(Datum::String(float_value.to_string()))),
            "magnitude" => Ok(player.alloc_datum(Datum::Float(float_value.abs()))),
            // Vector-like access: return 0.0 for x/y/z when a float is used where a vector was expected
            "x" | "y" | "z" => Ok(player.alloc_datum(Datum::Float(0.0))),
            // Director allows trig/math functions as numeric properties via the dot
            // syntax: `n.cos` is equivalent to `cos(n)`, `n.sqrt` to `sqrt(n)`, etc.
            // Inputs/outputs are in radians for trig, matching Director's globals.
            "sin" => Ok(player.alloc_datum(Datum::Float(float_value.sin()))),
            "cos" => Ok(player.alloc_datum(Datum::Float(float_value.cos()))),
            "tan" => Ok(player.alloc_datum(Datum::Float(float_value.tan()))),
            "asin" => Ok(player.alloc_datum(Datum::Float(float_value.asin()))),
            "acos" => Ok(player.alloc_datum(Datum::Float(float_value.acos()))),
            "atan" => Ok(player.alloc_datum(Datum::Float(float_value.atan()))),
            "sqrt" => Ok(player.alloc_datum(Datum::Float(float_value.sqrt()))),
            "log" => Ok(player.alloc_datum(Datum::Float(float_value.ln()))),
            "exp" => Ok(player.alloc_datum(Datum::Float(float_value.exp()))),
            _ => Err(ScriptError::new(format!(
                "Cannot get float property {}",
                prop_name
            ))),
        }
    }
}
