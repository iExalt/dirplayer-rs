use log::{warn, debug};
use wasm_bindgen::prelude::*;

use super::{
    cast::CastHandlers,
    datum_handlers::{
        bitmap::BitmapDatumHandlers,
        list_handlers::ListDatumHandlers,
        point::PointDatumHandlers,
        prop_list::PropListDatumHandlers,
        rect::RectDatumHandlers,
        script_instance::ScriptInstanceDatumHandlers,
        sound_channel::SoundChannelDatumHandlers,
    },
    movie::MovieHandlers,
    net::NetHandlers,
    string::StringHandlers,
    types::TypeHandlers,
};
use std::collections::{HashMap, VecDeque};
use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};
use rand::Rng;

use crate::{
    director::lingo::datum::{Datum, DatumType, datum_bool},
    js_api::JsApi,
    player::{
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode, bitmap::bitmap::{Bitmap, PaletteRef, get_system_default_palette}, datum_formatting::{format_concrete_datum, format_datum}, geometry::IntRect, handlers::datum_handlers::xml::XmlHelper, keyboard_map, owner_key_string, reserve_player_mut, reserve_player_ref, score::{get_concrete_sprite_rect, get_sprite_rect_in_context}, session::ExecutionContext, symbols::symbol_table::SymbolTable, trace_output, xtra::manager::call_xtra_instance_handler
    },
};

// JS bridge names use the `dirplayer_` prefix so this fork's globals don't
// collide with stock Ruffle if both are loaded on the same page (e.g. via a
// browser extension). Matching JS-side definitions live in
// src/services/flashPlayerManager.ts::initFlashBridge.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "dirplayer_ruffleGetVariable", catch)]
    fn ruffle_get_variable(sprite_num: i32, path: &str) -> Result<JsValue, JsValue>;

    // Per-sprite Flash bridge — each Flash sprite has its own Ruffle
    // instance keyed by sprite_num.
    #[wasm_bindgen(js_name = "dirplayer_ruffleSetVariable", catch)]
    fn ruffle_set_variable(sprite_num: i32, path: &str, value: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFunction", catch)]
    fn ruffle_call_function(sprite_num: i32, path: &str, args_xml: &str) -> Result<JsValue, JsValue>;

    /// Always pass a string here — JS-side parses int vs label.
    /// Director's gotoFrame accepts both `gotoFrame(5)` (numeric) and
    /// `gotoFrame("warm0")` (label); routing labels through `int_value()`
    /// silently parses to 0 and breaks animation.
    #[wasm_bindgen(js_name = "dirplayer_ruffleGoToFrame")]
    fn ruffle_goto_frame(sprite_num: i32, frame_or_label: &str);

    #[wasm_bindgen(js_name = "dirplayer_ruffleStop")]
    fn ruffle_stop(sprite_num: i32);

    #[wasm_bindgen(js_name = "dirplayer_rufflePlayOwned")]
    fn ruffle_play_owned(owner_key: &str, sprite_num: i32);

    #[wasm_bindgen(js_name = "dirplayer_ruffleRewind")]
    fn ruffle_rewind(sprite_num: i32);

    #[wasm_bindgen(js_name = "dirplayer_ruffleIsPlaying")]
    fn ruffle_is_playing(sprite_num: i32) -> bool;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetFrameCount")]
    fn ruffle_get_frame_count(sprite_num: i32) -> i32;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetCurrentFrame")]
    fn ruffle_get_current_frame(sprite_num: i32) -> i32;

    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFrame")]
    fn ruffle_call_frame(sprite_num: i32, frame: i32);

    /// Classify what's under a sprite-local point: 0 = #background,
    /// 1 = #normal, 2 = #button, 3 = #editText (Director Flash hitTest values).
    #[wasm_bindgen(js_name = "dirplayer_ruffleHitTest")]
    fn ruffle_hit_test(sprite_num: i32, x: f64, y: f64) -> i32;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetFlashProperty", catch)]
    fn ruffle_get_flash_property(sprite_num: i32, target: &str, prop_num: i32) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleSetFlashProperty")]
    fn ruffle_set_flash_property(sprite_num: i32, target: &str, prop_num: i32, value: &str);
}

pub struct BuiltInHandlerManager {}

pub(crate) enum CallPreparation {
    Complete(DatumRef),
    Broadcast(crate::player::driver::BroadcastPlan),
    Single {
        receiver: DatumRef,
        handler_name: Symbol,
        args: Vec<DatumRef>,
    },
}

