use log::warn;

use super::handler_manager::BytecodeHandlerContext;
use crate::player::handlers::datum_handlers::{
    list_handlers::ListDatumHandlers, sound_channel::SoundChannelDatumHandlers,
};
use crate::player::scope::StackDatum;
use crate::{
    director::lingo::{
        constants::{
            get_anim_prop_name, get_cast_member_prop_name, get_sound_prop_name,
            get_sprite_prop_name, movie_prop_names, sprite_prop_names,
        },
        datum::{Datum, StringChunkType},
    },
    player::{
        allocator::{DatumAllocatorTrait, ScriptInstanceAllocatorTrait},
        handlers::datum_handlers::{
            cast_member_ref::CastMemberRefHandlers, string_chunk::StringChunkUtils,
        },
        score::{sprite_get_prop, sprite_set_prop},
        script::{
            get_current_handler_def, get_obj_prop, script_get_prop, script_get_static_prop,
            script_set_prop, script_set_static_prop,
        },
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, HandlerExecutionResult, ScriptError, ScriptErrorCode,
    },
};

pub struct GetSetBytecodeHandler {}
pub struct GetSetUtils {}

#[inline]
fn checked_read_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    match datum_ref {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            )
        }),
    }
}

#[inline]
fn checked_property_value<'a>(
    player: &'a DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = checked_read_datum(player, datum_ref)?;
    player.validate_movie_datum_shallow(symbols, datum)?;
    Ok(datum)
}

impl GetSetUtils {
    pub fn get_the_built_in_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        ctx: &BytecodeHandlerContext,
        prop_name: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        symbols
            .lower(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_name.into_builtin() {
            Some(BuiltInSymbol::ParamCount) => Ok(player.alloc_datum(Datum::Int(
                player.scopes.get(ctx.scope_ref()).unwrap().args.len() as i32,
            ))),
            Some(BuiltInSymbol::Result) => {
                let result_ref = player.last_handler_result.clone();
                checked_property_value(player, symbols, &result_ref)?;
                Ok(result_ref)
            }
            Some(BuiltInSymbol::Pi) => Ok(player.alloc_datum(Datum::Float(std::f64::consts::PI))),
            _ => player.get_movie_prop(symbols, prop_name),
        }
    }

    pub fn set_the_built_in_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        prop_name: Symbol,
        value: Datum,
    ) -> Result<(), ScriptError> {
        symbols
            .lower(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_name {
            _ => player.set_movie_prop(symbols, prop_name, value),
        }
    }

    pub fn get_top_level_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        prop_name: Symbol,
    ) -> Result<Datum, ScriptError> {
        symbols
            .lower(&prop_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_name.into_builtin() {
            Some(BuiltInSymbol::_Player) => Ok(Datum::PlayerRef),
            Some(BuiltInSymbol::_Movie) => Ok(Datum::MovieRef),
            Some(BuiltInSymbol::_Mouse) => Ok(Datum::MouseRef),
            Some(BuiltInSymbol::_System) => Ok(Datum::MovieRef), // _system properties like randomSeed are movie-level
            Some(BuiltInSymbol::_Sound) => Ok(Datum::MovieRef), // _sound properties like soundDevice are movie-level
            Some(BuiltInSymbol::_Key) => Ok(Datum::PlayerRef), // _key properties handled via PlayerRef
            _ => Err(ScriptError::new(format!(
                "Invalid top level prop: {}",
                symbols.display(&prop_name).unwrap_or("<foreign symbol>")
            ))),
        }
    }
}

impl GetSetBytecodeHandler {
    /// Walk the receiver's ancestor chain to find the instance whose script
    /// matches the handler's script_ref. This is the "handler's owning instance"
    /// — the correct target for bare property access (getprop/setprop) in
    /// Director's prototypal inheritance model.
    fn find_handler_level_instance(
        player: &DirPlayer,
        receiver: &crate::player::script_ref::ScriptInstanceRef,
        script_ref: &crate::player::cast_lib::CastMemberRef,
    ) -> Result<crate::player::script_ref::ScriptInstanceRef, ScriptError> {
        let mut walk_ref = receiver.clone();
        for _ in 0..100 {
            let instance = player
                .allocator
                .get_script_instance_opt(&walk_ref)
                .ok_or_else(|| ScriptError::new("foreign or stale ScriptInstanceRef".to_owned()))?;
            if instance.script == *script_ref {
                return Ok(walk_ref);
            }
            let next = instance.ancestor.clone();
            match next {
                Some(ancestor_ref) => walk_ref = ancestor_ref,
                None => break,
            }
        }
        // Fallback to receiver if handler's script not found in chain
        Ok(receiver.clone())
    }

    pub fn get_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let name_id = player.get_ctx_current_bytecode(ctx).obj as u16;
            let prop_name = ctx.get_name(name_id).to_owned();
            symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let (receiver, script_ref, cached) = {
                let scope = player.scopes.get(ctx.scope_ref()).unwrap();
                (
                    scope.receiver.clone(),
                    scope.script_ref.clone(),
                    scope.cached_handler_instance.clone(),
                )
            };

