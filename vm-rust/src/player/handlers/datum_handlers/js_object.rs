use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::validate_direct_symbol_fields,
        js_lingo_loader::JsObjectHandle,
        symbols::{symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    },
};

/// Preparation helpers for owner-bound JS object operations. Runtime access
/// is performed by the session executor after the player borrow ends.
pub struct JsObjectDatumHandlers;

impl JsObjectDatumHandlers {
    pub(crate) fn handle_of(
        player: &DirPlayer,
        datum: &DatumRef,
    ) -> Result<JsObjectHandle, ScriptError> {
        match player.allocator.try_get_datum(datum) {
            Some(Datum::JsObjectRef(handle)) => Ok(handle.clone()),
            Some(_) => Err(ScriptError::new("Expected JS object reference".to_owned())),
            None => Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "foreign or stale JS object reference".to_owned(),
            )),
        }
    }

    pub(crate) fn name(
        symbols: &mut SymbolTable,
        name: Symbol,
    ) -> Result<String, ScriptError> {
        symbols
            .display(&name)
            .map(str::to_owned)
            .map_err(|_| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign JS object property symbol".to_owned(),
                )
            })
    }

    pub(crate) fn prepare_call(
        player: &DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<(JsObjectHandle, String, Vec<DatumRef>), ScriptError> {
        let handle = Self::handle_of(player, datum)?;
        let name = Self::name(symbols, handler_name)?;
        for arg in args {
            if !matches!(arg, DatumRef::Void) && player.allocator.try_get_datum(arg).is_none() {
                return Err(ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale JS object argument".to_owned(),
                ));
            }
        }
        Ok((handle, name, args.to_vec()))
    }

    pub(crate) fn prepare_get(
        player: &DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        prop_name: Symbol,
    ) -> Result<(JsObjectHandle, String), ScriptError> {
        Ok((Self::handle_of(player, datum)?, Self::name(symbols, prop_name)?))
    }

    pub(crate) fn prepare_set(
        player: &DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        prop_name: Symbol,
        value_ref: &DatumRef,
    ) -> Result<(JsObjectHandle, String, DatumRef), ScriptError> {
        let value = match value_ref {
            DatumRef::Void => &Datum::Void,
            _ => player.allocator.try_get_datum(value_ref).ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale JS property value".to_owned(),
                )
            })?,
        };
        validate_direct_symbol_fields(value, symbols)?;
        Ok((
            Self::handle_of(player, datum)?,
            Self::name(symbols, prop_name)?,
            value_ref.clone(),
        ))
    }
}