impl BuiltInHandlerManager {
    /// Prepare the global `call` verb while the explicit session context is
    /// borrowed.  Collection receivers retain ordered child descriptors;
    /// scalar receivers are handed to the canonical datum dispatcher by the
    /// session, so unsupported Xtra/network work remains an owned request.
    pub(crate) fn prepare_call(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<CallPreparation, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new("call requires a handler and receiver".to_owned()));
        }
        let name_datum = Self::checked_sync_datum(runtime, &args[0])?.clone();
        if matches!(name_datum, Datum::Void) {
            return Ok(CallPreparation::Complete(DatumRef::Void));
        }
        if !name_datum.is_symbol() && !name_datum.is_string() {
            return Err(ScriptError::new(format!(
                "Handler name must be a symbol (got {}: {:?})",
                name_datum.type_str(),
                name_datum.string_value(runtime.symbols).unwrap_or_default(),
            )));
        }
        let handler_name = name_datum.symbol_value(runtime.symbols)?;
        let receiver = args[1].clone();
        let receiver_datum = Self::checked_sync_datum(runtime, &receiver)?.clone();
        let child_args = args[2..].to_vec();
        let mut receivers = Vec::new();
        match receiver_datum {
            Datum::PropList(entries, ..) => {
                for (_, value_ref) in entries {
                    Self::append_call_receivers(runtime, &value_ref, &mut receivers)?;
                }
            }
            Datum::List(_, entries, _) => {
                for value_ref in entries {
                    Self::append_call_receivers(runtime, &value_ref, &mut receivers)?;
                }
            }
            _ => {
                return Ok(CallPreparation::Single {
                    receiver,
                    handler_name,
                    args: child_args,
                });
            }
        }
        let mut calls = Vec::new();
        for receiver in receivers {
            calls.push(crate::player::handlers::types::AncestorCall {
                receiver: receiver.clone(),
                source: receiver,
                handler_name: handler_name.clone(),
                args: child_args.clone(),
            });
        }
        Ok(CallPreparation::Broadcast(crate::player::driver::BroadcastPlan {
            calls,
            fallback: Vec::new(),
            initial_return: runtime.player.alloc_datum(Datum::Null),
            handled: true,
            continue_on_error: false,
        }))
    }

    fn append_call_receivers(
        runtime: &mut ExecutionContext<'_>,
        value_ref: &DatumRef,
        out: &mut Vec<crate::player::script_ref::ScriptInstanceRef>,
    ) -> Result<(), ScriptError> {
        use crate::player::allocator::ScriptInstanceAllocatorTrait;
        let value = Self::checked_sync_datum(runtime, value_ref)?.clone();
        match value {
            Datum::ScriptInstanceRef(instance_ref) => {
                runtime.player.allocator.get_script_instance_opt(&instance_ref)
                    .ok_or_else(|| ScriptError::new_code(ScriptErrorCode::InvalidReference, "stale call receiver".to_owned()))?;
                out.push(instance_ref);
            }
            Datum::SpriteRef(sprite_id) => {
                let fallback = runtime.player.movie.score.get_sprite(sprite_id)
                    .map(|sprite| sprite.script_instance_list.clone()).unwrap_or_default();
                out.extend(crate::player::session::checked_sprite_script_instance_ids(
                    runtime, sprite_id, fallback.as_slice(),
                )?);
            }
            Datum::Int(_) => {}
            _ => {}
        }
        Ok(())
    }
    /// Prepare the ordered child calls made by `sendSprite`.  This is the
    /// synchronous half of the legacy async implementation: all consumed
    /// handles are checked while the owning session is borrowed, then the
    /// driver mounts each child in order after this borrow ends.
    pub(crate) fn prepare_send_sprite(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<crate::player::driver::BroadcastPlan, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new(
                "sendSprite requires a sprite number and message".to_owned(),
            ));
        }
        let sprite_num = Self::checked_sync_datum(runtime, &args[0])?.int_value()? as i16;
        let message_datum = Self::checked_sync_datum(runtime, &args[1])?.clone();
        let message = message_datum.symbol_value(runtime.symbols)?;
        let fallback = runtime
            .player
            .movie
            .score
            .get_sprite(sprite_num)
            .ok_or_else(|| ScriptError::new(format!("sendSprite: sprite {sprite_num} not found")))?
            .script_instance_list
            .clone();
        let receivers = crate::player::session::checked_sprite_script_instance_ids(
            runtime,
            sprite_num,
            fallback.as_slice(),
        )?;
        let calls = Self::prepare_ordered_child_calls(runtime, message.clone(), &args[2..], receivers)?;
        let fallback = Self::prepare_static_event_calls(runtime, message, &args[2..])?;
        Ok(crate::player::driver::BroadcastPlan {
            calls,
            fallback,
            initial_return: DatumRef::Void,
            handled: false,
            continue_on_error: true,
        })
    }

    /// Prepare the stage and active film-loop receiver order used by
    /// `sendAllSprites`.  Receiver lookup stays lazy at execution time, but
    /// every retained receiver is checked before it can leave the session.
    pub(crate) fn prepare_send_all_sprites(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<crate::player::driver::BroadcastPlan, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "sendAllSprites requires a message".to_owned(),
            ));
        }
        let message_datum = Self::checked_sync_datum(runtime, &args[0])?.clone();
        let message = message_datum.symbol_value(runtime.symbols)?;
        let mut receivers = crate::player::session::active_stage_script_instance_ids_checked(runtime)?;
        for (member_ref, filmloop_frame) in runtime.player.get_active_filmloop_scores() {
            if let Some(filmloop_score) = runtime
                .player
                .movie
                .cast_manager
                .find_member_by_ref(&member_ref)
                .and_then(|member| match &member.member_type {
                    crate::player::cast_member::CastMemberType::FilmLoop(film_loop) => {
                        Some(&film_loop.score)
                    }
                    _ => None,
                })
            {
                receivers.extend(
                    filmloop_score.get_active_script_instance_list_for_frame(filmloop_frame),
                );
            }
        }
        let calls = Self::prepare_ordered_child_calls(runtime, message.clone(), &args[1..], receivers)?;
        let fallback = Self::prepare_static_event_calls(runtime, message, &args[1..])?;
        Ok(crate::player::driver::BroadcastPlan {
            calls,
            fallback,
            initial_return: DatumRef::Void,
            handled: false,
            continue_on_error: true,
        })
    }

    fn prepare_static_event_calls(
        runtime: &mut ExecutionContext<'_>,
        message: Symbol,
        args: &[DatumRef],
    ) -> Result<Vec<crate::player::driver::StaticEventCall>, ScriptError> {
        let frame_ref = runtime.player.movie.score.get_script_in_frame(runtime.player.movie.current_frame)
            .map(|member| crate::player::cast_lib::CastMemberRef {
                cast_lib: member.cast_lib.into(),
                cast_member: member.cast_member.into(),
            });
        let movie_scripts = runtime.player.movie.cast_manager.get_movie_scripts();
        let movie_scripts = movie_scripts.as_ref().ok_or_else(|| ScriptError::new(
            "active movie scripts are unavailable".to_owned(),
        ))?;
        let mut calls = Vec::new();
        if let Some(member_ref) = frame_ref {
            calls.push(crate::player::driver::StaticEventCall {
                receiver: runtime.player.movie.frame_script_instance.clone(),
                member_ref,
                handler_name: message.clone(),
                args: args.to_vec(),
            });
        }
        for script in movie_scripts {
            calls.push(crate::player::driver::StaticEventCall {
                receiver: None,
                member_ref: script.member_ref.clone(),
                handler_name: message.clone(),
                args: args.to_vec(),
            });
        }
        Ok(calls)
    }

    fn prepare_ordered_child_calls(
        runtime: &mut ExecutionContext<'_>,
        message: Symbol,
        args: &[DatumRef],
        receivers: Vec<crate::player::script_ref::ScriptInstanceRef>,
    ) -> Result<Vec<crate::player::handlers::types::AncestorCall>, ScriptError> {
        use crate::player::allocator::ScriptInstanceAllocatorTrait;
        let mut calls = Vec::new();
        for receiver in receivers {
            runtime
                .player
                .allocator
                .get_script_instance_opt(&receiver)
                .ok_or_else(|| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("send receiver {receiver} is stale"),
                ))?;
            calls.push(crate::player::handlers::types::AncestorCall {
                receiver: receiver.clone(),
                source: receiver,
                handler_name: message.clone(),
                args: args.to_vec(),
            });
            // The deferred resolver performs the lookup immediately before
            // each child, preserving mutation-sensitive handler discovery.
        }
        Ok(calls)
    }

    fn checked_sync_datum<'a>(
        runtime: &'a ExecutionContext<'_>,
        datum_ref: &DatumRef,
    ) -> Result<&'a Datum, ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => runtime
                .player
                .allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum_ref}"),
                ))?,
        };
        crate::player::compare::validate_direct_symbol_fields(datum, runtime.symbols)?;
        Ok(datum)
    }

    /// Dispatch a global collection verb after resolving its receiver through
    /// the session-owned allocator. `list_allowed` and `prop_list_allowed`
    /// preserve the old dispatcher branches; some verbs intentionally support
    /// only one collection kind.
    fn try_collection_call(
        runtime: &mut ExecutionContext<'_>,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
        list_allowed: bool,
        prop_list_allowed: bool,
    ) -> Result<Option<DatumRef>, ScriptError> {
        let Some(receiver) = args.first() else {
            return Err(ScriptError::new(format!(
                "{} requires a receiver",
                runtime.symbols.display(&handler_name).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            )));
        };
        let receiver_type = Self::checked_sync_datum(runtime, receiver)?.type_enum();
        let call_args = args[1..].to_vec();
        match receiver_type {
            DatumType::List | DatumType::XmlChildNodes if list_allowed => Ok(Some(
                ListDatumHandlers::call(runtime, receiver, handler_name, &call_args)?
            )),
            DatumType::PropList if prop_list_allowed => Ok(Some(
                PropListDatumHandlers::call(runtime, receiver, handler_name, &call_args)?
            )),
            _ => Ok(None),
        }
    }

    /// Resolve a sprite/integer datum to its sprite number for Flash bridge
    /// calls. Per-sprite Ruffle instances mean sprite_num is the lookup
    /// key; cast_lib/cast_member are returned alongside for callers that
    /// still need them (e.g. building FlashObjectRefs).
    fn resolve_flash_member(datum_ref: &DatumRef) -> Result<Option<(i32, i32, i32)>, ScriptError> {
        reserve_player_ref(|player| {
            let datum = player.get_datum(datum_ref);
            let sprite_num = match datum {
                Datum::SpriteRef(n) => *n,
                Datum::Int(n) => *n as i16,
                _ => return Ok(None),
            };
            let sprite = match player.movie.score.get_sprite(sprite_num) {
                Some(s) => s,
                None => return Ok(None),
            };
            match &sprite.member {
                Some(member_ref) => Ok(Some((
                    sprite_num as i32,
                    member_ref.cast_lib as i32,
                    member_ref.cast_member as i32,
                ))),
                None => Ok(None),
            }
        })
    }

    /// Like `resolve_flash_member`, but only returns Some when the
    /// sprite's member is actually a Flash cast member. Used by the
    /// `stop` / `play` / `rewind` Lingo built-ins so we route those to
    /// the Ruffle bridge only for Flash sprites — `stop sound 1`,
    /// `stop(member …)` etc. continue to fall through to the existing
    /// SWA no-op (which is correct for those operands in the web port).
    fn resolve_flash_sprite_strict(datum_ref: &DatumRef) -> Result<Option<i32>, ScriptError> {
        use crate::player::cast_member::CastMemberType;
        reserve_player_ref(|player| {
            let datum = player.get_datum(datum_ref);
            let sprite_num = match datum {
                Datum::SpriteRef(n) => *n,
                Datum::Int(n) => *n as i16,
                _ => return Ok(None),
            };
            let sprite = match player.movie.score.get_sprite(sprite_num) {
                Some(s) => s,
                None => return Ok(None),
            };
            let member_ref = match &sprite.member {
                Some(m) => m,
                None => return Ok(None),
            };
            let member = match player.movie.cast_manager.find_member_by_ref(member_ref) {
                Some(m) => m,
                None => return Ok(None),
            };
            if matches!(member.member_type, CastMemberType::Flash(_)) {
                Ok(Some(sprite_num as i32))
            } else {
                Ok(None)
            }
        })
    }

    fn param(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let param_number = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
        let scope_ref = runtime.player.current_scope_ref();
        let scope = runtime.player.scopes.get(scope_ref).unwrap();
        let result = scope.args[(param_number - 1) as usize].clone();
        Self::checked_sync_datum(runtime, &result)?;
        Ok(result)
    }

    /// `the paramCount` — Director 11.5 Scripting Dictionary: "indicates the
    /// number of parameters sent to the current handler". Counted over the same
    /// scope `param()` indexes into, so a method's `me` counts as parameter 1
    /// and `paramCount` agrees with the highest valid `param(n)`.
    fn param_count(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let scope_ref = runtime.player.current_scope_ref();
        let count = runtime.player.scopes.get(scope_ref).map_or(0, |scope| scope.args.len());
        Ok(runtime.player.alloc_datum(Datum::Int(count as i32)))
    }

    fn count(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let obj = Self::checked_sync_datum(runtime, &args[0])?;
        match obj {
            Datum::Void => Ok(runtime.player.alloc_datum(Datum::Int(0))),
            Datum::List(_, list, ..) => {
                Ok(runtime.player.alloc_datum(Datum::Int(list.len() as i32)))
            }
            Datum::PropList(prop_list, ..) => {
                Ok(runtime.player.alloc_datum(Datum::Int(prop_list.len() as i32)))
            }
            _ => Err(ScriptError::new(format!(
                "Cannot get count of non-list (type: {})",
                obj.type_str()
            ))),
        }
    }

    fn forward_bitmap_handler(runtime: &mut ExecutionContext<'_>, handler_name: Symbol, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let handler_display = runtime
            .symbols
            .display(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            .to_owned();
        let Some(bitmap_ref) = args.first() else {
            return Err(ScriptError::new(format!(
                "{} requires an image argument",
                handler_display
            )));
        };
        let handler_args = args[1..].to_vec();
        BitmapDatumHandlers::call(
            runtime.player,
            runtime.symbols,
            bitmap_ref,
            handler_name,
            &handler_args,
        )
    }

    fn get_pos_global(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        // getPos(list, value) - find position of value in list.
        // Uses the looser membership equality so Symbol/String pairs with
        // matching text match each other, mirroring Director — verified with
        // `put getPos([#foo, #bar], "foo")` returning 1 in Director 11.5.
        use crate::player::compare::datum_equals_member;
        let list_datum = Self::checked_sync_datum(runtime, &args[0])?;
        let search_ref = &args[1];
        let found = match list_datum {
            Datum::List(_, items, _) => {
                let items = items.clone();
                let mut found = None;
                for (i, item_ref) in items.iter().enumerate() {
                    let item = Self::checked_sync_datum(runtime, item_ref)?;
                    let search = Self::checked_sync_datum(runtime, search_ref)?;
                    if datum_equals_member(item, search, &runtime.player.allocator, runtime.symbols)? {
                        found = Some(i + 1);
                        break;
                    }
                }
                found
            }
            Datum::PropList(pairs, _) => {
                let pairs = pairs.clone();
                let mut found = None;
                for (i, (_, val_ref)) in pairs.iter().enumerate() {
                    let val = Self::checked_sync_datum(runtime, val_ref)?;
                    let search = Self::checked_sync_datum(runtime, search_ref)?;
                    if datum_equals_member(val, search, &runtime.player.allocator, runtime.symbols)? {
                        found = Some(i + 1);
                        break;
                    }
                }
                found
            }
            _ => return Err(ScriptError::new("getPos: not a list".to_owned())),
        };
        Ok(runtime.player.alloc_datum(Datum::Int(found.unwrap_or(0) as i32)))
    }

    fn get_at(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let object_ref = args.first().ok_or_else(|| ScriptError::new("getAt requires a receiver".to_string()))?;
        let index_ref = args.get(1).ok_or_else(|| ScriptError::new("getAt requires an index".to_string()))?;
        let object_type = Self::checked_sync_datum(runtime, object_ref)?.type_enum();
        let index_datum = Self::checked_sync_datum(runtime, index_ref)?;

        // Check if it's a FlashObjectRef first (needs special handling to avoid nested locks)
        if object_type == DatumType::FlashObjectRef {
            let position = index_datum.int_value().unwrap_or(0);
            let flash_ref = reserve_player_ref(|player| {
                match player.get_datum(object_ref) {
                    Datum::FlashObjectRef(flash_ref) => flash_ref.clone(),
                    _ => unreachable!(),
                }
            });
            // Flash arrays use 0-based indexing
            let prop_name = position.to_string();
            let prop_ref = reserve_player_mut(|player| {
                player.alloc_datum(Datum::FlashObjectRef(flash_ref))
            });
            return crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::get_prop(&prop_ref, &prop_name);
        }

        // `getAt(propList, #key)` — the dictionary describes `getAt(list, position)`
        // but states "The getAt command works with linear and property lists", and
        // Director accepts a PROPERTY as the second argument on a property list
        // (dkbarrel's `Game Cod1.movePlayer` does `getAt(terrainWalls, #down)` on
        // [#center: 300, #left: 300, #down: 300, …]). Coercing the symbol to an
        // integer gave position 0 and raised "Index 0 out of bounds". A non-numeric
        // key on a property list is a key lookup; integer keys keep their positional
        // meaning, so `getAt(propList, 2)` is unchanged.
        let prop_lookup = object_type == DatumType::PropList
            && !matches!(Self::checked_sync_datum(runtime, index_ref)?, Datum::Int(_) | Datum::Float(_));
        if prop_lookup {
            return runtime.with_player_and_symbols(|player, symbols| {
                let (prop_list, is_sorted) = match player.get_datum(object_ref) {
                    Datum::PropList(pl, sorted) => (pl.clone(), *sorted),
                    _ => unreachable!(),
                };
                crate::player::handlers::datum_handlers::prop_list::PropListUtils::get_at(
                    &prop_list,
                    index_ref,
                    &player.allocator,
                    symbols,
                    is_sorted,
                )
            }).and_then(|result| {
                Self::checked_sync_datum(runtime, &result).map(|_| result)
            });
        }

        let position = index_datum.int_value()?;
        let result = runtime.with_player_and_symbols(|player, symbols| {
            let obj = player.get_datum(object_ref);
            let index = (position - 1) as usize;

            match obj {
                Datum::Point(vals, flags) => {
                    if index >= 2 {
                        return Err(ScriptError::new(format!(
                            "point index {} out of bounds",
                            position
                        )));
                    }
                    Ok(player.alloc_datum(Datum::inline_component_to_datum(vals[index], Datum::inline_is_float(*flags, index))))
                }

                Datum::Rect(vals, flags) => {
                    if index >= 4 {
                        return Err(ScriptError::new(format!(
                            "rect index {} out of bounds",
                            position
                        )));
                    }
                    Ok(player.alloc_datum(Datum::inline_component_to_datum(vals[index], Datum::inline_is_float(*flags, index))))
                }
                Datum::List(datum_type, list, ..) => {
                    let index = if *datum_type == crate::director::lingo::datum::DatumType::XmlChildNodes {
                        position as usize // 0-based for Flash/XML arrays
                    } else {
                        (position - 1) as usize // 1-based for Lingo lists
                    };
                    if index >= list.len() {
                        return Err(ScriptError::new(format!(
                            "Index {} out of bounds for list of length {}",
                            position,
                            list.len()
                        )));
                    }
                    let result = list[index].clone();

                    Ok(result)
                }
                Datum::PropList(prop_list, ..) => {
                    let index = (position - 1) as usize;
                    if index >= prop_list.len() {
                        return Err(ScriptError::new(format!(
                            "Index {} out of bounds for proplist of length {}",
                            position,
                            prop_list.len()
                        )));
                    }
                    let result = prop_list[index].1.clone();

                    Ok(result)
                }
                _ => {
                    Err(ScriptError::new(format!(
                        "Cannot getAt of non-list (type: {})",
                        obj.type_str()
                    )))
                }
            }
        })?;
        Self::checked_sync_datum(runtime, &result).map(|_| result)
    }

    fn get_last(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let obj = Self::checked_sync_datum(runtime, &args[0])?;
        let result = match obj {
            Datum::List(_, list, ..) => list.back().cloned().unwrap_or(DatumRef::Void),
            Datum::PropList(prop_list, ..) => prop_list
                .back()
                .map(|(_, value_ref)| value_ref.clone())
                .unwrap_or(DatumRef::Void),
            _ => return Err(ScriptError::new(format!(
                "Cannot getLast of non-list (type: {})",
                obj.type_str()
            ))),
        };
        Self::checked_sync_datum(runtime, &result)?;
        Ok(result)
    }

    fn set_at(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let list_ref = args.first().ok_or_else(|| ScriptError::new("setAt requires a receiver".to_string()))?;
        let index_ref = args.get(1).ok_or_else(|| ScriptError::new("setAt requires an index".to_string()))?;
        let value_ref = args.get(2).ok_or_else(|| ScriptError::new("setAt requires a value".to_string()))?;
        Self::checked_sync_datum(runtime, list_ref)?;
        Self::checked_sync_datum(runtime, index_ref)?;
        Self::checked_sync_datum(runtime, value_ref)?;
        runtime.with_player_and_symbols(|player, symbols| {
            let list_ref = &args[0];
            // `setAt` on a PROPERTY list accepts a property KEY (string/symbol),
            // not only an integer position — Director's Hey-Arnold gObstacles does
            // both: `setAt(gPersonList, dTargetPerson, "walking")` (int position)
            // and `setAt(gPersonList, pShape, "free")` (pShape = "gerald", a key).
            // Route a non-numeric key to the property setter; int_value() would
            // coerce "gerald" to 0 → "Index 0 out of bounds".
            let key_is_name = matches!(
                player.get_datum(&args[1]),
                Datum::String(_) | Datum::Symbol(_)
            );
            let is_prop_list = matches!(player.get_datum(list_ref), Datum::PropList(..));
            if is_prop_list && key_is_name {
                crate::player::handlers::datum_handlers::prop_list::PropListUtils::set_at(
                    player, symbols, list_ref, index_ref, value_ref,
                )?;
                return Ok(());
            }
            let position = player.get_datum(index_ref).int_value()?;
            let new_value = value_ref.clone();
            let is_zero_based = matches!(player.get_datum(list_ref), Datum::List(crate::director::lingo::datum::DatumType::XmlChildNodes, ..));
            let index = if is_zero_based { position as usize } else { (position - 1) as usize };
            
            let list_datum = player.get_datum(list_ref);
            debug!(
                "setAt: list={}, index={}, new_value={}", 
                format_concrete_datum(list_datum, symbols, player)?,
                position,
                format_concrete_datum(player.get_datum(&new_value), symbols, player)?,
            );
            
            // Validate the new_value type BEFORE taking mutable borrow
            let new_value_datum = player.get_datum(&new_value).clone();

            // Director 11.5 Scripting Dictionary (setAt): when the position is
            // past the end of a LINEAR list, Director grows the list, filling
            // the intervening "blank" entries with 0. Pre-allocate that fill
            // here — before the &mut borrow below, since alloc needs its own
            // borrow — but only when growth is actually required. (Property
            // lists intentionally error instead; see the PropList arm.)
            let list_grow_fill = match player.get_datum(list_ref) {
                Datum::List(_, list, ..)
                    if (is_zero_based || position >= 1) && index >= list.len() =>
                {
                    Some(player.alloc_datum(Datum::Int(0)))
                }
                _ => None,
            };

            // Now take the mutable borrow
            let list_datum = player.get_datum_mut(list_ref);
            match list_datum {
                Datum::Point(vals, flags) => {
                    if index >= 2 {
                        return Err(ScriptError::new(format!(
                            "point index {} out of bounds",
                            position
                        )));
                    }

                    let (component_val, is_float) = Datum::datum_to_inline_component(&new_value_datum)?;
                    vals[index] = component_val;
                    Datum::inline_set_float(flags, index, is_float);

                    Ok(())
                }
                Datum::Rect(vals, flags) => {
                    if index >= 4 {
                        return Err(ScriptError::new(format!(
                            "rect index {} out of bounds",
                            position
                        )));
                    }

                    let (component_val, is_float) = Datum::datum_to_inline_component(&new_value_datum)?;
                    vals[index] = component_val;
                    Datum::inline_set_float(flags, index, is_float);

                    Ok(())
                }
                Datum::List(_, list, ..) => {
                    if !is_zero_based && position < 1 {
                        // Director tolerates setAt at position <= 0 on a linear
                        // list as a no-op (mirrors ListDatumHandlers::set_at);
                        // guarding here also avoids the usize underflow of
                        // `index` below turning a grow into a huge allocation.
                        Ok(())
                    } else if index < list.len() {
                        list[index] = new_value;
                        Ok(())
                    } else {
                        // Grow to `position`, padding the blank entries with 0.
                        let fill = list_grow_fill.clone().unwrap_or(DatumRef::Void);
                        while list.len() < index {
                            list.push_back(fill.clone());
                        }
                        list.push_back(new_value);
                        Ok(())
                    }
                }
                Datum::PropList(prop_list, ..) => {
                    if index < prop_list.len() {
                        prop_list[index].1 = new_value;
                        Ok(())
                    } else {
                        Err(ScriptError::new(format!("Index {} out of bounds", position)))
                    }
                }
                _ => Err(ScriptError::new(format!(
                    "Cannot setAt of type {} (must be list, proplist, point, or rect)", 
                    list_datum.type_str()
                ))),
            }
        })?;
        Ok(DatumRef::Void)
    }

    pub fn put(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            trace_output(runtime.player, "--");
            return Ok(DatumRef::Void);
        }

        let output = if args.len() == 1 {
            let first_arg = Self::checked_sync_datum(runtime, &args[0])?;
            Self::format_for_put(first_arg, runtime)?
        } else {
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let datum = Self::checked_sync_datum(runtime, arg)?;
                parts.push(match datum {
                    Datum::String(s) => s.clone(),
                    _ => crate::player::datum_formatting::format_concrete_datum(
                        datum,
                        runtime.symbols,
                        runtime.player,
                    )?,
                });
            }
            parts.join(" ")
        };

        trace_output(runtime.player, &format!("-- {}", output));
        Ok(DatumRef::Void)
    }

    fn format_for_put(
        datum: &Datum,
        runtime: &ExecutionContext<'_>,
    ) -> Result<String, ScriptError> {
        match datum {
            // Strings are output with quotes
            Datum::String(s) => Ok(format!("\"{}\"", s)),
            
            // Numbers are output without quotes
            Datum::Int(i) => Ok(i.to_string()),
            
            // Symbols are output with # prefix
            Datum::Symbol(s) => Ok(format!(
                "#{}",
                runtime
                    .symbols
                    .display(s)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            )),
            
            // Void outputs as <Void>
            Datum::Void | Datum::Null => Ok("<Void>".to_string()),
            
            // Lists
            Datum::List(_, list, _) => {
                let mut items = Vec::with_capacity(list.len());
                for datum_ref in list {
                    let item = Self::checked_sync_datum(runtime, datum_ref)?;
                    items.push(Self::format_for_put(item, runtime)?);
                }
                Ok(format!("[{}]", items.join(", ")))
            },
            
            // Everything else uses default formatting
            _ => crate::player::datum_formatting::format_concrete_datum(
                datum,
                runtime.symbols,
                runtime.player,
            ),
        }
    }

    pub fn inspect(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.len() != 1 {
            return Err(ScriptError::new(
                "inspect requires exactly 1 argument".to_string(),
            ));
        }
        Self::checked_sync_datum(runtime, &args[0])?;
        runtime.with_player_and_symbols(|player, symbols| {
            let datum = player.get_datum(&args[0]);
            match datum {
                Datum::BitmapRef(bitmap_ref) => {
                    let src = player
                        .bitmap_manager
                        .get_bitmap_handle(bitmap_ref)
                        .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

                    let w = src.width;
                    let h = src.height;
                    let palettes = player.movie.cast_manager.palettes();
                    let mut dest = Bitmap::new(w, h, 32, 32, 0, PaletteRef::BuiltIn(get_system_default_palette()));
                    let rect = IntRect::from(0, 0, w as i32, h as i32);
                    dest.copy_pixels(&palettes, src, rect.clone(), rect, &HashMap::new(), None);

                    JsApi::dispatch_debug_bitmap(w as u32, h as u32, &dest.data);
                }
                Datum::List(..) | Datum::PropList(..) | Datum::ScriptInstanceRef(..) => {
                    JsApi::dispatch_debug_datum(&args[0], symbols, player);
                    player.debug_datum_refs.push(args[0].clone());
                }
                _ => {
                    return Err(ScriptError::new(format!(
                        "inspect does not support type: {}",
                        datum.type_str()
                    )));
                }
            }
            Ok(())
        })?;
        Ok(DatumRef::Void)
    }

    fn clear_globals(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.player.globals.clear();
        runtime.player.initialize_globals();
        Ok(DatumRef::Void)
    }

    fn random(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        {
            // The Director 11.5 Scripting Dictionary documents only the
            // single-arg form random(n) → a random integer in 1..n. Many
            // shipped Shockwave games also rely on an undocumented two-arg
            // form random(min, max) → a random integer in [min, max] inclusive
            // (bogey_nights uses e.g. `random(-6, -3)` for a splash's launch
            // velocity and `random(40, 120)` for spawn ranges — with no custom
            // `on random` handler). The spec is silent on the two-arg form
            // rather than forbidding it, so we support both. Inferred from the
            // calling movie, not the 11.5 dictionary.
            if args.len() >= 2 {
                let a = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
                let b = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let span = hi - lo + 1; // inclusive range size, always >= 1
                // next_random_int(n) returns 1..n from the seeded sequence;
                // rebase it into [lo, hi] so deterministic replay still holds.
                let value = match runtime.player.movie.next_random_int(span) {
                    Some(v) => lo + (v - 1),
                    None => runtime.player.rng.random_range(lo..=hi),
                };
                return Ok(runtime.player.alloc_datum(Datum::Int(value)));
            }

            let max = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
            if max <= 0 {
                return Ok(runtime.player.alloc_datum(Datum::Int(0)));
            }

            // Director's random(n) returns a value from 1 to n (inclusive)
            let random_int = match runtime.player.movie.next_random_int(max) {
                Some(value) => value,
                None => {
                    runtime.player.rng.random_range(1..=max)
                }
            };

            Ok(runtime.player.alloc_datum(Datum::Int(random_int)))
        }
    }

    /// `randomVector()` (Director 11.5 dictionary): top-level function returning a
    /// unit vector — a uniformly random point on the surface of the unit sphere,
    /// guaranteed length 1. No parameters. Uses the cylinder/Archimedes method
    /// (z uniform in [-1,1], azimuth uniform in [0, 2pi)) which is exactly uniform.
    fn random_vector(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let z: f64 = runtime.player.rng.random_range(-1.0..1.0);
        let phi: f64 = runtime.player.rng.random_range(0.0..std::f64::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Ok(runtime.player.alloc_datum(Datum::Vector([r * phi.cos(), r * phi.sin(), z])))
    }

    fn bit_and(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let a = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
        let b = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
        Ok(runtime.player.alloc_datum(Datum::Int(a & b)))
    }

    fn bit_or(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let a = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
        let b = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
        Ok(runtime.player.alloc_datum(Datum::Int(a | b)))
    }

    fn bit_not(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let a = Self::checked_sync_datum(runtime, &args[0])?.int_value()?;
        Ok(runtime.player.alloc_datum(Datum::Int(!a)))
    }

    pub fn has_async_handler(name: Symbol) -> bool {
        // Lingo handler names are case-insensitive. `SymbolTable::intern`
        // lowercases before interning, so symbol identity already carries that:
        // `do("sendsprite 1,#endcamera")` (Dora Soccer) resolves to the same
        // symbol as `sendSprite`, with no per-call lowercasing.
        let Some(name_builtin) = name.into_builtin() else { return false; };
        match name_builtin {
            BuiltInSymbol::Call => true,
            BuiltInSymbol::New => true,
            BuiltInSymbol::NewObject => true,
            BuiltInSymbol::CallAncestor => true,
            BuiltInSymbol::SendSprite => true,
            BuiltInSymbol::SendAllSprites => true,
            BuiltInSymbol::Value => true,
            BuiltInSymbol::Do => true,
            BuiltInSymbol::UpdateStage => true,
            BuiltInSymbol::Go => true,
            // `play movie X` / `play frame X of movie Y` compile to play(frame, movie)
            // and must load a movie like `go` (async). The 1-arg Flash form is handled
            // synchronously inside the async arm.
            BuiltInSymbol::Play => true,
            BuiltInSymbol::Nothing => true,
            // Old-style Lingo lets `importFileInto member, url, props` be
            // called as a global verb; route it to the same async impl as
            // the method form `member.importFileInto(url, props)`.
            BuiltInSymbol::ImportFileInto => true,
            _ => false,
        }
    }

    pub fn call_handler(
        runtime: &mut ExecutionContext<'_>,
        name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        // Validate the handler symbol against this session before dispatching.
        // Builtin identity lookup alone only understands builtin-tagged symbols;
        // the authoritative table still must reject a foreign dynamic symbol.
        runtime
            .symbols
            .display(&name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match name.into_builtin() {
            Some(BuiltInSymbol::CastLib) => CastHandlers::cast_lib(args),
            Some(BuiltInSymbol::FindEmpty) => CastHandlers::find_empty(args),
            Some(BuiltInSymbol::PreloadNetThing) => NetHandlers::preload_net_thing(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::NetDone) => NetHandlers::net_done(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::NetAbort) => NetHandlers::net_abort(runtime.player, runtime.symbols, args),
            // Cast/movie preload + unload commands: dirplayer loads everything
            // synchronously up front, so there is nothing to (un)cache. Accept
            // as no-ops (netjack startMovie calls preLoadCast).
            Some(BuiltInSymbol::MoveToFront | BuiltInSymbol::PreloadMember | BuiltInSymbol::PreloadBuffer | BuiltInSymbol::UnloadMember | BuiltInSymbol::Beep | BuiltInSymbol::PreLoadCast | BuiltInSymbol::UnLoadCast | BuiltInSymbol::PreLoadMovie | BuiltInSymbol::UnLoad) => Ok(DatumRef::Void),
            // Director's flushInputEvents() discards queued mouse/key events;
            // the browser player has no such queue to flush.
            Some(BuiltInSymbol::FlushInputEvents) => Ok(DatumRef::Void),
            Some(BuiltInSymbol::PuppetTempo) => MovieHandlers::puppet_tempo(runtime, args),
            Some(BuiltInSymbol::Objectp) => TypeHandlers::objectp(runtime, args),
            Some(BuiltInSymbol::Voidp) => TypeHandlers::voidp(runtime, args),
            Some(BuiltInSymbol::Listp) => TypeHandlers::listp(runtime, args),
            Some(BuiltInSymbol::Symbolp) => TypeHandlers::symbolp(runtime, args),
            Some(BuiltInSymbol::Stringp) => TypeHandlers::stringp(runtime, args),
            Some(BuiltInSymbol::Integerp) => TypeHandlers::integerp(runtime, args),
            Some(BuiltInSymbol::Floatp) => TypeHandlers::floatp(runtime, args),
            Some(BuiltInSymbol::Offset) => StringHandlers::offset(runtime, args),
            Some(BuiltInSymbol::Length) => StringHandlers::length(runtime, args),
            Some(BuiltInSymbol::Script) => MovieHandlers::script(runtime, args),
            Some(BuiltInSymbol::Void) => TypeHandlers::void(args),
            Some(BuiltInSymbol::Param) => Self::param(runtime, args),
            Some(BuiltInSymbol::ParamCount) => Self::param_count(runtime, args),
            Some(BuiltInSymbol::Count) => Self::count(runtime, args),
            Some(BuiltInSymbol::CreateMask) => Self::forward_bitmap_handler(runtime, Symbol::builtin(BuiltInSymbol::CreateMask), args),
            Some(BuiltInSymbol::CreateMatte) => Self::forward_bitmap_handler(runtime, Symbol::builtin(BuiltInSymbol::CreateMatte), args),
            Some(BuiltInSymbol::GetAt) => Self::get_at(runtime, args),
            Some(BuiltInSymbol::GetLast) => {
                Self::get_last(runtime, args)
            }
            Some(BuiltInSymbol::GetPos) => Self::get_pos_global(runtime, args),
            Some(BuiltInSymbol::SetAt) => Self::set_at(runtime, args),
            Some(BuiltInSymbol::Ilk) => TypeHandlers::ilk(runtime, args),
            Some(BuiltInSymbol::Member) => MovieHandlers::member(runtime, args),
            // Director 4 syntax: `cast(N)` / `the X of cast(N)` references a
            // member in the single (internal) cast. Equivalent to `member(N)`
            // in D5+. Movies authored in D4 still use it.
            Some(BuiltInSymbol::Cast) => MovieHandlers::member(runtime, args),
            Some(BuiltInSymbol::Space) => StringHandlers::space(runtime, args),
            Some(BuiltInSymbol::Integer) => TypeHandlers::integer(runtime, args),
            Some(BuiltInSymbol::String) => StringHandlers::string(runtime, args),
            Some(BuiltInSymbol::CharToNum) => StringHandlers::char_to_num(runtime, args),
            Some(BuiltInSymbol::NumToChar) => StringHandlers::num_to_char(runtime, args),
            Some(BuiltInSymbol::Float) => TypeHandlers::float(runtime, args),
            Some(BuiltInSymbol::Put) => Self::put(runtime, args),
            Some(BuiltInSymbol::Inspect) => Self::inspect(runtime, args),
            Some(BuiltInSymbol::Random) => Self::random(runtime, args),
            Some(BuiltInSymbol::RandomVector) => Self::random_vector(runtime, args),
            Some(BuiltInSymbol::BitAnd) => Self::bit_and(runtime, args),
            Some(BuiltInSymbol::BitOr) => Self::bit_or(runtime, args),
            Some(BuiltInSymbol::BitNot) => Self::bit_not(runtime, args),
            Some(BuiltInSymbol::Symbol) => TypeHandlers::symbol(runtime, args),
            Some(BuiltInSymbol::PuppetSprite) => MovieHandlers::puppet_sprite(runtime, args),
            Some(BuiltInSymbol::ClearGlobals) => Self::clear_globals(runtime, args),
            Some(BuiltInSymbol::Sprite) => MovieHandlers::sprite(runtime, args),
            Some(BuiltInSymbol::Point) => TypeHandlers::point(runtime, args),
            Some(BuiltInSymbol::ClickLoc) => Ok(runtime.player.alloc_datum(Datum::Point([
                runtime.player.movie.click_loc.0 as f64,
                runtime.player.movie.click_loc.1 as f64,
            ], 0))),
            Some(BuiltInSymbol::ConstrainH) => {
                let sprite_num = Self::checked_sync_datum(runtime, &args[0])?.int_value()? as i16;
                let posn = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
                let (left, right) = runtime.player.movie.score.get_sprite(sprite_num)
                    .map(|sprite| { let rect = get_concrete_sprite_rect(runtime.player, sprite); (rect.left, rect.right) })
                    .unwrap_or((0, 0));
                Ok(runtime.player.alloc_datum(Datum::Int(posn.max(left).min(right))))
            }
            Some(BuiltInSymbol::ConstrainV) => {
                let sprite_num = Self::checked_sync_datum(runtime, &args[0])?.int_value()? as i16;
                let posn = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
                let (top, bottom) = runtime.player.movie.score.get_sprite(sprite_num)
                    .map(|sprite| { let rect = get_concrete_sprite_rect(runtime.player, sprite); (rect.top, rect.bottom) })
                    .unwrap_or((0, 0));
                Ok(runtime.player.alloc_datum(Datum::Int(posn.max(top).min(bottom))))
            }
            // `stop` / `play` / `rewind` / `pause` are overloaded Lingo
            // built-ins. Historically the web port stubbed them all to
            // no-op because they targeted SWA sound channels (`stop sound 1`)
            // which we don't implement. But Director also uses them on
            // Flash sprites (`stop(sprite N)`, `play(sprite N)`,
            // `rewind(sprite N)`) — storyscramble's BS38 calls
            // `stop(sprite(me.spriteNum))` right after `gotoFrame(...,31)`
            // to park the bubble. Route Flash-sprite operands to the
            // Ruffle bridge; everything else (sound channels, members,
            // bare integers that don't resolve to a Flash sprite) keeps
            // the historical no-op behaviour.
            Some(BuiltInSymbol::Stop) => {
                if args.len() >= 1 {
                    if let Some(sn) = Self::resolve_flash_sprite_strict(&args[0])? {
                        ruffle_stop(sn);
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::Play) => {
                if args.len() >= 1 {
                    if let Some(sn) = Self::resolve_flash_sprite_strict(&args[0])? {
                        // play() is resume-semantic: it OVERRIDES a prior
                        // `sprite.frame = N` hold, so clear the sprite's asserted
                        // frame — otherwise a fresh instance would be pinned
                        // (stopped) at N instead of playing (StoryScramble's
                        // grow-bubble does `frame = N; play()` and must animate).
                        reserve_player_mut(|player| {
                            player.movie.score.get_sprite_mut(sn as i16).flash_asserted_frame = None;
                        });
                        let owner_key = reserve_player_ref(|player| owner_key_string(&player.owner));
                        ruffle_play_owned(&owner_key, sn);
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::Rewind) => {
                if args.len() >= 1 {
                    if let Some(sn) = Self::resolve_flash_sprite_strict(&args[0])? {
                        ruffle_rewind(sn);
                    }
                }
                Ok(DatumRef::Void)
            }
            // Director 11.5 `hold()` — global form `hold(sprite N)`. Maps to the
            // same root-timeline stop as `stop()` (spec: stops the Flash sprite,
            // audio would continue — we don't split audio). See the sprite-method
            // `hold` arm in datum_handlers/sprite.rs.
            Some(BuiltInSymbol::Hold) => {
                if args.len() >= 1 {
                    if let Some(sn) = Self::resolve_flash_sprite_strict(&args[0])? {
                        ruffle_stop(sn);
                    }
                }
                Ok(DatumRef::Void)
            }
            // `pause` halts the playhead on the current frame; `continue`
            // resumes it. Both are Director 6-era playback commands, dropped
            // from the 11.5 Scripting Dictionary in favour of go/updateStage,
            // but still live in the runtime and still emitted by movies of that
            // vintage. `pause` is already a no-op here, so `continue` has
            // nothing to resume and is one too — the pair has to agree, or a
            // movie that calls only `continue` errors while one that calls both
            // does not.
            //
            // Merlin's Revenge 3 calls it defensively before skipping a loop
            // iteration (`continue()` then `next repeat` in
            // collectionsMaster.initCollections), where it is a no-op in
            // Director as well since nothing paused the playhead.
            Some(BuiltInSymbol::Pause | BuiltInSymbol::Continue) => Ok(DatumRef::Void),
            Some(BuiltInSymbol::Cursor) => TypeHandlers::cursor(runtime, args),
            Some(BuiltInSymbol::ExternalParamCount) => MovieHandlers::external_param_count(runtime, args),
            Some(BuiltInSymbol::ExternalParamName) => MovieHandlers::external_param_name(runtime, args),
            Some(BuiltInSymbol::ExternalParamValue) => MovieHandlers::external_param_value(runtime, args),
            Some(BuiltInSymbol::GetNetText) => NetHandlers::get_net_text(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::Timeout) => TypeHandlers::timeout(runtime, args),
            Some(BuiltInSymbol::Rect) => TypeHandlers::rect(runtime, args),
            Some(BuiltInSymbol::GetStreamStatus) => NetHandlers::get_stream_status(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::NetError) => NetHandlers::net_error(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::NetStatus) => NetHandlers::net_status(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::NetTextResult) => NetHandlers::net_text_result(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::PostNetText) => NetHandlers::post_net_text(runtime.player, runtime.symbols, args),
            Some(BuiltInSymbol::Rgb) => TypeHandlers::rgb(runtime, args),
            Some(BuiltInSymbol::List) => TypeHandlers::list(runtime, args),
            Some(BuiltInSymbol::Image) => TypeHandlers::image(runtime, args),
            Some(BuiltInSymbol::Filter) => TypeHandlers::filter(runtime, args),
            Some(BuiltInSymbol::NewMatrix) => TypeHandlers::new_matrix(runtime, args),
            Some(BuiltInSymbol::ConstraintDesc) => TypeHandlers::constraint_desc(runtime, args),
            // Director allows both the method form `bitmap.getPixel(x, y)` and
            // the global form `getPixel(bitmap, x, y)` (chapter 15). Same for
            // `setPixel`. Both end up at the BitmapDatumHandlers entry — the
            // global form just strips the bitmap from arg[0] and forwards the
            // rest as the method args.
            Some(BuiltInSymbol::GetPixel) => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "getPixel requires a bitmap argument".to_string(),
                    ));
                }
                let rest: Vec<DatumRef> = args.iter().skip(1).cloned().collect();
                BitmapDatumHandlers::get_pixel(runtime.player, runtime.symbols, &args[0], &rest)
            }
            Some(BuiltInSymbol::SetPixel) => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "setPixel requires a bitmap argument".to_string(),
                    ));
                }
                let rest: Vec<DatumRef> = args.iter().skip(1).cloned().collect();
                BitmapDatumHandlers::set_pixel(runtime.player, runtime.symbols, &args[0], &rest)
            }
            Some(BuiltInSymbol::Chars) => StringHandlers::chars(runtime, args),
            Some(BuiltInSymbol::PaletteIndex) => TypeHandlers::palette_index(runtime, args),
            Some(BuiltInSymbol::Abs) => TypeHandlers::abs(runtime, args),
            Some(BuiltInSymbol::Xtra) => TypeHandlers::xtra(runtime, args),
            Some(BuiltInSymbol::StopEvent) => MovieHandlers::stop_event(runtime, args),
            Some(BuiltInSymbol::GetPref) => MovieHandlers::get_pref(runtime, args),
            Some(BuiltInSymbol::SetPref) => MovieHandlers::set_pref(runtime, args),
            Some(BuiltInSymbol::UrlEncode) => StringHandlers::url_encode(runtime, args),
            Some(BuiltInSymbol::GoToNetPage) => MovieHandlers::go_to_net_page(runtime, args),
            Some(BuiltInSymbol::GoToNetMovie) => MovieHandlers::go_to_net_movie(runtime, args),
            Some(BuiltInSymbol::Pass) => MovieHandlers::pass(runtime, args),
            Some(BuiltInSymbol::Union) => TypeHandlers::union(runtime, args),
            // inflate(rect, w, h) — the top-level form of rect.inflate(w, h).
            Some(BuiltInSymbol::Inflate) if !args.is_empty() => {
                runtime.with_player_and_symbols(|player, symbols| {
                    RectDatumHandlers::inflate(player, symbols, &args[0], &args[1..])
                })
            }
            Some(BuiltInSymbol::Inflate) => TypeHandlers::inflate(runtime, args),
            Some(BuiltInSymbol::BitXor) => TypeHandlers::bit_xor(runtime, args),
            Some(BuiltInSymbol::Power) => TypeHandlers::power(runtime, args),
            Some(BuiltInSymbol::Add) => TypeHandlers::add(runtime, args),
            Some(BuiltInSymbol::Abort) => Err(ScriptError::new_code(ScriptErrorCode::Abort, "abort".to_string())),
            Some(BuiltInSymbol::MouseDown) => {
                runtime.player.input_polled = true;
                Ok(runtime.player.alloc_datum(datum_bool(runtime.player.movie.mouse_down)))
            }
            Some(BuiltInSymbol::RightMouseDown) => {
                // Right button IS tracked (right_mouse_down/right_mouse_up JS exports set
                // movie.right_mouse_down; the `the rightMouseDown` property reads it too).
                // The function form was stubbed to FALSE, so polling movies (Rasterwerks
                // C_Input.ReadMouse → KEY_ALTFIRE) never saw right-click → the sniper scope
                // never engaged. Mirror the `mousedown` function above.
                Ok(runtime.player.alloc_datum(datum_bool(runtime.player.movie.right_mouse_down)))
            }
            Some(BuiltInSymbol::GetRendererServices) => {
                // Return a prop list with renderer info stubs
                let player = &mut *runtime.player;
                let symbols = &mut *runtime.symbols;
                {
                    let make_sym = |p: &mut DirPlayer, symbols: &mut SymbolTable, s: &str| p.alloc_datum(Datum::Symbol(symbols.intern(s)));
                    let make_str = |p: &mut DirPlayer, s: &str| p.alloc_datum(Datum::String(s.to_string()));
                    let make_int = |p: &mut DirPlayer, n: i32| p.alloc_datum(Datum::Int(n));

                    // rendererDeviceList
                    let rdl_key = make_sym(player, symbols, "rendererDeviceList");
                    let device = make_str(player, "WebGL2");
                    let rdl_val = player.alloc_datum(Datum::List(DatumType::List, VecDeque::from(vec![device]), false));

                    // renderer
                    let rend_key = make_sym(player, symbols, "renderer");
                    let rend_val = make_str(player, "#openGL");

                    // Hardware info as nested proplist
                    let vendor_k = make_sym(player, symbols, "vendor");
                    let vendor_v = make_str(player, "WebGL");
                    let model_k = make_sym(player, symbols, "model");
                    let model_v = make_str(player, "WebGL2 Renderer");
                    let version_k = make_sym(player, symbols, "version");
                    let version_v = make_str(player, "2.0");
                    let max_tex_k = make_sym(player, symbols, "maxTextureSize");
                    let max_tex_v = make_int(player, 4096);
                    let tex_fmt_k = make_sym(player, symbols, "supportedTextureRenderFormats");
                    let fmt = make_str(player, "rgba8880");
                    let tex_fmt_v = player.alloc_datum(Datum::List(DatumType::List, VecDeque::from(vec![fmt]), false));
                    let tex_units_k = make_sym(player, symbols, "textureUnits");
                    let tex_units_v = make_int(player, 8);
                    let depth_k = make_sym(player, symbols, "depthBufferRange");
                    let depth_v = make_int(player, 24);
                    let color_k = make_sym(player, symbols, "colorBufferRange");
                    let color_v = make_int(player, 32);

                    let hw_info = player.alloc_datum(Datum::PropList(VecDeque::from(vec![
                        (vendor_k, vendor_v), (model_k, model_v), (version_k, version_v),
                        (max_tex_k, max_tex_v), (tex_fmt_k, tex_fmt_v), (tex_units_k, tex_units_v),
                        (depth_k, depth_v), (color_k, color_v),
                    ]), false));
                    let hw_key = make_sym(player, symbols, "hardwareInfo");

                    let result = player.alloc_datum(Datum::PropList(VecDeque::from(vec![
                        (rdl_key, rdl_val), (rend_key, rend_val), (hw_key, hw_info),
                    ]), false));
                    Ok(result)
                }
            }
            Some(BuiltInSymbol::GetVariable) => {
                // Flash (SWF) member interop — getVariable(sprite, path [, asObjectFlag]).
                //
                // The optional 3rd arg == 0 is Director's "return the value as a
                // Flash object handle" flag (e.g. getVariable(sprite N, "_level0", 0)
                // grabs the SWF's _level0 timeline object). Otherwise a scalar
                // string is returned.
                //
                // Resolve the sprite NUMBER directly (not via resolve_flash_member,
                // which returns None when sprite.member isn't committed to the
                // channel yet — a beginSprite can run before the Flash member is
                // assigned). For the object form we then ALWAYS return a
                // sprite-bound FlashObjectRef, never VOID: a VOID handle stored in
                // a global (gDemoFlash) crashes the later gDemoFlash.play(). The
                // binding also makes that deferred call dispatch to the right
                // sprite even though the member wasn't resolvable at capture time.
                if args.len() >= 2 {
                    let sn_opt: Option<i16> = reserve_player_ref(|player| {
                        Ok::<Option<i16>, ScriptError>(match player.get_datum(&args[0]) {
                            Datum::SpriteRef(n) => Some(*n),
                            Datum::Int(n) => Some(*n as i16),
                            _ => None,
                        })
                    })?;
                    let path = Self::checked_sync_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
                    let return_as_object: bool = if args.len() >= 3 {
                        reserve_player_ref(|player| {
                            Ok::<bool, ScriptError>(player.get_datum(&args[2]).int_value().unwrap_or(1) == 0)
                        })?
                    } else {
                        false
                    };
                    if let Some(sn) = sn_opt {
                        let (cl, cm): (i32, i32) = reserve_player_ref(|player| {
                            Ok::<(i32, i32), ScriptError>(player
                                .movie
                                .score
                                .get_sprite(sn)
                                .and_then(|s| s.member.as_ref())
                                .map(|m| (m.cast_lib as i32, m.cast_member as i32))
                                .unwrap_or((0, 0)))
                        })?;
                        // Member-swap guard: when the score just swapped this
                        // sprite to a NEW Flash member, the PREVIOUS member's
                        // Ruffle instance can still be resident until the new one
                        // loads. Reading the old member's variables is wrong —
                        // BioBoxing's menu frame swaps sprite 1 loading.swf ->
                        // start.swf, and reading loading.swf's stale `/:cont="1"`
                        // (instead of start.swf's "00") skips the menu. If the
                        // sprite's CURRENT (cl,cm) isn't in `flash_sprite_loaded`,
                        // treat the VALUE read as not-ready (VOID) so the frame's
                        // `if cont = 1 ... else go(the frame)` loop holds until the
                        // new member loads. (Object form below still returns a
                        // sprite-bound handle.) Safe for Storyscramble in-place
                        // same-number reloads: its `invalidate_flash_for_cast_lib`
                        // already pulls the entry from this set.
                        let member_ready = (cl == 0 && cm == 0)
                            || reserve_player_ref(|player| {
                                Ok::<bool, ScriptError>(player.flash_sprite_loaded.contains(&(sn, cl, cm)))
                            })?;
                        if !return_as_object && !member_ready {
                            return Ok(DatumRef::Void);
                        }
                        match ruffle_get_variable(sn as i32, &path) {
                            Ok(val) => {
                                if let Some(s) = val.as_string() {
                                    // Ruffle's GetVariable coerces an AS OBJECT to
                                    // "[object Object]" / "[type Object]" /
                                    // "[object MovieClip]". For the OBJECT form the
                                    // coercion text is not the variable's value and
                                    // must not be returned as one — the dictionary
                                    // is explicit that "if it is returned as a
                                    // string, the string will not be a valid object
                                    // reference" (getVariable(), 11.5). Fall through
                                    // to the FlashObjectRef below instead.
                                    //
                                    // The sprite-METHOD form
                                    // (`sprite(N).getVariable(path, 0)`) has guarded
                                    // this since DGS; the top-level
                                    // `getVariable(sprite(N), path, 0)` never did.
                                    // Agent Free Ride's OffGame reads its root
                                    // timeline that way and then calls
                                    // `.mcLoading.gotoAndStop(pct)` on the result,
                                    // which died with "No handler gotoAndStop for
                                    // string datum". It only showed in the browser
                                    // extension: there the sync bridge marshals the
                                    // result through JSON and String(), so the
                                    // MovieClip always arrives as that text, while
                                    // the dev UI calls Ruffle directly and gets a
                                    // non-string back.
                                    let is_object_coercion = return_as_object
                                        && (s.starts_with("[object ") || s.starts_with("[type "));
                                    if !is_object_coercion {
                                        return reserve_player_mut(|player| {
                                            Ok(player.alloc_datum(Datum::String(s)))
                                        });
                                    }
                                }
                            }
                            Err(e) => warn!("getVariable error: {:?}", e),
                        }
                        if return_as_object {
                            return reserve_player_mut(|player| {
                                use crate::director::lingo::datum::FlashObjectRef;
                                Ok(player.alloc_datum(Datum::FlashObjectRef(
                                    FlashObjectRef::from_path_with_sprite(&path, cl, cm, sn as i32),
                                )))
                            });
                        }
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::SetVariable) => {
                // Flash (SWF) member interop — setVariable(sprite, path, value)
                if args.len() >= 3 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    let path = Self::checked_sync_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
                    let value = Self::checked_sync_datum(runtime, &args[2])?.string_value(runtime.symbols)?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        if let Err(e) = ruffle_set_variable(sn, &path, &value) {
                            warn!("setVariable error: {:?}", e);
                        }
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::GoToFrame) => {
                // Flash (SWF) member interop — goToFrame(sprite, frame_or_label)
                if args.len() >= 2 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    let frame_or_label = Self::checked_sync_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        ruffle_goto_frame(sn, &frame_or_label);
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::CallFrame) => {
                if args.len() >= 2 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    let frame = reserve_player_ref(|player| {
                        player.get_datum(&args[1]).int_value()
                    })?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        ruffle_call_frame(sn, frame);
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::GetFlashProperty) => {
                if args.len() >= 3 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    let target = Self::checked_sync_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
                    let prop_num = reserve_player_ref(|player| {
                        player.get_datum(&args[2]).int_value()
                    })?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        match ruffle_get_flash_property(sn, &target, prop_num) {
                            Ok(val) => {
                                if let Some(s) = val.as_string() {
                                    return reserve_player_mut(|player| {
                                        Ok(player.alloc_datum(Datum::String(s)))
                                    });
                                }
                            }
                            Err(e) => warn!("getFlashProperty error: {:?}", e),
                        }
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::SetFlashProperty) => {
                if args.len() >= 4 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    let target = Self::checked_sync_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
                    let prop_num = reserve_player_ref(|player| {
                        player.get_datum(&args[2]).int_value()
                    })?;
                    let value = Self::checked_sync_datum(runtime, &args[3])?.string_value(runtime.symbols)?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        ruffle_set_flash_property(sn, &target, prop_num, &value);
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::HitTest) => {
                if args.len() >= 3 {
                    let member_ref = Self::resolve_flash_member(&args[0])?;
                    // Director stage coords; rebase to sprite-local for the
                    // classifier by subtracting the sprite's top-left.
                    let x = reserve_player_ref(|player| {
                        player.get_datum(&args[1]).int_value()
                    })?;
                    let y = reserve_player_ref(|player| {
                        player.get_datum(&args[2]).int_value()
                    })?;
                    if let Some((sn, _cl, _cm)) = member_ref {
                        let rect = reserve_player_ref(|player| {
                            get_sprite_rect_in_context(player, sn as i16)
                        });
                        let lx = (x - rect.0 as i32) as f64;
                        let ly = (y - rect.1 as i32) as f64;
                        // 0 = #background, 1 = #normal, 2 = #button, 3 = #editText.
                        let symbol = match ruffle_hit_test(sn, lx, ly) {
                            2 => "button",
                            3 => "editText",
                            1 => "normal",
                            _ => "background",
                        };
                        return Ok(runtime.player.alloc_datum(Datum::Symbol(runtime.symbols.intern(symbol))));
                    }
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::TellTarget) => {
                if args.len() >= 2 {
                    // tellTarget is complex; for now just log it
                    let target = Self::checked_sync_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
                    debug!("tellTarget: target={}", target);
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::GetaProp) => TypeHandlers::get_a_prop(runtime, args),
            Some(BuiltInSymbol::Inside) => {
                if args.len() < 2 {
                    return Err(ScriptError::new("inside requires a point and rectangle".to_string()));
                }
                runtime.with_player_and_symbols(|player, symbols| {
                    PointDatumHandlers::inside(player, symbols, &args[0], &args[1..])
                })
            }
            Some(BuiltInSymbol::AddProp) => Self::try_collection_call(runtime, name, args, false, true)?.ok_or_else(|| ScriptError::new("Cannot addProp on non-prop list".to_string())),
            Some(BuiltInSymbol::DeleteProp) => Self::try_collection_call(runtime, name, args, false, true)?.ok_or_else(|| ScriptError::new("Cannot deleteProp on non-prop list".to_string())),
            Some(BuiltInSymbol::Append) => Self::try_collection_call(runtime, name, args, true, false)?.ok_or_else(|| ScriptError::new("Cannot append to non-list".to_string())),
            Some(BuiltInSymbol::DeleteAt) => Self::try_collection_call(runtime, name, args, true, true)?.ok_or_else(|| ScriptError::new("Cannot delete at non list".to_string())),
            Some(BuiltInSymbol::DeleteOne) => Self::try_collection_call(runtime, name, args, true, false)?.ok_or_else(|| ScriptError::new("Cannot delete one from non-list".to_string())),
            Some(BuiltInSymbol::DeleteAll) => Self::try_collection_call(runtime, name, args, true, false)?.ok_or_else(|| ScriptError::new("Cannot delete all from non-list".to_string())),
            Some(BuiltInSymbol::GetOne) => Self::try_collection_call(runtime, name, args, true, true)?.ok_or_else(|| ScriptError::new("Cannot get one at non list".to_string())),
            Some(BuiltInSymbol::FindPos) => Self::try_collection_call(runtime, name, args, true, true)?.ok_or_else(|| ScriptError::new("Cannot findPos on non-list".to_string())),
            Some(BuiltInSymbol::FindPosNear) => Self::try_collection_call(runtime, name, args, true, true)?.ok_or_else(|| ScriptError::new("Cannot findPosNear on non-list".to_string())),
            Some(BuiltInSymbol::SetProp) => {
                let datum = &args[0];
                let datum_type = Self::checked_sync_datum(runtime, datum)?.type_enum();
                let args = &args[1..].to_vec();
                match datum_type {
                    DatumType::PropList => runtime.with_player_and_symbols(|player, symbols| {
                        PropListDatumHandlers::set_opt_prop(player, symbols, datum, args)
                    }),
                    DatumType::ScriptInstanceRef => ScriptInstanceDatumHandlers::set_prop(runtime.player, runtime.symbols, datum, args),
                    // `member(x).char[a..b] = v` compiles to
                    // setProp(member, #char, a, b, v) â€” a chunk write into the
                    // member's text, keeping everything outside the range.
                    DatumType::CastMemberRef => Self::set_member_chunk(runtime, datum, args),
                    _ => Err(ScriptError::new(
                        "Cannot setProp on non-prop list or child object".to_string(),
                    )),
                }
            }
            // The global getPos has Director's loose Symbol/String membership
            // behavior and is kept in the explicit helper above.
            Some(BuiltInSymbol::SetaProp) => {
                let datum = &args[0];
                let datum_type = Self::checked_sync_datum(runtime, datum)?.type_enum();
                let args = &args[1..].to_vec();
                match datum_type {
                    DatumType::PropList => runtime.with_player_and_symbols(|player, symbols| {
                        PropListDatumHandlers::set_opt_prop(player, symbols, datum, args)
                    }),
                    DatumType::ScriptInstanceRef => ScriptInstanceDatumHandlers::set_a_prop(runtime.player, runtime.symbols, datum, args),
                    _ => Err(ScriptError::new(
                        "Cannot setaProp on non-prop list or child object".to_string(),
                    )),
                }
            }
            Some(BuiltInSymbol::AddAt) => Self::try_collection_call(runtime, name, args, true, false)?.ok_or_else(|| ScriptError::new("Cannot addAt to non-list".to_string())),
            Some(BuiltInSymbol::GetNodes) => Self::get_nodes(runtime, args),
            // `move(member, dest)` is the command form of the Member method
            // `member.move(dest)` — Agent Free Ride's Miniclip wrapper calls it
            // that way (`move(hsController, member(1, 2))`). Delegate with
            // args[0] as the receiver, exactly as `duplicate` does below.
            Some(BuiltInSymbol::Move) if !args.is_empty() => {
                let receiver = &args[0];
                let rest = args[1..].to_vec();
                crate::player::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers::call(
                    runtime, receiver,
                    Symbol::builtin(BuiltInSymbol::Move),
                    &rest,
                )
            }
            Some(BuiltInSymbol::Duplicate) => {
                let item = &args[0];
                let duplicate_args = args[1..].to_vec();
                if let Some(result) = Self::try_collection_call(runtime, name, args, true, true)? {
                    return Ok(result);
                }
                Self::checked_sync_datum(runtime, item)?;
                reserve_player_mut(|player| match player.get_datum(item) {
                    Datum::Point(vals, flags) => {
                        Ok(player.alloc_datum(Datum::Point(*vals, *flags)))
                    }
                    Datum::Rect(vals, flags) => {
                        Ok(player.alloc_datum(Datum::Rect(*vals, *flags)))
                    }
                    Datum::String(s) => Ok(player.alloc_datum(Datum::String(s.clone()))),
                    Datum::Int(i) => Ok(player.alloc_datum(Datum::Int(*i))),
                    Datum::Float(f) => Ok(player.alloc_datum(Datum::Float(*f))),
                    Datum::Symbol(s) => Ok(player.alloc_datum(Datum::Symbol(s.clone()))),
                    Datum::ColorRef(c) => Ok(player.alloc_datum(Datum::ColorRef(c.clone()))),
                    Datum::Vector(v) => Ok(player.alloc_datum(Datum::Vector(*v))),
                    Datum::Transform3d(t) => Ok(player.alloc_datum(Datum::transform3d(**t))),
                    Datum::CastMember(r) => Ok(player.alloc_datum(Datum::CastMember(r.clone()))),
                    Datum::BitmapRef(bitmap_ref) => {
                        // `duplicate(image)` returns an independent copy of the
                        // bitmap; the result is an ephemeral so it's freed when
                        // the caller's DatumRef drops.
                        let new_bitmap = player
                            .bitmap_manager
                            .get_bitmap_handle(bitmap_ref)
                            .ok_or_else(|| {
                                ScriptError::new("duplicate(): source bitmap not found".to_string())
                            })?
                            .clone();
                        let new_ref = player.bitmap_manager.add_ephemeral_bitmap(new_bitmap);
                        let handle = player.bitmap_handle_for_id(new_ref)?;
                        Ok(player.alloc_datum(Datum::BitmapRef(handle)))
                    }
                    _ => Err(ScriptError::new(format!(
                        "duplicate() not implemented for type {}",
                        player.get_datum(item).type_str()
                    ))),
                })
            }
            Some(BuiltInSymbol::GetProp) => {
                Self::try_collection_call(runtime, name, args, false, true)?
                    .ok_or_else(|| ScriptError::new("Cannot getProp on non-prop list".to_string()))
            }
            Some(BuiltInSymbol::Min) => TypeHandlers::min(runtime, args),
            Some(BuiltInSymbol::Max) => TypeHandlers::max(runtime, args),
            Some(BuiltInSymbol::Sort) => TypeHandlers::sort(runtime, args),
            Some(BuiltInSymbol::Intersect) => TypeHandlers::intersect(runtime, args),
            Some(BuiltInSymbol::MapFn) => TypeHandlers::map(runtime, args),
            Some(BuiltInSymbol::PointToChar) => TypeHandlers::point_to_char(runtime, args),
            Some(BuiltInSymbol::ScrollByLine) => TypeHandlers::scroll_by_line(runtime, args),
            Some(BuiltInSymbol::ScrollByPage) => TypeHandlers::scroll_by_page(runtime, args),
            Some(BuiltInSymbol::Rollover) => MovieHandlers::rollover(runtime, args),
            // Legacy function form of the read-only mouse position properties
            // (`mouseH()` / `mouseV()` — same value as `the mouseH` / `the mouseV`),
            // in movie/stage pixels. Used by older movies for hit-region tests.
            Some(BuiltInSymbol::MouseH) => Ok(runtime.player.alloc_datum(Datum::Int(runtime.player.mouse_loc.0))),
            Some(BuiltInSymbol::MouseV) => Ok(runtime.player.alloc_datum(Datum::Int(runtime.player.mouse_loc.1))),
            Some(BuiltInSymbol::GetPropAt) => TypeHandlers::get_prop_at(runtime, args),
            Some(BuiltInSymbol::PuppetSound) => MovieHandlers::puppet_sound(args),
            Some(BuiltInSymbol::Pi) => TypeHandlers::pi(runtime, args),
            Some(BuiltInSymbol::Sin) => TypeHandlers::sin(runtime, args),
            Some(BuiltInSymbol::Cos) => TypeHandlers::cos(runtime, args),
            Some(BuiltInSymbol::Sqrt) => TypeHandlers::sqrt(runtime, args),
            Some(BuiltInSymbol::Tan) => TypeHandlers::tan(runtime, args),
            Some(BuiltInSymbol::Atan) => TypeHandlers::atan(runtime, args),
            Some(BuiltInSymbol::Sound) => TypeHandlers::sound(runtime, args),
            Some(BuiltInSymbol::Vector) => TypeHandlers::vector(runtime, args),
            Some(BuiltInSymbol::Transform) => TypeHandlers::transform3d(runtime, args),
            Some(BuiltInSymbol::Color) => TypeHandlers::color(runtime, args),
            Some(BuiltInSymbol::Date) => TypeHandlers::date(runtime, args),
            // `_system.time()` — Director 11.5 Scripting Dictionary, "time() (System)":
            // "returns the current time in the system clock as a string. The format of
            // the time string depends on the computer's time settings." No parameters.
            // `_system` resolves to the movie datum here (get_set.rs), so the call lands
            // in this built-in dispatcher; without it AreaZero's `[M] Init Game` raised
            // "No handler time for datum <_movie>" on its startup log line.
            Some(BuiltInSymbol::Time) => TypeHandlers::time(runtime, args),
            Some(BuiltInSymbol::KeyPressed) => Self::key_pressed(runtime, args),
            // Legacy function-call forms of the modifier-key state properties (Director
            // 11.5 Scripting Dictionary: Key properties `the shiftDown` / `controlDown` /
            // `optionDown` / `commandDown`, read-only). Movies call e.g. `shiftDown()`.
            Some(BuiltInSymbol::ShiftDown) => Ok(runtime.player.alloc_datum(datum_bool(runtime.player.keyboard_manager.is_shift_down()))),
            Some(BuiltInSymbol::ControlDown) => Ok(runtime.player.alloc_datum(datum_bool(runtime.player.keyboard_manager.is_control_down()))),
            Some(BuiltInSymbol::OptionDown | BuiltInSymbol::AltDown) => Ok(runtime.player.alloc_datum(datum_bool(runtime.player.keyboard_manager.is_alt_down()))),
            Some(BuiltInSymbol::CommandDown) => Ok(runtime.player.alloc_datum(datum_bool(runtime.player.keyboard_manager.is_command_down()))),
            Some(BuiltInSymbol::ShowGlobals) => Self::show_globals(runtime),
            Some(BuiltInSymbol::TellStreamStatus) => Self::tell_stream_status(runtime, args),
            Some(BuiltInSymbol::Frame) => Ok(runtime.player.alloc_datum(Datum::Int(runtime.player.movie.current_frame as i32))),
            Some(BuiltInSymbol::Label) => Self::label(runtime, args),
            Some(BuiltInSymbol::Alert) => Self::alert(runtime, args),
            Some(BuiltInSymbol::Objectp) => Self::object_p(runtime, args),
            Some(BuiltInSymbol::SoundBusy) => TypeHandlers::sound_busy(args),
            // `stopSound` (Director 11.5 Scripting Dictionary p.872) —
            // legacy command that stops the sound currently playing on
            // sound channel 1 (the implicit puppetSound channel). Fugue
            // No.4 Narrative mouseUp calls it inside `if soundBusy(1)
            // then stopSound()` to interrupt playback before checking
            // for clicked underline targets — without it, soundBusy(1)
            // stays true forever and the underline branch never runs.
            Some(BuiltInSymbol::StopSound) => {
                reserve_player_mut(|player| {
                    if let Some(ch) = player.sound_manager.get_channel(0) {
                        ch.borrow_mut().stop_sound();
                    }
                });
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::Delay) => MovieHandlers::delay(args),
            Some(BuiltInSymbol::Halt) => MovieHandlers::halt(args),
            Some(BuiltInSymbol::StartTimer) => Self::start_timer(runtime, args),
            Some(BuiltInSymbol::ExternalEvent) => Self::external_event(runtime, args),
            Some(BuiltInSymbol::DontPassEvent) => Self::dont_pass_event(runtime, args),
            Some(BuiltInSymbol::FrameReady) => Self::frame_ready(runtime, args),
            Some(BuiltInSymbol::Marker) => Self::marker(runtime, args),
            Some(BuiltInSymbol::Play) => {
                // play member("name") - play a sound on channel 1
                if args.is_empty() {
                    return Ok(DatumRef::Void);
                }
                runtime.with_player_and_symbols(|player, symbols| {
                    let channel_datum = player.alloc_datum(Datum::SoundChannel(1));
                    SoundChannelDatumHandlers::call(player, symbols, &channel_datum, Symbol::builtin(BuiltInSymbol::Play), args)
                })
            }
            Some(BuiltInSymbol::SpriteBox) => {
                // spriteBox(sprite, left, top, right, bottom)
                if args.len() < 5 {
                    return Err(ScriptError::new(
                        "spriteBox requires 5 arguments (sprite, left, top, right, bottom)".to_string(),
                    ));
                }
                reserve_player_mut(|player| {
                    let sprite_num = player.get_datum(&args[0]).to_sprite_ref()?;
                    let left = player.get_datum(&args[1]).int_value()?;
                    let top = player.get_datum(&args[2]).int_value()?;
                    let right = player.get_datum(&args[3]).int_value()?;
                    let bottom = player.get_datum(&args[4]).int_value()?;

                    // Set dimensions first so the rect computation uses the new size
                    let sprite = player.movie.score.get_sprite_mut(sprite_num);
                    sprite.width = right - left;
                    sprite.height = bottom - top;
                    sprite.has_size_changed = true;

                    // Now compute the rect with the new dimensions (scaled reg_point reflects new size)
                    let sprite = player.movie.score.get_sprite(sprite_num).unwrap();
                    let current_rect = get_concrete_sprite_rect(player, sprite);

                    // Adjust position so the displayed left/top match the desired values
                    let sprite = player.movie.score.get_sprite_mut(sprite_num);
                    sprite.loc_h += left - current_rect.left;
                    sprite.loc_v += top - current_rect.top;

                    Ok(DatumRef::Void)
                })
            }
            Some(BuiltInSymbol::PuppetTransition) => {
                // puppetTransition(int {, time, size, area}) or (transitionMemberRef).
                // Applies the transition between the current stage and the next frame;
                // we hand it to the same engine the score transition channel uses.
                // (Director 11.5 Scripting Dictionary: time is in quarter-seconds.)
                reserve_player_mut(|player| {
                    let arg0 = args.get(0).map(|d| player.get_datum(d).clone());
                    let info = match arg0 {
                        Some(Datum::Int(code)) => {
                            let time_qs = args.get(1).map(|d| player.get_datum(d).int_value().unwrap_or(0)).unwrap_or(0);
                            let size = args.get(2).map(|d| player.get_datum(d).int_value().unwrap_or(1)).unwrap_or(1);
                            Some(crate::player::cast_member::TransitionInfo {
                                transition_type: code.clamp(1, 52) as u8,
                                chunk_size: size.clamp(1, 128) as u8,
                                // quarter-seconds → ms
                                duration_ms: (time_qs.max(0).min(120) as u16).saturating_mul(250),
                            })
                        }
                        Some(Datum::CastMember(r)) => {
                            player.movie.cast_manager.find_member_by_ref(&r).and_then(|m| match &m.member_type {
                                crate::player::cast_member::CastMemberType::Transition(t) => Some(t.info),
                                _ => None,
                            })
                        }
                        _ => None,
                    };
                    if let Some(info) = info {
                        player.pending_transition = Some(info);
                        player.begin_transition_hold(info.duration_ms);
                    }
                    Ok(DatumRef::Void)
                })
            }
            Some(BuiltInSymbol::Preload) => {
                // All cast data is resident in memory (WASM) — there's nothing to
                // stream in, so preload completes instantly. Per the 11.5 Scripting
                // Dictionary, preLoad returns the number of the last frame it loaded:
                //   0 args -> current frame .. last frame of movie -> last frame
                //   1 arg  -> current frame .. frameN              -> frameN
                //   2 args -> frameA .. frameB                     -> frameB
                // A frame arg may be a number or a marker label.
                fn resolve_frame(player: &crate::player::DirPlayer, dref: &DatumRef) -> i32 {
                    let d = player.get_datum(dref);
                    if let Datum::String(s) = d {
                        if let Some(fl) = player
                            .movie
                            .score
                            .frame_labels
                            .iter()
                            .find(|fl| fl.label.eq_ignore_ascii_case(s.as_str()))
                        {
                            return fl.frame_num;
                        }
                    }
                    d.int_value().unwrap_or(0)
                }
                reserve_player_mut(|player| {
                    let last = if args.len() >= 2 {
                        resolve_frame(player, &args[1])
                    } else if args.len() == 1 {
                        resolve_frame(player, &args[0])
                    } else {
                        player.movie.score.frame_count.unwrap_or(1) as i32
                    };
                    Ok(player.alloc_datum(Datum::Int(last)))
                })
            }
            Some(BuiltInSymbol::CharPosToLoc) => {
                reserve_player_mut(|player| {
                    if args.len() < 2 {
                        return Err(ScriptError::new(
                            "charPosToLoc requires 2 arguments (member, charPos)".to_string(),
                        ));
                    }
                    let member_ref = player.get_datum(&args[0]).to_member_ref()?;
                    let char_pos = player.get_datum(&args[1]).int_value()?;

                    let member = player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .ok_or_else(|| ScriptError::new("Member not found".to_string()))?;

                    let (text, fixed_line_space, top_spacing, char_spacing, member_width, font_name, font_size, alignment, tab_stops, word_wrap) = match &member.member_type {
                        crate::player::cast_member::CastMemberType::Text(t) => {
                            (t.text.clone(), t.fixed_line_space, t.top_spacing, t.char_spacing as i16, t.width as i16, t.font.clone(), t.font_size, t.alignment.clone(), t.tab_stops.clone(), t.word_wrap)
                        }
                        crate::player::cast_member::CastMemberType::Field(f) => {
                            (f.text.clone(), f.fixed_line_space, f.top_spacing, 0, f.width as i16, f.font.clone(), f.font_size, f.alignment.clone(), Vec::new(), f.word_wrap)
                        }
                        crate::player::cast_member::CastMemberType::Button(b) => {
                            (b.field.text.clone(), b.field.fixed_line_space, b.field.top_spacing, 0, b.field.width as i16, b.field.font.clone(), b.field.font_size, b.field.alignment.clone(), Vec::new(), b.field.word_wrap)
                        }
                        _ => {
                            return Err(ScriptError::new(
                                "charPosToLoc requires a text, field, or button member".to_string(),
                            ))
                        }
                    };
                    // Effective wrap width: only honour the member's authored
                    // width when the member ACTUALLY wraps (word_wrap = true).
                    // For non-wrapping members (Habbo's Text Wrapper Class
                    // composer creates `new(#text)` members whose word_wrap
                    // defaults differ from field defaults), charPosToLoc must
                    // return single-line absolute positions even when the
                    // text would visually overflow `member.width` — the
                    // composer's `pTextMem.charPosToLoc(char.count).locH + 16`
                    // formula depends on this to compute the final bitmap
                    // width BEFORE growing the rect.
                    let effective_wrap_width: i16 = if word_wrap { member_width } else { 0 };

                    let align_kind: u8 = if alignment == BuiltInSymbol::Center {
                        1
                    } else if alignment == BuiltInSymbol::Right {
                        2
                    } else {
                        0
                    };

                    // Resolve the member's actual font the same way text.rs .image does.
                    // If it's a PFR/bitmap font we can measure locally via get_text_char_pos;
                    // otherwise we delegate to Canvas2D measure_text_native so the width
                    // matches what was rasterised into the member's .image.
                    let font_size_opt = if font_size > 0 { Some(font_size) } else { None };
                    let loaded_font = if !font_name.is_empty() {
                        player.font_manager.get_font_with_cast_and_bitmap(
                            &font_name,
                            &player.movie.cast_manager,
                            &mut player.bitmap_manager,
                            font_size_opt,
                            None,
                        )
                    } else {
                        None
                    };
                    let is_pfr = loaded_font.as_ref().map_or(false, |f| f.char_widths.is_some());

                    // char_pos is 1-based; convert to 0-based index. Also cap to text length.
                    let index = if char_pos > 0 { (char_pos - 1) as usize } else { 0 };

                    if !is_pfr && !font_name.is_empty() {
                        // Native Canvas2D path: measure the substring up to `index` using the
                        // member's font so the returned x matches the rasterised image width.
                        // Honours explicit `\r`/`\n` line breaks AND `word_wrap` + `member.width`
                        // greedy word-wrap, so the returned y advances per wrapped line. Without
                        // wrap awareness the script-side `getBubbleSize` pattern (which calls
                        // `charPosToLoc(textLength)` to get bubble height) returns single-line
                        // height for multi-line wrapped text and the resulting copyPixels src
                        // rect is too short — only the first wrapped line gets copied.
                        let display_font_name = if font_name.is_empty() { "Arial".to_string() } else { font_name.clone() };
                        let display_font_size = if font_size > 0 { font_size } else { 12 };

                        let chars_vec: Vec<char> = text.chars().collect();
                        let target = index.min(chars_vec.len());

                        // Build the measurement context once and reuse it for all
                        // word-wrap probes below.
                        let measure_ctx = {
                            use wasm_bindgen::JsCast;
                            let font_str = format!("{}px {}", display_font_size, display_font_name);
                            web_sys::window()
                                .and_then(|w| w.document())
                                .and_then(|d| d.create_element("canvas").ok())
                                .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
                                .and_then(|c| c.get_context("2d").ok().flatten())
                                .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
                                .map(|ctx| { ctx.set_font(&font_str); ctx })
                        };
                        let measure = |s: &str| -> f64 {
                            measure_ctx.as_ref()
                                .and_then(|ctx| ctx.measure_text(s).ok())
                                .map(|m| m.width()).unwrap_or(0.0)
                        };

                        // Greedy word-wrap: returns the (start_char, end_char_exclusive)
                        // bounds in `line` of each sub-line after wrapping at `wrap_w`
                        // pixels. Whitespace-only break preferred; falls back to hard
                        // break when a single token already exceeds `wrap_w`.
                        let wrap_sub_lines = |line: &str, wrap_w: f64| -> Vec<(usize, usize)> {
                            let chars: Vec<char> = line.chars().collect();
                            if chars.is_empty() { return vec![(0, 0)]; }
                            if wrap_w <= 0.0 || wrap_w == f64::INFINITY {
                                return vec![(0, chars.len())];
                            }
                            let mut subs = Vec::new();
                            let mut start = 0usize;
                            let mut last_space: Option<usize> = None;
                            let mut i = 0usize;
                            while i < chars.len() {
                                let c = chars[i];
                                if c == ' ' || c == '\t' { last_space = Some(i); }
                                let probe: String = chars[start..=i].iter().collect();
                                if measure(&probe) > wrap_w && i > start {
                                    let break_at = match last_space {
                                        Some(lb) if lb > start => lb,
                                        _ => i,
                                    };
                                    subs.push((start, break_at));
                                    let mut next = break_at;
                                    if next < chars.len() && (chars[next] == ' ' || chars[next] == '\t') {
                                        next += 1;
                                    }
                                    start = next;
                                    last_space = None;
                                    i = start;
                                    continue;
                                }
                                i += 1;
                            }
                            subs.push((start, chars.len()));
                            subs
                        };

                        let want_wrap = word_wrap && member_width > 0;
                        let wrap_w = if want_wrap { member_width as f64 } else { f64::INFINITY };

                        // Locate the natural line (split on \r/\n) containing `target`,
                        // then locate the sub-line within it.
                        let normalised: String = text.replace("\r\n", "\n").replace('\r', "\n");
                        let natural_lines: Vec<&str> = normalised.split('\n').collect();
                        let mut consumed = 0usize;
                        let mut natural_idx = 0usize;
                        let mut pos_in_natural = 0usize;
                        let mut found = false;
                        for (li, line) in natural_lines.iter().enumerate() {
                            let line_len = line.chars().count();
                            if target <= consumed + line_len {
                                natural_idx = li;
                                pos_in_natural = target - consumed;
                                found = true;
                                break;
                            }
                            consumed += line_len + 1;
                        }
                        if !found {
                            natural_idx = natural_lines.len().saturating_sub(1);
                            pos_in_natural = natural_lines.last().map_or(0, |l| l.chars().count());
                        }

                        // Sum sub-line counts of natural lines preceding the target's.
                        let mut total_line_idx = 0usize;
                        for li in 0..natural_idx {
                            let subs = wrap_sub_lines(natural_lines[li], wrap_w);
                            total_line_idx += subs.len().max(1);
                        }

                        // Within the target's natural line, find which sub-line owns `pos_in_natural`.
                        let target_line = natural_lines[natural_idx];
                        let subs = wrap_sub_lines(target_line, wrap_w);
                        let mut sub_idx = 0usize;
                        let mut sub_start = 0usize;
                        let mut sub_end = target_line.chars().count();
                        for (idx, (s, e)) in subs.iter().enumerate() {
                            if pos_in_natural <= *e {
                                sub_idx = idx;
                                sub_start = *s;
                                sub_end = *e;
                                break;
                            }
                        }
                        let target_chars: Vec<char> = target_line.chars().collect();
                        let prefix: String = target_chars[sub_start..pos_in_natural.min(target_chars.len())]
                            .iter().collect();
                        let sub_line_text: String = target_chars[sub_start..sub_end.min(target_chars.len())]
                            .iter().collect();
                        let prefix_w = measure(&prefix);
                        let line_w = measure(&sub_line_text);
                        let line_idx = total_line_idx + sub_idx;

                        let start_x = match align_kind {
                            1 if member_width > 0 => ((member_width as f64 - line_w) / 2.0).max(0.0),
                            2 if member_width > 0 => (member_width as f64 - line_w).max(0.0),
                            _ => 0.0,
                        };
                        let width = (start_x + prefix_w).round() as i32;

                        // Use the renderer's actual wrap function
                        // `wrap_lines_with_spans` to count the visual line
                        // that contains the target char. Anything else
                        // (per-char measureText, bitmap-font advance sums)
                        // diverges from the rendered layout by a few px
                        // per line — drift of 30-40 lines accumulates over
                        // a 26k-char field and lands AdvanceScroll several
                        // sections away from the intended header.
                        let line_step = if fixed_line_space > 0 {
                            fixed_line_space as i32
                        } else {
                            display_font_size as i32
                        };
                        // Convert char_pos (1-based) to byte position.
                        let target_byte: usize = text
                            .char_indices()
                            .nth(index)
                            .map(|(b, _)| b)
                            .unwrap_or_else(|| text.len());
                        // Look up the actual loaded font (PFR1 if available).
                        let render_font = player.font_manager.get_font_with_cast_and_bitmap(
                            &display_font_name,
                            &player.movie.cast_manager,
                            &mut player.bitmap_manager,
                            Some(display_font_size),
                            None,
                        ).or_else(|| player.font_manager.get_system_font());
                        let visual_line = if let Some(font) = render_font {
                            let lines = crate::player::bitmap::bitmap::Bitmap
                                ::wrap_lines_with_spans(&text, &font, if effective_wrap_width > 0 { effective_wrap_width as i32 } else { i32::MAX });
                            let mut idx = 0usize;
                            for (i, ls) in lines.iter().enumerate() {
                                if target_byte >= ls.start && target_byte <= ls.end {
                                    idx = i;
                                    break;
                                }
                                if i == lines.len() - 1 { idx = i; }
                            }
                            idx as i32
                        } else {
                            line_idx as i32
                        };
                        // See branch B: a trailing newline's location is the line
                        // it creates, so charPosToLoc(char.count) on text ending
                        // in \r gives the full height for GetMaxScroll. Gated to
                        // the last char + a newline so letter-terminated text
                        // returns the char's line top (Coke Studios' shrink loop
                        // requires charPosToLoc(1).locV == charPosToLoc(len).locV
                        // for single-line text — see branch B note).
                        let trailing_nl_a = index + 1 >= text.chars().count()
                            && matches!(text.chars().nth(index), Some('\r') | Some('\n'));
                        let y = top_spacing as i32
                            + (visual_line + if trailing_nl_a { 1 } else { 0 }) * line_step;

                        Ok(player.alloc_datum(Datum::Point([width as f64, y as f64], 0)))
                    } else {
                        let font = loaded_font
                            .or_else(|| player.font_manager.get_system_font())
                            .ok_or_else(|| ScriptError::new("No font available".to_string()))?;
                        let params = crate::player::font::DrawTextParams {
                            font: &font,
                            line_height: None,
                            line_spacing: fixed_line_space,
                            top_spacing,
                            char_spacing,
                            // Honour word_wrap: only constrain measurement
                            // by the authored width when the member actually
                            // wraps. Non-wrapping members (Habbo composer's
                            // `new(#text)` titles) need single-line absolute
                            // positions.
                            member_width: if effective_wrap_width > 0 { Some(effective_wrap_width) } else { None },
                            // Match the renderer's per-field space-min clamp
                            // (0.30 * font_size) so charPosToLoc and the
                            // hit-test stay aligned with the drawn layout.
                            // Without this, `the locToCharPos` returns a
                            // char position from a hit-test layout with
                            // narrower spaces than the rendered one — so
                            // a click on visual "Christ's passion" maps
                            // into the following "the sign of the cross"
                            // chunk.
                            min_space_advance: {
                                let sz = font.font_size.max(font.char_height) as i32;
                                let v = ((sz as f32) * 0.30).round() as i16;
                                if v > 0 { Some(v) } else { None }
                            },
                            per_char_advances: None,
                        };
                        // Tab-aware char position. Coke Studios' userlist computes
                        // the dotted-separator bounds via two charPosToLoc calls,
                        // one landing on a char inside "Go!" (past the tab) and
                        // one on the line's last char. We need the tab to advance
                        // to its tab-stop for those positions to bracket the
                        // empty space between the roomname column and the "Go!"
                        // column — which is where the dotted line should span.
                        // When the requested char index is BEFORE any tab on its
                        // line we stay at the pre-tab x so Lingo that asks for
                        // positions inside the name column (e.g. underline draws)
                        // still returns the usual advance-based x.
                        let (x_from_zero, y) = if text.contains('\t') && !tab_stops.is_empty() {
                            let eff_lh = if font.font_size > 0 { font.font_size } else { font.char_height };
                            let line_step = fixed_line_space.max(eff_lh) as i16 + 1;
                            // Helper: width of a substring (chars only, excluding control chars).
                            let segment_width = |chars: &[char], from: usize| -> i16 {
                                let mut w: i16 = 0;
                                for c in chars.iter().skip(from) {
                                    if *c == '\t' || *c == '\r' || *c == '\n' { break; }
                                    w = w.saturating_add(
                                        font.get_char_advance(*c as u8) as i16 + 1 + char_spacing,
                                    );
                                }
                                w
                            };
                            let chars: Vec<char> = text.chars().collect();
                            let mut x: i16 = 0;
                            let mut y: i16 = top_spacing;
                            let mut current_line_tab_count: usize = 0;
                            let mut char_i: usize = 0;
                            let mut prev_was_cr = false;
                            let mut result: Option<(i16, i16)> = None;
                            while char_i < chars.len() {
                                if char_i == index {
                                    result = Some((x, y));
                                    break;
                                }
                                let c = chars[char_i];
                                if c == '\n' && prev_was_cr {
                                    prev_was_cr = false;
                                    char_i += 1;
                                    continue;
                                }
                                if c == '\r' || c == '\n' {
                                    prev_was_cr = c == '\r';
                                    x = 0;
                                    y = y.saturating_add(line_step);
                                    current_line_tab_count = 0;
                                } else if c == '\t' {
                                    prev_was_cr = false;
                                    if let Some(stop) = tab_stops.get(current_line_tab_count) {
                                        let stop_pos = stop.position as i16;
                                        // Match the renderer's flush_line tab logic
                                        // (text.rs): right/center tabs look ahead at
                                        // the next segment's width to anchor the
                                        // segment to the stop's right edge / centre.
                                        let next_seg_w = segment_width(&chars, char_i + 1);
                                        let new_x = match stop.tab_type.as_str() {
                                            "right" => (stop_pos - next_seg_w).max(x),
                                            "center" => (stop_pos - next_seg_w / 2).max(x),
                                            _ => stop_pos.max(x), // #left / #decimal
                                        };
                                        x = new_x;
                                    }
                                    current_line_tab_count += 1;
                                } else {
                                    prev_was_cr = false;
                                    let adv = font.get_char_advance(c as u8) as i16
                                        + 1 + char_spacing;
                                    x = x.saturating_add(adv);
                                }
                                char_i += 1;
                            }
                            // If we exited the loop without hitting `index` (target
                            // beyond end-of-text), result stays None and we fall back
                            // to the final (x, y).
                            if result.is_none() && char_i == index {
                                result = Some((x, y));
                            }
                            result.unwrap_or((x, y))
                        } else {
                            // Use the renderer's own wrap_lines_with_spans
                            // to find the visual line containing the target
                            // char. Any other algorithm (my chars-based pre-
                            // scan in get_text_char_pos, Canvas2D per-char
                            // measureText) diverges by a few px per line
                            // and lands the AdvanceScroll target several
                            // sections away from the intended header.
                            let target_byte: usize = text
                                .char_indices()
                                .nth(index)
                                .map(|(b, _)| b)
                                .unwrap_or_else(|| text.len());
                            let line_step = if fixed_line_space > 0 {
                                fixed_line_space as i16
                            } else if font.font_size > 0 {
                                font.font_size as i16
                            } else {
                                font.char_height as i16
                            };
                            let lines = crate::player::bitmap::bitmap::Bitmap
                                ::wrap_lines_with_spans(&text, &font, if effective_wrap_width > 0 { effective_wrap_width as i32 } else { i32::MAX });
                            let mut visual_idx: usize = 0;
                            for (i, ls) in lines.iter().enumerate() {
                                if target_byte >= ls.start && target_byte <= ls.end {
                                    visual_idx = i;
                                    break;
                                }
                                if i + 1 == lines.len() {
                                    visual_idx = i;
                                }
                            }
                            // x: position within the line, computed by
                            // get_text_char_pos (which handles char-level
                            // x correctly). y: from visual_idx * line_step.
                            let (x_pos, _y_unused) = crate::player::font::get_text_char_pos(&text, &params, index);
                            // A trailing newline's location is the line it
                            // CREATES (the next one), not the line it ends — so
                            // charPosToLoc(char.count) on text ending in \r gives
                            // the full height for GetMaxScroll (spectral-wizard's
                            // help-story scroll). Gated to the last char AND a
                            // newline: charPosToLoc(textLength) on letter-
                            // terminated text must return that char's line TOP so
                            // it equals charPosToLoc(1).locV for single-line text.
                            // Coke Studios' window shrink-to-one-line loop spins
                            // `while charPosToLoc(m,1).locV <> charPosToLoc(m,len).locV`
                            // — a non-newline-gated bump never converged → freeze.
                            let trailing_nl_b = index + 1 >= text.chars().count()
                                && matches!(text.chars().nth(index), Some('\r') | Some('\n'));
                            let y_pos = top_spacing.saturating_add(
                                (visual_idx as i16 + if trailing_nl_b { 1 } else { 0 }) * line_step,
                            );
                            (x_pos, y_pos)
                        };

                        // Apply alignment offset so the returned x matches the pixel position
                        // in the rasterised image (the bitmap render centres/right-aligns the
                        // line inside member_width; see text.rs flush_line at lines 888-892).
                        //
                        // EXCEPTION: when the line has a right or center tab, the renderer
                        // anchors segments to those stops and skips alignment (see
                        // flush_line's `has_right_tab` branch). We must skip alignment too,
                        // otherwise charPosToLoc returns positions shifted by the centring
                        // amount even though the rendered text is left-anchored. Coke
                        // Studios userlist hits this: alignment=#center inherited from the
                        // loading screen, but each row uses [#left at 18, #right at pwidth-1]
                        // tabs — Lingo's dotted-line bounds were drawn off-canvas because
                        // dotleft/dotright both got an extra ~71px centring offset.
                        let line_has_anchor_tab = !tab_stops.is_empty()
                            && tab_stops.iter().any(|t| {
                                t.tab_type == BuiltInSymbol::Right || t.tab_type == BuiltInSymbol::Center
                            });
                        let start_x = if align_kind != 0 && member_width > 0 && !line_has_anchor_tab {
                            // Compute the width of the line that `index` falls on, using the
                            // same advance-per-char sum as flush_line.
                            let normalised: String = text.replace("\r\n", "\n").replace('\r', "\n");
                            let mut consumed = 0usize;
                            let target = index.min(text.chars().count());
                            let mut hit_line: Option<String> = None;
                            for line in normalised.split('\n') {
                                let line_len = line.chars().count();
                                if target <= consumed + line_len {
                                    hit_line = Some(line.to_string());
                                    break;
                                }
                                consumed += line_len + 1;
                            }
                            let line = hit_line.unwrap_or_else(|| {
                                normalised.split('\n').last().unwrap_or("").to_string()
                            });
                            let line_width: i32 = line
                                .chars()
                                .map(|c| font.get_char_advance(c as u8) as i32 + char_spacing as i32)
                                .sum();
                            match align_kind {
                                1 => ((member_width as i32 - line_width) / 2).max(0),
                                2 => (member_width as i32 - line_width).max(0),
                                _ => 0,
                            }
                        } else {
                            0
                        };

                        let x = x_from_zero as i32 + start_x;
                        Ok(player.alloc_datum(Datum::Point([x as f64, y as f64], 0)))
                    }
                })
            }
            Some(BuiltInSymbol::LocVToLinePos | BuiltInSymbol::LinePosToLocV | BuiltInSymbol::LocToCharPos | BuiltInSymbol::CharPosToLoc)
                if !args.is_empty() =>
            {
                // Global form `locVToLinePos(member, loc)` etc. Director exposes
                // these text/field position helpers both as member methods and as
                // globals whose first arg is the member. Delegate to the member's
                // own handler (implemented in cast_member/text.rs & field.rs).
                let rest = args[1..].to_vec();
                crate::player::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers::call(
                    runtime, &args[0], name, &rest,
                )
            }
            _ => {
                let handler_name = runtime
                    .symbols
                    .display(&name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                    .to_owned();
                // Check if first arg is an xtra instance - if so, forward to the xtra instance handler
                if let Some(receiver_ref) = args.first() {
                    let receiver = match Self::checked_sync_datum(runtime, receiver_ref)? {
                        Datum::XtraInstance(name, id) => Some((name.clone(), *id)),
                        _ => None,
                    };
                    if let Some((xtra_name, instance_id)) = receiver {
                        let remaining_args = args[1..].to_vec();
                        // RuntimeSession::dispatch_global_inner converts registered
                        // external Multiuser/Curl instances into owned pending
                        // requests before this synchronous fallback is reached.
                        // The fallback therefore remains owner-bound for builtin
                        // instance handlers without reintroducing global managers.
                        if matches!(xtra_name.to_ascii_lowercase().as_str(), "multiuser" | "curl" | "fileio" | "xmlparser") {
                            return crate::player::xtra::manager::call_instance_handler_explicit(
                                runtime.player,
                                runtime.symbols,
                                &xtra_name,
                                receiver_ref,
                                &handler_name,
                                &remaining_args,
                            );
                        }
                        return call_xtra_instance_handler(
                            runtime.player,
                            &xtra_name,
                            instance_id,
                            &handler_name,
                            &remaining_args,
                            runtime.symbols,
                        );
                    }
                }
                // Static-only Xtras (OpenURL, SysMenu, BudAPI, Curl statics).
                if let Some(res) = crate::player::xtra::manager::try_call_xtra_static_handler_explicit(
                    runtime.player,
                    runtime.symbols,
                    &handler_name,
                    args,
                ) {
                    return res;
                }
                if let Some(res) =
                    crate::player::xtra::manager::try_call_xtra_static_handler_with_symbols(
                        runtime.player,
                        &handler_name,
                        args,
                        runtime.symbols,
                    )
                {
                    return res;
                }
                // Ordinary unknown-handler diagnostics consume arguments only
                // while formatting the final error. Xtra routes above may
                // intentionally ignore malformed or foreign trailing args.
                let checked_args = args
                    .iter()
                    .map(|arg| Self::checked_sync_datum(runtime, arg).map(Clone::clone))
                    .collect::<Result<Vec<_>, _>>()?;
                let formatted_args = runtime.with_player_and_symbols(|player, symbols| {
                    let mut s = String::new();
                    for arg in &checked_args {
                        if !s.is_empty() { s.push_str(", "); }
                        s.push_str(&format_concrete_datum(arg, symbols, player)?);
                    }
                    Ok::<String, ScriptError>(s)
                })?;
                let msg = format!("No built-in handler: {}({})", handler_name, formatted_args);
                warn!("{msg}");
                return Err(ScriptError::new(msg));
            }
        }
    }

    fn alert(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let message = Self::checked_sync_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
        trace_output(runtime.player, &format!("Alert: {}", message));
        Ok(DatumRef::Void)
    }

    fn label(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            let label_name = Self::checked_sync_datum(runtime, &args[0])
                ?.string_value(runtime.symbols)?;

            debug!("Searching for label: {}", label_name);

            let label_name_lower = label_name.to_lowercase();

            let label = runtime
                .player
                .movie
                .score
                .frame_labels
                .iter()
                .find(|label| label.label.to_lowercase() == label_name_lower);

            // Log result
            if let Some(lbl) = label {
                debug!(
                    "Found label '{}' at frame {}",
                    lbl.label, lbl.frame_num
                );
            } else {
                warn!("Label not found");
            }

            Ok(runtime.player.alloc_datum(Datum::Int(
                label.map_or(0, |label| label.frame_num as i32),
            )))
    }

    fn show_globals(runtime: &mut ExecutionContext<'_>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            trace_output(player, "--- Global Variables ---");
            let rendered = player
                .globals
                .iter()
                .map(|(name, value)| {
                    let value = format_datum(value, symbols, player)?;
                    let name = symbols
                        .display(name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    Ok::<_, ScriptError>(format!("{} = {}", name, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for line in rendered {
                trace_output(player, &line);
            }
            Ok(DatumRef::Void)
        })
    }

    pub fn key_pressed(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Self::checked_sync_datum(runtime, &args[0])?;
        runtime.with_player_and_symbols(|player, symbols| {
            // Mark this iteration as input-polling so the bytecode busy-wait
            // yield only kicks in for `repeat while keyPressed(...)` spins.
            player.input_polled = true;
            let arg_datum = player.get_datum(&args[0]);

            // An INTEGER argument is a direct key code (this is how games store
            // their configurable keys — spectral-wizard's `settingsKeys` uses
            // SW codes like #jump:7 (X), #attack:6 (Z), #down:125). Handle it
            // BEFORE the string path: `Datum::Int(7).string_value()` is "7",
            // which the length-1 branch below would misread as the *character*
            // '7' and look up the digit-7 key — so keyPressed(7) checked the
            // wrong key and jump/attack never fired. Single-/double-digit codes
            // were the visible failures; multi-digit ones (arrows) happened to
            // round-trip through the number-parse branch.
            if let crate::director::lingo::datum::Datum::Int(code) = arg_datum {
                // Director truncates the key code to its low byte. Mac virtual
                // key codes span 0-127 (the arrows are 123-126), so the low
                // byte IS the whole code space and 256 means code 0 — the `a`
                // key. The dictionary doesn't document the truncation; it's
                // inferred from Merlin's Revenge 3, whose #wasd binding stores
                // `#left: 256` and moves left in Director. The movie's own
                // key-config code treats 0 as "unassigned", so `a` needed a
                // non-zero stand-in.
                //
                // Masking can only change values >= 256, which no real key code
                // reaches, so nothing that matches today stops matching.
                let code = *code & 0xFF;
                // An ASCII letter code (A-Z/a-z) maps through the char table;
                // any other integer is used as the SW key code verbatim.
                let key_code = if (65..=90).contains(&code) || (97..=122).contains(&code) {
                    let ch = (code as u8 as char).to_lowercase().next().unwrap();
                    *keyboard_map::get_char_to_keycode_map().get(&ch).unwrap_or(&(code as u16))
                } else {
                    code as u16
                };
                let is_pressed = player
                    .keyboard_manager
                    .down_keys
                    .iter()
                    .any(|key| key.code == key_code);
                return Ok(player.alloc_datum(datum_bool(is_pressed)));
            }

            let key_code = if let Ok(key_str) = arg_datum.string_value(symbols) {
                // Named key constants. Director's SPACE/RETURN/TAB/... are
                // character constants, but movies pass them to keyPressed in a
                // form that reaches us as the symbolic name (e.g. spectral-
                // wizard's `keyPressed("SPACE")` from `keyPressed(SPACE)`).
                // Map the common names to Mac virtual key codes.
                // Normalise to LOWERCASE and match lowercase literals. This
                // upper-cased the input while the arms were written lower/mixed
                // case, so only ESCAPE/ESC could ever match and `keyPressed(SPACE)`
                // fell through to the single-character branch, failed `len() == 1`
                // and raised "cannot parse string 'SPACE'".
                if let Some(code) = match key_str.to_ascii_lowercase().as_str() {
                    "space" => Some(49u16),
                    "return" => Some(36),
                    "enter" => Some(76),
                    "tab" => Some(48),
                    "backspace" => Some(51),
                    "escape" | "esc" => Some(53),
                    _ => None,
                } {
                    code
                }
                // STRING: check if it's a single character
                else if key_str.len() == 1 {
                    // Single character - convert to Director key code
                    let ch = key_str
                        .chars()
                        .next()
                        .unwrap();

                    // First check for special Director characters (arrow keys, etc.)
                    // These are control characters like ASCII 28-31 for arrow keys
                    if let Some(&code) = keyboard_map::get_director_special_char_to_keycode_map().get(&ch) {
                        code
                    } else {
                        // Regular character - lowercase and look up
                        let ch_lower = ch.to_lowercase().next().unwrap();
                        *keyboard_map::get_char_to_keycode_map()
                            .get(&ch_lower)
                            .unwrap_or(&0)
                    }
                } else {
                    // Try to parse as number string (like "123")
                    if let Ok(code) = key_str.parse::<i32>().map(|c| c & 0xFF) {
                        // Check if it's an ASCII letter code that needs mapping
                        let mapped_code = if (65..=90).contains(&code) || (97..=122).contains(&code)
                        {
                            let ch = (code as u8 as char).to_lowercase().next().unwrap();
                            *keyboard_map::get_char_to_keycode_map()
                                .get(&ch)
                                .unwrap_or(&(code as u16))
                        } else {
                            code as u16
                        };
                        mapped_code
                    } else {
                        return Err(ScriptError::new(format!(
                            "keyPressed: cannot parse string '{}'",
                            key_str
                        )));
                    }
                }
            } else if let Ok(code) = arg_datum.int_value() {
                // INTEGER: Check if it's an ASCII code that needs mapping
                let mapped_code = if (65..=90).contains(&code) || (97..=122).contains(&code) {
                    let ch = (code as u8 as char).to_lowercase().next().unwrap();
                    *keyboard_map::get_char_to_keycode_map()
                        .get(&ch)
                        .unwrap_or(&(code as u16))
                } else {
                    code as u16
                };
                mapped_code
            } else {
                return Err(ScriptError::new(
                    "keyPressed expects a string or integer".to_string(),
                ));
            };

            // Check if any currently pressed key matches this code
            let is_pressed = player
                .keyboard_manager
                .down_keys
                .iter()
                .any(|key| key.code == key_code);

            // Debug: log arrow key checks (per-key counters)
            if is_pressed && (key_code == 123 || key_code == 124) {
                static KP_LR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if KP_LR.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5 {
                    debug!(
                        "[KEY-STEER] keyPressed({}) = TRUE ({})",
                        key_code, if key_code == 123 { "LEFT" } else { "RIGHT" }
                    );
                }
            }
            if is_pressed && (key_code == 125 || key_code == 126) {
                static KP_UD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if KP_UD.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                    debug!(
                        "[KEY-DRIVE] keyPressed({}) = TRUE ({})",
                        key_code, if key_code == 126 { "UP" } else { "DOWN" }
                    );
                }
            }

            Ok(player.alloc_datum(datum_bool(is_pressed)))
        })
    }

    pub fn get_nodes(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new(
                "getNodes requires 2 arguments: xml_node, node_name".to_string(),
            ));
        }

        let xml_node = Self::checked_sync_datum(runtime, &args[0])?.clone();
        let node_name = Self::checked_sync_datum(runtime, &args[1])?
            .string_value(runtime.symbols)?;
        runtime.with_player_and_symbols(|player, symbols| {
            debug!("🔧 getNodes called for node type: {}", node_name);

            // Get the XML node ID
            let xml_id = match &xml_node {
                Datum::XmlRef(id) => *id,
                _ => {
                    return Err(ScriptError::new(
                        "First argument must be an XML node reference".to_string(),
                    ));
                }
            };

            // Use XmlHelper to search for matching nodes
            let matching_nodes = XmlHelper::find_nodes_by_name(player, xml_id, &node_name);

            Ok(player.alloc_datum(Datum::List(
                crate::director::lingo::datum::DatumType::List,
                VecDeque::from(matching_nodes),
                false,
            )))
        })
    }

    fn tell_stream_status(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.len() < 1 {
            return Err(ScriptError::new(
                "tellStreamStatus requires 1 argument".to_string(),
            ));
        }

        let enable = Self::checked_sync_datum(runtime, &args[0])?
            .bool_value()
            .unwrap_or(false);
        runtime.with_player_and_symbols(|player, _symbols| {
            player.enable_stream_status_handler = enable;
            if enable {
                // Clear reported state so all tasks get fresh callbacks
                player.stream_status_reported.clear();
            }
            Ok(player.alloc_datum(Datum::Int(enable as i32)))
        })
    }
    
    fn void_p(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Ok(runtime.player.alloc_datum(Datum::Int(1)));
        }
        let datum = Self::checked_sync_datum(runtime, &args[0])?;
        let is_void = matches!(datum, Datum::Void | Datum::Null);
        Ok(runtime.player.alloc_datum(Datum::Int(if is_void { 1 } else { 0 })))
    }
    
    fn object_p(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            if args.is_empty() {
                return Ok(runtime.player.alloc_datum(Datum::Int(0)));
            }
            
            let datum = Self::checked_sync_datum(runtime, &args[0])?;
            
            // Director considers these as objects (not primitives)
            let is_object = matches!(
                datum,
                Datum::ScriptInstanceRef(_)
                | Datum::SpriteRef(_)
                | Datum::CastMember(_)
                | Datum::List(..)
                | Datum::PropList(..)
                | Datum::BitmapRef(_)
                | Datum::ScriptRef(_)
                | Datum::XmlRef(_)
                | Datum::Xtra(_)
                | Datum::XtraInstance(..)
                | Datum::Matte(..)
                | Datum::PlayerRef
                | Datum::MovieRef
                | Datum::MouseRef
                | Datum::Stage
                | Datum::CastLib(_)
                | Datum::DateRef(_)
                | Datum::MathRef(_)
                | Datum::SoundRef(_)
                | Datum::SoundChannel(_)
                | Datum::CursorRef(_)
                | Datum::TimeoutRef(_)
            );
            
            Ok(runtime.player.alloc_datum(Datum::Int(if is_object { 1 } else { 0 })))
    }

    fn start_timer(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player(|player| {
            // Reset the start_time to current time
            player.start_time = chrono::Local::now();
            Ok(DatumRef::Void)
        })
    }

    pub fn external_event(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let event_string = Self::checked_sync_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
        debug!("🔔 externalEvent: {}", event_string);
        crate::js_api::JsApi::dispatch_external_event(&event_string);
        Ok(DatumRef::Void)
    }

    fn dont_pass_event(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player(|player| {
            let scope_ref = player.current_scope_ref();
            if let Some(scope) = player.scopes.get_mut(scope_ref) {
                scope.passed = false;  // Set passed to false to stop propagation
            }
            Ok(DatumRef::Void)
        })
    }

    fn frame_ready(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        // Resolve each consumed argument immediately before converting it so
        // an ordinary conversion error on an earlier argument does not
        // consume a later reference as a side effect.
        let (start_frame, end_frame) = if args.is_empty() {
            let current = runtime.player.movie.current_frame;
            (current, current)
        } else {
            let start = Self::checked_sync_datum(runtime, &args[0])?
                .int_value()? as u32;
            let end = if args.len() > 1 {
                Self::checked_sync_datum(runtime, &args[1])?
                    .int_value()? as u32
            } else {
                start
            };
            (start, end)
        };
        runtime.with_player(|player| {

            debug!("frameReady checking frames {} to {}", start_frame, end_frame);

            // Check if frame range is valid
            if start_frame < 1 || end_frame < start_frame {
                return Ok(player.alloc_datum(Datum::Int(0)));
            }

            // Collect all unique cast member references used in the frame range
            let mut cast_members_to_check = std::collections::HashSet::new();
            
            for frame_num in start_frame..=end_frame {
                // Check channel initialization data for this frame
                for (frame_index, channel_index, data) in &player.movie.score.channel_initialization_data {
                    if *frame_index + 1 == frame_num {
                        // Skip empty sprites
                        if data.cast_lib > 0 && data.cast_member > 0 {
                            cast_members_to_check.insert((data.cast_lib, data.cast_member));
                        }
                    }
                }
            }

            debug!("Found {} unique cast members to check", cast_members_to_check.len());

            // Check if all cast members are loaded
            for (cast_lib, cast_member_num) in cast_members_to_check {
                if let Ok(cast) = player.movie.cast_manager.get_cast(cast_lib as u32) {
                    if let Some(cast_member) = cast.members.get(&(cast_member_num as u32)) {
                        // Check if the cast member is fully loaded based on its type
                        match &cast_member.member_type {
                            crate::player::cast_member::CastMemberType::Bitmap(bitmap_member) => {
                                // Check if bitmap is loaded by checking if it exists in bitmap_manager
                                if player.bitmap_manager.get_bitmap(bitmap_member.image_ref).is_none() {
                                    debug!("Cast member {}.{} (bitmap) not ready", cast_lib, cast_member_num);
                                    return Ok(player.alloc_datum(Datum::Int(0)));
                                }
                            }
                            crate::player::cast_member::CastMemberType::Sound(_sound_member) => {
                                // Sound members are loaded when the cast member exists
                                // No additional check needed
                            }
                            // Other types (text, field, shape, script) are generally ready immediately
                            _ => {}
                        }
                    } else {
                        // Cast member doesn't exist - this is OK in Director
                        // Missing cast members are just treated as empty/not displayed
                        debug!("Cast member {}.{} doesn't exist (skipping)", cast_lib, cast_member_num);
                    }
                } else {
                    // Cast lib doesn't exist - this is also OK
                    debug!("Cast lib {} doesn't exist (skipping)", cast_lib);
                }
            }

            debug!("All frames ready!");
            Ok(player.alloc_datum(Datum::Int(1)))
        })
    }

    fn marker(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
            if args.is_empty() {
                return Err(ScriptError::new("marker requires 1 argument".to_string()));
            }

            let arg = Self::checked_sync_datum(runtime, &args[0])?.clone();
            let player = &mut *runtime.player;
            
            match arg {
                // marker(n) returns the frame number of the nth marker relative to current frame.
                // marker(0) = current marker (nearest at or before current frame)
                // marker(1) = next marker, marker(-1) = previous marker, etc.
                Datum::Int(offset) => {
                    let current_frame = player.movie.current_frame as i32;
                    let labels = &player.movie.score.frame_labels;

                    // Find the index of the current marker (last marker at or before current frame)
                    let current_idx = labels.iter().rposition(|l| l.frame_num <= current_frame);

                    let target_idx = if offset >= 0 {
                        current_idx.map(|i| i as i32 + offset).unwrap_or(offset - 1)
                    } else {
                        current_idx.map(|i| i as i32 + offset).unwrap_or(-1)
                    };

                    if target_idx >= 0 && (target_idx as usize) < labels.len() {
                        Ok(player.alloc_datum(Datum::Int(labels[target_idx as usize].frame_num)))
                    } else {
                        // Out of range - return 0
                        Ok(player.alloc_datum(Datum::Int(0)))
                    }
                }
                // If argument is a string, return the frame number of that marker
                Datum::String(marker_name) => {
                    let marker_name_lower = marker_name.to_lowercase();
                    let marker = player
                        .movie
                        .score
                        .frame_labels
                        .iter()
                        .find(|label| label.label.to_lowercase() == marker_name_lower);
                    
                    Ok(player.alloc_datum(Datum::Int(
                        marker.map_or(0, |label| label.frame_num as i32),
                    )))
                }
                Datum::Symbol(symbol) => {
                    let marker_name = runtime
                        .symbols
                        .display(&symbol)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    let marker_name_lower = marker_name.to_lowercase();
                    let marker = player
                        .movie
                        .score
                        .frame_labels
                        .iter()
                        .find(|label| label.label.to_lowercase() == marker_name_lower);
                    
                    Ok(player.alloc_datum(Datum::Int(
                        marker.map_or(0, |label| label.frame_num as i32),
                    )))
                }
                _ => Err(ScriptError::new(format!(
                    "marker expects string or integer, got {}",
                    arg.type_str()
                ))),
            }
    }
    /// `setProp(member, #char|#word|#item|#line, first, last, value)` â€” the
    /// compiled form of `member(x).char[a..b] = value`. Replaces exactly that
    /// range of the member's text and leaves the rest alone. `last` is
    /// optional: `member(x).char[a] = v` compiles with four arguments.
    pub fn set_member_chunk(
        runtime: &mut ExecutionContext<'_>,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        use crate::director::lingo::datum::{StringChunkExpr, StringChunkType};
        use crate::player::handlers::datum_handlers::string_chunk::StringChunkUtils;
        use crate::player::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers;
        if args.len() < 3 {
            return Err(ScriptError::new(format!(
                "setProp on a member needs a chunk kind, a range and a value (got {} args)",
                args.len()
            )));
        }
        let datum_value = Self::checked_sync_datum(runtime, datum)?.clone();
        let member_ref = match &datum_value {
            Datum::CastMember(member_ref) => member_ref.to_owned(),
            other => {
                return Err(ScriptError::new(format!(
                    "setProp: expected a cast member, got {}", other.type_str()
                )))
            }
        };
        let kind = match Self::checked_sync_datum(runtime, &args[0])? {
            Datum::Symbol(sym) => sym.clone(),
            other => {
                return Err(ScriptError::new(format!(
                    "setProp: expected a chunk symbol, got {}", other.type_str()
                )))
            }
        };
        let chunk_type = StringChunkType::from_symbol(&kind, runtime.symbols)?;
        let first = Self::checked_sync_datum(runtime, &args[1])?.int_value()?;
        let (last, value_ref) = if args.len() >= 4 {
            let last = Self::checked_sync_datum(runtime, &args[2])?.int_value()?;
            let value = Self::checked_sync_datum(runtime, &args[3])?
                .string_value(runtime.symbols)?;
            (last, value)
        } else {
            let value = Self::checked_sync_datum(runtime, &args[2])?
                .string_value(runtime.symbols)?;
            (first, value)
        };
        let replacement = value_ref;
        let current_result = CastMemberRefHandlers::get_prop(
            runtime.player,
            runtime.symbols,
            &member_ref,
            Symbol::builtin(BuiltInSymbol::Text),
        ).and_then(|d| d.string_value(runtime.symbols));
        let current = match current_result {
            Ok(text) => text,
            Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
            Err(_) => String::new(),
        };
        let item_delimiter = runtime.player.movie.item_delimiter;
        let chunk_expr = StringChunkExpr { chunk_type, start: first, end: last, item_delimiter };
        let new_text = StringChunkUtils::string_by_putting_into_chunk(&current, &chunk_expr, &replacement)?;
        CastMemberRefHandlers::set_prop(
            runtime.player,
            runtime.symbols,
            &member_ref,
            Symbol::builtin(BuiltInSymbol::Text),
            Datum::String(new_text),
        )?;
        Ok(DatumRef::Void)
    }

}


/// Count word-wrap visual line breaks (NOT source `\r\n` breaks) that
/// occur strictly before `target_char_idx` in `text`, given a wrap
/// width and font. Used by `charPosToLoc` for native-rendered fields
/// so the returned y matches the wrapped visual layout. Width is
/// measured via Canvas2D `measureText` so it agrees with the
/// rasterizer.
fn count_wraps_before_index(
    text: &str,
    font_name: &str,
    font_size: u16,
    wrap_w: i16,
    target_char_idx: usize,
) -> usize {
    use wasm_bindgen::JsCast;
    let ctx_opt = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.create_element("canvas").ok())
        .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .and_then(|c| c.get_context("2d").ok().flatten())
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok());
    let Some(ctx) = ctx_opt else { return 0; };
    ctx.set_font(&format!("{}px {}", font_size, font_name));
    // Walk source lines; for each line, simulate word-wrap and count
    // breaks that occur at char positions < target_char_idx.
    let normalised: String = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut consumed_chars = 0usize;
    let mut extra_wraps = 0usize;
    for source_line in normalised.split('\n') {
        let line_chars: Vec<char> = source_line.chars().collect();
        let line_len = line_chars.len();
        if consumed_chars + line_len + 1 < target_char_idx {
            // Need full wrap count of this line, target is past it.
            extra_wraps += wraps_in_line(&ctx, source_line, wrap_w);
            consumed_chars += line_len + 1; // +1 for the \n
            continue;
        }
        // Target is somewhere on this source line. Count wraps that
        // happen before (target_char_idx - consumed_chars) chars in.
        let offset_in_line = target_char_idx.saturating_sub(consumed_chars).min(line_len);
        extra_wraps += wraps_in_line_up_to(&ctx, source_line, wrap_w, offset_in_line);
        break;
    }
    extra_wraps
}

