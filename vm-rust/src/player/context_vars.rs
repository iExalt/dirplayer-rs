use log::{debug, error};
use super::{
    bytecode::handler_manager::BytecodeHandlerContext,
    scope::ScopeRef,
    script::{get_current_handler_def, get_current_script, script_get_prop, script_set_prop},
    symbols::{symbol::Symbol, symbol_table::SymbolTable},
    DatumRef, DirPlayer, ScriptError,
};
use crate::director::lingo::datum::Datum;
use crate::player::bytecode::string::PutType;
use crate::player::cast_member::CastMemberType;
use web_sys::console;

fn validate_symbol<'a>(symbol: &Symbol, symbols: &'a SymbolTable) -> Result<&'a str, ScriptError> {
    symbols
        .display(symbol)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign.into())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use crate::{
        director::{chunks::{handler::HandlerDef, script::ScriptChunk}, enums::ScriptType},
        player::{
            bytecode::handler_manager::{BytecodeHandlerContext, HandlerCode},
            cast_lib::CastMemberRef,
            ownership::{OwnerKey, OwnerToken},
            script::Script,
            symbols::{symbol::Symbol, symbol_table::SymbolTable},
            scope::ScopeRef,
            ScopeToken, DirPlayer,
        },
    };

    fn make_player() -> DirPlayer {
        let (tx, _rx) = async_std::channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey { session: 31, player: 8, generation: 1 }),
        )
    }

    fn make_context(player: &DirPlayer, slot: ScopeRef) -> BytecodeHandlerContext {
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let script = Rc::new(Script {
            member_ref: CastMemberRef { cast_lib: 0, cast_member: 0 },
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
                names: Rc::from(Vec::<Symbol>::new()),
            },
            multiplier: 1,
        }
    }

    #[test]
    fn foreign_field_identifier_is_rejected_before_lookup_or_logging() {
        let mut player = make_player();
        let symbols = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();
        let foreign = player.alloc_datum(Datum::Symbol(foreign_table.intern("foreignField")));
        let slot = player.push_scope();
        let ctx = make_context(&player, slot);

        let get_result = player_get_context_var(&mut player, &foreign, None, 0x6, &ctx, &symbols);
        assert!(get_result.is_err());

        let value = player.alloc_datum(Datum::String("value".to_owned()));
        let set_result = player_set_context_var(
            &mut player,
            &foreign,
            None,
            0x6,
            &value,
            PutType::Into,
            &ctx,
            &symbols,
        );
        assert!(set_result.is_err());
        player.pop_scope();
    }
}

pub(crate) fn validate_direct_identifier(datum: &Datum, symbols: &SymbolTable) -> Result<(), ScriptError> {
    if let Datum::Symbol(symbol) = datum {
        validate_symbol(symbol, symbols)?;
    }
    Ok(())
}

