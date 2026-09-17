use std::collections::VecDeque;
use crate::{
    director::lingo::datum::{Datum, DatumType, datum_bool},
    player::{
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode, allocator::ScriptInstanceAllocatorTrait, cast_lib::CastMemberRef, handlers::types::TypeUtils, player_handle_scope_return, reserve_player_mut, reserve_player_ref, script::{Script, ScriptHandlerRef, script_get_prop, script_set_prop}, script_ref::ScriptInstanceRef, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable}, virtual_scripts::VirtualScriptRegistry
    },
};
use crate::player::script::script_get_prop_opt;

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        DatumRef::Ref(_) => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            )
        })?,
    };
    if let Datum::ScriptInstanceRef(instance_ref) = datum {
        player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "foreign or stale ScriptInstanceRef".to_owned(),
            ))?;
    }
    crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
    Ok(datum)
}

pub struct ScriptInstanceDatumHandlers {}
pub struct ScriptInstanceUtils {}

/// A script-instance method is either complete in the current owner borrow or
/// becomes a child bytecode invocation.  The child owns all handles needed to
/// resume and never carries a `DirPlayer` reference across a wait.
pub(crate) enum ScriptInstanceCallPlan {
    Complete(DatumRef),
    Child {
        receiver: ScriptInstanceRef,
        handler_ref: ScriptHandlerRef,
        args: Vec<DatumRef>,
    },
}