            let result = if let Some(instance_ref) = receiver {
                // In Director, bare property access (getprop) resolves on the
                // handler's owning instance in the ancestor chain, not on the
                // top-level receiver.
                let handler_instance = if let Some(ref c) = cached {
                    player.allocator.get_script_instance_opt(c).ok_or_else(|| {
                        ScriptError::new("foreign or stale ScriptInstanceRef".to_owned())
                    })?;
                    c.clone()
                } else {
                    let hi = Self::find_handler_level_instance(player, &instance_ref, &script_ref)?;
                    player
                        .scopes
                        .get_mut(ctx.scope_ref())
                        .unwrap()
                        .cached_handler_instance = Some(hi.clone());
                    hi
                };
                let getter_result = script_get_prop(player, symbols, &handler_instance, prop_name);
                if !ctx.scope.validate_top(player) {
                    return Err(crate::player::cancelled_scope_error());
                }
                getter_result?
            } else {
                let getter_result = script_get_static_prop(player, symbols, &script_ref, prop_name);
                if !ctx.scope.validate_top(player) {
                    return Err(crate::player::cancelled_scope_error());
                }
                getter_result?
            };
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn set_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id).to_owned();

        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let (value_ref, receiver, script_ref, cached) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let value_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (
                    value_ref,
                    scope.receiver.clone(),
                    scope.script_ref.clone(),
                    scope.cached_handler_instance.clone(),
                )
            };

            match receiver {
                Some(instance_ref) => {
                    if *instance_ref == 0 {
                        return Err(ScriptError::new(format!(
                            "Can't set prop {} of Void",
                            symbols.display(&prop_name).map_err(|_| {
                                crate::player::symbols::symbol::SymbolError::Foreign
                            })?
                        )));
                    }
                    // Resolve on handler's owning instance level (see get_prop comment)
                    let handler_instance = if let Some(ref c) = cached {
                        c.clone()
                    } else {
                        let hi =
                            Self::find_handler_level_instance(player, &instance_ref, &script_ref)?;
                        player
                            .scopes
                            .get_mut(ctx.scope_ref())
                            .unwrap()
                            .cached_handler_instance = Some(hi.clone());
                        hi
                    };
                    script_set_prop(
                        player,
                        symbols,
                        &handler_instance,
                        prop_name,
                        &value_ref,
                        false,
                    )?;
                    Ok(HandlerExecutionResult::Advance)
                }
                None => {
                    script_set_static_prop(
                        player,
                        symbols,
                        &script_ref,
                        prop_name,
                        &value_ref,
                        true,
                    )?;
                    Ok(HandlerExecutionResult::Advance)
                }
            }
        }
    }

    pub fn get_obj_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let name_id = player.get_ctx_current_bytecode(ctx).obj as u16;
            let prop_name = ctx.get_name(name_id).to_owned();
            symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            // Pop the object reference from the stack
            let obj_datum_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };

            if let Some(Datum::XmlRef(xml_id)) = player.allocator.try_get_datum(&obj_datum_ref) {
                let getter_result =
                    crate::player::handlers::datum_handlers::xml::XmlDatumHandlers::get_prop(
                        player,
                        symbols,
                        &obj_datum_ref,
                        prop_name,
                    );

                if !ctx.scope.validate_top(player) {
                    return Err(crate::player::cancelled_scope_error());
                }
                let result_ref = getter_result?;
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.stack.push(result_ref);

                return Ok(HandlerExecutionResult::Advance);
            }

            let getter_result = get_obj_prop(player, symbols, &obj_datum_ref, prop_name);
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let result_ref = getter_result?;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);
            Ok(HandlerExecutionResult::Advance)
        }
    }

    pub fn get_movie_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        let result_ref = player.get_movie_prop(symbols, prop_name)?;
        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
        scope.stack.push(result_ref);
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn set(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let property_id_ref = scope
                .stack
                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                .unwrap();
            let property_id = player.allocator.get_datum(&property_id_ref).int_value()?;
            let value_ref = scope
                .stack
                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                .unwrap();
            let value = player.get_datum(&value_ref).clone();

            let property_type = player.get_ctx_current_bytecode(ctx).obj;
            match property_type {
                0x00 => {
                    if property_id <= 0x0b {
                        // movie prop
                        let prop_name = movie_prop_names().get(&(property_id as u16)).unwrap();
                        GetSetUtils::set_the_built_in_prop(
                            player,
                            symbols,
                            Symbol::builtin(*prop_name),
                            value,
                        )?;
                        Ok(HandlerExecutionResult::Advance)
                    } else {
                        // last chunk
                        Err(ScriptError::new(format!(
                            "Invalid propertyType/propertyID for kOpSet: {}",
                            property_type
                        )))
                    }
                }
                0x04 => {
                    // Sound channel properties
                    let prop_name = get_sound_prop_name(property_id as u16);
                    let channel_num_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let channel_num = player.get_datum(&channel_num_ref).int_value()?;

                    // Create a SoundChannel datum with the channel number
                    let sound_channel_datum =
                        player.alloc_datum(Datum::SoundChannel(channel_num as u16));

                    SoundChannelDatumHandlers::set_prop(
                        player,
                        symbols,
                        &sound_channel_datum,
                        Symbol::builtin(prop_name),
                        &value_ref,
                    )?;
                    Ok(HandlerExecutionResult::Advance)
                }
                0x06 => {
                    let prop_name = get_sprite_prop_name(property_id as u16);
                    let datum_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let sprite_num = player.get_datum(&datum_ref).int_value()?;
                    sprite_set_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        Symbol::builtin(prop_name),
                        value,
                    )?;
                    Ok(HandlerExecutionResult::Advance)
                }
                0x07 => {
                    let prop_name = get_anim_prop_name(property_id as u16);
                    player.set_movie_prop(symbols, Symbol::builtin(prop_name), value)?;
                    Ok(HandlerExecutionResult::Advance)
                }
                0x0b => {
                    // Field member property (e.g. set the foreColor of field X)
                    let cast_lib_datum = if player.movie.dir_version >= 500 {
                        let r = {
                            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                            scope
                                .stack
                                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                                .unwrap()
                        };
                        Some(player.get_datum(&r).clone())
                    } else {
                        None
                    };
                    let member_id_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let member_id_datum = player.get_datum(&member_id_ref).clone();
                    let prop_name = get_cast_member_prop_name(property_id as u16);
                    let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                        symbols,
                        &member_id_datum,
                        cast_lib_datum.as_ref(),
                        &player.allocator,
                    )?;
                    match member_ref {
                        Some(member_ref) => {
                            CastMemberRefHandlers::set_prop(
                                player,
                                symbols,
                                &member_ref,
                                prop_name.into(),
                                value,
                            )?;
                            Ok(HandlerExecutionResult::Advance)
                        }
                        None => {
                            warn!("set field prop '{}': member not found", prop_name);
                            Ok(HandlerExecutionResult::Advance)
                        }
                    }
                }
                0x0a | 0x0c => {
                    // Property with chunk expression. 0x0a = of a `member`
                    // (e.g. set the foreColor of word X of member Y); 0x0c =
                    // of a `field` (e.g. set the textStyle of line N of field
                    // X). Same stack layout: cast_lib (only if dir>=500),
                    // then the member/field id, then the 8 chunk-range
                    // values. The D4 client (issue-188) movie's `markLine`
                    // handler uses 0x0c to highlight the clicked topic line.
                    let cast_lib_datum = if player.movie.dir_version >= 500 {
                        let r = {
                            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                            scope
                                .stack
                                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                                .unwrap()
                        };
                        Some(player.get_datum(&r).clone())
                    } else {
                        None
                    };
                    let member_id_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let member_id_datum = player.get_datum(&member_id_ref).clone();
                    // Pop the 8 chunk-range values. Top of stack is last_line,
                    // then first_line, last_item, first_item, last_word,
                    // first_word, last_char, first_char (bottom).
                    let chunk_refs = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        let mut v = Vec::with_capacity(8);
                        for _ in 0..8 {
                            v.push(
                                scope
                                    .stack
                                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                                    .unwrap(),
                            );
                        }
                        v
                    };
                    let last_line = player.get_datum(&chunk_refs[0]).int_value().unwrap_or(0);
                    let first_line = player.get_datum(&chunk_refs[1]).int_value().unwrap_or(0);
                    let last_item = player.get_datum(&chunk_refs[2]).int_value().unwrap_or(0);
                    let first_item = player.get_datum(&chunk_refs[3]).int_value().unwrap_or(0);
                    let last_word = player.get_datum(&chunk_refs[4]).int_value().unwrap_or(0);
                    let first_word = player.get_datum(&chunk_refs[5]).int_value().unwrap_or(0);
                    let last_char = player.get_datum(&chunk_refs[6]).int_value().unwrap_or(0);
                    let first_char = player.get_datum(&chunk_refs[7]).int_value().unwrap_or(0);
                    let prop_name = get_cast_member_prop_name(property_id as u16);
                    let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                        symbols,
                        &member_id_datum,
                        cast_lib_datum.as_ref(),
                        &player.allocator,
                    )?;
                    match member_ref {
                        Some(member_ref) => {
                            use crate::director::lingo::datum::{StringChunkExpr, StringChunkType};
                            let chunk_expr: Option<StringChunkExpr> =
                                if first_char != 0 || last_char != 0 {
                                    Some(StringChunkExpr {
                                        chunk_type: StringChunkType::Char,
                                        start: first_char,
                                        end: last_char,
                                        item_delimiter: player.movie.item_delimiter,
                                    })
                                } else if first_word != 0 || last_word != 0 {
                                    Some(StringChunkExpr {
                                        chunk_type: StringChunkType::Word,
                                        start: first_word,
                                        end: last_word,
                                        item_delimiter: player.movie.item_delimiter,
                                    })
                                } else if first_item != 0 || last_item != 0 {
                                    Some(StringChunkExpr {
                                        chunk_type: StringChunkType::Item,
                                        start: first_item,
                                        end: last_item,
                                        item_delimiter: player.movie.item_delimiter,
                                    })
                                } else if first_line != 0 || last_line != 0 {
                                    Some(StringChunkExpr {
                                        chunk_type: StringChunkType::Line,
                                        start: first_line,
                                        end: last_line,
                                        item_delimiter: player.movie.item_delimiter,
                                    })
                                } else {
                                    None
                                };

                            let lc = prop_name.to_ascii_lowercase();
                            let is_style_prop = lc == "textstyle" || lc == "fontstyle";
                            // Per-character style set (`set the textStyle of
                            // line N of field` and friends): translate the
                            // chunk to a byte range and rewrite the field's
                            // STXT formatting runs. Whole-field set (no chunk
                            // expr) also resets font_style so the getter and
                            // any later uniform read agree.
                            let handled = if is_style_prop {
                                use crate::player::cast_member::{
                                    text_style_string_to_byte, CastMemberType,
                                };
                                use crate::player::handlers::datum_handlers::string_chunk::StringChunkHandlers;
                                crate::player::compare::validate_direct_symbol_fields(
                                    &value, symbols,
                                )?;
                                let new_style = text_style_string_to_byte(
                                    &value.string_value(symbols).unwrap_or_default(),
                                );
                                if let Some(member) = player
                                    .movie
                                    .cast_manager
                                    .find_mut_member_by_ref(&member_ref)
                                {
                                    if let CastMemberType::Field(field) = &mut member.member_type {
                                        let text = field.text.clone();
                                        let (char_start, char_end) = match &chunk_expr {
                                            Some(ce) => {
                                                StringChunkHandlers::resolve_chunk_char_range(
                                                    &text, ce,
                                                )
                                            }
                                            None => (0usize, text.chars().count()),
                                        };
                                        // STXT runs use BYTE positions; map the
                                        // char range to byte offsets (text may
                                        // contain multi-byte UTF-8 chars).
                                        let byte_start = text
                                            .char_indices()
                                            .nth(char_start)
                                            .map(|(b, _)| b)
                                            .unwrap_or_else(|| text.len())
                                            as u32;
                                        let byte_end = text
                                            .char_indices()
                                            .nth(char_end)
                                            .map(|(b, _)| b)
                                            .unwrap_or_else(|| text.len())
                                            as u32;
                                        field.apply_style_to_byte_range(
                                            byte_start, byte_end, new_style,
                                        );
                                        if chunk_expr.is_none() {
                                            field.font_style = value
                                                .string_value(symbols)
                                                .unwrap_or_else(|_| "plain".to_string());
                                        }
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            if !handled {
                                // Non-style prop, or not a field: fall back to
                                // the member-wide setter (chunk info ignored,
                                // matching the prior behaviour for these).
                                CastMemberRefHandlers::set_prop(
                                    player,
                                    symbols,
                                    &member_ref,
                                    Symbol::builtin(prop_name),
                                    value,
                                )?;
                            }
                            Ok(HandlerExecutionResult::Advance)
                        }
                        None => {
                            warn!(
                                "set cast member chunk prop '{}': member not found",
                                prop_name
                            );
                            Ok(HandlerExecutionResult::Advance)
                        }
                    }
                }
                0x09 => {
                    // Cast member property (kTheCast)
                    // Stack has: [member_id, castLib(D5+), value(popped), prop_id(popped)]
                    let cast_lib_datum = if player.movie.dir_version >= 500 {
                        let r = {
                            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                            scope
                                .stack
                                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                                .unwrap()
                        };
                        Some(player.get_datum(&r).clone())
                    } else {
                        None
                    };
                    let member_id_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let member_id_datum = player.get_datum(&member_id_ref).clone();
                    let prop_name = get_cast_member_prop_name(property_id as u16);
                    let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                        symbols,
                        &member_id_datum,
                        cast_lib_datum.as_ref(),
                        &player.allocator,
                    )?;
                    match member_ref {
                        Some(member_ref) => {
                            CastMemberRefHandlers::set_prop(
                                player,
                                symbols,
                                &member_ref,
                                prop_name.into(),
                                value,
                            )?;
                            Ok(HandlerExecutionResult::Advance)
                        }
                        None => {
                            warn!("set cast member prop '{}': member not found", prop_name);
                            Ok(HandlerExecutionResult::Advance)
                        }
                    }
                }
                _ => Err(ScriptError::new(format!(
                    "Invalid propertyType/propertyID for kOpSet: {}",
                    property_type
                ))),
            }
        }
    }

    pub fn get_global(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        runtime.with_player_and_symbols(|player, symbols| {
            let prop_display = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let value_ref = match player.globals.get(&prop_name) {
                Some(v) => v.clone(),
                None => {
                    // Lingo globals are case-insensitive. An Xtra may write a global
                    // in a different case than a script reads it — 3D Groove's
                    // GetCollide writes `collidez` while leo3d reads `CollideZ` — so
                    // fall back to a case-insensitive match before yielding VOID.
                    // Only runs on a miss, so exact hits (constants like PI) are
                    // unaffected.
                    let mut value_ref = DatumRef::Void;
                    for (key, value) in &player.globals {
                        if symbols
                            .lower(key)
                            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                            .eq_ignore_ascii_case(prop_display)
                        {
                            value_ref = value.clone();
                            break;
                        }
                    }
                    value_ref
                }
            };
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(value_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn set_global(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        runtime.with_player(|player| {
            let value_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };

            // In Lingo, strings are value types and should be copied when assigned
            // to prevent mutations from affecting the original variable
            let value_ref = match player.get_datum(&value_ref) {
                Datum::String(s) => player.alloc_datum(Datum::String(s.clone())),
                _ => value_ref,
            };

            player.globals.insert(prop_name.to_owned(), value_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn get_field(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            let cast_id_ref = if player.movie.dir_version >= 500 {
                let cast_id_ref = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                Some(cast_id_ref)
            } else {
                None
            };
            let field_name_or_num_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };
            let cast_id = if let Some(cast_id_ref) = cast_id_ref {
                player.get_datum(&cast_id_ref)
            } else {
                &Datum::Int(0)
            };
            let field_name_or_num = player.get_datum(&field_name_or_num_ref);

            let field_value = player.movie.cast_manager.get_field_value_by_identifiers(
                symbols,
                field_name_or_num,
                Some(cast_id),
                &player.allocator,
            )?;
            let result_id = player.alloc_datum(Datum::String(field_value));

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        }
    }

    pub fn get_local(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            // `name_int` IS the dense slot; the old code mapped it through
            // `local_name_ids` only to build a hash key.
            let slot = (player.get_ctx_current_bytecode(ctx).obj as u32 / ctx.multiplier) as usize;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let value = scope.local(slot);
            scope.stack.push_value(value);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn set_local(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let slot = (player.get_ctx_current_bytecode(ctx).obj as u32 / ctx.multiplier) as usize;

            let value = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.stack.pop_value().unwrap_or(StackDatum::Void)
            };

            // In Lingo, strings are value types and should be copied when
            // assigned, so a later mutation cannot reach the original. Only a
            // `Ref` can be a string, so the inline int/symbol/void cases skip
            // the arena lookup this used to do unconditionally.
            let value = match value {
                StackDatum::Ref(dr) => match player.get_datum(&dr) {
                    Datum::String(s) => {
                        let copy = Datum::String(s.clone());
                        StackDatum::Ref(player.alloc_datum(copy))
                    }
                    _ => StackDatum::Ref(dr),
                },
                other => other,
            };

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.set_local(slot, value);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn get_param(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let param_number = player.get_ctx_current_bytecode(ctx).obj as u32 / ctx.multiplier;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let result = scope
                .args
                .get(param_number as usize)
                .unwrap_or(&DatumRef::Void)
                .clone();
            scope.stack.push(result);
        });
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn set_param(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let bytecode_obj = player.get_ctx_current_bytecode(ctx).obj as u32 / ctx.multiplier;
            let (arg_count, arg_index, value_ref) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let arg_count = scope.args.len();
                let arg_index = bytecode_obj as usize;
                let value_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (arg_count, arg_index, value_ref)
            };

            // In Lingo, strings are value types and should be copied when assigned
            // to prevent mutations from affecting the original variable
            let value_ref = match player.get_datum(&value_ref) {
                Datum::String(s) => player.alloc_datum(Datum::String(s.clone())),
                _ => value_ref,
            };

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            if arg_index < scope.args.len() {
                scope.args[arg_index] = value_ref;
                Ok(HandlerExecutionResult::Advance)
            } else {
                scope.args.resize(arg_count.max(arg_index), DatumRef::Void);
                scope.args.insert(arg_index, value_ref);
                Ok(HandlerExecutionResult::Advance)
            }
        })
    }

    pub fn set_movie_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        runtime.with_player_and_symbols(|player, symbols| {
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let value_ref = scope
                .stack
                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                .unwrap();
            // Resolve the owning datum reference before dispatch, but let the
            // movie setter preserve ignored/no-op assignments without eagerly
            // inspecting a value that it will discard.
            let value = checked_read_datum(player, &value_ref)?.clone();
            player.set_movie_prop(symbols, prop_name, value)?;
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn the_built_in(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            let result_id = GetSetUtils::get_the_built_in_prop(player, symbols, ctx, prop_name)?;

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope
                .stack
                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager); // empty arglist
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        }
    }

    pub fn get_chained_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let name_id = player.get_ctx_current_bytecode(ctx).obj as u16;
            let prop_name = ctx.get_name(name_id).to_owned();
            let prop_name_text = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned();
            let obj_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap_or(DatumRef::Void)
            };

            // Clone the datum type first
            let obj_type = checked_read_datum(player, &obj_ref)?.type_enum();

            // Check if prop_name is a numeric index
            let is_numeric_index = prop_name_text.parse::<i32>().is_ok();

            let result_ref = match obj_type {
                crate::director::lingo::datum::DatumType::SpriteRef => {
                    // Handle sprite references
                    let sprite_num = player.get_datum(&obj_ref).to_sprite_ref()?;

                    // Try built-in sprite properties FIRST
                    // This ensures properties like 'visible', 'loc', etc. work correctly
                    let sprite_result = crate::player::score::sprite_get_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        prop_name.clone(),
                    );
                    if !ctx.scope.validate_top(player) {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    match sprite_result {
                        Ok(datum) => {
                            let result = player
                                .last_sprite_prop_ref
                                .take()
                                .unwrap_or_else(|| player.alloc_datum(datum));
                            if !ctx.scope.validate_top(player) {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                            scope.stack.push(result);
                            return Ok(HandlerExecutionResult::Advance);
                        }
                        Err(_) => {
                            // Not a built-in property, try script instances
                            // Clone the script instance list to avoid borrow issues
                            let instance_refs = {
                                let sprite = player.movie.score.get_sprite(sprite_num);
                                sprite
                                    .map(|s| s.script_instance_list.clone())
                                    .unwrap_or_default()
                            };

                            // Try to get the property from script instances
                            for instance_ref in instance_refs {
                                let getter_result = crate::player::script::script_get_prop(
                                    player,
                                    symbols,
                                    &instance_ref,
                                    prop_name.clone(),
                                );
                                if !ctx.scope.validate_top(player) {
                                    return Err(crate::player::cancelled_scope_error());
                                }
                                let result = getter_result?;
                                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                                scope.stack.push(result);
                                return Ok(HandlerExecutionResult::Advance);
                            }

                            // Property not found anywhere
                            if !ctx.scope.validate_top(player) {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                            scope.stack.push(DatumRef::Void);
                            return Ok(HandlerExecutionResult::Advance);
                        }
                    }
                }
                crate::director::lingo::datum::DatumType::XmlRef => {
                    let getter_result =
                        crate::player::handlers::datum_handlers::xml::XmlDatumHandlers::get_prop(
                            player, symbols, &obj_ref, prop_name,
                        );
                    if !ctx.scope.validate_top(player) {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    getter_result?
                }
                crate::director::lingo::datum::DatumType::String => {
                    match prop_name.into_builtin() {
                        Some(BuiltInSymbol::Length) => {
                            let len = if let Datum::String(s) = player.get_datum(&obj_ref) {
                                s.chars().count() as i32
                            } else {
                                unreachable!()
                            };
                            player.alloc_datum(Datum::Int(len))
                        }
                        Some(BuiltInSymbol::Char)
                        | Some(BuiltInSymbol::Line)
                        | Some(BuiltInSymbol::Word)
                        | Some(BuiltInSymbol::Item) => {
                            // `string.char` / `.line` / `.word` / `.item` (no
                            // index) returns a StringChunk that represents
                            // the *collection* of chunks of that kind. The
                            // chunk_expr covers all chunks (start=1, end=N),
                            // so `.count` returns N and `[i]` indexes the
                            // i-th chunk. Without this, scripts that do
                            // `member.text.line.count` / `text.line[i]` hit
                            // the catch-all and error with "Invalid string
                            // built-in property line".
                            use crate::director::lingo::datum::{StringChunkExpr, StringChunkType};
                            use crate::player::handlers::datum_handlers::string_chunk::StringChunkUtils;
                            let s_clone = if let Datum::String(s) = player.get_datum(&obj_ref) {
                                s.clone()
                            } else {
                                unreachable!()
                            };
                            let chunk_type = StringChunkType::from_symbol(&prop_name, symbols)?;
                            let delim = player.movie.item_delimiter;
                            let count = StringChunkUtils::resolve_chunk_count(
                                &s_clone,
                                chunk_type.clone(),
                                delim,
                            )? as i32;
                            let chunk_expr = StringChunkExpr {
                                chunk_type,
                                start: 1,
                                end: count.max(1),
                                item_delimiter: delim,
                            };
                            player.alloc_datum(Datum::StringChunk(
                                crate::director::lingo::datum::StringChunkSource::Datum(
                                    obj_ref.clone(),
                                ),
                                chunk_expr,
                                s_clone,
                            ))
                        }
                        _ => {
                            let getter_result =
                                get_obj_prop(player, symbols, &obj_ref, prop_name.clone());
                            if !ctx.scope.validate_top(player) {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            let result = getter_result?;
                            // Track sub-property refs for Transform3D compound assignment
                            // (e.g., transform.position.z = value needs to write back to transform)
                            let is_transform = matches!(
                                obj_type,
                                crate::director::lingo::datum::DatumType::Transform3d
                            );
                            let is_sub_prop = matches!(
                                prop_name_text.to_ascii_lowercase().as_str(),
                                "position" | "rotation" | "scale" | "x" | "y" | "z"
                            );
                            if is_transform && is_sub_prop {
                                let result_type = checked_read_datum(player, &result)?.type_enum();
                                if matches!(
                                    result_type,
                                    crate::director::lingo::datum::DatumType::Vector
                                ) {
                                    if player.transform_sub_refs.len() > 32 {
                                        player.transform_sub_refs.drain(0..16);
                                    }
                                    player.transform_sub_refs.push((
                                        result.clone(),
                                        obj_ref.clone(),
                                        prop_name,
                                    ));
                                }
                            }
                            result
                        }
                    }
                }
                crate::director::lingo::datum::DatumType::List => {
                    // Handle numeric indices for lists
                    if is_numeric_index {
                        let index = prop_name_text.parse::<i32>().unwrap();
                        if let Datum::List(_, list, _) = player.get_datum(&obj_ref) {
                            // Lingo uses 1-based indexing
                            let zero_based_index = (index - 1) as usize;
                            if zero_based_index < list.len() {
                                list[zero_based_index].clone()
                            } else {
                                return Err(ScriptError::new(format!(
                                    "List index {} out of bounds (list has {} items)",
                                    index,
                                    list.len()
                                )));
                            }
                        } else {
                            unreachable!()
                        }
                    } else {
                        // Route all property access to ListDatumHandlers
                        let getter_result =
                            ListDatumHandlers::get_prop(player, symbols, &obj_ref, prop_name);
                        if !ctx.scope.validate_top(player) {
                            return Err(crate::player::cancelled_scope_error());
                        }
                        getter_result?
                    }
                }
                crate::director::lingo::datum::DatumType::PropList => {
                    let (entries, is_sorted) = {
                        let (entries, is_sorted) = player.get_datum(&obj_ref).to_map_tuple()?;
                        (entries.clone(), is_sorted)
                    };
                    let getter_result = crate::player::handlers::datum_handlers::prop_list::PropListUtils::get_prop_or_built_in(
                        player,
                        symbols,
                        &entries,
                        prop_name.clone(),
                        is_sorted,
                    );
                    if !ctx.scope.validate_top(player) {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    getter_result?
                }
                crate::director::lingo::datum::DatumType::ScriptInstanceRef => {
                    // If it's a numeric index, try to find a default indexable property
                    if is_numeric_index {
                        // Common indexable properties in Director scripts
                        let indexable_property_names = vec![
                            BuiltInSymbol::ASquares,
                            BuiltInSymbol::List,
                            BuiltInSymbol::Items,
                            BuiltInSymbol::Data,
                        ];

                        let mut found_indexable = None;
                        for prop in indexable_property_names {
                            let getter_result =
                                get_obj_prop(player, symbols, &obj_ref, Symbol::builtin(prop));
                            if !ctx.scope.validate_top(player) {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            let prop_ref = getter_result?;
                            // Check if this property is a list. Void is the
                            // valid miss sentinel for an absent script prop.
                            if matches!(
                                checked_read_datum(player, &prop_ref)?,
                                Datum::List(_, _, _)
                            ) {
                                found_indexable = Some(prop_ref);
                                break;
                            }
                        }

                        if let Some(list_ref) = found_indexable {
                            // Now index into the list
                            let index = prop_name_text.parse::<i32>().unwrap();
                            if let Datum::List(_, list, _) = player.get_datum(&list_ref) {
                                // Lingo uses 1-based indexing
                                let zero_based_index = (index - 1) as usize;
                                if zero_based_index < list.len() {
                                    list[zero_based_index].clone()
                                } else {
                                    return Err(ScriptError::new(format!(
                                        "List index {} out of bounds (list has {} items)",
                                        index,
                                        list.len()
                                    )));
                                }
                            } else {
                                return Err(ScriptError::new(format!(
                                    "Internal error: Property was a list but now isn't"
                                )));
                            }
                        } else {
                            return Err(ScriptError::new(format!(
                                "Cannot use numeric index '{}' on script instance - no indexable property found (tried: aSquares, list, items, data)",
                                prop_name_text
                            )));
                        }
                    } else {
                        // Regular property access
                        let getter_result = get_obj_prop(player, symbols, &obj_ref, prop_name);
                        if !ctx.scope.validate_top(player) {
                            return Err(crate::player::cancelled_scope_error());
                        }
                        getter_result?
                    }
                }
                _ => {
                    let getter_result = get_obj_prop(player, symbols, &obj_ref, prop_name.clone());
                    if !ctx.scope.validate_top(player) {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    let result = getter_result?;
                    // Track sub-property refs for Transform3D compound assignment
                    // (e.g., transform.position.z = value needs to write back to transform)
                    if matches!(
                        obj_type,
                        crate::director::lingo::datum::DatumType::Transform3d
                    ) {
                        if matches!(
                            prop_name_text.to_ascii_lowercase().as_str(),
                            "position" | "rotation" | "scale"
                        ) {
                            if !ctx.scope.validate_top(player) {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            let result_type = checked_read_datum(player, &result)?.type_enum();
                            if matches!(
                                result_type,
                                crate::director::lingo::datum::DatumType::Vector
                            ) {
                                if player.transform_sub_refs.len() > 32 {
                                    player.transform_sub_refs.drain(0..16);
                                }
                                player.transform_sub_refs.push((
                                    result.clone(),
                                    obj_ref.clone(),
                                    prop_name.clone(),
                                ));
                            }
                        }
                    }
                    result
                }
            };

            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);
            Ok(HandlerExecutionResult::Advance)
        }
    }

    pub fn get(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            let prop_id = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };
            let prop_id = player.get_datum(&prop_id).int_value()?;
            let prop_type = player.get_ctx_current_bytecode(ctx).obj;
            let max_movie_prop_id = *movie_prop_names().keys().max().unwrap();

            let result = if prop_type == 0 && prop_id <= max_movie_prop_id as i32 {
                // movie prop
                let prop_name = movie_prop_names().get(&(prop_id as u16)).unwrap();
                GetSetUtils::get_the_built_in_prop(
                    player,
                    symbols,
                    ctx,
                    Symbol::builtin(*prop_name),
                )
            } else if prop_type == 0 {
                // last chunk
                let string_id = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                let string = player.get_datum(&string_id).string_value(symbols)?;
                let chunk_type = StringChunkType::from(&(prop_id - 0x0b));
                let last_chunk = StringChunkUtils::resolve_last_chunk(
                    &string,
                    chunk_type,
                    player.movie.item_delimiter,
                )?;

                Ok(player.alloc_datum(Datum::String(last_chunk)))
            } else if prop_type == 0x06 {
                // sprite prop
                let prop_name = sprite_prop_names().get(&(prop_id as u16));
                if prop_name.is_some() {
                    let datum_ref = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let sprite_num = player.get_datum(&datum_ref).int_value()?;
                    let result = sprite_get_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        Symbol::builtin(*prop_name.unwrap()),
                    )?;
                    Ok(player
                        .last_sprite_prop_ref
                        .take()
                        .unwrap_or_else(|| player.alloc_datum(result)))
                } else {
                    Err(ScriptError::new(format!(
                        "kOpGet sprite prop {} not implemented",
                        prop_id
                    )))
                }
            } else if prop_type == 0x07 {
                // anim prop
                let anim_datum = player.get_anim_prop(prop_id as u16)?;
                Ok(player.alloc_datum(anim_datum))
            } else if prop_type == 0x08 {
                // anim2 prop
                let datum = if prop_id == 0x02 && player.movie.dir_version >= 500 {
                    // the number of castMembers supports castLib selection from Director 5.0
                    let cast_lib_id = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    let cast_lib_id = player.get_datum(&cast_lib_id);
                    let bypass_castlib_selection =
                        cast_lib_id.is_int() && cast_lib_id.int_value()? == 0;
                    if bypass_castlib_selection {
                        player.get_anim2_prop(prop_id as u16)?
                    } else {
                        let cast = {
                            if cast_lib_id.is_string() {
                                player
                                    .movie
                                    .cast_manager
                                    .get_cast_by_name(&cast_lib_id.string_value(symbols)?)
                            } else {
                                player
                                    .movie
                                    .cast_manager
                                    .get_cast_or_null(cast_lib_id.int_value()? as u32)
                            }
                        };
                        match cast {
                            Some(cast) => Datum::Int(cast.max_member_id() as i32),
                            None => return Err(ScriptError::new(format!("kOpSet cast not found"))),
                        }
                    }
                } else {
                    player.get_anim2_prop(prop_id as u16)?
                };
                Ok(player.alloc_datum(datum))
            } else if prop_type == 0x09 {
                // Cast member property (kTheCast)
                let cast_lib_datum = if player.movie.dir_version >= 500 {
                    let r = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    Some(player.get_datum(&r).clone())
                } else {
                    None
                };
                let member_id_ref = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                let member_id_datum = player.get_datum(&member_id_ref).clone();
                let prop_name = Symbol::builtin(get_cast_member_prop_name(prop_id as u16));
                let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                    symbols,
                    &member_id_datum,
                    cast_lib_datum.as_ref(),
                    &player.allocator,
                )?;
                match member_ref {
                    Some(member_ref) => {
                        let result = CastMemberRefHandlers::get_prop(
                            player,
                            symbols,
                            &member_ref,
                            prop_name,
                        )?;
                        Ok(player.alloc_datum(result))
                    }
                    None => {
                        warn!(
                            "get cast member prop '{}': member not found",
                            symbols.display(&prop_name).map_err(|_| {
                                crate::player::symbols::symbol::SymbolError::Foreign
                            })?
                        );
                        Ok(player.alloc_datum(Datum::Void))
                    }
                }
            } else if prop_type == 0x0a || prop_type == 0x0c {
                // Cast member property with chunk expression (e.g. the
                // textStyle of char X of member Y). prop_type 0x0c is the
                // FIELD analog (`the textStyle of line N of field X`) — same
                // stack layout (cast_lib only if dir>=500, then the
                // field/member id, then the 8 chunk-range values). In D4 the
                // client movie's `markLine` handler reads `the textStyle of
                // line curline of field fieldname` via 0x0c; resolving the
                // field name string through find_member_ref_by_identifiers
                // reaches the same Field/Text member, so both types share
                // this path. The previous version
                // discarded all 8 chunk refs and read the MEMBER-wide
                // property, which made `the textStyle of char N of member`
                // collapse to the member's overall font_style — Fugue No.4
                // Narrative's underline-detection loop saw "plain" for
                // every char and never routed clicks to underlined words.
                let cast_lib_datum = if player.movie.dir_version >= 500 {
                    let r = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    Some(player.get_datum(&r).clone())
                } else {
                    None
                };
                let member_id_ref = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                let member_id_datum = player.get_datum(&member_id_ref).clone();
                // Pop the 8 chunk-range values. Top of stack is last_line, then
                // first_line, last_item, first_item, last_word, first_word,
                // last_char, first_char (bottom).
                let chunk_refs = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    let mut v = Vec::with_capacity(8);
                    for _ in 0..8 {
                        v.push(
                            scope
                                .stack
                                .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                                .unwrap(),
                        );
                    }
                    v
                };
                let last_line = player.get_datum(&chunk_refs[0]).int_value().unwrap_or(0);
                let first_line = player.get_datum(&chunk_refs[1]).int_value().unwrap_or(0);
                let last_item = player.get_datum(&chunk_refs[2]).int_value().unwrap_or(0);
                let first_item = player.get_datum(&chunk_refs[3]).int_value().unwrap_or(0);
                let last_word = player.get_datum(&chunk_refs[4]).int_value().unwrap_or(0);
                let first_word = player.get_datum(&chunk_refs[5]).int_value().unwrap_or(0);
                let last_char = player.get_datum(&chunk_refs[6]).int_value().unwrap_or(0);
                let first_char = player.get_datum(&chunk_refs[7]).int_value().unwrap_or(0);
                let prop_name = Symbol::builtin(get_cast_member_prop_name(prop_id as u16));
                let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                    symbols,
                    &member_id_datum,
                    cast_lib_datum.as_ref(),
                    &player.allocator,
                )?;
                match member_ref {
                    Some(member_ref) => {
                        // Determine which chunk type is active (only one
                        // pair is non-zero in Director's bytecode for
                        // chunked prop reads).
                        use crate::director::lingo::datum::{StringChunkExpr, StringChunkType};
                        let chunk_expr: Option<StringChunkExpr> =
                            if first_char != 0 || last_char != 0 {
                                Some(StringChunkExpr {
                                    chunk_type: StringChunkType::Char,
                                    start: first_char,
                                    end: last_char,
                                    item_delimiter: player.movie.item_delimiter,
                                })
                            } else if first_word != 0 || last_word != 0 {
                                Some(StringChunkExpr {
                                    chunk_type: StringChunkType::Word,
                                    start: first_word,
                                    end: last_word,
                                    item_delimiter: player.movie.item_delimiter,
                                })
                            } else if first_item != 0 || last_item != 0 {
                                Some(StringChunkExpr {
                                    chunk_type: StringChunkType::Item,
                                    start: first_item,
                                    end: last_item,
                                    item_delimiter: player.movie.item_delimiter,
                                })
                            } else if first_line != 0 || last_line != 0 {
                                Some(StringChunkExpr {
                                    chunk_type: StringChunkType::Line,
                                    start: first_line,
                                    end: last_line,
                                    item_delimiter: player.movie.item_delimiter,
                                })
                            } else {
                                None
                            };

                        // For per-char style props (textStyle / fontStyle),
                        // read the active STXT formatting run directly.
                        // Director 11.5 Scripting Dictionary p.949: returns
                        // the style of the character at position N.
                        let chunk_result: Option<Datum> = if let Some(ref ce) = chunk_expr {
                            let lc = symbols
                                .display(&prop_name)
                                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                                .to_ascii_lowercase();
                            use crate::player::cast_member::CastMemberType;
                            use crate::player::handlers::datum_handlers::string_chunk::{
                                StringChunkHandlers, StringChunkUtils,
                            };
                            if lc == "textstyle" || lc == "fontstyle" {
                                if let Some(member) =
                                    player.movie.cast_manager.find_member_by_ref(&member_ref)
                                {
                                    let text = match &member.member_type {
                                        CastMemberType::Field(f) => f.text.clone(),
                                        CastMemberType::Text(t) => t.text.clone(),
                                        _ => String::new(),
                                    };
                                    let (char_start, _char_end) =
                                        StringChunkHandlers::resolve_chunk_char_range(&text, ce);
                                    // STXT formatting_runs use BYTE positions
                                    // in Director's text data, not char
                                    // positions. For text with multi-byte
                                    // UTF-8 chars (ñ, é, ç in Fugue No.4's
                                    // Spanish/Portuguese headers and body),
                                    // converting char_start to its byte
                                    // offset is required so the run boundary
                                    // lookup picks the run that contains the
                                    // visible character — without this,
                                    // chars after the first multi-byte char
                                    // get the wrong style and `chunk =
                                    // "Español"` ends up comparing the
                                    // 8-char slice "Español " against
                                    // the 7-char literal.
                                    let byte_start: usize = text
                                        .char_indices()
                                        .nth(char_start)
                                        .map(|(b, _)| b)
                                        .unwrap_or_else(|| text.len());
                                    let (bold, italic, underline) = match &member.member_type {
                                        CastMemberType::Field(f) => {
                                            let style = f
                                                .formatting_runs
                                                .iter()
                                                .rev()
                                                .find(|r| (r.start_position as usize) <= byte_start)
                                                .map(|r| r.style)
                                                .unwrap_or(0);
                                            (
                                                (style & 0x01) != 0,
                                                (style & 0x02) != 0,
                                                (style & 0x04) != 0,
                                            )
                                        }
                                        CastMemberType::Text(t) => {
                                            let mut pos = 0usize;
                                            let mut found = (false, false, false);
                                            for span in &t.html_styled_spans {
                                                let len = span.text.chars().count();
                                                if char_start < pos + len {
                                                    found = (
                                                        span.style.bold,
                                                        span.style.italic,
                                                        span.style.underline,
                                                    );
                                                    break;
                                                }
                                                pos += len;
                                            }
                                            found
                                        }
                                        _ => (false, false, false),
                                    };
                                    // For CHUNKED access (`the textStyle of
                                    // char N` or `the fontStyle of char N`)
                                    // Director returns a STRING form.
                                    let mut parts: Vec<&str> = Vec::new();
                                    if bold {
                                        parts.push("bold");
                                    }
                                    if italic {
                                        parts.push("italic");
                                    }
                                    if underline {
                                        parts.push("underline");
                                    }
                                    let s = if parts.is_empty() {
                                        "plain".to_string()
                                    } else {
                                        parts.join(",")
                                    };
                                    let _ = lc;
                                    Some(Datum::String(s))
                                } else {
                                    None
                                }
                            } else if lc == "text" {
                                if let Some(member) =
                                    player.movie.cast_manager.find_member_by_ref(&member_ref)
                                {
                                    let text = match &member.member_type {
                                        CastMemberType::Field(f) => f.text.clone(),
                                        CastMemberType::Text(t) => t.text.clone(),
                                        _ => String::new(),
                                    };
                                    let s = StringChunkUtils::resolve_chunk_expr_string(&text, ce)?;
                                    Some(Datum::String(s))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(d) = chunk_result {
                            Ok(player.alloc_datum(d))
                        } else {
                            // Other props (foreColor, name, etc.) keep
                            // current member-wide behaviour.
                            let result = CastMemberRefHandlers::get_prop(
                                player,
                                symbols,
                                &member_ref,
                                prop_name,
                            )?;
                            Ok(player.alloc_datum(result))
                        }
                    }
                    None => {
                        warn!(
                            "get cast member chunk prop '{}': member not found",
                            symbols.display(&prop_name).map_err(|_| {
                                crate::player::symbols::symbol::SymbolError::Foreign
                            })?
                        );
                        Ok(player.alloc_datum(Datum::Void))
                    }
                }
            } else if prop_type == 0x0b {
                // Field member property (e.g. the foreColor of field X)
                let cast_lib_datum = if player.movie.dir_version >= 500 {
                    let r = {
                        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                        scope
                            .stack
                            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                            .unwrap()
                    };
                    Some(player.get_datum(&r).clone())
                } else {
                    None
                };
                let member_id_ref = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                let member_id_datum = player.get_datum(&member_id_ref).clone();
                let prop_name = Symbol::builtin(get_cast_member_prop_name(prop_id as u16));
                let member_ref = player.movie.cast_manager.find_member_ref_by_identifiers(
                    symbols,
                    &member_id_datum,
                    cast_lib_datum.as_ref(),
                    &player.allocator,
                )?;
                match member_ref {
                    Some(member_ref) => {
                        let result = CastMemberRefHandlers::get_prop(
                            player,
                            symbols,
                            &member_ref,
                            prop_name,
                        )?;
                        Ok(player.alloc_datum(result))
                    }
                    None => {
                        warn!(
                            "get field prop '{}': member not found",
                            symbols.display(&prop_name).map_err(|_| {
                                crate::player::symbols::symbol::SymbolError::Foreign
                            })?
                        );
                        Ok(player.alloc_datum(Datum::Void))
                    }
                }
            } else if prop_type == 0x01 {
                // number of chunks
                let string_id = {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope
                        .stack
                        .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                        .unwrap()
                };
                let string = player.get_datum(&string_id).string_value(symbols)?;
                let chunk_type = StringChunkType::from(&prop_id);
                let chunks = StringChunkUtils::resolve_chunk_list(
                    &string,
                    chunk_type,
                    player.movie.item_delimiter,
                )?;
                Ok(player.alloc_datum(Datum::Int(chunks.len() as i32)))
            } else {
                Err(ScriptError::new(format!(
                    "OpCode.kOpGet call not implemented propertyID={} propertyType={}",
                    prop_id, prop_type
                )))
            }?;

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result);
            Ok(HandlerExecutionResult::Advance)
        }
    }

    pub fn get_top_level_prop(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let name_id = runtime.player.get_ctx_current_bytecode(ctx).obj as u16;
        let prop_name = ctx.get_name(name_id);
        runtime.with_player_and_symbols(|player, symbols| {
            let result = GetSetUtils::get_top_level_prop(player, symbols, prop_name)?;
            let result_id = player.alloc_datum(result);
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod property_read_tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use async_std::channel;

    use crate::{
        director::{
            chunks::{
                handler::{Bytecode, HandlerDef},
                script::ScriptChunk,
            },
            enums::ScriptType,
            lingo::opcode::OpCode,
        },
        player::{
            bytecode::handler_manager::{BytecodeHandlerContext, HandlerCode},
            cast_lib::{CastLib, CastMemberRef},
            ownership::OwnerToken,
            scope::ScopeRef,
            script::Script,
            script_ref::ScriptInstanceRef,
            session::ExecutionContext,
            symbols::{symbol::Symbol, symbol_table::SymbolTable},
            virtual_scripts::{VirtualScriptHandler, VirtualScriptRegistry},
            DirPlayer, ScopeToken, ScriptError, ScriptErrorCode,
        },
    };

    struct ResettingGetter {
        returns_error: bool,
    }

    impl VirtualScriptHandler for ResettingGetter {
        fn get_prop(
            &self,
            player: &mut DirPlayer,
            _symbols: &SymbolTable,
            instance: &ScriptInstanceRef,
            _name: Symbol,
        ) -> Result<Option<DatumRef>, ScriptError> {
            let callback_result = player.alloc_datum(Datum::Int(42));
            let stale_instance = instance.clone();

            player.reset();
            let replacement_slot = player.push_scope();
            player.scopes[replacement_slot].cached_handler_instance = Some(stale_instance);
            let sentinel = player.alloc_datum(Datum::Int(777));
            player.scopes[replacement_slot].stack.push(sentinel);

            if self.returns_error {
                Err(ScriptError::new("getter callback failed".to_owned()))
            } else {
                Ok(Some(callback_result))
            }
        }
    }

    fn player_with_cast() -> (DirPlayer, SymbolTable) {
        let (tx, _rx) = channel::unbounded();
        let mut player = DirPlayer::new_with_owner(tx, OwnerToken::transitional());
        player
            .movie
            .cast_manager
            .casts
            .push(CastLib::test_external(1, 0));
        (player, SymbolTable::new())
    }

    fn context_for(
        player: &DirPlayer,
        slot: ScopeRef,
        opcode: OpCode,
        name: Symbol,
    ) -> BytecodeHandlerContext {
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![Bytecode::new(opcode, 0, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let script = Rc::new(Script {
            member_ref: CastMemberRef {
                cast_lib: 0,
                cast_member: 0,
            },
            name: String::new(),
            chunk: ScriptChunk {
                script_number: 0,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers: fxhash::FxHashMap::default(),
            handler_names_raw: vec![],
            handler_names: vec![],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        });
        let scope = &player.scopes[slot];
        BytecodeHandlerContext {
            scope: ScopeToken {
                owner: player.owner.clone(),
                slot,
                generation: scope.generation,
                epoch: player.scope_invalidation_epoch,
            },
            code: HandlerCode {
                script,
                handler,
                names: Rc::from(vec![name]),
            },
            multiplier: 1,
        }
    }

    #[test]
    fn getter_callback_cannot_write_replacement_scope_on_success_or_error() {
        for returns_error in [false, true] {
            let (mut player, mut symbols) = player_with_cast();
            let handler = Rc::new(ResettingGetter { returns_error });
            let script_ref =
                VirtualScriptRegistry::register(&mut player, "ResettingGetter", handler);
            let (instance_ref, _) =
                VirtualScriptRegistry::create_instance(&mut player, &symbols, &script_ref).unwrap();
            let property = symbols.intern("callbackProperty");
            let slot = player.push_scope();
            player.scopes[slot].receiver = Some(instance_ref.clone());
            player.scopes[slot].script_ref = script_ref;
            let ctx = context_for(&player, slot, OpCode::GetProp, property);

            let result = {
                let mut runtime = ExecutionContext {
                    player_id: 1,
                    symbols: &mut symbols,
                    player: &mut player,
                };
                GetSetBytecodeHandler::get_prop(&mut runtime, &ctx)
            };

            assert_eq!(result.err().unwrap().code, ScriptErrorCode::Abort);
            assert_eq!(player.scopes[0].stack.len(), 1);
            let sentinel = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes[0]
                    .stack
                    .get_ref_with(0, allocator, bitmap_manager)
                    .unwrap()
            };
            assert!(matches!(player.get_datum(&sentinel), Datum::Int(777)));
            assert_eq!(
                player.scopes[0]
                    .cached_handler_instance
                    .as_ref()
                    .map(ScriptInstanceRef::id),
                Some(instance_ref.id())
            );
        }
    }

    #[test]
    fn chained_property_accepts_void_as_the_missing_object_sentinel() {
        let (mut player, mut symbols) = player_with_cast();
        let property = symbols.intern("missingProperty");
        let slot = player.push_scope();
        player.scopes[slot].stack.push(DatumRef::Void);
        let ctx = context_for(&player, slot, OpCode::GetChainedProp, property);

        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            GetSetBytecodeHandler::get_chained_prop(&mut runtime, &ctx)
        };

        assert!(result.is_ok());
        let value = {
            let (scopes, allocator, bitmap_manager) = (
                &mut player.scopes,
                &mut player.allocator,
                &mut player.bitmap_manager,
            );
            scopes[slot]
                .stack
                .pop_ref_with(allocator, bitmap_manager)
                .unwrap()
        };
        assert!(matches!(player.get_datum(&value), Datum::Void));
    }

    #[test]
    fn global_get_rejects_foreign_name_before_stack_mutation() {
        let (mut player, mut symbols) = player_with_cast();
        let mut foreign_symbols = SymbolTable::new();
        let foreign_name = foreign_symbols.intern("foreignGlobal");
        let slot = player.push_scope();
        let ctx = context_for(&player, slot, OpCode::GetGlobal, foreign_name);
        let stack_len = player.scopes[slot].stack.len();

        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            GetSetBytecodeHandler::get_global(&mut runtime, &ctx)
        };

        assert!(result.is_err());
        assert_eq!(player.scopes[slot].stack.len(), stack_len);
    }
}
