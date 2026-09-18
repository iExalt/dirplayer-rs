use std::{cell::RefCell, rc::Rc};

use fxhash::FxHashMap;
use itertools::Itertools;
use log::warn;

use crate::{
    director::{
        chunks::{handler::HandlerDef, script::ScriptChunk},
        enums::ScriptType,
        lingo::{datum::Datum, script::ScriptContext},
    },
    player::symbols::{builtin::BuiltInSymbol, symbol::Symbol},
};

use super::ci_string::{CiStr, CiString};

use super::{
    allocator::ScriptInstanceAllocatorTrait,
    bytecode::handler_manager::BytecodeHandlerContext,
    cast_lib::{CastLoadRequest, CastMemberRef, CastNotificationOutbox, PropertyLoadPreparation},
    datum_formatting::{format_concrete_datum, format_datum},
    handlers::{
        datum_handlers::{
            bitmap::BitmapDatumHandlers, cast_member::shockwave3d::Shockwave3dMemberHandlers,
            cast_member_ref::CastMemberRefHandlers, color::ColorDatumHandlers,
            date::DateDatumHandlers, float::FloatDatumHandlers, int::IntDatumHandlers,
            list_handlers::ListDatumUtils, math::MathDatumHandlers, point::PointDatumHandlers,
            prop_list::PropListUtils, rect::RectDatumHandlers,
            sound_channel::SoundChannelDatumHandlers, string::StringDatumUtils,
            string_chunk::StringChunkHandlers, symbol::SymbolDatumHandlers,
            timeout::TimeoutDatumHandlers, vector::VectorDatumHandlers, void::VoidDatumHandlers,
            xml::XmlDatumHandlers,
        },
        types::TypeUtils,
    },
    reserve_player_mut, reserve_player_ref,
    scope::Scope,
    score::{get_sprite_script_instance_ids_checked, sprite_get_prop, sprite_set_prop},
    script_ref::ScriptInstanceRef,
    stage::{get_stage_prop, set_stage_prop},
    symbols::symbol_table::SymbolTable,
    DatumRef, DirPlayer, ScriptError,
};

#[derive(Clone)]
pub struct Script {
    pub member_ref: CastMemberRef,
    pub name: String,
    pub chunk: ScriptChunk,
    pub script_type: ScriptType,
    pub handlers: FxHashMap<Symbol, Rc<HandlerDef>>,
    pub handler_names_raw: Vec<String>,
    pub handler_names: Vec<Symbol>,
    pub properties: RefCell<FxHashMap<Symbol, DatumRef>>,
}

pub type ScriptInstanceId = u32;
pub type ScriptHandlerRefDef<'a> = (CastMemberRef, &'a Rc<HandlerDef>);

pub struct ScriptInstance {
    pub instance_id: ScriptInstanceId,
    pub script: CastMemberRef,
    pub ancestor: Option<ScriptInstanceRef>,
    pub properties: FxHashMap<Symbol, DatumRef>,
    pub begin_sprite_called: bool,
}

impl ScriptInstance {
    pub fn new(
        instance_id: ScriptInstanceId,
        script_ref: CastMemberRef,
        script_def: &Script,
        lctx: &ScriptContext,
        symbols: &mut SymbolTable,
    ) -> ScriptInstance {
        let mut properties = FxHashMap::default();

        for prop_name_id in &script_def.chunk.property_name_ids {
            let prop_name = lctx.names.get(*prop_name_id as usize);

            let prop_symbol = if let Some(prop_name) = prop_name {
                symbols.intern(prop_name)
            } else {
                symbols.intern(&format!("prop_{}", prop_name_id))
            };
            properties.insert(prop_symbol, DatumRef::Void);
        }

        ScriptInstance {
            instance_id,
            script: script_ref,
            ancestor: None,
            properties,
            begin_sprite_called: false,
        }
    }
}

impl Script {
    pub fn get_own_handler_ref_at(&self, index: usize) -> Option<ScriptHandlerRef> {
        self.handler_names
            .get(index)
            .map(|x| (self.member_ref.clone(), x.clone()))
    }

    pub fn get_own_handler(&self, name: Symbol) -> Option<&Rc<HandlerDef>> {
        self.handlers.get(&name)
    }

    pub fn get_own_handler_by_local_name_id(&self, name_id: u16) -> Option<&Rc<HandlerDef>> {
        self.handlers
            .iter()
            .find(|x| x.1.name_id == name_id)
            .map(|x| x.1)
    }

    pub fn get_handler(&self, name: Symbol) -> Option<ScriptHandlerRefDef> {
        return self
            .get_own_handler(name)
            .map(|x| (self.member_ref.clone(), x));
    }

    pub fn get_own_handler_ref(&self, name: Symbol) -> Option<ScriptHandlerRef> {
        self.get_own_handler(name.clone())
            .map(|_| (self.member_ref.clone(), name.clone()))
    }
}

pub type ScriptHandlerRef = (CastMemberRef, Symbol);

#[inline]
fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            )
        }),
    }?;
    if let Datum::ScriptInstanceRef(instance_ref) = datum {
        player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef".to_owned(),
                )
            })?;
    }
    Ok(datum)
}

pub fn script_get_prop_opt(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    script_instance_ref: &ScriptInstanceRef,
    prop_name: Symbol,
) -> Result<Option<DatumRef>, ScriptError> {
    symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
    player
        .allocator
        .get_script_instance_opt(script_instance_ref)
        .ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale ScriptInstanceRef".to_owned(),
            )
        })?;

    // Check virtual script handler first
    match super::virtual_scripts::VirtualScriptRegistry::try_get_instance_prop(
        player,
        symbols,
        script_instance_ref,
        prop_name.clone(),
    ) {
        Ok(Some(datum_ref)) => {
            let datum = checked_datum(player, &datum_ref)?;
            crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
            return Ok(Some(datum_ref));
        }
        Ok(None) => {}
        Err(error) if error.code == crate::player::ScriptErrorCode::InvalidReference => {
            return Err(error);
        }
        Err(_) => {}
    }

    // A virtual handler may re-enter the player and invalidate the arena, so
    // validate the receiver again before touching its properties.
    let script_instance = player
        .allocator
        .get_script_instance_opt(script_instance_ref)
        .ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale ScriptInstanceRef".to_owned(),
            )
        })?;

    // Resolve the builtin identity ONCE. `eq_builtin` calls `into_builtin`,
    // which is a `spur_to_builtin` hash lookup — so testing ancestor/script/ilk
    // separately cost three hash lookups on every property read, before the
    // instance's own properties were even consulted. Most property names are
    // not builtins, so this usually resolves to `None` and skips all three.
    match prop_name.into_builtin() {
        Some(BuiltInSymbol::Ancestor) => {
            let script_instance = player.allocator.get_script_instance(&script_instance_ref);
            if let Some(ancestor_id) = &script_instance.ancestor {
                return Ok(Some(
                    player.alloc_datum(Datum::ScriptInstanceRef(ancestor_id.clone())),
                ));
            } else {
                return Ok(Some(DatumRef::Void));
            }
        }
        Some(BuiltInSymbol::Script) => {
            let script_instance = player.allocator.get_script_instance(&script_instance_ref);
            return Ok(Some(
                player.alloc_datum(Datum::ScriptRef(script_instance.script.clone())),
            ));
        }
        Some(BuiltInSymbol::Ilk) => {
            return Ok(Some(player.alloc_datum(Datum::Symbol(Symbol::builtin(
                BuiltInSymbol::Instance,
            )))));
        }
        _ => {}
    }

    // Try to find the property on the current instance first
    if let Some(prop) = script_instance.properties.get(&prop_name) {
        return Ok(Some(prop.clone()));
    }

    // Check ancestor for the property
    if script_instance.ancestor.is_some() {
        let ancestor_ref = script_instance.ancestor.as_ref().unwrap().clone();
        if let Some(result) =
            script_get_prop_opt(player, symbols, &ancestor_ref, prop_name.clone())?
        {
            return Ok(Some(result));
        }
    }

    // The ancestor lookup may invoke a virtual getter. Revalidate the
    // original receiver before falling back to receiver-owned built-ins;
    // that callback can have reset or replaced the allocator arena.
    let script_instance = player
        .allocator
        .get_script_instance_opt(script_instance_ref)
        .ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale ScriptInstanceRef".to_owned(),
            )
        })?;

    // Fall back to built-in properties if not found in instance or ancestors
    if prop_name.eq_builtin(BuiltInSymbol::Class) || prop_name.eq_builtin(BuiltInSymbol::Script) {
        return Ok(Some(
            player.alloc_datum(Datum::ScriptRef(script_instance.script.clone())),
        ));
    }

    Ok(None)
}