impl ScriptInstanceUtils {
    pub fn get_script<'a>(
        datum: &DatumRef,
        player: &'a DirPlayer,
    ) -> Result<(ScriptInstanceRef, &'a Script), ScriptError> {
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            DatumRef::Ref(_) => player.allocator.try_get_datum(datum).ok_or_else(|| {
                ScriptError::new_code(ScriptErrorCode::InvalidReference, "invalid datum reference".to_owned())
            })?,
        };
        match datum {
            Datum::ScriptInstanceRef(instance_ref) => {
                let instance_id = **instance_ref;
                let instance = player
                    .allocator
                    .get_script_instance_opt(&instance_ref)
                    .ok_or(ScriptError::new(format!(
                        "Script instance {instance_id} not found"
                    )))?;
                let script = player
                    .movie
                    .cast_manager
                    .get_script_by_ref(&instance.script)
                    .ok_or(ScriptError::new(format!("Script not found")))?;
                Ok((instance_ref.clone(), script))
            }
            _ => Err(ScriptError::new(format!(
                "Cannot get script from non-script instance ({})",
                datum.type_str()
            ))),
        }
    }

    #[allow(dead_code)]
    pub fn get_instance_script_def<'a>(
        instance_ref: &ScriptInstanceRef,
        player: &'a DirPlayer,
    ) -> &'a Script {
        let script_ref = player
            .allocator
            .get_script_instance(&instance_ref)
            .script
            .to_owned();
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(&script_ref)
            .unwrap();
        script
    }

    pub fn get_handler(
        name: Symbol,
        datum: &DatumRef,
        player: &DirPlayer,
    ) -> Result<Option<ScriptHandlerRef>, ScriptError> {
        // let script = ScriptInstanceUtils::get_script(datum, player)?;
        // Self::get_script_instance_handler(name, script, player)
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            DatumRef::Ref(_) => player.allocator.try_get_datum(datum).ok_or_else(|| {
                ScriptError::new_code(ScriptErrorCode::InvalidReference, "invalid datum reference".to_owned())
            })?,
        };
        match datum {
            Datum::ScriptInstanceRef(instance_ref) => {
                Self::get_script_instance_handler(name, instance_ref, player)
            }
            _ => Err(ScriptError::new(format!(
                "Cannot get handler from non-script instance ({})",
                datum.type_str()
            ))),
        }
    }

    pub fn get_script_instance_handler(
        name: Symbol,
        instance_ref: &ScriptInstanceRef,
        player: &DirPlayer,
    ) -> Result<Option<ScriptHandlerRef>, ScriptError> {
        let instance_id = **instance_ref;
        let instance = player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| ScriptError::new(format!("Script instance {instance_id} not found")))?;
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(&instance.script);
        let Some(script) = script else {
            // Script not found — cast library may be unloaded or empty
            return Ok(None);
        };
        let own_handler = script.get_own_handler_ref(name.clone());
        if let Some(own_handler) = own_handler {
            return Ok(Some(own_handler));
        }
        if let Some(ancestor_instance_ref) = &instance.ancestor {
            ScriptInstanceUtils::get_script_instance_handler(name.clone(), ancestor_instance_ref, player)
        } else {
            Ok(None)
        }
    }

    pub fn get_handler_from_first_arg(
        player: &DirPlayer,
        symbols: &SymbolTable,
        args: &[DatumRef],
        handler_name: &Symbol,
    ) -> Result<Option<(Option<ScriptInstanceRef>, ScriptHandlerRef)>, ScriptError> {
        symbols
            .lower(handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let Some(first_arg) = args.first() else {
            return Ok(None);
        };
        let first_arg = match first_arg {
            DatumRef::Void => return Ok(None),
            DatumRef::Ref(_) => player
                .allocator
                .try_get_datum(first_arg)
                .ok_or_else(|| ScriptError::new("invalid first handler argument reference".to_owned()))?,
        };
        match first_arg {
            Datum::ScriptRef(script_ref) => {
                let Some(script) = player.movie.cast_manager.get_script_by_ref(script_ref) else {
                    return Err(ScriptError::new(format!(
                        "Script {}:{} not found",
                        script_ref.cast_lib, script_ref.cast_member
                    )));
                };
                Ok(script
                    .get_own_handler_ref(handler_name.clone())
                    .map(|handler| (None, handler)))
            }
            Datum::ScriptInstanceRef(script_instance_ref) => {
                Ok(ScriptInstanceUtils::get_script_instance_handler(
                    handler_name.clone(),
                    script_instance_ref,
                    player,
                )?
                .map(|handler| (Some(script_instance_ref.clone()), handler)))
            }
            Datum::SpriteRef(sprite_num) => {
                let sprite = player
                    .movie
                    .score
                    .get_sprite(*sprite_num)
                    .ok_or_else(|| ScriptError::new(format!("sprite {} not found", sprite_num)))?;
                for instance_ref in &sprite.script_instance_list {
                    if let Some(handler) = ScriptInstanceUtils::get_script_instance_handler(
                        handler_name.clone(),
                        instance_ref,
                        player,
                    )? {
                        return Ok(Some((Some(instance_ref.clone()), handler)));
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub fn set_at(
        datum: &DatumRef,
        key: Symbol,
        value: &DatumRef,
        player: &mut DirPlayer,
        symbols: &SymbolTable,
    ) -> Result<(), ScriptError> {
        symbols
            .lower(&key)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let self_instance_id = match checked_datum(player, datum, symbols)? {
            Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Cannot set ancestor on non-script instance".to_string(),
                ))
            }
        };
        player
            .allocator
            .get_script_instance_opt(&self_instance_id)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef".to_owned(),
                )
            })?;
        match symbols.lower(&key).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)? {
            "ancestor" => {
                let value_datum = checked_datum(player, value, symbols)?.to_owned();
                match value_datum {
                    Datum::Void => {
                        // No-op — see the matching arm in script.rs::script_set_prop.
                        // Habbo v7's Thread Manager assigns VOID mid-chain and
                        // relies on the previously-set ancestor surviving.
                        Ok(())
                    }
                    Datum::ScriptInstanceRef(ancestor_instance_id) => {
                        player.allocator.get_script_instance_opt(&ancestor_instance_id)
                            .ok_or_else(|| ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "stale ancestor ScriptInstanceRef".to_owned(),
                            ))?;
                        let script_instance =
                            player.allocator.get_script_instance_mut(&self_instance_id);
                        script_instance.ancestor = Some(ancestor_instance_id);
                        Ok(())
                    }
                    // For non-ScriptInstanceRef ancestors (like TimeoutInstance),
                    // store in properties map so method calls can be delegated
                    _ => {
                        let script_instance =
                            player.allocator.get_script_instance_mut(&self_instance_id);
                        script_instance.properties.insert(Symbol::builtin(BuiltInSymbol::Ancestor), value.clone());
                        Ok(())
                    }
                }
            }
            _ => script_set_prop(player, symbols, &self_instance_id, key, value, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::channel;
    use crate::director::chunks::handler::Bytecode;
    use crate::director::chunks::script::ScriptChunk;
    use crate::director::enums::ScriptType;
    use crate::director::lingo::opcode::OpCode;
    use crate::player::cast_lib::CastLib;
    use crate::player::ownership::{OwnerKey, OwnerToken};
    use crate::player::script::ScriptInstance;
    use std::rc::Rc;

    fn test_player(player: u64) -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey {
                session: 900,
                player,
                generation: 1,
            }),
        )
    }

    fn test_script(
        symbols: &SymbolTable,
        member_ref: CastMemberRef,
        handler_names: &[Symbol],
    ) -> Rc<Script> {
        let mut handlers = fxhash::FxHashMap::default();
        for (index, name) in handler_names.iter().enumerate() {
            handlers.insert(
                name.clone(),
                Rc::new(crate::director::chunks::handler::HandlerDef {
                    name_id: index as u16,
                    bytecode_array: vec![Bytecode::new(OpCode::Ret, 0, 0)],
                    bytecode_index_map: fxhash::FxHashMap::default(),
                    argument_name_ids: vec![],
                    local_name_ids: vec![],
                    global_name_ids: vec![],
                    compiled_ir: std::cell::RefCell::new(None),
                }),
            );
        }
        Rc::new(Script {
            member_ref,
            name: "local-call-test".to_owned(),
            chunk: ScriptChunk {
                script_number: 1,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: std::collections::HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers,
            handler_names_raw: handler_names
                .iter()
                .map(|name| symbols.lower(name).unwrap().to_owned())
                .collect(),
            handler_names: handler_names.to_vec(),
            properties: std::cell::RefCell::new(fxhash::FxHashMap::default()),
        })
    }

    fn install_scripts(player: &mut DirPlayer, scripts: &[(CastMemberRef, Rc<Script>)]) {
        let mut cast = CastLib::test_external(1, 0);
        for (member_ref, script) in scripts {
            cast.scripts.insert(member_ref.cast_member as u32, script.clone());
        }
        player.movie.cast_manager.casts.push(cast);
    }

    #[test]
    fn first_argument_resolution_preserves_precedence_and_rejects_foreign_refs() {
        let mut player = test_player(1);
        let mut symbols = SymbolTable::with_owner(crate::player::symbols::symbol_table::SymbolOwner {
            session: 900,
            generation: 1,
        });
        let handler_name = symbols.intern("localCallTarget");
        let other_name = symbols.intern("otherLocalCallTarget");
        let own_member = CastMemberRef { cast_lib: 1, cast_member: 1 };
        let ancestor_member = CastMemberRef { cast_lib: 1, cast_member: 2 };
        let empty_member = CastMemberRef { cast_lib: 1, cast_member: 3 };
        install_scripts(
            &mut player,
            &[
                (
                    own_member.clone(),
                    test_script(&symbols, own_member.clone(), &[handler_name.clone()]),
                ),
                (
                    ancestor_member.clone(),
                    test_script(&symbols, ancestor_member.clone(), &[handler_name.clone()]),
                ),
                (
                    empty_member.clone(),
                    test_script(&symbols, empty_member.clone(), &[]),
                ),
            ],
        );

        let ancestor = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 1,
            script: ancestor_member.clone(),
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let child = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 2,
            script: empty_member,
            ancestor: Some(ancestor.clone()),
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });

        let script_arg = player.alloc_datum(Datum::ScriptRef(own_member.clone()));
        let instance_arg = player.alloc_datum(Datum::ScriptInstanceRef(child.clone()));
        let direct = ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[script_arg],
            &handler_name,
        )
        .unwrap()
        .unwrap();
        assert!(direct.0.is_none());
        assert_eq!(direct.1.0, own_member);

        let inherited = ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[instance_arg.clone()],
            &handler_name,
        )
        .unwrap()
        .unwrap();
        let inherited_receiver = inherited.0.expect("ancestor handler has receiver");
        assert_eq!(inherited_receiver.id(), child.id());
        assert!(inherited_receiver.owner().same_identity(child.owner()));
        assert_eq!(inherited.1.0, ancestor_member);

        player.movie.score.set_channel_count(2);
        let sprite_direct = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 3,
            script: own_member.clone(),
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        player
            .movie
            .score
            .get_sprite_mut(1)
            .script_instance_list
            .extend([child.clone(), sprite_direct.clone()]);
        let sprite = player.alloc_datum(Datum::SpriteRef(1));
        let sprite_result = ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[sprite],
            &handler_name,
        )
        .unwrap()
        .unwrap();
        let sprite_receiver = sprite_result.0.expect("sprite handler has receiver");
        assert_eq!(sprite_receiver.id(), child.id());
        assert!(sprite_receiver.owner().same_identity(child.owner()));
        assert_eq!(sprite_result.1.0, ancestor_member);

        let missing_handler_arg = player.alloc_datum(Datum::ScriptRef(own_member.clone()));
        let missing_handler = ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[missing_handler_arg],
            &other_name,
        )
        .unwrap();
        assert!(missing_handler.is_none());

        let missing_script_arg = player.alloc_datum(Datum::ScriptRef(CastMemberRef {
            cast_lib: 1,
            cast_member: 99,
        }));
        let missing_script = ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[missing_script_arg],
            &handler_name,
        );
        assert!(missing_script.is_err());

        let missing_instance = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 5,
            script: CastMemberRef {
                cast_lib: 1,
                cast_member: 100,
            },
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        assert!(ScriptInstanceUtils::get_script_instance_handler(
            handler_name.clone(),
            &missing_instance,
            &player,
        )
        .unwrap()
        .is_none());

        assert!(ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[DatumRef::Void],
            &handler_name,
        )
        .unwrap()
        .is_none());

        let mut foreign_symbols = SymbolTable::new();
        let foreign_name = foreign_symbols.intern("localCallTarget");
        assert!(ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[instance_arg],
            &foreign_name,
        )
        .is_err());

        let mut other_player = test_player(2);
        let foreign_datum = other_player.alloc_datum(Datum::Int(1));
        assert!(ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[foreign_datum],
            &handler_name,
        )
        .is_err());

        let foreign_instance = other_player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 4,
            script: own_member,
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let foreign_instance_datum = player.alloc_datum(Datum::ScriptInstanceRef(foreign_instance));
        assert!(ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[foreign_instance_datum],
            &handler_name,
        )
        .is_err());

        let invalid_sprite = player.alloc_datum(Datum::SpriteRef(99));
        assert!(ScriptInstanceUtils::get_handler_from_first_arg(
            &player,
            &symbols,
            &[invalid_sprite],
            &handler_name,
        )
        .is_err());
    }

    #[test]
    fn checked_ancestor_boundaries_preserve_void_and_reject_foreign_fallbacks() {
        let mut player = test_player(1);
        let mut symbols = SymbolTable::with_owner(crate::player::symbols::symbol_table::SymbolOwner {
            session: 900,
            generation: 1,
        });

        // The foreign ancestor deliberately reuses the child's numeric id. The
        // ownership token must win over the numeric cycle/lookup shortcut.
        let mut foreign_player = test_player(2);
        let foreign_ancestor = foreign_player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 10,
            script: CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let child = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 10,
            script: CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: Some(foreign_ancestor),
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let value = player.alloc_datum(Datum::Int(3));
        let prop_name = symbols.intern("foreignFallbackOnly");
        let error = crate::player::script::script_set_prop(
            &mut player,
            &symbols,
            &child,
            prop_name.clone(),
            &value,
            false,
        )
        .expect_err("foreign ancestor must not become a local fallback");
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
        assert!(!player
            .allocator
            .get_script_instance(&child)
            .properties
            .contains_key(&prop_name));

        // VOID assignment leaves an established local ancestor attached.
        let ancestor = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 12,
            script: CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: None,
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let child = player.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 13,
            script: CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: Some(ancestor.clone()),
            properties: fxhash::FxHashMap::default(),
            begin_sprite_called: false,
        });
        let child_datum = player.alloc_datum(Datum::ScriptInstanceRef(child.clone()));
        ScriptInstanceUtils::set_at(
            &child_datum,
            Symbol::builtin(BuiltInSymbol::Ancestor),
            &DatumRef::Void,
            &mut player,
            &symbols,
        )
        .expect("ancestor = VOID is a no-op");
        assert_eq!(
            player
                .allocator
                .get_script_instance(&child)
                .ancestor
                .as_ref()
                .map(ScriptInstanceRef::id),
            Some(ancestor.id())
        );
    }
}

