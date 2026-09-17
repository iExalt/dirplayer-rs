use crate::{
    director::lingo::datum::{Datum, StringChunkType, datum_bool},
    player::{
        DirPlayer, ScriptError,
        cast_lib::CastMemberRef,
        cast_member::ButtonType,
        handlers::datum_handlers::cast_member_ref::{borrow_member_mut_with_player, checked_get_datum}, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
    },
};

pub struct ButtonMemberHandlers {}

fn builtin_symbol(symbol: &Symbol, symbols: &SymbolTable) -> Result<Option<BuiltInSymbol>, ScriptError> {
    match symbol.into_builtin_or_error(symbols) {
        Ok(value) => Ok(Some(value)),
        Err(crate::player::symbols::symbol::SymbolError::NotBuiltin { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl ButtonMemberHandlers {
    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &crate::player::DatumRef,
        handler_name: Symbol,
        args: &Vec<crate::player::DatumRef>,
    ) -> Result<crate::player::DatumRef, ScriptError> {
        match builtin_symbol(&handler_name, symbols)? {
            Some(BuiltInSymbol::Count) => {
                let member_ref = checked_get_datum(player, datum, symbols)?.to_member_ref()?;
                let member = player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .unwrap();
                let button = member.member_type.as_button().unwrap();
                let count_of = checked_get_datum(player, &args[0], symbols)?.symbol_value(symbols)?;
                use crate::player::handlers::datum_handlers::string_chunk::StringChunkUtils;
                
                let delimiter = player.movie.item_delimiter;
                let count = StringChunkUtils::resolve_chunk_count(
                    &button.field.text,
                    StringChunkType::from_symbol(&count_of, symbols)?,
                    delimiter,
                )?;
                Ok(player.alloc_datum(Datum::Int(count as i32)))
            }
            _ => Err(ScriptError::new(format!(
                "No handler {} for button member",
                symbols.display(&handler_name).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        cast_member_ref: &CastMemberRef,
        prop: Symbol,
    ) -> Result<Datum, ScriptError> {
        let member = player
            .movie
            .cast_manager
            .find_member_by_ref(cast_member_ref)
            .unwrap();
        let button = member.member_type.as_button().unwrap();

        match builtin_symbol(&prop, symbols)? {
            Some(BuiltInSymbol::Text) => Ok(Datum::String(button.field.text.to_owned())),
            Some(BuiltInSymbol::Font) => Ok(Datum::String(button.field.font.to_owned())),
            Some(BuiltInSymbol::FontSize) => Ok(Datum::Int(button.field.font_size as i32)),
            Some(BuiltInSymbol::FontStyle) => Ok(Datum::String(button.field.font_style.to_string())),
            Some(BuiltInSymbol::Alignment) => Ok(Datum::String(button.field.alignment.to_string())),
            Some(BuiltInSymbol::Width) => Ok(Datum::Int(button.field.width as i32)),
            Some(BuiltInSymbol::Height) => Ok(Datum::Int(button.field.height as i32)),
            Some(BuiltInSymbol::Hilite) => Ok(datum_bool(button.hilite)),
            Some(BuiltInSymbol::ButtonType) => Ok(Datum::Symbol(Symbol::builtin(button.button_type.symbol()))),
            Some(BuiltInSymbol::ForeColor) => {
                match &button.field.fore_color {
                    Some(crate::player::sprite::ColorRef::Rgb(r, _, _)) => Ok(Datum::Int(*r as i32)),
                    Some(crate::player::sprite::ColorRef::PaletteIndex(idx)) => Ok(Datum::Int(*idx as i32)),
                    None => Ok(Datum::Int(255)),
                }
            }
            Some(BuiltInSymbol::WordWrap) => Ok(datum_bool(button.field.word_wrap)),
            Some(BuiltInSymbol::Border) => Ok(Datum::Int(button.field.border as i32)),
            Some(BuiltInSymbol::Editable) => Ok(datum_bool(button.field.editable)),
            _ => Err(ScriptError::new(format!(
                "Button member doesn't support property {}",
                symbols.display(&prop).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        member_ref: &CastMemberRef,
        prop: Symbol,
        value: Datum,
    ) -> Result<(), ScriptError> {
        crate::player::compare::validate_direct_symbol_fields(&value, symbols)?;
        match builtin_symbol(&prop, symbols)? {
            Some(BuiltInSymbol::Text) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.string_value(symbols),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().field.set_text_preserving_caret(value?.trim_end_matches('\0').to_string());
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::Hilite) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.bool_value(),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().hilite = value?;
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::ButtonType) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.string_value(symbols),
                |cast_member, value, symbols| {
                    let type_str = value?;
                    let button = cast_member.member_type.as_button_mut().unwrap();
                    match type_str.to_lowercase().as_str() {
                        "pushbutton" | "#pushbutton" => button.button_type = ButtonType::PushButton,
                        "checkbox" | "#checkbox" => button.button_type = ButtonType::CheckBox,
                        "radiobutton" | "#radiobutton" => button.button_type = ButtonType::RadioButton,
                        _ => return Err(ScriptError::new(format!("Unknown button type: {}", type_str))),
                    }
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::Font) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.string_value(symbols),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().field.font = value?;
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::FontSize) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.int_value(),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().field.font_size = value? as u16;
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::Alignment) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.string_value(symbols),
                |cast_member, value, symbols| {
                    let value = value?;
                    cast_member.member_type.as_button_mut().unwrap().field.alignment =
                        symbols.intern(&value).into_builtin_or_error(symbols)?;
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::Width) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.int_value(),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().field.width = value? as u16;
                    Ok(())
                },
            ),
            Some(BuiltInSymbol::Height) => borrow_member_mut_with_player(
                player,
                symbols,
                member_ref,
                |_player, symbols| value.int_value(),
                |cast_member, value, symbols| {
                    cast_member.member_type.as_button_mut().unwrap().field.height = value? as u16;
                    Ok(())
                },
            ),
            _ => Err(ScriptError::new(format!(
                "Cannot set button member prop {}",
                symbols.display(&prop).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }
}