pub fn read_context_var_args(
    player: &mut DirPlayer,
    var_type: u32,
    scope_ref: ScopeRef,
) -> (DatumRef, Option<DatumRef>) {
    let cast_id = if var_type == 0x6 && player.movie.dir_version >= 500 {
        // field cast ID
        let (scopes, allocator, bitmap_manager) =
            (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
        Some(scopes.get_mut(scope_ref).unwrap().stack.pop_ref_with(allocator, bitmap_manager).unwrap())
    } else {
        None
    };
    let (scopes, allocator, bitmap_manager) =
        (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
    let id = scopes.get_mut(scope_ref).unwrap().stack.pop_ref_with(allocator, bitmap_manager).unwrap();
    (id, cast_id)
}

pub fn player_get_context_var(
    player: &mut DirPlayer,
    id_ref: &DatumRef,
    _cast_id_ref: Option<&DatumRef>,
    var_type: u32,
    ctx: &BytecodeHandlerContext,
    symbols: &SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let variable_multiplier = ctx.multiplier;
    let id = player.get_datum(id_ref);
    validate_direct_identifier(id, symbols)?;
    if let Some(cast_id_ref) = _cast_id_ref {
        validate_direct_identifier(player.get_datum(cast_id_ref), symbols)?;
    }
    let handler = get_current_handler_def(ctx);

    match var_type {
        // global
        0x1 | 0x2 => {
            let global_name = if let Datum::Symbol(name) = id {
                name.clone()
            } else {
                let name_index = (id.int_value()? / variable_multiplier as i32) as usize;
                let name_id = handler.global_name_ids[name_index];
                ctx.get_name(name_id)
            };
            validate_symbol(&global_name, symbols)?;
            let value_ref = player
                .globals
                .get(&global_name)
                .unwrap_or(&DatumRef::Void)
                .clone();
            Ok(value_ref)
        }
        // property/instance
        0x3 => {
            let prop_name = if let Datum::Symbol(name) = id {
                // PushVarRef pushes a Symbol with the property name
                name.clone()
            } else {
                let name_index = (id.int_value()? / variable_multiplier as i32) as usize;
                let script = get_current_script(ctx);
                let prop_name_id = script.chunk.property_name_ids[name_index];
                ctx.get_name(prop_name_id)
            };
            validate_symbol(&prop_name, symbols)?;
            let scope = player.scopes.get(ctx.scope_ref()).unwrap();
            let receiver = scope.receiver.clone();
            let script_ref = scope.script_ref.clone();
            if let Some(instance_ref) = receiver {
                script_get_prop(player, symbols, &instance_ref, prop_name)
            } else {
                // Static property on script
                let script_rc = player.movie.cast_manager.get_script_by_ref(&script_ref).unwrap();
                let properties = script_rc.properties.borrow();
                Ok(properties.get(&prop_name).unwrap_or(&DatumRef::Void).clone())
            }
        }
        0x4 => {
            // arg
            let arg_index = (id.int_value()? / variable_multiplier as i32) as usize;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let arg_val_ref = scope.args.get(arg_index).unwrap();
            // The argument value is already stored in the active scope; name
            // lookup is only needed for global/local/property resolution.
            Ok(arg_val_ref.clone())
        }
        0x5 => {
            // local
            // Resolve to the dense SLOT. The symbol form searches the name
            // table by position (the position IS the slot); the int form
            // already carries the slot.
            let local_name_ids = &handler.local_name_ids;
            let slot = if let Datum::Symbol(name) = id {
                let name_text = validate_symbol(name, symbols)?;
                match local_name_ids.iter().position(|&nid| {
                    ctx.code
                        .names
                        .get(nid as usize)
                        .is_some_and(|candidate| candidate == name)
                }) {
                    Some(slot) => slot,
                    None => return Err(ScriptError::new(format!("Local variable '{}' not found", name_text))),
                }
            } else {
                (id.int_value()? / variable_multiplier as i32) as usize
            };
            let local = player.scopes.get(ctx.scope_ref()).unwrap().local(slot);
            crate::player::interp_stats::record_ctxvar_local(false);
            Ok(local.into_ref_with(&mut player.allocator, &mut player.bitmap_manager))
        }
        0x6 => {
            // field
            // `id_ref` is the field's identifier (e.g. member number)
            // `_cast_id_ref` might indicate which cast lib to search in
            let id_datum = player.get_datum(id_ref);

            let cast_id_datum = _cast_id_ref.map(|r| player.get_datum(r));
            let cast_id_datum_opt = cast_id_datum.as_ref().map(|d| *d);

            let text = player.movie.cast_manager.get_field_value_by_identifiers(
                symbols,
                id_datum,
                cast_id_datum_opt,
                &player.allocator,
            )?;

            Ok(player.alloc_datum(Datum::String(text)))
        }
        _ => Err(ScriptError::new(format!(
            "Invalid context var type: {}",
            var_type
        ))),
    }
}

pub fn player_set_context_var(
    player: &mut DirPlayer,
    id_ref: &DatumRef,
    cast_id_ref: Option<&DatumRef>,
    var_type: u32,
    value_ref: &DatumRef,
    put_type: PutType,
    ctx: &BytecodeHandlerContext,
    symbols: &SymbolTable,
) -> Result<(), ScriptError> {
    let variable_multiplier = ctx.multiplier;
    let handler = get_current_handler_def(ctx);
    let id_datum = player.get_datum(id_ref);
    validate_direct_identifier(id_datum, symbols)?;
    if let Some(cast_id_ref) = cast_id_ref {
        validate_direct_identifier(player.get_datum(cast_id_ref), symbols)?;
    }

    match var_type {
        // global
        0x1 | 0x2 => {
            let global_name = if let Datum::Symbol(name) = id_datum {
                name.clone()
            } else {
                let name_index = (id_datum.int_value()? / variable_multiplier as i32) as usize;
                let name_id = handler.global_name_ids[name_index];
                ctx.get_name(name_id)
            };
            validate_symbol(&global_name, symbols)?;
            player.globals.insert(global_name, value_ref.clone());
            Ok(())
        }
        // property/instance
        0x3 => {
            let prop_name = if let Datum::Symbol(name) = id_datum {
                // PushVarRef pushes a Symbol with the property name
                name.clone()
            } else {
                let name_index = (id_datum.int_value()? / variable_multiplier as i32) as usize;
                let script = get_current_script(ctx);
                let prop_name_id = script.chunk.property_name_ids[name_index];
                ctx.get_name(prop_name_id)
            };
            validate_symbol(&prop_name, symbols)?;
            let scope = player.scopes.get(ctx.scope_ref()).unwrap();
            if let Some(instance_ref) = scope.receiver.clone() {
                script_set_prop(player, symbols, &instance_ref, prop_name, value_ref, true)
            } else {
                let scope = player.scopes.get(ctx.scope_ref()).unwrap();
                let script_ref = scope.script_ref.clone();
                let script_rc = player.movie.cast_manager.get_script_by_ref(&script_ref).unwrap();
                let mut properties = script_rc.properties.borrow_mut();
                properties.insert(prop_name, value_ref.clone());
                Ok(())
            }
        }
        0x4 => {
            // argument
            let arg_index = (id_datum.int_value()? / variable_multiplier as i32) as usize;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let arg_val_ref = scope.args.get_mut(arg_index).unwrap();
            *arg_val_ref = value_ref.clone();
            Ok(())
        }
        0x5 => {
            // local
            let local_name_ids = &handler.local_name_ids;
            let slot = if let Datum::Symbol(name) = id_datum {
                let name_text = validate_symbol(name, symbols)?;
                match local_name_ids.iter().position(|&nid| {
                    ctx.code
                        .names
                        .get(nid as usize)
                        .is_some_and(|candidate| candidate == name)
                }) {
                    Some(slot) => slot,
                    None => return Err(ScriptError::new(format!("Local variable '{}' not found", name_text))),
                }
            } else {
                (id_datum.int_value()? / variable_multiplier as i32) as usize
            };
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.set_local(slot, crate::player::scope::StackDatum::Ref(value_ref.clone()));
            crate::player::interp_stats::record_ctxvar_local(true);
            Ok(())
        }
        0x6 => {
            // FIELD variable

            // Get the value to write
            let new_value = player.get_datum(value_ref).string_value(symbols)?;

            // Map cast_id_ref to Datum if provided
            let cast_id_opt: Option<&Datum> = cast_id_ref.map(|r| player.get_datum(r));

            // Attempt to find the member reference
            let member_ref_opt = player
                .movie
                .cast_manager
                .find_member_ref_by_identifiers(
                    symbols,
                    &player.get_datum(id_ref),
                    cast_id_opt,
                    &player.allocator,
                )
                .map_err(|e| ScriptError::new(format!("Error finding member: {:#?}", e)))?;

            let member_ref = match member_ref_opt {
                Some(r) => r,
                None => {
                    error!("❌ Field member not found by identifiers");
                    return Err(ScriptError::new("Field member not found".to_string()));
                }
            };

            // Now safely mutate the member
            if let Some(member) = player
                .movie
                .cast_manager
                .find_mut_member_by_ref(&member_ref)
            {
                match &mut member.member_type {
                    CastMemberType::Field(field) => {
                        let combined = match put_type {
                            PutType::Into => new_value,
                            PutType::Before => {
                                let mut combined = new_value;
                                combined.push_str(&field.text);
                                combined
                            }
                            PutType::After => {
                                let mut combined = field.text.clone();
                                combined.push_str(&new_value);
                                combined
                            }
                        };
                        field.set_text_preserving_caret(combined);
                        Ok(())
                    }
                    CastMemberType::Text(text) => {
                        let combined = match put_type {
                            PutType::Into => new_value,
                            PutType::Before => {
                                let mut combined = new_value;
                                combined.push_str(&text.text);
                                combined
                            }
                            PutType::After => {
                                let mut combined = text.text.clone();
                                combined.push_str(&new_value);
                                combined
                            }
                        };
                        text.set_text_preserving_caret(combined);
                        Ok(())
                    }
                    CastMemberType::Button(button) => {
                        let combined = match put_type {
                            PutType::Into => new_value,
                            PutType::Before => {
                                let mut combined = new_value;
                                combined.push_str(&button.field.text);
                                combined
                            }
                            PutType::After => {
                                let mut combined = button.field.text.clone();
                                combined.push_str(&new_value);
                                combined
                            }
                        };
                        button.field.set_text_preserving_caret(combined);
                        Ok(())
                    }
                    other => {
                        debug!("Member exists but is not a Field, Text, or Button: {:?}", other);
                        Err(ScriptError::new(
                            "Cast member exists but is not a Field, Text, or Button".to_string(),
                        ))
                    }
                }
            } else {
                console::log_1(
                    &format!(
                        "❌ Member reference found but no mutable member exists: {:?}",
                        member_ref
                    )
                    .into(),
                );
                Err(ScriptError::new(
                    "Field member not found in cast_manager".to_string(),
                ))
            }
        }
        _ => Err(ScriptError::new(format!(
            "set Invalid context var type: {}",
            var_type
        ))),
    }
}