/// Total visual-line breaks (excluding the implicit final line) when
/// `line` is word-wrapped at `wrap_w` pixels. Uses Canvas2D for width.
fn wraps_in_line(
    ctx: &web_sys::CanvasRenderingContext2d,
    line: &str,
    wrap_w: i16,
) -> usize {
    wraps_in_line_up_to(ctx, line, wrap_w, line.chars().count())
}

/// Visual-line breaks that occur in `line` before reaching
/// `target_char_offset` chars into it. Mirrors EXACTLY the renderer's
/// word-level wrap algorithm in `measure_text_native_styled` —
/// candidates are built by joining the next whitespace-separated word
/// and measured as a whole string (so Canvas2D's natural kerning is
/// preserved). Per-char measurement, by contrast, over-estimates
/// because kerning isn't applied between independent calls.
fn wraps_in_line_up_to(
    ctx: &web_sys::CanvasRenderingContext2d,
    line: &str,
    wrap_w: i16,
    target_char_offset: usize,
) -> usize {
    if line.is_empty() || wrap_w <= 0 { return 0; }
    let wrap_width = wrap_w as f64;
    // Tokenize the line: each token is either a word (run of non-
    // whitespace) or a whitespace gap. We need to know the cumulative
    // char count up to and including each token so we can stop counting
    // wraps once we've passed `target_char_offset`.
    #[derive(Clone)]
    struct Token<'a> { text: &'a str, char_count: usize, is_ws: bool }
    let mut tokens: Vec<Token> = Vec::new();
    let mut iter = line.char_indices().peekable();
    while let Some(&(start, c)) = iter.peek() {
        let is_ws = c.is_whitespace();
        let mut end = start;
        let mut count = 0;
        while let Some(&(p, ch)) = iter.peek() {
            if ch.is_whitespace() != is_ws { break; }
            end = p + ch.len_utf8();
            count += 1;
            iter.next();
        }
        tokens.push(Token { text: &line[start..end], char_count: count, is_ws });
    }
    // Now run the renderer's algorithm: `current` accumulates the
    // running line; for each non-whitespace word, build a candidate
    // `current + word` and check if it overflows wrap_width.
    let mut current = String::new();
    let mut wraps = 0usize;
    let mut consumed_chars = 0usize;
    for tok in &tokens {
        if tok.is_ws {
            // Whitespace between words. Renderer's split_whitespace
            // discards these but joins with a single ' '. We mirror
            // that by appending ' ' to current if current is non-empty.
            // Match renderer's `format!("{} {}", current, word)` join.
            consumed_chars += tok.char_count;
            continue;
        }
        // Non-whitespace word.
        let candidate = if current.is_empty() {
            tok.text.to_string()
        } else {
            format!("{} {}", current, tok.text)
        };
        let w = ctx.measure_text(&candidate).map(|m| m.width()).unwrap_or(0.0);
        if w > wrap_width && !current.is_empty() {
            // Wrap before this word. The wrap-point sits at the start
            // of THIS word in the original line.
            if consumed_chars <= target_char_offset {
                wraps += 1;
            }
            current = tok.text.to_string();
        } else {
            current = candidate;
        }
        consumed_chars += tok.char_count;
        if consumed_chars > target_char_offset { break; }
    }
    wraps
}