impl ScriptInstanceDatumHandlers {
    /// Find a non-ScriptInstance ancestor (like TimeoutInstance) in the properties
    fn find_non_script_ancestor(datum: &DatumRef, player: &DirPlayer) -> Option<DatumRef> {
        let instance_ref = match player.get_datum(datum) {
            Datum::ScriptInstanceRef(r) => r.clone(),
            _ => return None,
        };

        // Walk the ancestor chain looking for non-ScriptInstance ancestors in properties
        let mut current_instance_ref = Some(instance_ref);
        let mut depth = 0;
        while let Some(ref inst_ref) = current_instance_ref {
            depth += 1;
            if depth > 100 {
                break;
            }

            let instance = player.allocator.get_script_instance(inst_ref);

            // Check if this instance has a non-ScriptInstance ancestor in properties
            if let Some(ancestor_prop_ref) = instance.properties.get(&Symbol::builtin(BuiltInSymbol::Ancestor)) {
                let ancestor_datum = player.get_datum(ancestor_prop_ref);
                match ancestor_datum {
                    // If ancestor is not a ScriptInstanceRef, return it for delegation
                    Datum::ScriptInstanceRef(next_ref) => {
                        current_instance_ref = Some(next_ref.clone());
                        continue;
                    }
                    Datum::Void | Datum::Int(0) => {
                        // No ancestor - continue to struct field
                    }
                    _ => {
                        // Non-ScriptInstance ancestor (e.g., TimeoutInstance)
                        return Some(ancestor_prop_ref.clone());
                    }
                }
            }

            // Check the struct field for ScriptInstance ancestors
            if let Some(ref ancestor_ref) = instance.ancestor {
                current_instance_ref = Some(ancestor_ref.clone());
            } else {
                break;
            }
        }

        None
    }

