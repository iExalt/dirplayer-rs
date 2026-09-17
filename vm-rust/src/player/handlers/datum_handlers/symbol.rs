use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError,
    },
};

pub struct SymbolDatumHandlers {}

impl SymbolDatumHandlers {
    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        _args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let _datum = checked_datum(player, &datum, symbols)?;
            let handler_name = symbols
                .display(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            Err(ScriptError::new(format!(
                "No handler {handler_name} for symbol"
            )))
        })
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        _: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_lower {
            "ilk" => Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Symbol)))),
            _ => Err(ScriptError::new(format!(
                "Cannot get symbol property {}",
                prop_name
            ))),
        }
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