pub fn script_get_static_prop(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    script_ref: &CastMemberRef,
    prop_name: Symbol,
) -> Result<DatumRef, ScriptError> {
    let prop_name_text = symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
        .to_owned();
    let script_rc = match player.movie.cast_manager.get_script_by_ref(&script_ref) {
        Some(script_rc) => script_rc,
        None => {
            return Err(ScriptError::new(format!(
                "Cannot get static property {} — script not found ({}:{})",
                prop_name_text, script_ref.cast_lib, script_ref.cast_member
            )))
        }
    };
    let script = script_rc.as_ref();
    let properties = script.properties.borrow();
    if let Some(prop) = properties.get(&prop_name) {
        let prop = prop.clone();
        let datum = checked_datum(player, &prop)?;
        crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
        Ok(prop)
    } else {
        Err(ScriptError::new(format!(
            "Cannot get static property {} on script {}",
            prop_name_text, script.name
        )))
    }
}

pub fn script_set_static_prop(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    script_ref: &CastMemberRef,
    prop_name: Symbol,
    value_ref: &DatumRef,
    required: bool,
) -> Result<(), ScriptError> {
    symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
    let script_rc = match player.movie.cast_manager.get_script_by_ref(&script_ref) {
        Some(script_rc) => script_rc,
        None => {
            // A real movie should never reach here: the trampoline's per-opcode
            // generation guard aborts a handler whose scope was reset by a movie
            // change before it can run an op like this. If it does fire, surface
            // it (don't panic as the old code did) so the underlying cause is
            // visible rather than masked.
            return Err(ScriptError::new(format!(
                "Cannot set static property {} — script not found ({}:{})",
                symbols.display(&prop_name).unwrap_or("<foreign symbol>"),
                script_ref.cast_lib,
                script_ref.cast_member
            )));
        }
    };
    let script = script_rc.as_ref();
    let mut properties = script.properties.borrow_mut();

    if required && !properties.contains_key(&prop_name) {
        return Err(ScriptError::new(format!(
            "Cannot set static property {} on script {}",
            symbols.display(&prop_name).unwrap_or("<foreign symbol>"),
            script.name
        )));
    } else {
        let value = checked_datum(player, value_ref)?.clone();
        crate::player::compare::validate_direct_symbol_fields(&value, symbols)?;
        if let Datum::ScriptInstanceRef(value_instance_ref) = &value {
            player
                .allocator
                .get_script_instance_opt(value_instance_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "foreign or stale ScriptInstanceRef value".to_owned(),
                    )
                })?;
        }
        properties.insert(prop_name.clone(), value_ref.clone());
        Ok(())
    }
}

pub fn script_get_prop(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    script_instance_ref: &ScriptInstanceRef,
    prop_name: Symbol,
) -> Result<DatumRef, ScriptError> {
    if let Some(prop) =
        script_get_prop_opt(player, symbols, script_instance_ref, prop_name.clone())?
    {
        let datum = checked_datum(player, &prop)?;
        crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
        Ok(prop)
    } else if prop_name == Symbol::builtin(BuiltInSymbol::Count) {
        // In Director, .count on a non-list object returns 1
        Ok(player.alloc_datum(Datum::Int(1)))
    } else if prop_name == Symbol::builtin(BuiltInSymbol::SpriteNum) {
        // spriteNum is a built-in property for behaviors — if not explicitly set,
        // look up which sprite channel this instance belongs to.
        // Resolve sprite ownership from the live/cached scriptInstanceList.
        let stage_channel_snapshots: Vec<(i16, i32, Vec<ScriptInstanceRef>)> = player
            .movie
            .score
            .channels
            .iter()
            .map(|channel| {
                (
                    channel.sprite.number as i16,
                    channel.sprite.number as i32,
                    channel.sprite.script_instance_list.clone(),
                )
            })
            .collect();
        for (sprite_id, channel_number, fallback) in stage_channel_snapshots {
            let instance_ids = get_sprite_script_instance_ids_checked(
                player,
                symbols,
                sprite_id,
                fallback.as_slice(),
            )?;
            if instance_ids
                .iter()
                .any(|si| si.id() == script_instance_ref.id())
            {
                let datum_ref = player.alloc_datum(Datum::Int(channel_number));
                return Ok(datum_ref);
            }
        }
        // Also check the cache — behaviors may be in cache but not in script_instance_list Vec
        Ok(player.alloc_datum(Datum::Int(0)))
    } else {
        // Director silently returns VOID when reading a property that doesn't
        // exist on an instance (or anywhere in its ancestor chain) — many
        // Shockwave movies rely on this, e.g. `repeat with x in me.oItem.someList`
        // where `someList` is only populated in some code paths. Raising a
        // ScriptError here breaks those movies even though they ran fine in
        // original Director. Log once per miss so real typos are still noticeable
        // in the console.
        let script_instance = player
            .allocator
            .get_script_instance_opt(script_instance_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef".to_owned(),
                )
            })?;
        let valid_props: Vec<Symbol> = script_instance.properties.keys().cloned().collect();
        warn!(
            "script_get_prop: undefined property '{}' on {} → returning VOID. Valid properties: {}",
            symbols.display(&prop_name).unwrap_or("<foreign symbol>"),
            match format_concrete_datum(
                &Datum::ScriptInstanceRef(script_instance_ref.clone()),
                symbols,
                player,
            ) {
                Ok(text) => text,
                Err(error) => format!("<format error: {error}>"),
            },
            valid_props
                .iter()
                .map(|name| symbols.display(name).unwrap_or("<foreign symbol>"))
                .join(", ")
        );
        Ok(DatumRef::Void)
    }
}

