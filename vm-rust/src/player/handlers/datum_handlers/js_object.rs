use crate::{
    director::lingo::datum::Datum,
    player::{
        js_lingo_loader::{
            get_js_object, get_js_object_prop, invoke_js_object_method, set_js_object_prop,
        },
        compare::validate_direct_symbol_fields,
        reserve_player_ref, symbols::{symbol::Symbol, symbol_table::SymbolTable}, DatumRef, DirPlayer, ScriptError,
        ScriptErrorCode,
    },
};

pub struct JsObjectDatumHandlers {}

impl JsObjectDatumHandlers {
    fn handle_of(player: &DirPlayer, datum: &DatumRef) -> Result<u32, ScriptError> {
        match player.get_datum(datum) {
            Datum::JsObjectRef(id) => Ok(*id),
            _ => Err(ScriptError::new("Expected JS object reference".to_string())),
        }
    }

    pub fn call(
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let id = reserve_player_ref(|player| Self::handle_of(player, datum))?;
        let handler_name = symbols
            .display(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            .to_owned();
        match invoke_js_object_method(id, &handler_name, args, symbols) {
            Some(Ok(result)) => Ok(result),
            Some(Err(message)) => Err(ScriptError::new(format!(
                "{} (JS object handler {})",
                message, handler_name
            ))),
            None => Err(ScriptError::new_code(
                ScriptErrorCode::HandlerNotFound,
                format!("No handler {} for JS object datum", handler_name),
            )),
        }
    }

    /// Property reads fall back to VOID for an absent property, matching how
    /// Lingo reads a missing property off a script instance (and how JS reads
    /// `undefined`) rather than raising.
    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        prop_name: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let id = Self::handle_of(player, datum)?;
        let prop_name = symbols
            .display(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            .to_owned();
        match get_js_object_prop(id, &prop_name, symbols) {
            Some(Ok(value)) => Ok(value),
            Some(Err(message)) => Err(ScriptError::new(message)),
            None => Ok(DatumRef::Void),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        prop_name: Symbol,
        value_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let id = Self::handle_of(player, datum)?;
        let Some((_, obj)) = get_js_object(id) else {
            return Ok(());
        };
        let prop_name = symbols
            .display(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            .to_owned();
        let value_datum = match value_ref {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(value_ref)
                .ok_or_else(|| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale JS property value".to_owned(),
                ))?,
        };
        validate_direct_symbol_fields(value_datum, symbols)?;
        let value = crate::player::js_lingo_loader::datum_ref_to_js_value(player, symbols, value_ref)
            .map_err(|error| ScriptError::new(error.message))?;
        set_js_object_prop(&obj, &prop_name, value);
        Ok(())
    }
}