    fn find_non_script_ancestor_checked(
        datum: &DatumRef,
        player: &DirPlayer,
        symbols: &SymbolTable,
    ) -> Result<Option<DatumRef>, ScriptError> {
        let instance_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptInstanceRef(r) => r.clone(),
            _ => return Ok(None),
        };
        let mut current = Some(instance_ref);
        let mut visited = std::collections::HashSet::new();
        while let Some(instance_ref) = current {
            let instance = player
                .allocator
                .get_script_instance_opt(&instance_ref)
                .ok_or_else(|| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("stale ScriptInstanceRef {}", instance_ref.id()),
                ))?;
            if !visited.insert(instance_ref.id()) {
                break;
            }
            if let Some(ancestor_ref) = instance.properties.get(&Symbol::builtin(BuiltInSymbol::Ancestor)) {
                match checked_datum(player, ancestor_ref, symbols)? {
                    Datum::ScriptInstanceRef(next) => {
                        current = Some(next.clone());
                        continue;
                    }
                    Datum::Void | Datum::Int(0) => {}
                    _ => return Ok(Some(ancestor_ref.clone())),
                }
            }
            current = instance.ancestor.clone();
        }
        Ok(None)
    }

    pub fn prepare_call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<ScriptInstanceCallPlan, ScriptError> {
        let instance_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
            _ => return Err(ScriptError::new(
                "Cannot call handler on non-script instance".to_owned(),
            )),
        };
        if let Some(handler_ref) = ScriptInstanceUtils::get_handler(
            handler_name.clone(), datum, player,
        )? {
            return Ok(ScriptInstanceCallPlan::Child {
                receiver: instance_ref,
                handler_ref,
                args: args.to_vec(),
            });
        }
        // Preserve the legacy precedence: virtual instance handlers run
        // before the built-in/system-event no-op fallback in `call`.
        if let Some(result) = VirtualScriptRegistry::try_call_instance_handler(
            player,
            symbols,
            &instance_ref,
            handler_name.clone(),
            &args.to_vec(),
        )? {
            checked_datum(player, &result, symbols)?;
            return Ok(ScriptInstanceCallPlan::Complete(result));
        }

        // `forget` is a method on the non-script timeout ancestor.  The
        // synchronous fallback in `call` has a compatibility no-op for
        // wrapper instances, so resolve this ancestor before falling through
        // in order to preserve the async dispatch path's timeout behavior.
        if handler_name.eq_builtin(BuiltInSymbol::Forget) {
            if let Some(ancestor_ref) =
                Self::find_non_script_ancestor_checked(datum, player, symbols)?
            {
                match checked_datum(player, &ancestor_ref, symbols)?.type_enum() {
                    DatumType::TimeoutRef
                    | DatumType::TimeoutInstance
                    | DatumType::TimeoutFactory => {
                        let result = super::timeout::TimeoutDatumHandlers::call(
                            player,
                            symbols,
                            &ancestor_ref,
                            handler_name,
                            &args.to_vec(),
                        )?;
                        return Ok(ScriptInstanceCallPlan::Complete(result));
                    }
                    _ => {}
                }
            }
        }

        Ok(ScriptInstanceCallPlan::Complete(Self::call(
            player,
            symbols,
            datum,
            handler_name,
            &args.to_vec(),
        )?))
    }

    pub fn has_async_handler(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        name: Symbol,
    ) -> Result<bool, ScriptError> {
        let instance_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
            _ => return Ok(false),
        };
        if VirtualScriptRegistry::has_instance_handler(
            player,
            symbols,
            &instance_ref,
            name.clone(),
        )? {
            return Ok(true);
        }
        if ScriptInstanceUtils::get_handler(name.clone(), datum, player)?.is_some() {
            return Ok(true);
        }
        if let Some(ancestor_ref) = Self::find_non_script_ancestor(datum, player) {
            if matches!(checked_datum(player, &ancestor_ref, symbols)?, Datum::TimeoutInstance(_))
                && (name.eq_builtin(BuiltInSymbol::Forget)
                    || name.eq_builtin(BuiltInSymbol::New))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn get_at(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let key = checked_datum(player, &args[0], symbols)?.string_value(symbols)?;
        match key.as_str() {
                "ancestor" => {
                    let datum = checked_datum(player, datum, symbols)?;
                    let script_instance = player
                        .allocator
                        .get_script_instance_opt(datum.to_script_instance_ref()?)
                        .ok_or_else(|| ScriptError::new_code(ScriptErrorCode::InvalidReference, "stale ScriptInstanceRef".to_owned()))?;
                    let ancestor = script_instance.ancestor.clone();
                    if let Some(ancestor_ref) = &ancestor {
                        player
                            .allocator
                            .get_script_instance_opt(ancestor_ref)
                            .ok_or_else(|| ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "foreign or stale ancestor ScriptInstanceRef".to_owned(),
                            ))?;
                    }
                    let result = player.alloc_datum(if let Some(ancestor) = ancestor {
                            Datum::ScriptInstanceRef(ancestor)
                        } else {
                            Datum::Int(0)
                        });
                    checked_datum(player, &result, symbols)?;
                    Ok(result)
                }
                _ => Self::get_a_prop(player, symbols, datum, args),
            }
    }

    fn set_at(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let key = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let value_ref = &args[1];

            ScriptInstanceUtils::set_at(datum, key, value_ref, player, symbols)?;
            Ok(DatumRef::Void)
    }

    pub fn set_a_prop(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let prop_name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let value_ref = &args[1];

            let instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set property on non-script instance".to_string(),
                    ))
                }
            };
            let value = checked_datum(player, value_ref, symbols)?;
            if let Datum::ScriptInstanceRef(value_instance_ref) = value {
                player
                    .allocator
                    .get_script_instance_opt(value_instance_ref)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "foreign or stale ScriptInstanceRef value".to_owned(),
                        )
                    })?;
            }
            script_set_prop(player, symbols, &instance_ref, prop_name, value_ref, false)
                .map(|_| DatumRef::Void)
    }

    pub fn get_prop(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let list_prop_name_ref = &args[1];

            let local_prop_name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot get property on non-script instance".to_string(),
                    ))
                }
            };

            let local_prop_ref = script_get_prop(player, symbols, &instance_ref, local_prop_name)?;
            let result = TypeUtils::get_sub_prop(&local_prop_ref, list_prop_name_ref, player, symbols)?;
            Ok(result)
    }

    pub fn set_prop(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let list_prop_name_ref = &args[1];
            let value_ref = &args[2];

            let local_prop_name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot set property on non-script instance".to_string(),
                    ))
                }
            };

            let local_prop_ref = script_get_prop(player, symbols, &instance_ref, local_prop_name)?;
            let value = checked_datum(player, value_ref, symbols)?;
            if let Datum::ScriptInstanceRef(value_instance_ref) = value {
                player
                    .allocator
                    .get_script_instance_opt(value_instance_ref)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "foreign or stale ScriptInstanceRef value".to_owned(),
                        )
                    })?;
            }
            TypeUtils::set_sub_prop(&local_prop_ref, list_prop_name_ref, value_ref, player, symbols)?;

            Ok(DatumRef::Void)
    }

    pub fn handler(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let (_, script) = ScriptInstanceUtils::get_script(datum, player)?;
            let own_handler = script.get_own_handler(name);
            Ok(player.alloc_datum(datum_bool(own_handler.is_some())))
    }

    pub fn count(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot count non-script instance".to_string(),
                    ))
                }
            };
            let prop_name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let prop_value = script_get_prop(player, symbols, &instance_ref, prop_name.clone())?;
            let prop_value_datum = checked_datum(player, &prop_value, symbols)?;
            let count = match prop_value_datum {
                Datum::List(_, list, _) => list.len(),
                Datum::PropList(prop_list, ..) => prop_list.len(),
                other => {
                    return Err(ScriptError::new(format!(
                        "Cannot count non-list property {} (type {})",
                        symbols.display(&prop_name).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?,
                        other.type_str()
                    )))
                }
            };
            Ok(player.alloc_datum(Datum::Int(count as i32)))
    }

    pub fn get_a_prop(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let prop_name = symbols.intern(&checked_datum(player, &args[0], symbols)?.string_value(symbols)?);
            let instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot get property on non-script instance".to_string(),
                    ))
                }
            };
            let prop_value = script_get_prop_opt(player, symbols, &instance_ref, prop_name)?.unwrap_or(DatumRef::Void);
            checked_datum(player, &prop_value, symbols)?;
            Ok(prop_value)
    }

    pub fn handlers(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let script_instance_ref = match checked_datum(player, datum, symbols)? {
                Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                _ => {
                    return Err(ScriptError::new(
                        "Cannot get handlers of non-script instance".to_string(),
                    ))
                }
            };
            let script_instance = player.allocator.get_script_instance_opt(&script_instance_ref)
                .ok_or_else(|| ScriptError::new_code(ScriptErrorCode::InvalidReference, "stale ScriptInstanceRef".to_owned()))?;
            let script = player
                .movie
                .cast_manager
                .get_script_by_ref(&script_instance.script);
            if script.is_none() {
                return Ok(player.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false)));
            }
            let script = script.unwrap();
            let handler_names = script.handler_names.iter().map(|name| {
                symbols.display(name).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                Ok::<_, ScriptError>(name.clone())
            }).collect::<Result<Vec<_>, _>>()?;
            let handler_name_datums: VecDeque<_> = handler_names
                .iter()
                .map(|name| player.alloc_datum(Datum::Symbol(name.clone())))
                .collect();
            Ok(player.alloc_datum(Datum::List(DatumType::List, handler_name_datums, false)))
    }

    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        symbols
            .lower(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::SetAt) => Self::set_at(player, symbols, datum, args),
            Some(BuiltInSymbol::Handler) => Self::handler(player, symbols, datum, args),
            Some(BuiltInSymbol::SetaProp) => Self::set_a_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::SetProp) => Self::set_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::GetProp) | Some(BuiltInSymbol::GetPropRef) => Self::get_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::GetaProp) => Self::get_a_prop(player, symbols, datum, args),
            Some(BuiltInSymbol::GetAt) => Self::get_at(player, symbols, datum, args),
            Some(BuiltInSymbol::Count) => Self::count(player, symbols, datum, args),
            Some(BuiltInSymbol::Handlers) => Self::handlers(player, symbols, datum, args),
            // getPropertyDescriptionList returns empty prop list if not implemented
            Some(BuiltInSymbol::GetPropertyDescriptionList) => Ok(player.alloc_datum(Datum::PropList(VecDeque::new(), false))),
            // Director system events that should be silently ignored if not implemented
            Some(BuiltInSymbol::ExitFrame) | Some(BuiltInSymbol::EnterFrame) | Some(BuiltInSymbol::PrepareFrame) | Some(BuiltInSymbol::Idle) | Some(BuiltInSymbol::StepFrame) |
            Some(BuiltInSymbol::MouseDown) | Some(BuiltInSymbol::MouseUp) | Some(BuiltInSymbol::MouseEnter) | Some(BuiltInSymbol::MouseLeave) | Some(BuiltInSymbol::MouseWithin) |
            Some(BuiltInSymbol::KeyDown) | Some(BuiltInSymbol::KeyUp) | Some(BuiltInSymbol::BeginSprite) | Some(BuiltInSymbol::EndSprite) | Some(BuiltInSymbol::PrepareMovie) |
            Some(BuiltInSymbol::StartMovie) | Some(BuiltInSymbol::StopMovie) | Some(BuiltInSymbol::Activate) | Some(BuiltInSymbol::Deactivate) |
            // forget is called on wrapper objects that may not have it - silently ignore
            Some(BuiltInSymbol::Forget) |
            // CS IsoAvatar.updateStatus invokes standOn on whatever action class
            // the under-foot furni has. getStandOnAtSquare returns any furni
            // whose type is "Stand_On" or "mirror", which matches some items
            // (e.g. Randomatic Number Podium, prodId 158) whose action class is
            // ACTION_ANIMATED_DICE — no standOn handler. The #standOnCheck
            // attribute on these items is a server-side concept, not a
            // client-side gate, so the client unconditionally calls standOn
            // and Director silently no-ops when the handler isn't there. Match
            // that here. has_async_handler still walks the ancestor chain
            // first, so call_async runs the real handler when one exists.
            Some(BuiltInSymbol::StandOn) => Ok(DatumRef::Void),
            _ => {
                // Check for virtual script handler
                let instance_ref = match checked_datum(player, datum, symbols)? {
                        Datum::ScriptInstanceRef(r) => r.clone(),
                        _ => return Err(ScriptError::new("Cannot call handler on non-script instance".to_owned())),
                };
                if let Some(result) = VirtualScriptRegistry::try_call_instance_handler(
                    player, symbols, &instance_ref, handler_name.clone(), args,
                )? {
                    checked_datum(player, &result, symbols)?;
                    return Ok(result);
                }

                // Check for non-ScriptInstance ancestor to delegate to (e.g., TimeoutInstance)
                let (script_name, script_missing) = {
                    let instance = player.allocator.get_script_instance_opt(&instance_ref)
                        .ok_or_else(|| ScriptError::new_code(ScriptErrorCode::InvalidReference, "stale ScriptInstanceRef".to_owned()))?;
                    match player.movie.cast_manager.get_script_by_ref(&instance.script) {
                        Some(script) => (script.name.clone(), false),
                        None => ("unknown".to_owned(), true),
                    }
                };

                // Resolve the ancestor before the missing-script fallback to preserve
                // traversal order. A missing script still prevents timeout delegation
                // and falls through to VOID after that lookup.
                let ancestor_ref = Self::find_non_script_ancestor_checked(datum, player, symbols)?;

                // If the script's cast library is unloaded/empty, silently ignore the call
                // after walking the ancestor chain, preserving the original lookup order.
                if script_missing {
                    return Ok(DatumRef::Void);
                }

                if let Some(ancestor_ref) = ancestor_ref {
                    use crate::director::lingo::datum::DatumType;
                    use super::timeout::TimeoutDatumHandlers;
                    match checked_datum(player, &ancestor_ref, symbols)?.type_enum() {
                        DatumType::TimeoutRef | DatumType::TimeoutInstance | DatumType::TimeoutFactory => {
                            return TimeoutDatumHandlers::call(
                                player,
                                symbols,
                                &ancestor_ref,
                                handler_name,
                                args,
                            );
                        }
                        _ => {}
                    }
                }

                // Log once when we hit this error for debugging
                static LOGGED_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !LOGGED_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    crate::console_warn!("ScriptInstance call error: handler={}, script={}", symbols.display(&handler_name).unwrap_or("<foreign symbol>"), script_name);
                }
                Err(ScriptError::new_code(
                    ScriptErrorCode::HandlerNotFound,
                    format!("No handler {} for script instance datum", symbols.display(&handler_name).unwrap_or("<foreign symbol>")),
                ))
            }
        }
    }

    /// Director's forget() method removes the script instance from the actorList.
    /// This is commonly used with script-based timeouts (like _TIMER_) that are stored in actorList.
    fn forget(player: &mut DirPlayer, symbols: &mut SymbolTable, datum: &DatumRef, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            // Get the actorList
            let actor_list_ref = player.globals.get(&BuiltInSymbol::ActorList.into()).cloned();

            if let Some(actor_list_ref) = actor_list_ref {
                let actor_list = checked_datum(player, &actor_list_ref, symbols)?.clone();
                if let Datum::List(dtype, items, sorted) = actor_list {
                    // Get the instance ID we're looking for
                    let target_id = match checked_datum(player, datum, symbols)? {
                        Datum::ScriptInstanceRef(instance_ref) => {
                            player.allocator.get_script_instance_opt(instance_ref)
                                .ok_or_else(|| ScriptError::new_code(
                                    ScriptErrorCode::InvalidReference,
                                    "foreign or stale target ScriptInstanceRef".to_owned(),
                                ))?;
                            Some(**instance_ref)
                        }
                        _ => None,
                    };

                    if let Some(target_id) = target_id {
                        // Find and remove the instance from the list
                        let mut new_items = VecDeque::with_capacity(items.len());
                        for item in &items {
                            match checked_datum(player, item, symbols)? {
                                Datum::ScriptInstanceRef(item_ref) => {
                                    player.allocator.get_script_instance_opt(item_ref)
                                        .ok_or_else(|| ScriptError::new_code(
                                            ScriptErrorCode::InvalidReference,
                                            "foreign or stale ScriptInstanceRef in actorList".to_owned(),
                                        ))?;
                                    if **item_ref != target_id {
                                        new_items.push_back(item.clone());
                                    }
                                }
                                _ => new_items.push_back(item.clone()),
                            }
                        }

                        // Update the actorList with the filtered list
                        let new_list = Datum::List(dtype, new_items, sorted);
                        let new_list_ref = player.alloc_datum(new_list);
                        player.globals.insert(BuiltInSymbol::ActorList.into(), new_list_ref);
                    }
                }
            }

            Ok(DatumRef::Void)
    }
}