pub fn script_set_prop(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    script_instance_ref: &ScriptInstanceRef,
    prop_name: Symbol,
    value_ref: &DatumRef,
    required: bool,
) -> Result<(), ScriptError> {
    symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
    player
        .allocator
        .get_script_instance_opt(script_instance_ref)
        .ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale ScriptInstanceRef".to_owned(),
            )
        })?;
    let value = checked_datum(player, value_ref)?.clone();
    crate::player::compare::validate_direct_symbol_fields(&value, symbols)?;
    if let Datum::ScriptInstanceRef(value_instance_ref) = &value {
        player
            .allocator
            .get_script_instance_opt(value_instance_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef value".to_owned(),
                )
            })?;
    }
    // Check virtual script handler first
    match super::virtual_scripts::VirtualScriptRegistry::try_set_instance_prop(
        player,
        symbols,
        script_instance_ref,
        prop_name.clone(),
        value_ref,
    ) {
        Ok(Some(())) => return Ok(()),
        Err(e) => return Err(e),
        Ok(None) => {}
    }

    // Try to set the property on the current instance
    let result = {
        if prop_name == Symbol::builtin(BuiltInSymbol::Ancestor) {
            // Mirrors the `obj.ancestor = …` path in ScriptInstanceDatumHandlers.
            match value.to_owned() {
                // `ancestor = VOID` is a NO-OP: Director's ancestor property only
                // accepts an object, and assigning VOID neither detaches the
                // current one nor errors.
                //
                // Both halves matter. Erroring aborts the caller — battleready's
                // `class_Animation_Looped.destroy` ends with `ancestor = VOID`, so
                // the whole teardown died there. But actually DETACHING breaks
                // Habbo v7: `Thread Manager Class.buildThreadObj` wires the chain
                // with `tTemp` starting at VOID —
                //   tBase[#ancestor] = tThreadObj      -- link to the thread object
                //   repeat with tClass in tClassList   -- tBase is element 1
                //     tObject[#ancestor] = tTemp       -- VOID on the first pass!
                //     tTemp = tObject
                // — so a detaching VOID wipes the link that was just made and
                // `me.getInterface()` (defined on Thread Instance Class) becomes
                // unreachable from every component built on that thread.
                Datum::Void => Ok(()),
                Datum::ScriptInstanceRef(ancestor_id) => {
                    player
                        .allocator
                        .get_script_instance_opt(&ancestor_id)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                "foreign or stale ancestor ScriptInstanceRef".to_owned(),
                            )
                        })?;
                    let script_instance = player
                        .allocator
                        .get_script_instance_mut(&script_instance_ref);
                    script_instance.ancestor = Some(ancestor_id);
                    Ok(())
                }
                // Non-instance ancestors (e.g. a TimeoutInstance) live in the
                // properties map so calls can still be delegated to them.
                _ => {
                    let script_instance = player
                        .allocator
                        .get_script_instance_mut(&script_instance_ref);
                    script_instance
                        .properties
                        .insert(Symbol::builtin(BuiltInSymbol::Ancestor), value_ref.clone());
                    Ok(())
                }
            }
        } else {
            let updated = {
                let script_instance = player
                    .allocator
                    .get_script_instance_mut(&script_instance_ref);
                if let Some(prop) = script_instance.properties.get_mut(&prop_name) {
                    *prop = value_ref.clone();
                    true
                } else {
                    false
                }
            };
            if updated {
                Ok(())
            } else {
                let instance_text = match format_concrete_datum(
                    &Datum::ScriptInstanceRef(script_instance_ref.clone()),
                    symbols,
                    player,
                ) {
                    Ok(text) => text,
                    Err(error) => format!("<format error: {error}>"),
                };
                Err(ScriptError::new(format!(
                    "Cannot set property {} found on script instance {}",
                    symbols.display(&prop_name).unwrap_or("<foreign symbol>"),
                    instance_text
                )))
            }
        }
    };
    // If the property was not found on the current instance, try to set it on the ancestor
    let result = match result {
        Ok(_) => Ok(()),
        Err(err) if err.code != crate::player::ScriptErrorCode::InvalidReference => {
            let script_instance = player
                .allocator
                .get_script_instance_opt(&script_instance_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "foreign or stale ScriptInstanceRef".to_owned(),
                    )
                })?;
            if let Some(ancestor_id) = &script_instance.ancestor {
                script_set_prop(
                    player,
                    symbols,
                    &ancestor_id.clone(),
                    prop_name.clone(),
                    value_ref,
                    true,
                )
            } else {
                Err(ScriptError::new("No ancestor found".to_string()))
            }
        }
        Err(err) if err.code == crate::player::ScriptErrorCode::InvalidReference => Err(err),
        Err(err) => Err(err),
    };
    let result = match result {
        Ok(_) => Ok(()),
        Err(err) => {
            if err.code == crate::player::ScriptErrorCode::InvalidReference {
                Err(err)
            } else if required {
                Err(err)
            } else {
                let script_instance = player
                    .allocator
                    .get_script_instance_mut(&script_instance_ref);
                script_instance
                    .properties
                    .insert(prop_name.clone(), value_ref.clone());
                Ok(())
            }
        }
    };

    result.map_err(|err| {
        if err.code == crate::player::ScriptErrorCode::InvalidReference {
            return err;
        }
        let instance_text = match format_concrete_datum(
            &Datum::ScriptInstanceRef(script_instance_ref.clone()),
            symbols,
            player,
        ) {
            Ok(text) => text,
            Err(error) => format!("<format error: {error}>"),
        };
        ScriptError::new(format!(
            "Error setting property {} on script instance {}: {}",
            symbols.display(&prop_name).unwrap_or("<foreign symbol>"),
            instance_text,
            err.message
        ))
    })
}

pub fn get_current_scope<'a>(
    player: &'a DirPlayer,
    ctx: &'a BytecodeHandlerContext,
) -> Option<&'a Scope> {
    player.scopes.get(ctx.scope_ref())
}

pub fn get_current_script(ctx: &BytecodeHandlerContext) -> &Script {
    ctx.code.script.as_ref()
}

pub fn get_current_handler_def(ctx: &BytecodeHandlerContext) -> &HandlerDef {
    ctx.code.handler.as_ref()
}

pub fn get_current_variable_multiplier(ctx: &BytecodeHandlerContext) -> u32 {
    ctx.multiplier
}

pub fn get_lctx_for_script<'a>(
    player: &'a DirPlayer,
    script: &'a Script,
) -> Option<&'a ScriptContext> {
    let cast = player
        .movie
        .cast_manager
        .get_cast(script.member_ref.cast_lib as u32)
        .unwrap();
    return cast.lctx.as_ref();
}

#[derive(Debug)]
pub enum SetObjPropOutcome {
    Applied,
    AwaitCastLoad(CastLoadRequest),
    FlashSet(crate::player::handlers::datum_handlers::flash_object::FlashSetPropertyRequest),
    JsObject {
        receiver: DatumRef,
        name: Symbol,
        value: DatumRef,
    },
}

fn checked_set_value(
    player: &DirPlayer,
    symbols: &SymbolTable,
    value_ref: &DatumRef,
) -> Result<Datum, ScriptError> {
    let value = checked_datum(player, value_ref)?.clone();
    crate::player::compare::validate_direct_symbol_fields(&value, symbols)?;
    Ok(value)
}

/// Set an object property while borrowing only the owning player and symbol
/// table.  A FileName network miss returns an owned request; the session must
/// apply it and invalidate Flash before resuming the opcode.
pub fn set_obj_prop_sync(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    obj_ref: &DatumRef,
    prop_name: Symbol,
    value_ref: &DatumRef,
    outbox: &mut CastNotificationOutbox,
) -> Result<SetObjPropOutcome, ScriptError> {
    let obj = checked_datum(player, obj_ref)?.clone();
    if matches!(obj, Datum::Void | Datum::Null) {
        return Ok(SetObjPropOutcome::Applied);
    }
    symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
    let applied = |result: Result<(), ScriptError>| result.map(|_| SetObjPropOutcome::Applied);
    match obj {
        Datum::CastLib(cast_lib) => {
            let value = checked_set_value(player, symbols, value_ref)?;
            let builtin = prop_name
                .into_builtin()
                .ok_or_else(|| ScriptError::new("castLib property must be a builtin".to_owned()))?;
            let prep = {
                let cast = player.movie.cast_manager.get_cast_mut(cast_lib);
                cast.set_prop(prop_name, value, &player.allocator, symbols, Some(outbox))?;
                if builtin == BuiltInSymbol::FileName {
                    Some(cast.prepare_property_load(
                        player.owner.key(),
                        player.net_manager.base_path.as_ref(),
                        player.net_manager.override_base_path.as_deref(),
                    ))
                } else {
                    None
                }
            };
            match prep {
                None | Some(PropertyLoadPreparation::Empty) => {
                    if builtin == BuiltInSymbol::FileName {
                        player.invalidate_flash_for_cast_lib(cast_lib as i32);
                    }
                    Ok(SetObjPropOutcome::Applied)
                }
                Some(PropertyLoadPreparation::Request(request)) => {
                    if let Some(file) = player.dir_cache.get(request.source_path()).cloned() {
                        let applied = player
                            .movie
                            .cast_manager
                            .get_cast_mut(cast_lib)
                            .apply_cached_property_file(
                                &request,
                                player.owner.key(),
                                file,
                                &mut player.bitmap_manager,
                                symbols,
                                outbox,
                            );
                        if !applied {
                            return Err(ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                "castLib load reservation was replaced".to_owned(),
                            ));
                        }
                        player.invalidate_flash_for_cast_lib(cast_lib as i32);
                        Ok(SetObjPropOutcome::Applied)
                    } else {
                        Ok(SetObjPropOutcome::AwaitCastLoad(request))
                    }
                }
            }
        }
        Datum::ScriptInstanceRef(script_instance_ref) => applied(script_set_prop(
            player,
            symbols,
            &script_instance_ref,
            prop_name,
            value_ref,
            false,
        )),
        Datum::SpriteRef(sprite_id) => applied(sprite_set_prop(
            player,
            symbols,
            sprite_id,
            prop_name,
            checked_set_value(player, symbols, value_ref)?,
        )),
        Datum::CastMember(member_ref) => applied(CastMemberRefHandlers::set_prop(
            player,
            symbols,
            &member_ref,
            prop_name,
            checked_set_value(player, symbols, value_ref)?,
        )),
        Datum::VectorVertexRef(member_ref, index) => {
            let value = checked_set_value(player, symbols, value_ref)?;
            let prop = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned();
            applied(crate::player::handlers::datum_handlers::cast_member::vector_shape::VectorShapeMemberHandlers::set_vertex_ref_prop(
                player, &member_ref, index, &prop, &value,
            ))
        }
        Datum::Stage => {
            checked_set_value(player, symbols, value_ref)?;
            applied(set_stage_prop(player, symbols, prop_name, value_ref))
        }
        Datum::BitmapRef(bitmap_ref) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(BitmapDatumHandlers::set_bitmap_ref_prop(
                player,
                symbols,
                &bitmap_ref,
                prop_name,
                value_ref,
            ))
        }
        Datum::Point(..) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(PointDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::TimeoutRef(_) | Datum::TimeoutInstance { .. } | Datum::TimeoutFactory => {
            checked_set_value(player, symbols, value_ref)?;
            applied(TimeoutDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::PropList(..) => {
            checked_set_value(player, symbols, value_ref)?;
            let key_ref = player.alloc_datum(Datum::Symbol(prop_name));
            applied(PropListUtils::set_prop(
                obj_ref, &key_ref, value_ref, player, symbols, false,
            ))
        }
        Datum::Rect(..) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(RectDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::StringChunk(..) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(StringChunkHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::ColorRef(..) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(ColorDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::PlayerRef => {
            checked_set_value(player, symbols, value_ref)?;
            applied(player.set_player_prop(symbols, prop_name, value_ref))
        }
        Datum::MouseRef => {
            checked_set_value(player, symbols, value_ref)?;
            applied(player.set_mouse_prop(prop_name, value_ref))
        }
        Datum::MovieRef => {
            let value = checked_set_value(player, symbols, value_ref)?;
            applied(player.set_movie_prop(symbols, prop_name, value))
        }
        Datum::ScriptRef(script_ref) => applied(script_set_static_prop(
            player,
            symbols,
            &script_ref,
            prop_name,
            value_ref,
            false,
        )),
        Datum::XmlRef(_) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(XmlDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::JsObjectRef(_) => Ok(SetObjPropOutcome::JsObject {
            receiver: obj_ref.clone(),
            name: prop_name,
            value: value_ref.clone(),
        }),
        Datum::DateRef(_) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(DateDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::MathRef(_) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(MathDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::Vector(..) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(VectorDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::SoundChannel(_) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(SoundChannelDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::FlashObjectRef(_) => {
            let value = checked_set_value(player, symbols, value_ref)?;
            Ok(SetObjPropOutcome::FlashSet(
                crate::player::handlers::datum_handlers::flash_object::prepare_set_prop(
                    player, symbols, obj_ref, prop_name, &value,
                )?,
            ))
        }
        Datum::Shockwave3dObjectRef(_) => {
            let value = checked_set_value(player, symbols, value_ref)?;
            let prop = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned();
            applied(crate::player::handlers::datum_handlers::shockwave3d_object::Shockwave3dObjectDatumHandlers::set_prop(
                player, symbols, obj_ref, &prop, &value,
            ))
        }
        Datum::Transform3d(_) => {
            checked_set_value(player, symbols, value_ref)?;
            applied(crate::player::handlers::datum_handlers::transform3d::Transform3dDatumHandlers::set_prop(
                player, symbols, obj_ref, prop_name, value_ref,
            ))
        }
        Datum::HavokObjectRef(_) => {
            checked_set_value(player, symbols, value_ref)?;
            let prop = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned();
            applied(crate::player::handlers::datum_handlers::havok_object::HavokObjectDatumHandlers::set_prop(
                player, symbols, obj_ref, &prop, value_ref.clone(),
            ))
        }
        Datum::PhysXObjectRef(_) => {
            checked_set_value(player, symbols, value_ref)?;
            let prop = symbols
                .display(&prop_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned();
            applied(crate::player::handlers::datum_handlers::physx_object::PhysXObjectDatumHandlers::set_prop(
                player, symbols, obj_ref, &prop, value_ref.clone(),
            ))
        }
        _ => {
            let description = format_datum(obj_ref, symbols, player)?;
            Err(ScriptError::new(format!(
                "set_obj_prop was passed an invalid datum: {description}"
            )))
        }
    }
}

pub fn get_obj_prop(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    obj_ref: &DatumRef,
    prop_name: Symbol,
) -> Result<DatumRef, ScriptError> {
    let prop_name_text = symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
        .to_owned();
    let obj_clone = checked_datum(player, obj_ref)?.clone();
    crate::player::compare::validate_direct_symbol_fields(&obj_clone, symbols)?;
    let prop_name_builtin = prop_name.into_builtin();

    // Universal type-check properties (work on any datum type)
    match prop_name_builtin {
        Some(BuiltInSymbol::Integerp) => {
            let is_int = matches!(obj_clone, Datum::Int(_));
            return Ok(player.alloc_datum(Datum::Int(if is_int { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Floatp) => {
            let is_float = matches!(obj_clone, Datum::Float(_));
            return Ok(player.alloc_datum(Datum::Int(if is_float { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Stringp) => {
            let is_string = matches!(obj_clone, Datum::String(_) | Datum::StringChunk(..));
            return Ok(player.alloc_datum(Datum::Int(if is_string { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Symbolp) => {
            let is_symbol = matches!(obj_clone, Datum::Symbol(_));
            return Ok(player.alloc_datum(Datum::Int(if is_symbol { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Listp) => {
            let is_list = matches!(obj_clone, Datum::List(..) | Datum::PropList(..));
            return Ok(player.alloc_datum(Datum::Int(if is_list { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Objectp) => {
            let is_obj = matches!(obj_clone, Datum::ScriptInstanceRef(_));
            return Ok(player.alloc_datum(Datum::Int(if is_obj { 1 } else { 0 })));
        }
        Some(BuiltInSymbol::Voidp) => {
            let is_void = matches!(obj_clone, Datum::Void);
            return Ok(player.alloc_datum(Datum::Int(if is_void { 1 } else { 0 })));
        }
        _ => {}
    }

    match obj_clone {
        Datum::CastLib(cast_lib) => {
            let cast_lib = player.movie.cast_manager.get_cast(cast_lib as u32)?;
            Ok(player.alloc_datum(cast_lib.get_prop(prop_name, symbols)?))
        }
        Datum::CastMember(member_ref) => {
            let result = CastMemberRefHandlers::get_prop(player, symbols, &member_ref, prop_name)?;
            Ok(player.alloc_datum(result))
        }
        // `member.vertex[i].handle1` etc. — read a sub-property of a
        // vectorShape vertex reference (produced by getPropRef).
        Datum::VectorVertexRef(member_ref, index) => {
            let result = crate::player::handlers::datum_handlers::cast_member::vector_shape::VectorShapeMemberHandlers
                ::get_vertex_ref_prop(player, &member_ref, index, &prop_name_text)?;
            Ok(player.alloc_datum(result))
        }
        Datum::ScriptInstanceRef(script_instance_id) => {
            script_get_prop(player, symbols, &script_instance_id, prop_name)
        }
        Datum::ScriptRef(script_ref) => script_get_static_prop(player, symbols, &script_ref, prop_name),
        Datum::PropList(prop_list, is_sorted) => {
            PropListUtils::get_prop_or_built_in(player, symbols, &prop_list, prop_name, is_sorted)
        }
        Datum::List(list_type, list, sorted) => {
            // Director: every datum has `.string` returning its textual form.
            // ListDatumUtils::get_prop can't format because it doesn't have
            // player context; intercept here. Mirrors the same intercept in
            // ListDatumHandlers::get_prop (which the bytecode `get_set` path
            // uses) — this script.rs path is reached by chained-prop access
            // patterns like `inList.string`.
            if prop_name_text.eq_ignore_ascii_case("string") {
                let datum_clone = Datum::List(list_type, list.clone(), sorted);
                let s = crate::player::datum_formatting::format_concrete_datum(&datum_clone, symbols, player)?;
                return Ok(player.alloc_datum(Datum::String(s)));
            }
            Ok(player.alloc_datum(ListDatumUtils::get_prop(
                &list,
                prop_name,
                &player.allocator,
                symbols,
            )?))
        }
        Datum::Stage => {
            let result = get_stage_prop(player, symbols, prop_name)?;
            Ok(player.alloc_datum(result))
        }
        Datum::Rect(..) => {
            Ok(player.alloc_datum(RectDatumHandlers::get_prop(player, symbols, obj_ref, prop_name)?))
        }
        Datum::Point(..) => {
            Ok(player.alloc_datum(PointDatumHandlers::get_prop(player, symbols, obj_ref, prop_name)?))
        }
        Datum::SpriteRef(sprite_id) => {
            let result = sprite_get_prop(player, symbols, sprite_id, prop_name)?;
            Ok(player.last_sprite_prop_ref.take()
                .unwrap_or_else(|| player.alloc_datum(result)))
        }
        Datum::BitmapRef(_) => BitmapDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::String(s) => {
            let value = StringDatumUtils::get_built_in_prop(player, symbols, &s, prop_name)?;
            Ok(player.alloc_datum(value))
        }
        Datum::StringChunk(ref source, ref chunk_expr, ref _str_val) => {
            match prop_name_builtin {
                Some(BuiltInSymbol::Count) => {
                    // Chunk count is `end - start + 1` over the chunk_expr.
                    // For the "whole collection" form produced by
                    // `string.line` etc. that becomes the line/word/item
                    // total. Director's `text.line.count`, `text.word.count`
                    // etc. read off this.
                    let n = (chunk_expr.end - chunk_expr.start + 1).max(0);
                    Ok(player.alloc_datum(Datum::Int(n)))
                }
                Some(BuiltInSymbol::Ref) => {
                    // .ref returns the chunk reference itself (a StringChunk datum)
                    Ok(obj_ref.clone())
                }
                Some(BuiltInSymbol::Range) => {
                    // .range returns point(startCharPos, endCharPos) — 1-based char positions in the source
                    use crate::player::handlers::datum_handlers::string_chunk::StringChunkUtils;
                    use crate::director::lingo::datum::StringChunkType;

                    let source_str = match source {
                        crate::director::lingo::datum::StringChunkSource::Datum(d) => {
                            checked_datum(player, d)?.string_value(symbols)?
                        },
                        crate::director::lingo::datum::StringChunkSource::Member(m) => {
                            let member = player.movie.cast_manager.find_member_by_ref(m)
                                .ok_or_else(|| ScriptError::new("Member not found for string chunk range".to_string()))?;
                            if let Some(field) = member.member_type.as_field() {
                                field.text.clone()
                            } else if let Some(text) = member.member_type.as_text() {
                                text.text.clone()
                            } else {
                                return Err(ScriptError::new("Member is not a text/field type".to_string()));
                            }
                        }
                    };

                    let chunk_list = StringChunkUtils::resolve_chunk_list(
                        &source_str,
                        chunk_expr.chunk_type.clone(),
                        chunk_expr.item_delimiter,
                    )?;

                    let (start_idx, end_idx_exclusive) = StringChunkUtils::vm_range_to_host(
                        (chunk_expr.start, chunk_expr.end),
                        chunk_list.len(),
                    );
                    // vm_range_to_host returns exclusive end; convert to inclusive for the loop below
                    let end_idx = if end_idx_exclusive > 0 { end_idx_exclusive - 1 } else { 0 };

                    // Calculate character positions based on chunk type
                    let (char_start, char_end) = match chunk_expr.chunk_type {
                        StringChunkType::Char => {
                            (start_idx as i32 + 1, end_idx_exclusive as i32)
                        }
                        _ => {
                            // For line/word/item, find character positions by summing chunk lengths + delimiters
                            let mut pos = 0usize;
                            let mut result_start = 0usize;
                            let delimiter_len = match chunk_expr.chunk_type {
                                StringChunkType::Line => {
                                    // Detect \r\n vs \r vs \n
                                    if source_str.contains("\r\n") { 2 } else { 1 }
                                }
                                StringChunkType::Item => 1, // delimiter char
                                StringChunkType::Word => 1, // whitespace
                                _ => 1,
                            };
                            for (i, chunk) in chunk_list.iter().enumerate() {
                                if i == start_idx {
                                    result_start = pos;
                                }
                                pos += chunk.chars().count();
                                if i == end_idx {
                                    break;
                                }
                                if i + 1 < chunk_list.len() {
                                    pos += delimiter_len;
                                }
                            }
                            let result_end = pos;
                            (result_start as i32 + 1, result_end as i32)
                        }
                    };

                    Ok(player.alloc_datum(Datum::Point([char_start as f64, char_end as f64], 0)))
                }
                Some(BuiltInSymbol::CharSpacing) => {
                    // Read charSpacing from the source member's styled spans, walking the source chain
                    if let Datum::StringChunk(ref source, _, _) = obj_clone {
                        let mut current_source = source.clone();
                        loop {
                            match current_source {
                                crate::director::lingo::datum::StringChunkSource::Member(ref member_ref) => {
                                    if let Some(member) = player.movie.cast_manager.find_member_by_ref(member_ref) {
                                        if let Some(text) = member.member_type.as_text() {
                                            return Ok(player.alloc_datum(Datum::Int(text.char_spacing)));
                                        }
                                    }
                                    break;
                                }
                                crate::director::lingo::datum::StringChunkSource::Datum(ref d) => {
                                    let inner = checked_datum(player, d)?.clone();
                                    if let Datum::StringChunk(inner_source, _, _) = inner {
                                        current_source = inner_source;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Ok(player.alloc_datum(Datum::Int(0)))
                }
                _ if matches!(prop_name_builtin,
                    Some(BuiltInSymbol::FixedLineSpace | BuiltInSymbol::TopSpacing | BuiltInSymbol::BottomSpacing
                    | BuiltInSymbol::Font | BuiltInSymbol::FontSize | BuiltInSymbol::FontStyle | BuiltInSymbol::TextStyle
                    | BuiltInSymbol::Color | BuiltInSymbol::BgColor | BuiltInSymbol::Alignment)) => {
                    // Per-chunk properties — walk to the source member and
                    // resolve the chunk's char range, then look up the
                    // active par_info / styled span. Director exposes
                    // `member.line[N].fixedLineSpace` etc.; without this
                    // branch the StringChunk would fall through to the
                    // string built-in handler and return Void / 0.
                    use crate::player::handlers::datum_handlers::string_chunk::StringChunkHandlers;
                    let resolved = StringChunkHandlers::walk_chunk_to_member_range(player, symbols, obj_ref)?;
                    let Some((member_ref, char_start, _char_end)) = resolved else {
                        let value = obj_clone.string_value(symbols)?;
                        let result = StringDatumUtils::get_built_in_prop(
                            player, symbols, &value, Symbol::builtin(prop_name_builtin.unwrap())
                        )?;
                        return Ok(player.alloc_datum(result));
                    };
                    let Some(member) = player.movie.cast_manager.find_member_by_ref(&member_ref) else {
                        return Ok(player.alloc_datum(Datum::Void));
                    };
                    // Field path: per-character styles live in the STXT
                    // formatting_runs (start_position + style byte).
                    // Director's `the textStyle of char N of member`
                    // returns a comma-separated string like "underline"
                    // or "bold,italic"; `fontStyle` returns a list of
                    // symbols. The Fugue No.4 Narrative script branches
                    // on `the textStyle of char ... = "underline"` so
                    // the string form has to work for fields. Other
                    // chunk-level field props still fall back to the
                    // member-wide values (fields don't have per-line
                    // par_infos like text members do).
                    // Snapshot the cast lib's font_table so font_id → name
                    // lookups inside the field branch don't need to re-borrow
                    // cast_manager (which is already lent immutably via the
                    // `member` borrow above). The font_table is the only
                    // authoritative mapping from STXT formatting_run.font_id
                    // to a resolved font name like "Arial Italic" — bit
                    // heuristics on font_id mis-tag bold vs italic depending
                    // on how the file packed its table.
                    let font_table_snapshot: std::collections::HashMap<u16, String> = player
                        .movie
                        .cast_manager
                        .get_cast(member_ref.cast_lib as u32)
                        .map(|cl| cl.font_table.clone())
                        .unwrap_or_default();
                    let resolve_run_style = |font_id: u16, style_byte: u8| -> (bool, bool, Option<String>) {
                        let resolved = font_table_snapshot.get(&font_id).cloned();
                        let name_bold = resolved.as_ref()
                            .map(|n| n.to_ascii_lowercase().contains("bold"))
                            .unwrap_or(false);
                        let name_italic = resolved.as_ref()
                            .map(|n| n.to_ascii_lowercase().contains("italic"))
                            .unwrap_or(false);
                        let bold = (style_byte & 0x01) != 0 || name_bold;
                        let italic = (style_byte & 0x02) != 0 || name_italic;
                        (bold, italic, resolved)
                    };
                    if let Some(field) = member.member_type.as_field() {
                        let lc = prop_name_text.to_ascii_lowercase();
                        if lc == "textstyle" || lc == "fontstyle" {
                            // STXT formatting_runs use BYTE positions, not
                            // char positions. Convert char_start -> byte
                            // offset so multi-byte chars (ñ/ç/é in
                            // Narrative's Spanish/Portuguese text) don't
                            // shift the run-boundary lookup by 1 char per
                            // multi-byte char before the queried char.
                            let byte_start: usize = field.text
                                .char_indices()
                                .nth(char_start)
                                .map(|(b, _)| b)
                                .unwrap_or_else(|| field.text.len());
                            let active_run = field.formatting_runs.iter()
                                .rev()
                                .find(|r| (r.start_position as usize) <= byte_start);
                            let (active_style, active_font_id) = active_run
                                .map(|r| (r.style, r.font_id))
                                .unwrap_or((0, 0));
                            // Resolve via the file's font_table (the only
                            // authoritative source). The STXT `style` byte
                            // can also independently signal bold/italic on
                            // movies that ship just one Arial cast member
                            // and rely on style bits — OR with name match.
                            let (bold, italic, _resolved) =
                                resolve_run_style(active_font_id, active_style);
                            let underline = (active_style & 0x04) != 0;
                            if lc == "textstyle" {
                                // Director string form (legacy `the textStyle`).
                                let mut parts: Vec<&str> = Vec::new();
                                if bold { parts.push("bold"); }
                                if italic { parts.push("italic"); }
                                if underline { parts.push("underline"); }
                                let s = if parts.is_empty() {
                                    "plain".to_string()
                                } else {
                                    parts.join(",")
                                };
                                return Ok(player.alloc_datum(Datum::String(s)));
                            } else {
                                // fontStyle list form.
                                let mut items = std::collections::VecDeque::new();
                                if bold {
                                    items.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Bold))));
                                }
                                if italic {
                                    items.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Italic))));
                                }
                                if underline {
                                    items.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Underline))));
                                }
                                if items.is_empty() {
                                    items.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Plain))));
                                }
                                return Ok(player.alloc_datum(Datum::List(
                                    crate::director::lingo::datum::DatumType::List,
                                    items,
                                    false,
                                )));
                            }
                        }
                        // `the font of char N..M of member` returns the
                        // font name of those chars. Director synthesizes
                        // a "FontName Bold *" / "FontName Italic *" name
                        // when the run has bold/italic style bits set —
                        // Fugue No.4 Cues#AdvanceScroll relies on this:
                        // `if member(nar).line[o].char[1..x].font =
                        //   "Arial Bold *"` to detect bold-styled
                        // hyperlink anchors. Look up the active run at
                        // char_start in field.formatting_runs and emit
                        // the synthesized name with the field's base
                        // font as prefix.
                        if lc == "font" {
                            let byte_start: usize = field.text
                                .char_indices()
                                .nth(char_start)
                                .map(|(b, _)| b)
                                .unwrap_or_else(|| field.text.len());
                            let active_run = field.formatting_runs.iter().rev()
                                .find(|r| (r.start_position as usize) <= byte_start);
                            let (active_style, active_font_id) = active_run
                                .map(|r| (r.style, r.font_id))
                                .unwrap_or((0, 0));
                            let (bold, italic, resolved_name) =
                                resolve_run_style(active_font_id, active_style);
                            // Prefer the font_table's resolved name (e.g.
                            // "Arial Italic") so the AdvanceScroll-style
                            // `if .font = "Arial Bold *"` comparisons see
                            // the exact variant the file authored. If the
                            // table doesn't list this id (fallback path),
                            // synthesise from the field's base font + the
                            // bold/italic flags we just derived.
                            let base_for_synth = field.font.trim_end_matches(" *")
                                .trim_end_matches('*')
                                .trim()
                                .to_string();
                            let s = if let Some(name) = resolved_name {
                                let trimmed = name.trim_end_matches(" *")
                                    .trim_end_matches('*')
                                    .trim()
                                    .to_string();
                                format!("{} *", trimmed)
                            } else {
                                let mut parts: Vec<&str> = Vec::new();
                                let base_owned = if base_for_synth.is_empty() { "Arial".to_string() } else { base_for_synth };
                                parts.push(base_owned.as_str());
                                if bold { parts.push("Bold"); }
                                if italic { parts.push("Italic"); }
                                format!("{} *", parts.join(" "))
                            };
                            return Ok(player.alloc_datum(Datum::String(s)));
                        }
                        let value = obj_clone.string_value(symbols)?;
                        let result = StringDatumUtils::get_built_in_prop(
                            player, symbols, &value, Symbol::builtin(prop_name_builtin.unwrap())
                        )?;
                        return Ok(player.alloc_datum(result));
                    }
                    let Some(text) = member.member_type.as_text() else {
                        return Ok(player.alloc_datum(Datum::Void));
                    };
                    // Look up the par_info active at the chunk's start
                    // position — line N's first character. par_run.position
                    // values reference text-character offsets, same as
                    // chunk char_start, so a direct walk works.
                    let mut active_idx: Option<u16> = None;
                    let pos = char_start as u32;
                    for run in &text.par_runs {
                        if run.position <= pos {
                            active_idx = Some(run.par_info_index);
                        } else {
                            break;
                        }
                    }
                    let par_info = active_idx
                        .and_then(|idx| text.par_infos.get(idx as usize))
                        .cloned();

                    // Locate the styled span containing the chunk's start
                    // char and snapshot its style fields — `player` is
                    // mutably borrowed below for `alloc_datum` so we can't
                    // hold a reference into the cast member at the same
                    // time.
                    let mut cum = 0usize;
                    let active_span = text.html_styled_spans.iter().find(|span| {
                        let span_chars = span.text.chars().count();
                        let end = cum + span_chars;
                        let hit = pos as usize >= cum && (pos as usize) < end.max(cum + 1);
                        cum = end;
                        hit
                    }).or_else(|| text.html_styled_spans.first());
                    let span_font_face: Option<String> = active_span
                        .and_then(|s| s.style.font_face.clone());
                    let span_font_size: Option<i32> = active_span
                        .and_then(|s| s.style.font_size);
                    let span_bold = active_span.map(|s| s.style.bold).unwrap_or(false);
                    let span_italic = active_span.map(|s| s.style.italic).unwrap_or(false);
                    let span_underline = active_span.map(|s| s.style.underline).unwrap_or(false);
                    let span_color: Option<u32> = active_span.and_then(|s| s.style.color);
                    let member_color = member.color.clone();
                    let member_bg_color = member.bg_color.clone();
                    let member_text_font = text.font.clone();
                    let member_text_font_size = text.font_size as i32;
                    let member_text_alignment = text.alignment.clone();
                    let par_infos_snapshot: Vec<i32> = text.par_infos
                        .iter()
                        .map(|pi| pi.line_spacing)
                        .collect();

                    // Drop the read borrow on `member` / `text` before
                    // touching `player.alloc_datum` (which needs `&mut player`).
                    drop(member);

                    match prop_name_builtin.unwrap() {
                        BuiltInSymbol::FixedLineSpace => {
                            // Per-line line_spacing with the same "0 means
                            // inherit / use document default" fallback the
                            // renderer applies — Director's getter returns
                            // the MAX non-zero line_spacing across the
                            // member's par_infos when this line's own value
                            // is 0. Junkbot v1 level.num: par_infos =
                            // [0, 16, 21, 0]; line[1] resolves to 0 →
                            // fallback → 21 (matches Director).
                            let val = par_info
                                .as_ref()
                                .map(|pi| pi.line_spacing)
                                .filter(|&s| s != 0)
                                .or_else(|| par_infos_snapshot.iter()
                                    .copied()
                                    .filter(|&s| s != 0)
                                    .max())
                                .unwrap_or(0);
                            Ok(player.alloc_datum(Datum::Int(val)))
                        },
                        BuiltInSymbol::TopSpacing => {
                            let val = par_info.as_ref().map(|pi| pi.top_spacing).unwrap_or(0);
                            Ok(player.alloc_datum(Datum::Int(val)))
                        },
                        BuiltInSymbol::BottomSpacing => {
                            let val = par_info.as_ref().map(|pi| pi.bottom_spacing).unwrap_or(0);
                            Ok(player.alloc_datum(Datum::Int(val)))
                        },
                        BuiltInSymbol::Alignment => {
                            let val = par_info.as_ref().map(|pi| pi.justification).unwrap_or(0);
                            let s = match val {
                                1 => BuiltInSymbol::Center,
                                2 => BuiltInSymbol::Right,
                                3 => BuiltInSymbol::Justify,
                                _ => member_text_alignment,
                            };
                            Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(s))))
                        },
                        BuiltInSymbol::Font => {
                            let val = span_font_face.unwrap_or(member_text_font);
                            Ok(player.alloc_datum(Datum::String(val)))
                        },
                        BuiltInSymbol::FontSize => {
                            let val = span_font_size.unwrap_or(member_text_font_size);
                            Ok(player.alloc_datum(Datum::Int(val)))
                        },
                        BuiltInSymbol::FontStyle => {
                            let mut item_refs = std::collections::VecDeque::new();
                            if span_bold {
                                item_refs.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Bold))));
                            }
                            if span_italic {
                                item_refs.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Italic))));
                            }
                            if span_underline {
                                item_refs.push_back(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Underline))));
                            }
                            Ok(player.alloc_datum(Datum::List(crate::director::lingo::datum::DatumType::List, item_refs, false)))
                        },
                        BuiltInSymbol::Color => {
                            let color_ref = if let Some(c) = span_color {
                                crate::player::sprite::ColorRef::Rgb(
                                    ((c >> 16) & 0xFF) as u8,
                                    ((c >> 8) & 0xFF) as u8,
                                    (c & 0xFF) as u8,
                                )
                            } else {
                                member_color
                            };
                            Ok(player.alloc_datum(Datum::ColorRef(color_ref)))
                        },
                        BuiltInSymbol::BgColor => {
                            Ok(player.alloc_datum(Datum::ColorRef(member_bg_color)))
                        },
                        _ => {
                            let value = obj_clone.string_value(symbols)?;
                            let result = StringDatumUtils::get_built_in_prop(
                                player, symbols, &value, Symbol::builtin(prop_name_builtin.unwrap()),
                            )?;
                            Ok(player.alloc_datum(result))
                        },
                    }
                }
                _ => {
                    let value = obj_clone.string_value(symbols)?;
                    let result = StringDatumUtils::get_built_in_prop(
                        player, symbols, &value,
                        crate::player::symbols::symbol::Symbol::builtin(prop_name_builtin.unwrap()),
                    )?;
                    Ok(player.alloc_datum(result))
                },
            }
        }
        Datum::TimeoutRef(_) | Datum::TimeoutInstance { .. } | Datum::TimeoutFactory
             => Ok(TimeoutDatumHandlers::get_prop(player, symbols, obj_ref, prop_name)?),
        Datum::Symbol(_) => SymbolDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::Void => VoidDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::Int(_) => IntDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::Float(_) => FloatDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::ColorRef(_) => ColorDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::PlayerRef => player.get_player_prop(symbols, prop_name),
        Datum::MouseRef => player.get_mouse_prop(prop_name),
        Datum::XmlRef(_) => XmlDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::JsObjectRef(_) => Err(ScriptError::new(
            "owner-bound JS object property requires the pending executor".to_owned(),
        )),
        Datum::DateRef(_) => DateDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::MathRef(_) => MathDatumHandlers::get_prop(player, symbols, obj_ref, prop_name),
        Datum::Vector(_) => {
            Ok(player.alloc_datum(VectorDatumHandlers::get_prop(player, symbols, obj_ref, prop_name)?))
        }
        Datum::SoundChannel(_) => Ok(player.alloc_datum(SoundChannelDatumHandlers::get_prop(
            player, symbols, obj_ref, prop_name,
        )?)),
        Datum::MovieRef => player.get_movie_prop(symbols, prop_name),
        Datum::FlashObjectRef(_) => {
            crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::get_prop(obj_ref, &prop_name_text)
        }
        Datum::Shockwave3dObjectRef(_) => {
            let prop_name_text = prop_name_text.to_owned();
            crate::player::handlers::datum_handlers::shockwave3d_object::Shockwave3dObjectDatumHandlers::get_prop(
                player,
                symbols,
                obj_ref,
                &prop_name_text,
            )
        }
        Datum::Transform3d(_) => {
            let result = crate::player::handlers::datum_handlers::transform3d::Transform3dDatumHandlers::get_prop(player, symbols, obj_ref, prop_name)?;
            Ok(player.alloc_datum(result))
        }
        Datum::HavokObjectRef(_) => {
            crate::player::handlers::datum_handlers::havok_object::HavokObjectDatumHandlers::get_prop(player, symbols, obj_ref, &prop_name_text)
        }
        Datum::PhysXObjectRef(_) => {
            crate::player::handlers::datum_handlers::physx_object::PhysXObjectDatumHandlers::get_prop(player, symbols, obj_ref, &prop_name_text)
        }
        // `xtra(i).name` — the classic feature-detect idiom, paired with
        // `the number of xtras`. NOTE: the Director 11.5 Scripting Dictionary
        // documents `xtra()` and the `xtraList` properties but has no entry
        // for a `name` property on an Xtra reference (it survives from the
        // D6/D7 `the name of xtra n` form), so this contract is INFERRED from
        // calling movies, not specified. The value matches the `#name` key
        // that `_player.xtraList` / `the xtraList` report for the same Xtra.
        Datum::Xtra(xtra_name) => {
            if prop_name_text.eq_ignore_ascii_case("name") {
                Ok(player.alloc_datum(Datum::String(xtra_name)))
            } else {
                Err(ScriptError::new(format!(
                    "Unknown xtra prop {} on xtra \"{}\"",
                    prop_name_text, xtra_name
                )))
            }
        }
        _ => {
            if prop_name_builtin == Some(BuiltInSymbol::Ilk) {
                let ilk = TypeUtils::get_datum_ilk(&obj_clone)?;
                Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(ilk))))
            } else {
                let obj_text = match format_datum(obj_ref, symbols, player) {
                    Ok(text) => text,
                    Err(error) => format!("<format error: {error}>"),
                };
                Err(ScriptError::new(
                    format!(
                        "get_obj_prop(\"{}\") was passed an invalid datum: {}",
                        prop_name_text,
                        obj_text
                    )
                    .to_string(),
                ))
            }
        }
    }
}
