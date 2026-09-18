use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use manual_future::ManualFuture;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use crate::player::{
    allocator::{DatumAllocatorTrait, ScriptInstanceAllocatorTrait},
    cast_lib::CastLib,
    script::{Script, ScriptInstance},
    symbols::{builtin::BuiltInSymbol, symbol::Symbol},
};
use crate::director::{
    chunks::{
        handler::{Bytecode, HandlerDef},
        script::ScriptChunk,
    },
    enums::ScriptType,
    lingo::{datum::Datum, opcode::OpCode},
};
use fxhash::FxHashMap;

use crate::player::{
    cast_lib::CastMemberRef,
    cast_member::{CastMember, CastMemberType, FlashMember, MovieMember, ScriptMember},
    commands::PlayerVMCommand,
    geometry::IntRect,
    score::{ScoreSpriteSpan, SpriteChannel},
};
use crate::player::testing_shared::{HarnessRuntime, TestHarness, SnapshotOutput, SpriteQuery};
use crate::BrowserFlashCapability;

/// A sender retained by a browser lifecycle fixture so it can prove that a
/// retired socket's send pump no longer reaches the host.  The owner and
/// generation are kept with it for the stale-event assertion as well.
struct MultiuserSocketProbe {
    sender: async_std::channel::Sender<Vec<u8>>,
    owner: crate::player::ownership::OwnerToken,
    instance_id: u32,
    generation: u64,
}

struct MultiuserServerState {
    opened_a: bool,
    opened_b: bool,
    closed_a: bool,
    received_stale_a: bool,
    received_survival_b: bool,
}

/// Symbols written by the synthetic playback scripts. The values live in the
/// owning player's globals, so the browser playback test observes real Lingo
/// handler effects instead of Rust-side lifecycle counters.
struct PlaybackEffectMarkers {
    init: Symbol,
    frame: Symbol,
    stop_movie: Symbol,
    end_sprite_after_stop: Symbol,
}

fn playback_counter_handler(global_index: u16, names_len: usize) -> Rc<HandlerDef> {
    Rc::new(HandlerDef {
        name_id: 0,
        bytecode_array: vec![
            Bytecode::new(OpCode::GetGlobal, global_index as i64, 0),
            Bytecode::new(OpCode::PushInt8, 1, 1),
            Bytecode::new(OpCode::Add, 0, 2),
            Bytecode::new(OpCode::SetGlobal, global_index as i64, 3),
            Bytecode::new(OpCode::Ret, 0, 4),
        ],
        bytecode_index_map: FxHashMap::default(),
        argument_name_ids: vec![],
        local_name_ids: vec![],
        global_name_ids: (1..names_len as u16).collect(),
        compiled_ir: RefCell::new(None),
    })
}

fn playback_end_sprite_handler(
    stop_index: u16,
    end_index: u16,
    names_len: usize,
) -> Rc<HandlerDef> {
    Rc::new(HandlerDef {
        name_id: 0,
        bytecode_array: vec![
            Bytecode::new(OpCode::GetGlobal, stop_index as i64, 0),
            Bytecode::new(OpCode::SetGlobal, end_index as i64, 1),
            Bytecode::new(OpCode::Ret, 0, 2),
        ],
        bytecode_index_map: FxHashMap::default(),
        argument_name_ids: vec![],
        local_name_ids: vec![],
        global_name_ids: (1..names_len as u16).collect(),
        compiled_ir: RefCell::new(None),
    })
}

fn read_playback_marker(
    handle: &crate::BrowserPlayerHandle,
    marker: &Symbol,
) -> Result<i32, String> {
    handle
        .with_context(|context| {
            let value = context
                .player
                .globals
                .get(marker)
                .and_then(|value| match context.player.get_datum(value) {
                    Datum::Int(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or(0);
            value
        })
        .map_err(|error| format!("playback marker read failed: {error:?}"))
}

fn create_public_play_test_container(label: &str) -> Result<web_sys::HtmlElement, String> {
    let document = web_sys::window()
        .ok_or_else(|| "browser window is required".to_owned())?
        .document()
        .ok_or_else(|| "browser document is required".to_owned())?;
    let element = document
        .create_element("div")
        .map_err(|error| format!("create {label} renderer container failed: {error:?}"))?;
    element
        .set_attribute("data-dirplayer-public-play", label)
        .map_err(|error| format!("tag {label} renderer container failed: {error:?}"))?;
    element
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| format!("{label} renderer container was not an HTML element"))
}

/// Register an explicitly owned browser handle through the production Flash
/// manager, retaining the handle's real owner-bound command sender. Nested
/// children receive their own owner-bound senders from `start_nested_movie_owned`.
async fn register_browser_handle_flash_owner(
    handle: &crate::BrowserPlayerHandle,
) -> Result<(BrowserFlashCapability, RegisteredFlashOwner), String> {
    let owner_key = handle.owner_identity();
    let mut owner_guard = RegisteredFlashOwner::pending(owner_key.clone());
    let (pending, command_tx) = handle
        .with_context(|context| {
            (
                context.player.flash_scripted_access_pending.clone(),
                context.player.queue_tx.clone(),
            )
        })
        .map_err(|error| format!("capture Flash readiness failed: {error:?}"))?;
    let capability = BrowserFlashCapability::new(
        handle.session().clone(),
        handle.player_id(),
        handle.owner().clone(),
        pending.clone(),
        command_tx.clone(),
    );
    let js_capability = BrowserFlashCapability::new(
        handle.session().clone(),
        handle.player_id(),
        handle.owner().clone(),
        pending,
        command_tx,
    );
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function =
        js_sys::Reflect::get(&window, &JsValue::from_str("dirplayer_registerFlashOwner"))
            .map_err(|_| "production Flash owner registrar is unavailable".to_owned())?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| "production Flash owner registrar is not callable".to_owned())?;
    // The fresh handle owner key is unique. Arm cleanup before the fallible
    // registrar call so a host that publishes a route and then throws still
    // gets reset/unregistered by the guard.
    owner_guard.mark_registered();
    let result = function
        .call2(
            &window,
            &JsValue::from_str(&owner_key),
            &JsValue::from(js_capability),
        )
        .map_err(|error| format!("registering Flash owner failed: {error:?}"))?;
    if let Ok(promise) = result.dyn_into::<js_sys::Promise>() {
        JsFuture::from(promise)
            .await
            .map_err(|error| format!("Flash owner registration failed: {error:?}"))?;
    }
    Ok((capability, owner_guard))
}

fn unregister_browser_handle_flash_owner(owner_key: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(value) = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_unregisterFlashOwner"),
        ) {
            if let Ok(function) = value.dyn_into::<js_sys::Function>() {
                let _ = function.call1(&window, &JsValue::from_str(owner_key));
            }
        }
    }
}

/// Owns one production Flash owner registration for the duration of a
/// fixture. Dropping it resets any live Ruffle instance before removing the
/// owner route, including when a fixture exits through an early `?`.
struct RegisteredFlashOwner {
    owner_key: String,
    registered: bool,
}

impl RegisteredFlashOwner {
    fn pending(owner_key: String) -> Self {
        Self {
            owner_key,
            registered: false,
        }
    }

    fn mark_registered(&mut self) {
        self.registered = true;
    }
}

impl Drop for RegisteredFlashOwner {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        crate::js_api::JsApi::dispatch_flash_reset_all(&self.owner_key);
        unregister_browser_handle_flash_owner(&self.owner_key);
    }
}

fn nested_flash_owner_keys() -> Result<Vec<String>, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testNestedFlashOwnerKeys"),
    )
    .map_err(|_| "nested Flash owner inspection helper is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "nested Flash owner inspection helper is not callable".to_owned())?;
    let values = function
        .call0(&window)
        .map_err(|error| format!("nested Flash owner inspection failed: {error:?}"))?;
    let values = values
        .dyn_into::<js_sys::Array>()
        .map_err(|_| "nested Flash owner inspection did not return an array".to_owned())?;
    let mut keys = values
        .iter()
        .filter_map(|value| value.as_string())
        .collect::<Vec<_>>();
    keys.sort();
    Ok(keys)
}

fn dispatch_flash_owner_action(owner_key: &str, method: &str) -> Result<bool, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testDispatchFlashOwnerAction"),
    )
    .map_err(|_| "Flash owner dispatch inspection helper is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "Flash owner dispatch inspection helper is not callable".to_owned())?;
    function
        .call2(
            &window,
            &JsValue::from_str(owner_key),
            &JsValue::from_str(method),
        )
        .map_err(|error| format!("stale Flash owner dispatch failed: {error:?}"))?
        .as_bool()
        .ok_or_else(|| "stale Flash owner dispatch did not return a boolean".to_owned())
}

fn queue_prepared_flash_action(owner_key: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testQueuePreparedFlashAction"),
    )
    .map_err(|_| "prepared Flash action helper is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "prepared Flash action helper is not callable".to_owned())?;
    function
        .call1(&window, &JsValue::from_str(owner_key))
        .map_err(|error| format!("queueing prepared Flash action failed: {error:?}"))?;
    Ok(())
}

fn probe_prepared_flash_owner(owner_key: &str) -> Result<bool, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testProbePreparedFlashOwner"),
    )
    .map_err(|_| "prepared Flash owner probe is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "prepared Flash owner probe is not callable".to_owned())?;
    function
        .call1(&window, &JsValue::from_str(owner_key))
        .map_err(|error| format!("probing prepared Flash owner failed: {error:?}"))?
        .as_bool()
        .ok_or_else(|| "prepared Flash owner probe did not return a boolean".to_owned())
}

fn fail_next_nested_callback_registration() -> Result<(), String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testFailNextNestedCallbackRegistration"),
    )
    .map_err(|_| "nested callback failure helper is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "nested callback failure helper is not callable".to_owned())?;
    function
        .call0(&window)
        .map_err(|error| format!("arming nested callback failure failed: {error:?}"))?;
    Ok(())
}

fn last_failed_nested_owner_key() -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testLastFailedNestedOwnerKey"),
    )
    .map_err(|_| "failed nested owner inspection helper is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "failed nested owner inspection helper is not callable".to_owned())?;
    function
        .call0(&window)
        .map_err(|error| format!("failed nested owner inspection failed: {error:?}"))?
        .as_string()
        .ok_or_else(|| "failed nested owner inspection returned no owner key".to_owned())
}

fn probe_failed_nested_callback_absent(owner_key: &str) -> Result<bool, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testProbeFailedNestedCallbackAbsent"),
    )
    .map_err(|_| "failed nested callback probe is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "failed nested callback probe is not callable".to_owned())?;
    function
        .call1(&window, &JsValue::from_str(owner_key))
        .map_err(|error| format!("failed nested callback probe failed: {error:?}"))?
        .as_bool()
        .ok_or_else(|| "failed nested callback probe did not return a boolean".to_owned())
}

fn probe_failed_nested_host_absent(owner_key: &str) -> Result<bool, String> {
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let function = js_sys::Reflect::get(
        &window,
        &JsValue::from_str("dirplayer_testProbeFailedNestedHostAbsent"),
    )
    .map_err(|_| "failed nested host probe is unavailable".to_owned())?
    .dyn_into::<js_sys::Function>()
    .map_err(|_| "failed nested host probe is not callable".to_owned())?;
    function
        .call1(&window, &JsValue::from_str(owner_key))
        .map_err(|error| format!("failed nested host probe failed: {error:?}"))?
        .as_bool()
        .ok_or_else(|| "failed nested host probe did not return a boolean".to_owned())
}

fn install_nested_movie_fixture(
    handle: &crate::BrowserPlayerHandle,
    member_number: u32,
    file_name: &str,
    bytes: &[u8],
) -> Result<CastMemberRef, String> {
    let member_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: member_number as i32,
    };
    handle
        .with_context(|context| {
            let mut movie = MovieMember::new();
            movie.file_name = file_name.to_owned();
            movie.base_url = "https://fixture.invalid/".to_owned();
            movie.bytes = Some(Rc::new(bytes.to_vec()));
            movie.width = 32;
            movie.height = 32;
            let member = CastMember::new(member_number, CastMemberType::Movie(movie));
            if let Some(cast) = context
                .player
                .movie
                .cast_manager
                .casts
                .iter_mut()
                .find(|cast| cast.number == 1)
            {
                cast.insert_member(member_ref.cast_member as u32, member, context.symbols);
            } else {
                let mut cast = CastLib::test_external(1, 0);
                cast.insert_member(member_ref.cast_member as u32, member, context.symbols);
                context.player.movie.cast_manager.casts.push(cast);
            }
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(member_ref);
            channel.sprite.width = 32;
            channel.sprite.height = 32;
            context.player.movie.rect = IntRect::from(0, 0, 32, 32);
            let score = &mut context.player.movie.score;
            score.channels = vec![SpriteChannel::new(0), channel];
            score.sprite_spans = vec![ScoreSpriteSpan {
                channel_number: 1,
                start_frame: 1,
                end_frame: 1,
                scripts: vec![],
            }];
            score.frame_count = Some(1);
            score.invalidate_span_channel_cache();
            Ok::<_, JsValue>(())
        })
        .and_then(|result| result)
        .map_err(|error| format!("install nested movie fixture failed: {error:?}"))?;
    Ok(member_ref)
}

fn install_pointer_flash_fixture(
    handle: &crate::BrowserPlayerHandle,
    bytes: &[u8],
) -> Result<CastMemberRef, String> {
    let member_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: 1,
    };
    handle
        .with_context(|context| {
            let member = CastMember::new(
                1,
                CastMemberType::Flash(FlashMember {
                    data: bytes.to_vec(),
                    reg_point: (0, 0),
                    flash_info: None,
                }),
            );
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(1, member, context.symbols);
            context.player.movie.cast_manager.casts.push(cast);
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(member_ref);
            channel.sprite.loc_h = 275;
            channel.sprite.loc_v = 200;
            channel.sprite.width = 550;
            channel.sprite.height = 400;
            context.player.movie.rect = IntRect::from(0, 0, 550, 400);
            let score = &mut context.player.movie.score;
            score.channels = vec![SpriteChannel::new(0), channel];
            score.sprite_spans = vec![ScoreSpriteSpan {
                channel_number: 1,
                start_frame: 1,
                end_frame: 1,
                scripts: vec![],
            }];
            score.frame_count = Some(1);
            score.invalidate_span_channel_cache();
            Ok::<_, JsValue>(())
        })
        .and_then(|result| result)
        .map_err(|error| format!("install pointer Flash fixture failed: {error:?}"))?;
    Ok(member_ref)
}

fn install_unpublished_flash_channel(handle: &crate::BrowserPlayerHandle) -> Result<(), String> {
    handle
        .with_context(|context| {
            let mut channel = SpriteChannel::new(1);
            channel.sprite.loc_h = 275;
            channel.sprite.loc_v = 200;
            channel.sprite.width = 550;
            channel.sprite.height = 400;
            context.player.movie.rect = IntRect::from(0, 0, 550, 400);
            let score = &mut context.player.movie.score;
            score.channels = vec![SpriteChannel::new(0), channel];
            score.sprite_spans = vec![ScoreSpriteSpan {
                channel_number: 1,
                start_frame: 1,
                end_frame: 1,
                scripts: vec![],
            }];
            score.frame_count = Some(1);
            score.invalidate_span_channel_cache();
            score.invalidate_render_channel_cache();
            Ok::<_, JsValue>(())
        })
        .and_then(|result| result)
        .map_err(|error| format!("install unpublished Flash channel failed: {error:?}"))
}

fn install_published_sprite_variable_fixture(
    handle: &crate::BrowserPlayerHandle,
    bytes: &[u8],
) -> Result<(), String> {
    let member_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: 1,
    };
    handle
        .with_context(|context| {
            let member = CastMember::new(
                1,
                CastMemberType::Flash(FlashMember {
                    data: bytes.to_vec(),
                    reg_point: (0, 0),
                    flash_info: None,
                }),
            );
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(1, member, context.symbols);
            context.player.movie.cast_manager.casts.push(cast);
            let previous = context
                .player
                .movie
                .score
                .channels
                .get(1)
                .and_then(|channel| channel.sprite.member)
                .map(|member| (member.cast_lib, member.cast_member));
            if let Some(channel) = context.player.movie.score.channels.get_mut(1) {
                channel.sprite.member = Some(member_ref);
            }
            context.player.movie.score.invalidate_span_channel_cache();
            context.player.movie.score.invalidate_render_channel_cache();
            // This is the production member-association boundary. In
            // particular, it preserves an object handle reserved while the
            // channel was absent instead of silently adopting a new generation.
            context
                .player
                .transition_flash_member(1, previous, Some((1, 1)));
            Ok::<_, JsValue>(())
        })
        .and_then(|result| result)
        .map_err(|error| format!("install published SpriteAsync Flash fixture failed: {error:?}"))
}

fn replace_sprite_variable_flash_fixture(
    handle: &crate::BrowserPlayerHandle,
    bytes: &[u8],
) -> Result<(), String> {
    let previous_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: 1,
    };
    let replacement_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: 2,
    };
    handle
        .with_context(|context| {
            let member = CastMember::new(
                2,
                CastMemberType::Flash(FlashMember {
                    data: bytes.to_vec(),
                    reg_point: (0, 0),
                    flash_info: None,
                }),
            );
            let cast = context
                .player
                .movie
                .cast_manager
                .casts
                .iter_mut()
                .find(|cast| cast.number == 1)
                .ok_or_else(|| JsValue::from_str("SpriteAsync replacement cast is absent"))?;
            cast.insert_member(2, member, context.symbols);
            let channel = context
                .player
                .movie
                .score
                .channels
                .get_mut(1)
                .ok_or_else(|| JsValue::from_str("SpriteAsync replacement channel is absent"))?;
            channel.sprite.member = Some(replacement_ref);
            context.player.movie.score.invalidate_span_channel_cache();
            context.player.movie.score.invalidate_render_channel_cache();
            context.player.transition_flash_member(
                1,
                Some((previous_ref.cast_lib, previous_ref.cast_member)),
                Some((replacement_ref.cast_lib, replacement_ref.cast_member)),
            );
            Ok::<_, JsValue>(())
        })
        .and_then(|result| result)
        .map_err(|error| format!("replace SpriteAsync Flash fixture failed: {error:?}"))
}

fn install_callback_script_fixture(
    handle: &crate::BrowserPlayerHandle,
) -> Result<(Symbol, Symbol, Symbol, Symbol), String> {
    handle
        .with_context(|context| {
            let handler_name = context.symbols.intern("onFlashCallback");
            let count_name = context.symbols.intern("dirplayerProbeInvocationCount");
            let text_name = context.symbols.intern("dirplayerProbeText");
            let number_name = context.symbols.intern("dirplayerProbeNumber");
            let object_name = context.symbols.intern("dirplayerProbeObject");
            let count_value = context.player.alloc_datum(Datum::Int(0));
            let text_value = context.player.alloc_datum(Datum::Void);
            let number_value = context.player.alloc_datum(Datum::Void);
            let object_value = context.player.alloc_datum(Datum::Void);
            context
                .player
                .globals
                .insert(count_name.clone(), count_value);
            context.player.globals.insert(text_name.clone(), text_value);
            context
                .player
                .globals
                .insert(number_name.clone(), number_value);
            context
                .player
                .globals
                .insert(object_name.clone(), object_value);
            let script_ref = CastMemberRef {
                cast_lib: 1,
                cast_member: 2,
            };
            let handler = Rc::new(HandlerDef {
                name_id: 0,
                bytecode_array: vec![
                    Bytecode::new(OpCode::GetGlobal, 1, 0),
                    Bytecode::new(OpCode::PushInt8, 1, 1),
                    Bytecode::new(OpCode::Add, 0, 2),
                    Bytecode::new(OpCode::SetGlobal, 1, 3),
                    Bytecode::new(OpCode::GetParam, 6, 4),
                    Bytecode::new(OpCode::SetGlobal, 2, 5),
                    Bytecode::new(OpCode::GetParam, 12, 6),
                    Bytecode::new(OpCode::SetGlobal, 3, 7),
                    Bytecode::new(OpCode::GetParam, 18, 8),
                    Bytecode::new(OpCode::SetGlobal, 4, 9),
                    Bytecode::new(OpCode::Ret, 0, 10),
                ],
                bytecode_index_map: FxHashMap::default(),
                argument_name_ids: vec![0, 1, 2, 3],
                local_name_ids: vec![],
                global_name_ids: vec![],
                compiled_ir: RefCell::new(None),
            });
            let script = Rc::new(Script {
                member_ref: script_ref.clone(),
                name: "callback-origin-script".to_owned(),
                chunk: ScriptChunk {
                    script_number: 2,
                    literals: vec![],
                    handlers: vec![],
                    property_name_ids: vec![],
                    property_defaults: HashMap::new(),
                },
                script_type: ScriptType::Score,
                handlers: FxHashMap::from_iter([(handler_name.clone(), handler)]),
                handler_names_raw: vec!["onFlashCallback".to_owned()],
                handler_names: vec![handler_name.clone()],
                properties: RefCell::new(FxHashMap::default()),
            });
            let member = CastMember::new(
                2,
                CastMemberType::Script(ScriptMember {
                    script_id: 2,
                    script_type: ScriptType::Score,
                    name: "callback-origin-script".to_owned(),
                }),
            );
            if let Some(cast) = context
                .player
                .movie
                .cast_manager
                .casts
                .iter_mut()
                .find(|cast| cast.number == 1)
            {
                cast.insert_member(2, member, context.symbols);
                cast.scripts.insert(2, script);
                cast.name_symbols = Rc::from(vec![
                    handler_name.clone(),
                    count_name.clone(),
                    text_name.clone(),
                    number_name.clone(),
                    object_name.clone(),
                ]);
            } else {
                let mut cast = CastLib::test_external(1, 0);
                cast.insert_member(2, member, context.symbols);
                cast.scripts.insert(2, script);
                cast.name_symbols = Rc::from(vec![
                    handler_name.clone(),
                    count_name.clone(),
                    text_name.clone(),
                    number_name.clone(),
                    object_name.clone(),
                ]);
                context.player.movie.cast_manager.casts.push(cast);
            }
            Ok::<_, JsValue>((count_name, text_name, number_name, object_name))
        })
        .and_then(|result| result)
        .map_err(|error| format!("install callback script fixture failed: {error:?}"))
}

fn prepare_owned_mouse_down(
    handle: &crate::BrowserPlayerHandle,
    x: i32,
    y: i32,
) -> Result<
    (
        crate::player::session::RuntimeSessionHandle,
        u32,
        crate::player::ownership::OwnerToken,
    ),
    crate::player::ScriptError,
> {
    let session = handle.session().clone();
    let player_id = handle.player_id();
    let owner = handle.owner().clone();
    let input_state = handle
        .with_context(|context| {
            if !context.player.is_playing {
                return Err(crate::player::ScriptError::new(
                    "owned mouse fixture is not playing".to_owned(),
                ));
            }
            if !context
                .player
                .movie
                .score
                .get_sorted_channels(context.player.movie.current_frame)
                .iter()
                .any(|channel| channel.number == 1)
            {
                return Err(crate::player::ScriptError::new(format!(
                    "owned mouse fixture channel is inactive at frame {}",
                    context.player.movie.current_frame
                )));
            }
            let hit = crate::player::score::get_sprite_at(context.player, x, y, false);
            if hit != Some(1) {
                return Err(crate::player::ScriptError::new(format!(
                    "owned mouse fixture selected sprite {hit:?} at ({x},{y})"
                )));
            }
            context.player.mouse_loc = (x, y);
            context.player.movie.mouse_down = true;
            Ok(())
        })
        .map_err(|error| {
            crate::player::ScriptError::new(format!("prepare mouse down failed: {error:?}"))
        })?;
    input_state?;
    Ok((session, player_id, owner))
}

async fn run_owned_mouse_down(
    session: crate::player::session::RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
    x: i32,
    y: i32,
) -> Result<crate::player::DatumRef, crate::player::ScriptError> {
    match run_owned_mouse_down_turn(session, player_id, owner, x, y).await {
        crate::player::commands::CommandTurn::Complete(result, _) => result,
        crate::player::commands::CommandTurn::Continue
        | crate::player::commands::CommandTurn::Waiting(_)
        | crate::player::commands::CommandTurn::Pending(_) => Err(crate::player::ScriptError::new(
            "owned mouse down did not complete synchronously".to_owned(),
        )),
    }
}

async fn run_owned_mouse_down_turn(
    session: crate::player::session::RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
    x: i32,
    y: i32,
) -> crate::player::commands::CommandTurn {
    crate::player::commands::run_player_command(
        PlayerVMCommand::MouseDown((x, y)),
        None,
        session,
        player_id,
        owner,
    )
    .await
}

async fn read_pointer_flash_int(
    handle: &crate::BrowserPlayerHandle,
    name: &str,
) -> Result<i32, String> {
    let session = handle.session().clone();
    let player_id = handle.player_id();
    let owner = handle.owner().clone();
    let value = crate::player::eval_lingo_command_owned(
        session.clone(),
        player_id,
        owner,
        format!("getVariable(sprite(1), \"_root.{name}\", 1)"),
    )
    .await
    .map_err(|error| format!("Flash pointer read failed for {name}: {error}"))?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            match context.player.get_datum(&value) {
                Datum::Int(value) => Ok(*value),
                other => Err(format!(
                    "Flash pointer {name} returned {}",
                    other.type_enum().type_str()
                )),
            }
        })
        .ok_or_else(|| "Flash pointer player disappeared during read".to_owned())?
}

async fn wait_for_pointer_flash_ready(handle: &crate::BrowserPlayerHandle) -> Result<(), String> {
    let generation = {
        let mut latched = None;
        for _ in 0..100 {
            let observed = handle
                .with_context(|context| context.player.flash_instance_generation(1))
                .map_err(|error| format!("capture pointer Flash generation failed: {error:?}"))?;
            if let Some(generation) = observed {
                latched = Some(generation);
                break;
            }
            async_std::task::sleep(std::time::Duration::from_millis(50)).await;
        }
        latched.ok_or_else(|| {
            "pointer Flash generation was not published within the bounded wait".to_owned()
        })?
    };
    let generation_current = handle
        .is_flash_instance_generation_current(1.0, generation as f64)
        .map_err(|error| format!("check pointer Flash generation failed: {error:?}"))?;
    if !generation_current {
        return Err(format!(
            "pointer Flash generation {generation} was replaced before readiness"
        ));
    }
    crate::player::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
        handle.owner(),
        1,
        generation,
    )
    .await
    .map_err(|error| {
        format!("pointer Flash instance readiness failed for generation {generation}: {error}")
    })?;
    let generation_current = handle
        .is_flash_instance_generation_current(1.0, generation as f64)
        .map_err(|error| format!("recheck pointer Flash generation failed: {error:?}"))?;
    if !generation_current {
        return Err(format!(
            "pointer Flash generation {generation} was replaced after readiness"
        ));
    }
    Ok(())
}

async fn eval_sprite_variable_datum(
    handle: &crate::BrowserPlayerHandle,
    expression: &str,
) -> Result<Datum, String> {
    let value = crate::player::eval_lingo_command_owned(
        handle.session().clone(),
        handle.player_id(),
        handle.owner().clone(),
        expression.to_owned(),
    )
    .await
    .map_err(|error| format!("SpriteAsync evaluator expression failed: {error}"))?;
    handle
        .with_context(|context| context.player.get_datum(&value).clone())
        .map_err(|error| format!("SpriteAsync evaluator datum read failed: {error:?}"))
}

async fn wait_for_nested_flash_frame(
    session: crate::player::session::RuntimeSessionHandle,
    child_id: u32,
    owner: crate::player::ownership::OwnerToken,
    expected: [u8; 3],
) -> Result<(), String> {
    let mut latched_generation = None;
    for _ in 0..100 {
        let observation = session
            .borrow_mut()
            .with_player(child_id, |context| {
                if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                    return Err(
                        "nested child owner became stale while awaiting Flash frame".to_owned()
                    );
                }
                let generation = context.player.flash_instance_generation(1);
                let current = generation
                    .map(|generation| {
                        context
                            .player
                            .is_flash_instance_generation_current(1, generation)
                    })
                    .unwrap_or(false);
                let frame_pixel = context
                    .player
                    .flash_frame_buffers
                    .get(&1)
                    .and_then(|bitmap_id| context.player.bitmap_manager.get_bitmap(*bitmap_id))
                    .and_then(|bitmap| {
                        let x = 20usize;
                        let y = 20usize;
                        let width = bitmap.width as usize;
                        let offset = (y.checked_mul(width)?.checked_add(x)?).checked_mul(4)?;
                        let rgba = bitmap.data.get(offset..offset.checked_add(4)?)?;
                        Some([rgba[0], rgba[1], rgba[2], rgba[3]])
                    });
                Ok((generation, current, frame_pixel))
            })
            .ok_or_else(|| "nested child disappeared while awaiting Flash frame".to_owned())??;
        let (generation, current, frame_pixel) = observation;
        if let Some(observed_generation) = generation {
            match latched_generation {
                Some(expected_generation) if observed_generation != expected_generation => {
                    return Err(format!(
                        "nested child Flash generation changed from {expected_generation} to {observed_generation}"
                    ));
                }
                None => latched_generation = Some(observed_generation),
                _ => {}
            }
        }
        if latched_generation.is_some() && !current {
            return Err(
                "nested child Flash generation became stale while awaiting frame".to_owned(),
            );
        }
        if let Some(pixel) = frame_pixel {
            if pixel_matches(pixel, expected) && latched_generation.is_some() && current {
                return Ok(());
            }
        }
        async_std::task::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(format!(
        "nested child Flash frame with expected color {expected:?} was not published within the bounded wait"
    ))
}

async fn prepare_pointer_player(
    swf: &[u8],
) -> Result<
    (
        crate::BrowserPlayerHandle,
        BrowserFlashCapability,
        RegisteredFlashOwner,
        web_sys::HtmlElement,
    ),
    String,
> {
    let handle = crate::BrowserPlayerHandle::new()
        .map_err(|error| format!("pointer BrowserPlayerHandle construction failed: {error:?}"))?;
    let (capability, owner_guard) = register_browser_handle_flash_owner(&handle).await?;
    install_pointer_flash_fixture(&handle, swf)?;
    let container = create_public_play_test_container("pointer")?;
    handle
        .create_canvas(container.clone())
        .map_err(|error| format!("pointer renderer creation failed: {error:?}"))?;
    handle
        .set_renderer_backend("Canvas2D".to_owned())
        .map_err(|error| format!("pointer Canvas2D backend selection failed: {error:?}"))?;
    crate::player::run_movie_init_owned(
        handle.session().clone(),
        handle.player_id(),
        handle.owner().clone(),
    )
    .await
    .map_err(|error| format!("pointer Flash startup failed: {error}"))?;
    handle
        .play()
        .map_err(|error| format!("pointer Flash playback start failed: {error:?}"))?;
    wait_for_pointer_flash_ready(&handle).await?;
    Ok((handle, capability, owner_guard, container))
}

fn read_canvas_pixel(container: &web_sys::HtmlElement, x: u32, y: u32) -> Result<[u8; 4], String> {
    let canvas = container
        .query_selector("canvas")
        .map_err(|error| format!("querying fixture canvas failed: {error:?}"))?
        .ok_or_else(|| "fixture renderer did not create a canvas".to_owned())?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "fixture renderer canvas has the wrong type".to_owned())?;
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("getting fixture 2D context failed: {error:?}"))?
        .ok_or_else(|| "fixture renderer has no 2D context".to_owned())?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|_| "fixture renderer context has the wrong type".to_owned())?;
    let data = context
        .get_image_data(x as f64, y as f64, 1.0, 1.0)
        .map_err(|error| format!("reading fixture pixel failed: {error:?}"))?
        .data();
    Ok([data[0], data[1], data[2], data[3]])
}

fn read_webgl2_pixel(container: &web_sys::HtmlElement, x: u32, y: u32) -> Result<[u8; 4], String> {
    let canvas = container
        .query_selector("canvas")
        .map_err(|error| format!("querying fixture WebGL canvas failed: {error:?}"))?
        .ok_or_else(|| "fixture renderer did not create a canvas".to_owned())?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "fixture renderer canvas has the wrong type".to_owned())?;
    if x >= canvas.width() || y >= canvas.height() {
        return Err(format!(
            "fixture WebGL pixel ({x},{y}) is outside {}x{} canvas",
            canvas.width(),
            canvas.height()
        ));
    }
    let gl = canvas
        .get_context("webgl2")
        .map_err(|error| format!("getting fixture WebGL2 context failed: {error:?}"))?
        .ok_or_else(|| "fixture renderer has no WebGL2 context".to_owned())?
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .map_err(|_| "fixture renderer context has the wrong type".to_owned())?;
    let mut data = [0u8; 4];
    let gl_y = canvas.height() - y - 1;
    gl.read_pixels_with_opt_u8_array(
        x as i32,
        gl_y as i32,
        1,
        1,
        web_sys::WebGl2RenderingContext::RGBA,
        web_sys::WebGl2RenderingContext::UNSIGNED_BYTE,
        Some(&mut data),
    )
    .map_err(|error| format!("reading fixture WebGL pixel failed: {error:?}"))?;
    Ok(data)
}

fn pixel_matches(pixel: [u8; 4], expected: [u8; 3]) -> bool {
    pixel[3] >= 240
        && pixel[0].abs_diff(expected[0]) <= 12
        && pixel[1].abs_diff(expected[1]) <= 12
        && pixel[2].abs_diff(expected[2]) <= 12
}

/// Browser test harness with an owned command and frame scheduler.
/// The wasm test entrypoint skips the production global loops, so each RAF
/// step explicitly drives the supplied session's timeout and frame turns.
pub struct BrowserTestPlayer {
    runtime: HarnessRuntime,
    renderer: crate::rendering::RendererStateHandle,
    command_tx: async_std::channel::Sender<crate::player::PlayerVMExecutionItem>,
    flash_capability: Option<BrowserFlashCapability>,
}

/// Restores the production Flash bridge globals after a transport fixture.
/// The evaluator calls these globals directly from Rust, so leaving a test
/// closure installed would contaminate the next browser test even when an
/// assertion returns early.
struct FlashOwnedRouteGuard {
    window: web_sys::Window,
    entries: Vec<(String, JsValue)>,
}

impl Drop for FlashOwnedRouteGuard {
    fn drop(&mut self) {
        for (name, value) in &self.entries {
            if value.is_undefined() {
                let _ = js_sys::Reflect::delete_property(&self.window, &JsValue::from_str(name));
            } else {
                let _ = js_sys::Reflect::set(&self.window, &JsValue::from_str(name), value);
            }
        }
    }
}

fn flash_owned_test_response(generation: f64, value: JsValue) -> JsValue {
    let response = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&response, &JsValue::from_str("ok"), &JsValue::TRUE);
    let _ = js_sys::Reflect::set(
        &response,
        &JsValue::from_str("generation"),
        &JsValue::from_f64(generation),
    );
    let _ = js_sys::Reflect::set(&response, &JsValue::from_str("value"), &value);
    response.into()
}

fn flash_owned_test_error(code: &str, message: &str) -> JsValue {
    let response = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&response, &JsValue::from_str("ok"), &JsValue::FALSE);
    let _ = js_sys::Reflect::set(
        &response,
        &JsValue::from_str("code"),
        &JsValue::from_str(code),
    );
    let _ = js_sys::Reflect::set(
        &response,
        &JsValue::from_str("message"),
        &JsValue::from_str(message),
    );
    response.into()
}

impl BrowserTestPlayer {
    pub async fn new() -> Self {
        let (command_tx, _command_rx) = async_std::channel::unbounded();
        let runtime = HarnessRuntime::new(command_tx.clone());
        let command_session = runtime.session();
        let command_player_id = runtime.player_id();
        let command_owner = runtime.owner().clone();
        crate::player::spawn_player_local(async move {
            crate::player::commands::run_command_loop(
                _command_rx,
                command_session,
                command_player_id,
                command_owner,
            )
            .await;
        });
        let renderer = crate::rendering::new_renderer_state();
        runtime
            .session()
            .borrow_mut()
            .bind_renderer_state(runtime.player_id(), &renderer);
        let mut harness = BrowserTestPlayer {
            runtime,
            renderer,
            command_tx,
            flash_capability: None,
        };
        // `preserve_external_params: false` — this is the per-TEST boundary, and
        // one movie's external params are never right for the next movie.
        harness.reset_player_with(false).await;
        harness
    }

    pub async fn test_flash_mouse_dispatch_decoder(&mut self) -> Result<(), String> {
        let valid = flash_owned_test_response(7.0, JsValue::UNDEFINED);
        crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
            valid, 7,
        )
        .map_err(|error| format!("valid mouse response rejected: {error}"))?;

        let host_error =
            crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                flash_owned_test_error("host-error", "host failed"),
                7,
            )
            .expect_err("host error unexpectedly decoded as success");
        if host_error.code != crate::player::ScriptErrorCode::Generic {
            return Err(format!("host-error mapped to {:?}", host_error.code));
        }
        if host_error.message != "host failed" {
            return Err(format!("host-error message was {:?}", host_error.message));
        }
        for code in [
            "unknown-owner",
            "disposed-owner",
            "stale-generation",
            "invalid-generation",
        ] {
            let message = format!("{code} rejected");
            let error = crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                flash_owned_test_error(code, &message),
                7,
            )
            .expect_err("owner rejection unexpectedly decoded as success");
            if error.code != crate::player::ScriptErrorCode::InvalidReference {
                return Err(format!("{code} mapped to {:?}", error.code));
            }
            if error.message != message {
                return Err(format!("{code} message was {:?}", error.message));
            }
        }

        let missing_generation = js_sys::Object::new();
        js_sys::Reflect::set(
            &missing_generation,
            &JsValue::from_str("ok"),
            &JsValue::TRUE,
        )
        .map_err(|_| "could not construct missing-generation response".to_owned())?;
        let missing_generation_error =
            crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                missing_generation.into(),
                7,
            )
            .expect_err("missing generation unexpectedly decoded");
        if missing_generation_error.message != "Flash mouse response has invalid generation" {
            return Err(format!(
                "missing-generation message was {:?}",
                missing_generation_error.message
            ));
        }

        for generation in [f64::NAN, 1.5, 9_007_199_254_740_992.0] {
            let error = crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                flash_owned_test_response(generation, JsValue::UNDEFINED),
                7,
            ).expect_err("unsafe generation unexpectedly decoded");
            if error.message != "Flash mouse response has invalid generation" {
                return Err(format!("unsafe-generation message was {:?}", error.message));
            }
        }
        let mismatch =
            crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                flash_owned_test_response(8.0, JsValue::UNDEFINED),
                7,
            )
            .expect_err("mismatched generation unexpectedly decoded");
        if mismatch.code != crate::player::ScriptErrorCode::InvalidReference {
            return Err(format!(
                "mismatched generation mapped to {:?}",
                mismatch.code
            ));
        }
        if mismatch.message != "Flash mouse response generation is stale" {
            return Err(format!(
                "mismatched-generation message was {:?}",
                mismatch.message
            ));
        }

        let malformed_ok = js_sys::Object::new();
        js_sys::Reflect::set(
            &malformed_ok,
            &JsValue::from_str("ok"),
            &JsValue::from_str("true"),
        )
        .map_err(|_| "could not construct malformed-ok response".to_owned())?;
        let malformed_ok_error =
            crate::player::handlers::datum_handlers::flash_object::decode_mouse_dispatch_response(
                malformed_ok.into(),
                7,
            )
            .expect_err("non-boolean ok unexpectedly decoded");
        if malformed_ok_error.message != "Flash mouse response has non-boolean ok" {
            return Err(format!(
                "malformed-ok message was {:?}",
                malformed_ok_error.message
            ));
        }
        Ok(())
    }

    pub async fn test_owned_mouse_pointer(&mut self) -> Result<(), String> {
        let (mut root_a, capability_a, owner_guard_a, _root_a_container) =
            prepare_pointer_player(include_bytes!("../../tests/fixtures/flash_mouse_a.swf"))
                .await?;
        let (root_b, capability_b, owner_guard_b, _root_b_container) =
            prepare_pointer_player(include_bytes!("../../tests/fixtures/flash_mouse_b.swf"))
                .await?;

        let owner_key_a = root_a.owner().key();
        let owner_key_b = root_b.owner().key();
        let harness_key = self.harness_runtime().owner().key();
        if owner_key_a == owner_key_b || owner_key_a == harness_key || owner_key_b == harness_key {
            return Err(format!(
                "pointer owner identity collision: A={owner_key_a:?}, B={owner_key_b:?}, harness={harness_key:?}"
            ));
        }

        for (handle, prefix) in [(&root_a, "a"), (&root_b, "b")] {
            for name in ["Moved", "Pressed", "PressSawMove", "Order"] {
                let full_name = format!("{prefix}{name}");
                if read_pointer_flash_int(handle, &full_name).await? != 0 {
                    return Err(format!(
                        "pointer global {full_name} was non-zero before input"
                    ));
                }
            }
        }

        let (session_a, player_a, owner_a) = prepare_owned_mouse_down(&root_a, 250, 200)
            .map_err(|error| format!("root A mouse setup failed: {error}"))?;
        run_owned_mouse_down(session_a, player_a, owner_a, 250, 200)
            .await
            .map_err(|error| format!("root A owned mouse down failed: {error}"))?;
        root_a
            .with_context(|context| context.player.movie.mouse_down = false)
            .map_err(|error| format!("root A mouse release state failed: {error:?}"))?;
        for (name, expected) in [
            ("aMoved", 1),
            ("aPressed", 1),
            ("aPressSawMove", 1),
            ("aOrder", 2),
        ] {
            if read_pointer_flash_int(&root_a, name).await? != expected {
                return Err(format!("root A {name} did not equal {expected}"));
            }
        }
        for name in ["bMoved", "bPressed", "bPressSawMove", "bOrder"] {
            if read_pointer_flash_int(&root_b, name).await? != 0 {
                return Err(format!("root B {name} changed during root A click"));
            }
        }

        let (session_b, player_b, owner_b) = prepare_owned_mouse_down(&root_b, 250, 200)
            .map_err(|error| format!("root B mouse setup failed: {error}"))?;
        run_owned_mouse_down(session_b, player_b, owner_b, 250, 200)
            .await
            .map_err(|error| format!("root B owned mouse down failed: {error}"))?;
        root_b
            .with_context(|context| context.player.movie.mouse_down = false)
            .map_err(|error| format!("root B mouse release state failed: {error:?}"))?;
        for (name, expected) in [
            ("bMoved", 1),
            ("bPressed", 1),
            ("bPressSawMove", 1),
            ("bOrder", 2),
        ] {
            if read_pointer_flash_int(&root_b, name).await? != expected {
                return Err(format!("root B {name} did not equal {expected}"));
            }
        }

        let stale_session = root_a.session().clone();
        let stale_player = root_a.player_id();
        let stale_owner = root_a.owner().clone();
        let stale_generation = root_a
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("capture root A Flash generation failed: {error:?}"))?
            .ok_or_else(|| "root A Flash generation was absent before replacement".to_owned())?;
        let replacement_generation = root_a
            .reserve_flash_instance_generation(1.0)
            .map_err(|error| format!("reserve replacement root A generation failed: {error:?}"))?
            as u64;
        if replacement_generation == stale_generation {
            return Err("root A replacement reused its Flash generation".to_owned());
        }
        let stale_generation_error =
            crate::player::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
                &stale_owner,
                1,
                stale_generation,
            )
            .await
            .expect_err("stale root A Flash generation unexpectedly remained ready");
        if stale_generation_error.message != "Flash readiness rejected: stale-generation" {
            return Err(format!(
                "stale root A Flash readiness returned {stale_generation_error}"
            ));
        }
        root_a
            .reset()
            .map_err(|error| format!("root A reset failed: {error:?}"))?;
        let reset_readiness_error =
            crate::player::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
                &stale_owner,
                1,
                replacement_generation,
            )
            .await
            .expect_err("retired root A owner unexpectedly remained Flash-ready");
        if reset_readiness_error.message != "Flash readiness wait was cancelled" {
            return Err(format!(
                "retired root A Flash readiness returned {reset_readiness_error}"
            ));
        }
        let preserved_pointer_fixture = root_a
            .with_context(|context| {
                let score = &context.player.movie.score;
                context.player.movie.rect.width() == 550
                    && context.player.movie.rect.height() == 400
                    && score.frame_count == Some(1)
                    && score.sprite_spans.iter().any(|span| {
                        span.channel_number == 1 && span.start_frame == 1 && span.end_frame == 1
                    })
                    && score.get_sprite(1).is_some_and(|sprite| {
                        sprite.member
                            == Some(CastMemberRef {
                                cast_lib: 1,
                                cast_member: 1,
                            })
                    })
            })
            .map_err(|error| format!("inspect recreated root A fixture failed: {error:?}"))?;
        if !preserved_pointer_fixture {
            return Err(
                "root A reset did not preserve its authored Flash score fixture".to_owned(),
            );
        }
        if crate::player::eval_lingo_command_owned(
            stale_session,
            stale_player,
            stale_owner.clone(),
            "1 + 1".to_owned(),
        )
        .await
        .is_ok()
        {
            return Err("stale root A owner accepted a post-reset command".to_owned());
        }
        drop(capability_a);
        drop(owner_guard_a);
        let (capability_a, owner_guard_a) = register_browser_handle_flash_owner(&root_a).await?;
        crate::player::run_movie_init_owned(
            root_a.session().clone(),
            root_a.player_id(),
            root_a.owner().clone(),
        )
        .await
        .map_err(|error| format!("recreated root A startup failed: {error}"))?;
        root_a
            .play()
            .map_err(|error| format!("recreated root A playback start failed: {error:?}"))?;
        wait_for_pointer_flash_ready(&root_a).await?;
        let (session_a, player_a, owner_a) = prepare_owned_mouse_down(&root_a, 250, 200)
            .map_err(|error| format!("recreated root A mouse setup failed: {error}"))?;
        run_owned_mouse_down(session_a, player_a, owner_a, 250, 200)
            .await
            .map_err(|error| format!("recreated root A mouse down failed: {error}"))?;
        if read_pointer_flash_int(&root_a, "aPressed").await? != 1 {
            return Err("recreated root A did not receive its owned press".to_owned());
        }
        let fresh_generation = root_a
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("capture fresh root A Flash generation failed: {error:?}"))?
            .ok_or_else(|| "fresh root A Flash generation was absent".to_owned())?;
        let fresh_owner = root_a.owner().clone();
        drop(owner_guard_a);
        let unregistered_error =
            crate::player::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
                &fresh_owner,
                1,
                fresh_generation,
            )
            .await
            .expect_err("unregistered root A Flash host unexpectedly remained ready");
        if unregistered_error.message != "Flash readiness rejected: unknown-owner" {
            return Err(format!(
                "unregistered root A Flash readiness returned {unregistered_error}"
            ));
        }
        drop(capability_a);
        drop(capability_b);
        drop(owner_guard_b);
        Ok(())
    }

    pub async fn test_owned_mouse_reset_reentry(&mut self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let armed = Rc::new(Cell::new(false));
        let reset_seen = Rc::new(Cell::new(false));
        let press_count = Rc::new(Cell::new(0u32));
        let armed_slot: Rc<RefCell<Option<crate::BrowserPlayerHandle>>> =
            Rc::new(RefCell::new(None));

        let reset_slot = armed_slot.clone();
        let reset_armed = armed.clone();
        let reset_seen_for_closure = reset_seen.clone();
        let reset_closure = Closure::wrap(Box::new(move || {
            if !reset_armed.get() {
                return;
            }
            let mut handle = reset_slot.borrow_mut().take();
            let succeeded = handle
                .as_mut()
                .map(|handle| handle.reset().is_ok())
                .unwrap_or(false);
            if let Some(handle) = handle {
                reset_slot.borrow_mut().replace(handle);
            }
            reset_seen_for_closure.set(succeeded);
        }) as Box<dyn FnMut()>);
        let reset_previous = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_testResetOwnerDuringFlashMove"),
        )
        .map_err(|_| "could not save reset hook".to_owned())?;
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("dirplayer_testResetOwnerDuringFlashMove"),
            reset_closure.as_ref(),
        )
        .map_err(|_| "could not install reset reentry hook".to_owned())?;
        let _reset_route = FlashOwnedRouteGuard {
            window: window.clone(),
            entries: vec![(
                "dirplayer_testResetOwnerDuringFlashMove".to_owned(),
                reset_previous,
            )],
        };

        let press_count_for_closure = press_count.clone();
        let press_closure = Closure::wrap(Box::new(move || {
            press_count_for_closure.set(press_count_for_closure.get() + 1);
        }) as Box<dyn FnMut()>);
        let press_previous = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_testRecordFlashPress"),
        )
        .map_err(|_| "could not save press hook".to_owned())?;
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("dirplayer_testRecordFlashPress"),
            press_closure.as_ref(),
        )
        .map_err(|_| "could not install press counter hook".to_owned())?;
        let _press_route = FlashOwnedRouteGuard {
            window: window.clone(),
            entries: vec![("dirplayer_testRecordFlashPress".to_owned(), press_previous)],
        };

        let (positive, positive_capability, positive_owner_guard, _positive_container) =
            prepare_pointer_player(include_bytes!(
                "../../tests/fixtures/flash_mouse_reentry.swf"
            ))
            .await?;
        let (session, player_id, owner) = prepare_owned_mouse_down(&positive, 250, 200)
            .map_err(|error| format!("positive-control mouse setup failed: {error}"))?;
        run_owned_mouse_down(session, player_id, owner, 250, 200)
            .await
            .map_err(|error| format!("positive-control mouse down failed: {error}"))?;
        if press_count.get() != 1 {
            return Err(format!(
                "positive-control press counter was {}",
                press_count.get()
            ));
        }
        drop(positive_owner_guard);
        drop(positive_capability);
        drop(positive);

        press_count.set(0);
        let (root_b, capability_b, owner_guard_b, _root_b_container) =
            prepare_pointer_player(include_bytes!("../../tests/fixtures/flash_mouse_b.swf"))
                .await?;
        for name in ["bMoved", "bPressed", "bPressSawMove", "bOrder"] {
            if read_pointer_flash_int(&root_b, name).await? != 0 {
                return Err(format!(
                    "B global {name} was non-zero before armed A dispatch"
                ));
            }
        }
        armed.set(true);
        let (armed_handle, armed_capability, armed_owner_guard, _armed_container) =
            prepare_pointer_player(include_bytes!(
                "../../tests/fixtures/flash_mouse_reentry.swf"
            ))
            .await?;
        let (session, player_id, owner) = prepare_owned_mouse_down(&armed_handle, 250, 200)
            .map_err(|error| format!("armed mouse setup failed: {error}"))?;
        armed_slot.borrow_mut().replace(armed_handle);
        let result =
            run_owned_mouse_down_turn(session.clone(), player_id, owner.clone(), 250, 200).await;
        let stale_error = match result {
            crate::player::commands::CommandTurn::Complete(Err(error), _) => error,
            crate::player::commands::CommandTurn::Complete(Ok(_), _) => {
                return Err(
                    "armed mouse down unexpectedly completed after synchronous reset".to_owned(),
                );
            }
            crate::player::commands::CommandTurn::Continue
            | crate::player::commands::CommandTurn::Waiting(_)
            | crate::player::commands::CommandTurn::Pending(_) => {
                return Err(
                    "armed mouse down did not return a complete stale-owner result".to_owned(),
                );
            }
        };
        if stale_error.code != crate::player::ScriptErrorCode::InvalidReference {
            return Err(format!(
                "armed mouse down returned {:?}, expected InvalidReference",
                stale_error.code
            ));
        }
        if !reset_seen.get() {
            return Err("move reentry did not synchronously reset the owner".to_owned());
        }
        if press_count.get() != 0 {
            return Err(format!(
                "armed press counter incremented to {}",
                press_count.get()
            ));
        }
        for name in ["bMoved", "bPressed", "bPressSawMove", "bOrder"] {
            if read_pointer_flash_int(&root_b, name).await? != 0 {
                return Err(format!("B global {name} changed during armed A reset"));
            }
        }
        if crate::player::eval_lingo_command_owned(session, player_id, owner, "1 + 1".to_owned())
            .await
            .is_ok()
        {
            return Err("stale reentry owner accepted a post-reset command".to_owned());
        }
        if let Some(armed_handle) = armed_slot.borrow_mut().take() {
            let current_key = armed_handle.owner_identity();
            crate::js_api::JsApi::dispatch_flash_reset_all(&current_key);
            drop(armed_capability);
            drop(armed_handle);
        }
        let (session_b, player_b, owner_b) = prepare_owned_mouse_down(&root_b, 250, 200)
            .map_err(|error| format!("B control setup failed: {error}"))?;
        run_owned_mouse_down(session_b, player_b, owner_b, 250, 200)
            .await
            .map_err(|error| format!("B control mouse down failed: {error}"))?;
        if read_pointer_flash_int(&root_b, "bPressed").await? != 1 {
            return Err("B control did not receive onPress after A reset reentry".to_owned());
        }
        drop(capability_b);
        drop(armed_owner_guard);
        drop(owner_guard_b);
        Ok(())
    }

    /// Exercise the owner-bound JS bridge and its evaluator continuation with
    /// a synthetic object, so browser coverage does not depend on a licensed
    /// movie asset. The nested value and global object reads use the same
    /// owner scheduler as production pending requests.
    pub async fn test_js_object_owner_bridge(&self) -> Result<(), String> {
        let session = self.runtime.session();
        let player_id = self.runtime.player_id();
        let owner = self.runtime.owner().clone();
        let object = Rc::new(RefCell::new(crate::player::js_lingo::value::JsObject::new()));
        object
            .borrow_mut()
            .set_own("value", crate::player::js_lingo::value::JsValue::Int(7));
        let interpreted_atom = Rc::new(crate::player::js_lingo::xdr::JsFunctionAtom {
            name: Some("interpretedThis".to_owned()),
            nargs: 0,
            extra: 0,
            nvars: 0,
            flags: 0,
            bindings: Vec::new(),
            script: crate::player::js_lingo::xdr::JsScriptIR {
                magic: 0,
                bytecode: vec![
                    crate::player::js_lingo::opcodes::JsOp::This as u8,
                    crate::player::js_lingo::opcodes::JsOp::Getprop as u8,
                    0,
                    0,
                    crate::player::js_lingo::opcodes::JsOp::Return as u8,
                ],
                prolog_length: 0,
                version: 150,
                atoms: vec![crate::player::js_lingo::xdr::JsAtom::String(
                    "value".to_owned(),
                )],
                source_notes: Vec::new(),
                filename: None,
                lineno: 1,
                max_stack_depth: 2,
                try_notes: Vec::new(),
            },
        });
        object.borrow_mut().set_own(
            "interpretedThis",
            crate::player::js_lingo::value::JsValue::Function(Rc::new(
                crate::player::js_lingo::value::JsFunction {
                    atom: interpreted_atom,
                    captured_scope: None,
                },
            )),
        );
        let factory_atom = Rc::new(crate::player::js_lingo::xdr::JsFunctionAtom {
            name: Some("factory".to_owned()),
            nargs: 0,
            extra: 0,
            nvars: 0,
            flags: 0,
            bindings: Vec::new(),
            script: crate::player::js_lingo::xdr::JsScriptIR {
                magic: 0,
                bytecode: vec![
                    crate::player::js_lingo::opcodes::JsOp::This as u8,
                    crate::player::js_lingo::opcodes::JsOp::Return as u8,
                ],
                prolog_length: 0,
                version: 150,
                atoms: Vec::new(),
                source_notes: Vec::new(),
                filename: None,
                lineno: 1,
                max_stack_depth: 1,
                try_notes: Vec::new(),
            },
        });
        object.borrow_mut().set_own(
            "factory",
            crate::player::js_lingo::value::JsValue::Function(Rc::new(
                crate::player::js_lingo::value::JsFunction {
                    atom: factory_atom,
                    captured_scope: None,
                },
            )),
        );
        object.borrow_mut().set_own(
            "echo",
            crate::player::js_lingo::value::JsValue::Native(Rc::new(
                crate::player::js_lingo::value::NativeFn {
                    name: "echo",
                    call: Box::new(|args| {
                        Ok(args
                            .first()
                            .cloned()
                            .unwrap_or(crate::player::js_lingo::value::JsValue::Undefined))
                    }),
                },
            )),
        );
        let handle = session
            .borrow_mut()
            .with_player_js(player_id, |context, registry| {
                let runtime = Rc::new(RefCell::new(
                    crate::player::js_lingo::interpreter::JsRuntime::new(),
                ));
                registry
                    .register_object(player_id, &context.player.owner, &runtime, &object)
                    .map_err(|error| error.message)
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())??;
        let mut receiver = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::JsObjectRef(
                        handle.clone(),
                    ))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let (_, _, read_receiver) = session
            .borrow_mut()
            .start_eval_request(
                player_id,
                crate::player::driver::InternalVmRequest::ObjectProperty {
                    receiver: receiver.clone(),
                    name: crate::player::symbols::symbol::Symbol::builtin(
                        crate::player::symbols::builtin::BuiltInSymbol::Value,
                    ),
                },
            )
            .map_err(|error| error.message)?;
        crate::player::commands::drive_pending_owner(&session, player_id, &owner).await;
        let read = read_receiver
            .recv()
            .await
            .map_err(|_| "synthetic JS getter completion was dropped".to_owned())??;
        let read_value = session
            .borrow_mut()
            .with_player(player_id, |context| context.player.get_datum(&read).clone())
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(read_value, crate::director::lingo::datum::Datum::Int(7)) {
            return Err("synthetic JS getter returned the wrong value".to_owned());
        }
        let replacement = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::Int(9))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let (_, _, set_receiver) = session
            .borrow_mut()
            .start_eval_request(
                player_id,
                crate::player::driver::InternalVmRequest::SetProperty {
                    receiver: receiver.clone(),
                    name: crate::player::symbols::symbol::Symbol::builtin(
                        crate::player::symbols::builtin::BuiltInSymbol::Value,
                    ),
                    value: replacement.clone(),
                },
            )
            .map_err(|error| error.message)?;
        crate::player::commands::drive_pending_owner(&session, player_id, &owner).await;
        set_receiver
            .recv()
            .await
            .map_err(|_| "synthetic JS setter completion was dropped".to_owned())??;
        let echo = session.borrow_mut().symbols_mut().intern("echo");
        let (_, _, call_receiver) = session
            .borrow_mut()
            .start_eval_request(
                player_id,
                crate::player::driver::InternalVmRequest::Object {
                    receiver: receiver.clone(),
                    name: echo,
                    args: vec![replacement.clone()],
                },
            )
            .map_err(|error| error.message)?;
        crate::player::commands::drive_pending_owner(&session, player_id, &owner).await;
        let echoed = call_receiver
            .recv()
            .await
            .map_err(|_| "synthetic JS call completion was dropped".to_owned())??;
        let echoed_value = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context.player.get_datum(&echoed).clone()
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(echoed_value, crate::director::lingo::datum::Datum::Int(9)) {
            return Err("synthetic JS call returned the wrong value".to_owned());
        }
        let interpreted_name = session.borrow_mut().symbols_mut().intern("interpretedThis");
        let (_, _, interpreted_receiver) = session
            .borrow_mut()
            .start_eval_request(
                player_id,
                crate::player::driver::InternalVmRequest::Object {
                    receiver: receiver.clone(),
                    name: interpreted_name,
                    args: Vec::new(),
                },
            )
            .map_err(|error| error.message)?;
        crate::player::commands::drive_pending_owner(&session, player_id, &owner).await;
        let interpreted = interpreted_receiver
            .recv()
            .await
            .map_err(|_| "synthetic interpreted call completion was dropped".to_owned())??;
        let interpreted_value = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context.player.get_datum(&interpreted).clone()
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(
            interpreted_value,
            crate::director::lingo::datum::Datum::Int(9)
        ) {
            return Err("interpreted this receiver returned the wrong value".to_owned());
        }
        let factory_name = session.borrow_mut().symbols_mut().intern("factory");
        let (_, _, factory_receiver) = session
            .borrow_mut()
            .start_eval_request(
                player_id,
                crate::player::driver::InternalVmRequest::Object {
                    receiver: receiver.clone(),
                    name: factory_name,
                    args: Vec::new(),
                },
            )
            .map_err(|error| error.message)?;
        crate::player::commands::drive_pending_owner(&session, player_id, &owner).await;
        let factory_result = factory_receiver
            .recv()
            .await
            .map_err(|_| "synthetic factory completion was dropped".to_owned())??;
        let factory_is_object = session
            .borrow_mut()
            .with_player(player_id, |context| {
                matches!(
                    context.player.get_datum(&factory_result),
                    crate::director::lingo::datum::Datum::JsObjectRef(_)
                )
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !factory_is_object {
            return Err("interpreted factory did not return a retained JS object".to_owned());
        }
        receiver = factory_result;
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                let global = context.symbols.intern("g");
                context.player.globals.insert(global, receiver);
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let source = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::String(
                        "1 + \"2\".value".to_owned(),
                    ))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let nested = crate::player::eval::invoke_value_request_owned(
            session.clone(),
            player_id,
            owner.clone(),
            source,
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        )
        .await
        .map_err(|error| error.message)?;
        let nested_value = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context.player.get_datum(&nested).clone()
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(nested_value, crate::director::lingo::datum::Datum::Int(3)) {
            return Err("synthetic nested value expression returned the wrong value".to_owned());
        }
        let list_source = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::String(
                        "[\"2\".value, 3]".to_owned(),
                    ))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let list = crate::player::eval::invoke_value_request_owned(
            session.clone(),
            player_id,
            owner.clone(),
            list_source,
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        )
        .await
        .map_err(|error| error.message)?;
        let list_value = session
            .borrow_mut()
            .with_player(player_id, |context| context.player.get_datum(&list).clone())
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let crate::director::lingo::datum::Datum::List(_, values, _) = list_value else {
            return Err(
                "synthetic nested list value expression returned the wrong type".to_owned(),
            );
        };
        if values.len() != 2
            || !self
                .runtime
                .with_context(|context| {
                    matches!(
                        context.player.get_datum(&values[0]),
                        crate::director::lingo::datum::Datum::Int(2)
                    ) && matches!(
                        context.player.get_datum(&values[1]),
                        crate::director::lingo::datum::Datum::Int(3)
                    )
                })
                .unwrap_or(false)
        {
            return Err(
                "synthetic nested list value expression returned the wrong values".to_owned(),
            );
        }
        let global_source = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::String(
                        "g.value".to_owned(),
                    ))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let global_value = crate::player::eval::invoke_value_request_owned(
            session,
            player_id,
            owner,
            global_source,
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        )
        .await
        .map_err(|error| error.message)?;
        let global_datum = self
            .runtime
            .with_context(|context| context.player.get_datum(&global_value).clone())
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(global_datum, crate::director::lingo::datum::Datum::Int(9)) {
            return Err("synthetic JS global value returned the wrong value".to_owned());
        }
        let fallback_source = self
            .runtime
            .with_context(|context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::String(
                        "g.value".to_owned(),
                    ))
            })
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        let fallback = crate::player::eval::invoke_value_request_owned(
            self.runtime.session(),
            player_id,
            self.runtime.owner().clone(),
            fallback_source,
            crate::player::driver::ValueEvaluationMode::StringPropertyFallback,
        )
        .await
        .map_err(|error| error.message)?;
        let fallback_datum = self
            .runtime
            .with_context(|context| context.player.get_datum(&fallback).clone())
            .ok_or_else(|| "browser fixture player disappeared".to_owned())?;
        if !matches!(fallback_datum, crate::director::lingo::datum::Datum::Int(9)) {
            return Err("synthetic JS global fallback value returned the wrong value".to_owned());
        }
        Ok(())
    }

    /// Exercise the public BrowserPlayerHandle input boundary with a
    /// synthetic linked movie.  This stays in the browser test harness so no
    /// production API or DCR asset is needed to validate owner-bound routing.
    pub async fn test_browser_handle_nested_input(&self) -> Result<(), String> {
        let mut handle = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("browser handle construction failed: {error:?}"))?;
        let parent_owner = handle.owner.clone();
        let member = CastMemberRef {
            cast_lib: 1,
            cast_member: 10,
        };
        let (child_tx, child_rx) = async_std::channel::unbounded();
        let (child_id, child_owner) = handle
            .session
            .borrow_mut()
            .register_nested_player(
                handle.player_id,
                &parent_owner,
                member,
                child_tx,
                async_std::channel::unbounded().0,
            )
            .map_err(|error| error.message.clone())?;
        handle
            .session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.movie.rect = IntRect::from_size(0, 0, 50, 40);
            });
        handle
            .with_context(|context| {
                context.player.stage_size = (640, 480);
                context.player.movie.rect = IntRect::from_size(0, 0, 640, 480);
                let mut lower = SpriteChannel::new(1);
                lower.sprite.member = Some(member);
                lower.sprite.loc_h = 0;
                lower.sprite.loc_v = 0;
                lower.sprite.width = 250;
                lower.sprite.height = 100;
                lower.sprite.loc_z = 1;
                lower.sprite.puppet = true;
                let mut top = SpriteChannel::new(2);
                top.sprite.member = Some(member);
                top.sprite.loc_h = 150;
                top.sprite.loc_v = 0;
                top.sprite.width = 100;
                top.sprite.height = 100;
                top.sprite.loc_z = 2;
                top.sprite.puppet = true;
                context.player.movie.score.channels = vec![SpriteChannel::new(0), lower, top];
                context.player.movie.score.invalidate_render_channel_cache();
            })
            .map_err(|error| format!("parent fixture setup failed: {error:?}"))?;

        handle
            .mouse_down(175.0, 50.0)
            .map_err(|error| format!("public mouse_down failed: {error:?}"))?;
        match child_rx
            .try_recv()
            .map_err(|error| format!("missing child mouse command: {error:?}"))?
            .command
        {
            PlayerVMCommand::MouseDown(local) if local == (12, 20) => {}
            _ => return Err("child received wrong mapped mouse_down".into()),
        }
        handle
            .with_context(|context| context.player.movie.mouse_down)
            .map_err(|error| format!("parent mouse state read failed: {error:?}"))?
            .then_some(())
            .ok_or_else(|| "parent mouse state was not set".to_owned())?;

        handle
            .mouse_up(175.0, 50.0)
            .map_err(|error| format!("public mouse_up failed: {error:?}"))?;
        match child_rx
            .try_recv()
            .map_err(|error| format!("missing child mouse_up command: {error:?}"))?
            .command
        {
            PlayerVMCommand::MouseUp(local) if local == (12, 20) => {}
            _ => return Err("child received wrong mapped mouse_up".into()),
        }
        if handle
            .session
            .borrow_mut()
            .with_player(child_id, |context| context.player.movie.mouse_down)
            .unwrap_or(true)
        {
            return Err("child mouse state was not released".into());
        }
        handle
            .key_down("a".to_owned(), 65)
            .map_err(|error| format!("public key_down failed: {error:?}"))?;
        match child_rx
            .try_recv()
            .map_err(|error| format!("missing child key command: {error:?}"))?
            .command
        {
            PlayerVMCommand::KeyDown(key, 65) if key == "a" => {}
            _ => return Err("child received wrong key_down command".into()),
        }
        if !handle
            .session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.keyboard_manager.is_key_down("a")
            })
            .unwrap_or(false)
        {
            return Err("child keyboard state was not set".into());
        }
        handle
            .with_context(|context| context.player.keyboard_manager.is_key_down("a"))
            .map_err(|error| format!("parent keyboard state read failed: {error:?}"))?
            .then_some(())
            .ok_or_else(|| "parent keyboard state was not set".to_owned())?;
        handle
            .key_up("a".to_owned(), 65)
            .map_err(|error| format!("public key_up failed: {error:?}"))?;
        match child_rx
            .try_recv()
            .map_err(|error| format!("missing child key_up command: {error:?}"))?
            .command
        {
            PlayerVMCommand::KeyUp(key, 65) if key == "a" => {}
            _ => return Err("child received wrong key_up command".into()),
        }
        if handle
            .session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.keyboard_manager.is_key_down("a")
            })
            .unwrap_or(true)
        {
            return Err("child keyboard state was not released".into());
        }

        handle
            .mouse_move(175.0, 50.0)
            .map_err(|error| format!("public mouse_move failed: {error:?}"))?;
        match child_rx
            .try_recv()
            .map_err(|error| format!("missing child mouse_move command: {error:?}"))?
            .command
        {
            PlayerVMCommand::MouseMove(local) if local == (12, 20) => {}
            _ => return Err("child received wrong mapped mouse_move".into()),
        }
        if handle
            .session
            .borrow_mut()
            .with_player(child_id, |context| context.player.mouse_loc)
            != Some((12, 20))
        {
            return Err("child mouse location was not updated".into());
        }

        handle
            .with_context(|context| context.player.wants_pointer_lock = true)
            .map_err(|error| format!("pointer-lock setup failed: {error:?}"))?;
        handle
            .mouse_move(200.0, 60.0)
            .map_err(|error| format!("pointer-lock mouse_move failed: {error:?}"))?;
        if child_rx.try_recv().is_ok() {
            return Err("pointer-lock mouse_move was routed to the child".into());
        }
        handle
            .with_context(|context| context.player.wants_pointer_lock = false)
            .map_err(|error| format!("pointer-lock teardown failed: {error:?}"))?;

        handle
            .with_context(|context| {
                for channel in context.player.movie.score.channels.iter_mut() {
                    if channel.number != 0 {
                        channel.sprite.visible = false;
                    }
                }
                context.player.movie.score.invalidate_render_channel_cache();
            })
            .map_err(|error| format!("hidden-channel setup failed: {error:?}"))?;
        handle
            .mouse_down(175.0, 50.0)
            .map_err(|error| format!("hidden-channel mouse_down failed: {error:?}"))?;
        if child_rx.try_recv().is_ok() {
            return Err("hidden linked channels received mouse_down".into());
        }
        handle
            .with_context(|context| context.player.movie.mouse_down)
            .map_err(|error| format!("hidden parent mouse state read failed: {error:?}"))?
            .then_some(())
            .ok_or_else(|| "hidden parent mouse state was not set".to_owned())?;
        handle
            .key_down("b".to_owned(), 66)
            .map_err(|error| format!("hidden-channel key_down failed: {error:?}"))?;
        if child_rx.try_recv().is_ok() {
            return Err("hidden linked channels received key_down".into());
        }
        handle
            .with_context(|context| context.player.keyboard_manager.is_key_down("b"))
            .map_err(|error| format!("hidden parent keyboard state read failed: {error:?}"))?
            .then_some(())
            .ok_or_else(|| "hidden parent keyboard state was not set".to_owned())?;
        handle
            .with_context(|context| {
                for channel in context.player.movie.score.channels.iter_mut() {
                    if channel.number != 0 {
                        channel.sprite.visible = true;
                    }
                }
                context.player.movie.score.invalidate_render_channel_cache();
            })
            .map_err(|error| format!("linked-channel restore failed: {error:?}"))?;

        // Reset must retire the nested child before the parent owner rotates;
        // a replacement parent cannot inherit the stale child capability.
        handle
            .reset()
            .map_err(|error| format!("browser handle reset failed: {error:?}"))?;
        if child_owner.is_arena_live() {
            return Err("nested child owner survived parent reset".into());
        }
        if handle
            .session
            .borrow_mut()
            .with_player(child_id, |_| ())
            .is_some()
        {
            return Err("nested child survived parent reset".into());
        }
        if !child_rx.is_closed() {
            return Err("nested child receiver remained open after reset".into());
        }
        handle
            .mouse_down(175.0, 50.0)
            .map_err(|error| format!("replacement parent mouse_down failed: {error:?}"))?;
        if child_rx.try_recv().is_ok() {
            return Err("replacement parent routed input to stale child".into());
        }
        Ok(())
    }

    /// Exercise the real owner-generation Flash readiness capabilities.  The
    /// first assertions deliberately hold the session borrow while publishing
    /// readiness; a capability setter must not re-enter that borrow.
    pub async fn test_flash_scripted_access_owner_capabilities(&mut self) -> Result<(), String> {
        let mut first = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("first browser handle construction failed: {error:?}"))?;
        let second = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("second browser handle construction failed: {error:?}"))?;

        let first_session = first.session.clone();
        let mut first_borrow = first_session.borrow_mut();
        first
            .set_flash_scripted_access_pending(true)
            .map_err(|error| format!("first readiness setter re-entered session: {error:?}"))?;
        let first_pending = first_borrow
            .with_player(first.player_id, |context| {
                context.player.flash_scripted_access_pending.get()
            })
            .ok_or_else(|| "first browser player disappeared".to_owned())?;
        if !first_pending {
            return Err("first readiness setter did not update its owner cell".to_owned());
        }
        drop(first_borrow);

        second
            .set_flash_scripted_access_pending(true)
            .map_err(|error| format!("second readiness setter failed: {error:?}"))?;
        let second_pending = second
            .session
            .borrow_mut()
            .with_player(second.player_id, |context| {
                context.player.flash_scripted_access_pending.get()
            })
            .ok_or_else(|| "second browser player disappeared".to_owned())?;
        if !second_pending {
            return Err("second readiness setter did not update its owner cell".to_owned());
        }
        let first_cell_before_reset = first.flash_scripted_access_pending.clone();
        let second_cell = second.flash_scripted_access_pending.clone();
        if Rc::ptr_eq(&first_cell_before_reset, &second_cell) {
            return Err("distinct browser owners share a Flash readiness cell".to_owned());
        }
        first
            .reset()
            .map_err(|error| format!("first browser handle reset failed: {error:?}"))?;
        if first_cell_before_reset.get() {
            return Err("reset did not clear the retired readiness cell".to_owned());
        }
        let first_replacement = first
            .session
            .borrow_mut()
            .with_player(first.player_id, |context| {
                context.player.flash_scripted_access_pending.get()
            })
            .ok_or_else(|| "replacement browser player disappeared".to_owned())?;
        if first_replacement {
            return Err("replacement owner inherited retired readiness state".to_owned());
        }
        if !second_cell.get() {
            return Err("resetting owner A cleared owner B readiness".to_owned());
        }
        first
            .set_flash_scripted_access_pending(true)
            .map_err(|error| format!("replacement readiness setter failed: {error:?}"))?;
        if !first.flash_scripted_access_pending.get() {
            return Err("replacement owner setter did not update its fresh cell".to_owned());
        }
        if !second_cell.get() {
            return Err("changing owner A changed owner B readiness".to_owned());
        }

        let stale_capability = self
            .flash_capability
            .take()
            .ok_or_else(|| "browser Flash capability was not registered".to_owned())?;
        self.reset_player_with(false).await;
        if stale_capability
            .set_flash_scripted_access_pending(true)
            .is_ok()
        {
            return Err("retired BrowserFlashCapability accepted a readiness update".to_owned());
        }
        let fresh_pending = self
            .harness_runtime()
            .with_context(|context| context.player.flash_scripted_access_pending.get())
            .ok_or_else(|| "fresh harness player disappeared".to_owned())?;
        if fresh_pending {
            return Err("fresh harness owner inherited stale capability state".to_owned());
        }

        let fresh_capability = self
            .flash_capability
            .as_ref()
            .ok_or_else(|| "fresh browser Flash capability was not registered".to_owned())?;
        let fresh_session = self.harness_runtime().session();
        let mut fresh_borrow = fresh_session.borrow_mut();
        fresh_capability
            .set_flash_scripted_access_pending(true)
            .map_err(|error| format!("fresh capability setter re-entered session: {error:?}"))?;
        let fresh_pending = fresh_borrow
            .with_player(self.harness_runtime().player_id(), |context| {
                context.player.flash_scripted_access_pending.get()
            })
            .ok_or_else(|| "fresh owner disappeared during capability test".to_owned())?;
        if !fresh_pending {
            return Err("fresh capability did not update its owner cell".to_owned());
        }
        drop(fresh_borrow);

        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let helper = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_testFlashOwnerCapability"),
        )
        .map_err(|_| "production FlashOwnerHost helper is unavailable".to_owned())?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "production FlashOwnerHost helper is not callable".to_owned())?;
        let owner_key = fresh_capability.owner_identity();
        let observed_states = Rc::new(RefCell::new(Vec::<bool>::new()));
        let observed_states_for_callback = observed_states.clone();
        let observed_session = self.harness_runtime().session();
        let observed_player_id = self.harness_runtime().player_id();
        let observe = Closure::wrap(Box::new(move |_phase: JsValue| {
            let pending = observed_session
                .borrow_mut()
                .with_player(observed_player_id, |context| {
                    context.player.flash_scripted_access_pending.get()
                })
                .unwrap_or(false);
            observed_states_for_callback.borrow_mut().push(pending);
        }) as Box<dyn FnMut(JsValue)>);
        let js_capability = BrowserFlashCapability::new(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
            self.harness_runtime()
                .with_context(|context| context.player.flash_scripted_access_pending.clone())
                .ok_or_else(|| "could not capture fresh Flash readiness cell".to_owned())?,
            self.command_tx.clone(),
        );
        let js_capability: JsValue = js_capability.into();
        let promise = helper
            .call3(
                &window,
                &JsValue::from_str(&owner_key),
                &js_capability,
                observe.as_ref(),
            )
            .map_err(|error| format!("production FlashOwnerHost invocation failed: {error:?}"))?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| "production FlashOwnerHost helper did not return a Promise".to_owned())?;
        JsFuture::from(promise)
            .await
            .map_err(|error| format!("production FlashOwnerHost lifecycle failed: {error:?}"))?;
        drop(observe);
        let observed_states = observed_states.borrow().clone();
        if observed_states != [true, false, true, false] {
            return Err(format!(
                "production FlashOwnerHost readiness transitions were {observed_states:?}"
            ));
        }
        let final_pending = self
            .harness_runtime()
            .with_context(|context| context.player.flash_scripted_access_pending.get())
            .ok_or_else(|| "fresh owner disappeared after FlashOwnerHost lifecycle".to_owned())?;
        if final_pending {
            return Err("production FlashOwnerHost did not clear scripted readiness".to_owned());
        }
        Ok(())
    }

    /// Start two linked movies through the production nested-owner path. Each
    /// root uses a distinct BrowserPlayerHandle, while both authored movies
    /// use local Flash channel 1. The first Flash read is issued immediately
    /// after startup without manually reserving a generation.
    pub async fn test_nested_flash_owned_fixture(&mut self) -> Result<(), String> {
        let mut root_a = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("root A construction failed: {error:?}"))?;
        let root_b = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("root B construction failed: {error:?}"))?;
        crate::player::testing_shared::log_test_action("nested fixture: root A registration begin");
        let (_root_a_capability, _root_a_owner_guard) =
            register_browser_handle_flash_owner(&root_a).await?;
        crate::player::testing_shared::log_test_action(
            "nested fixture: root A registration complete",
        );
        crate::player::testing_shared::log_test_action("nested fixture: root B registration begin");
        let (_root_b_capability, _root_b_owner_guard) =
            register_browser_handle_flash_owner(&root_b).await?;
        crate::player::testing_shared::log_test_action(
            "nested fixture: root B registration complete",
        );
        let root_a_container = create_public_play_test_container("nested-root-a")?;
        let root_b_container = create_public_play_test_container("nested-root-b")?;

        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let nested_registrar = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_registerNestedFlashOwner"),
        )
        .map_err(|_| "production nested Flash registrar is unavailable".to_owned())?;
        if !nested_registrar.is_function() {
            return Err("production nested Flash registrar is not callable".to_owned());
        }

        let member_a = install_nested_movie_fixture(
            &root_a,
            1,
            "nested_flash_a.dcr",
            include_bytes!("../../tests/fixtures/nested_flash_a.dcr"),
        )?;
        let member_b = install_nested_movie_fixture(
            &root_b,
            1,
            "nested_flash_b.dcr",
            include_bytes!("../../tests/fixtures/nested_flash_b.dcr"),
        )?;
        root_a
            .create_canvas(root_a_container.clone())
            .map_err(|error| format!("root A renderer creation failed: {error:?}"))?;
        root_b
            .create_canvas(root_b_container.clone())
            .map_err(|error| format!("root B renderer creation failed: {error:?}"))?;

        let start_child = |root: &crate::BrowserPlayerHandle, member_ref: CastMemberRef| {
            root.with_context(|context| {
                crate::player::nested::NestedMovieStart::from_context(context, member_ref)
            })
            .map_err(|error| format!("nested start capture failed: {error:?}"))?
            .map_err(|error| format!("nested start capture rejected: {}", error.message))
        };
        let start_a = start_child(&root_a, member_a)?;
        let start_b = start_child(&root_b, member_b)?;
        crate::player::testing_shared::log_test_action("nested fixture: child A start begin");
        let (child_a, child_owner_a) =
            crate::player::nested::start_nested_movie_owned(root_a.session().clone(), start_a)
                .await
                .map_err(|error| format!("root A nested startup failed: {error}"))?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: child A start complete id={child_a}"
        ));
        crate::player::testing_shared::log_test_action("nested fixture: child B start begin");
        let (child_b, child_owner_b) =
            crate::player::nested::start_nested_movie_owned(root_b.session().clone(), start_b)
                .await
                .map_err(|error| format!("root B nested startup failed: {error}"))?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: child B start complete id={child_b}"
        ));

        let root_key_a = root_a.owner().key();
        let root_key_b = root_b.owner().key();
        let child_key_a = child_owner_a.key();
        let child_key_b = child_owner_b.key();
        if child_a != child_b
            || child_key_a == child_key_b
            || root_key_a.session == root_key_b.session
            || child_key_a.session == child_key_b.session
            || child_key_a.session != root_key_a.session
            || child_key_b.session != root_key_b.session
            || child_key_a.player != child_a as u64
            || child_key_b.player != child_b as u64
        {
            return Err(format!(
                "nested fixture owner collision mismatch: child ids {child_a}/{child_b}, root sessions {}/{}, child keys {:?}/{:?}",
                root_key_a.session, root_key_b.session, child_key_a, child_key_b,
            ));
        }

        let read_fixture =
            |session: crate::player::session::RuntimeSessionHandle,
             child_id: u32,
             child_owner: crate::player::ownership::OwnerToken| async move {
                let value = crate::player::eval_lingo_command_owned(
                    session.clone(),
                    child_id,
                    child_owner.clone(),
                    "getVariable(sprite(1), \"_root.fixture\", 1)".to_owned(),
                )
                .await
                .map_err(|error| format!("nested Flash read failed: {error}"))?;
                session
                    .borrow_mut()
                    .with_player(child_id, |context| match context.player.get_datum(&value) {
                        Datum::Int(value) => Ok(*value),
                        other => Err(format!(
                            "nested Flash read returned type {}",
                            other.type_enum().type_str()
                        )),
                    })
                    .ok_or_else(|| "nested child disappeared during Flash read".to_owned())?
            };
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: initial A read begin child={child_a}"
        ));
        let value_a =
            read_fixture(root_a.session().clone(), child_a, child_owner_a.clone()).await?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: initial A read complete value={value_a}"
        ));
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: initial B read begin child={child_b}"
        ));
        let value_b =
            read_fixture(root_b.session().clone(), child_b, child_owner_b.clone()).await?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: initial B read complete value={value_b}"
        ));
        if value_a != 7 || value_b != 9 || value_a == value_b {
            return Err(format!(
                "authored nested Flash values were {value_a} and {value_b}"
            ));
        }

        wait_for_nested_flash_frame(
            root_a.session().clone(),
            child_a,
            child_owner_a.clone(),
            [0xC0, 0x20, 0x20],
        )
        .await?;
        wait_for_nested_flash_frame(
            root_b.session().clone(),
            child_b,
            child_owner_b.clone(),
            [0x20, 0x40, 0xC0],
        )
        .await?;

        let rendered_a = crate::player::nested::render_nested_players_owned(
            root_a.session(),
            root_a.player_id(),
            root_a.owner(),
        )
        .map_err(|error| format!("root A nested render failed: {error}"))?;
        let rendered_b = crate::player::nested::render_nested_players_owned(
            root_b.session(),
            root_b.player_id(),
            root_b.owner(),
        )
        .map_err(|error| format!("root B nested render failed: {error}"))?;
        if rendered_a != 1 || rendered_b != 1 {
            return Err(format!(
                "nested render counts were {rendered_a} and {rendered_b}"
            ));
        }
        root_a
            .draw_frame()
            .map_err(|error| format!("root A composition draw failed: {error:?}"))?;
        root_b
            .draw_frame()
            .map_err(|error| format!("root B composition draw failed: {error:?}"))?;
        let pixel_a = read_webgl2_pixel(&root_a_container, 20, 20)?;
        let pixel_b = read_webgl2_pixel(&root_b_container, 20, 20)?;
        if !pixel_matches(pixel_a, [0xC0, 0x20, 0x20])
            || !pixel_matches(pixel_b, [0x20, 0x40, 0xC0])
        {
            return Err(format!(
                "nested child/parent pixels were {:?} and {:?}",
                pixel_a, pixel_b
            ));
        }

        let old_child_owner_a = child_owner_a.clone();
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: child A reset begin child={child_a}"
        ));
        let fresh_child_owner_a = crate::player::nested::reset_nested_player_owned(
            root_a.session().clone(),
            child_a,
            old_child_owner_a.clone(),
        )
        .map_err(|error| format!("nested child reset failed: {error}"))?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: child A reset complete child={child_a}"
        ));
        if old_child_owner_a.is_arena_live() || !fresh_child_owner_a.is_arena_live() {
            return Err("nested child reset did not rotate its owner".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: stale child read begin child={child_a}"
        ));
        if crate::player::eval_lingo_command_owned(
            root_a.session().clone(),
            child_a,
            old_child_owner_a,
            "1 + 1".to_owned(),
        )
        .await
        .is_ok()
        {
            return Err("stale nested child owner accepted a post-reset command".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: stale child read rejected child={child_a}"
        ));
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child command begin child={child_a}"
        ));
        crate::player::eval_lingo_command_owned(
            root_a.session().clone(),
            child_a,
            fresh_child_owner_a.clone(),
            "1 + 1".to_owned(),
        )
        .await
        .map_err(|error| format!("fresh nested child command failed: {error}"))?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child command complete child={child_a}"
        ));
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child value read begin child={child_a}"
        ));
        if read_fixture(
            root_a.session().clone(),
            child_a,
            fresh_child_owner_a.clone(),
        )
        .await?
            != 7
        {
            return Err(
                "fresh nested child did not retain its authored Flash value after reset".to_owned(),
            );
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child value read complete child={child_a}"
        ));
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: sibling B read begin child={child_b}"
        ));
        let value_b_after_child_reset =
            read_fixture(root_b.session().clone(), child_b, child_owner_b.clone()).await?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: sibling B read complete value={value_b_after_child_reset}"
        ));
        if value_b_after_child_reset != 9 {
            return Err("resetting root A child changed root B".to_owned());
        }

        let owner_keys_before_failed_start = nested_flash_owner_keys()?;
        install_nested_movie_fixture(
            &root_b,
            2,
            "nested_flash_a.dcr",
            include_bytes!("../../tests/fixtures/nested_flash_a.dcr"),
        )?;
        fail_next_nested_callback_registration()?;
        let invalid_start = start_child(
            &root_b,
            CastMemberRef {
                cast_lib: 1,
                cast_member: 2,
            },
        )?;
        crate::player::testing_shared::log_test_action(
            "nested fixture: failed child start begin under root B",
        );
        if crate::player::nested::start_nested_movie_owned(root_b.session().clone(), invalid_start)
            .await
            .is_ok()
        {
            return Err("invalid nested startup unexpectedly succeeded".to_owned());
        }
        crate::player::testing_shared::log_test_action(
            "nested fixture: failed child start rejected under root B",
        );
        let owner_keys_after_failed_start = nested_flash_owner_keys()?;
        if owner_keys_after_failed_start != owner_keys_before_failed_start {
            return Err(format!(
                "failed nested startup leaked host owners: before={owner_keys_before_failed_start:?}, after={owner_keys_after_failed_start:?}"
            ));
        }
        let failed_owner_key = last_failed_nested_owner_key()?;
        if owner_keys_after_failed_start
            .iter()
            .any(|key| key == &failed_owner_key)
        {
            return Err("failed nested child remained in the production controller".to_owned());
        }
        if !probe_failed_nested_callback_absent(&failed_owner_key)? {
            return Err("failed nested child retained a production callback route".to_owned());
        }
        if !probe_failed_nested_host_absent(&failed_owner_key)? {
            return Err("failed nested child retained a production host route".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: root B post-failure read begin child={child_b}"
        ));
        if read_fixture(root_b.session().clone(), child_b, child_owner_b.clone()).await? != 9 {
            return Err("failed root B child startup changed root B".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: root B post-failure read complete child={child_b}"
        ));

        let root_a_key = root_a.owner_identity();
        unregister_browser_handle_flash_owner(&root_a_key);
        queue_prepared_flash_action(&root_a_key)?;
        let root_a_owner = root_a.owner().clone();
        crate::player::testing_shared::log_test_action("nested fixture: root A reset begin");
        root_a
            .reset()
            .map_err(|error| format!("root A reset failed: {error:?}"))?;
        if root_a_owner.is_arena_live() {
            return Err("root A owner remained live after reset".to_owned());
        }
        crate::player::testing_shared::log_test_action("nested fixture: root A reset complete");
        if _root_a_capability
            .set_flash_scripted_access_pending(true)
            .is_ok()
        {
            return Err("retired root A capability accepted a readiness update".to_owned());
        }
        if dispatch_flash_owner_action(&root_a_key, "onFlashResetAll")? {
            return Err("retired root A host route accepted a stale event".to_owned());
        }
        if probe_prepared_flash_owner(&root_a_key)? {
            return Err("prepared Flash action crossed the retired root A owner".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: root B delayed-publication read begin child={child_b}"
        ));
        let value_b_after_root_reset =
            read_fixture(root_b.session().clone(), child_b, child_owner_b.clone()).await?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: root B delayed-publication read complete value={value_b_after_root_reset}"
        ));
        if value_b_after_root_reset != 9 {
            return Err("resetting root A changed root B after delayed publication".to_owned());
        }
        crate::player::testing_shared::log_test_action(
            "nested fixture: fresh root A registration begin",
        );
        let (_fresh_root_a_capability, _fresh_root_a_owner_guard) =
            register_browser_handle_flash_owner(&root_a).await?;
        crate::player::testing_shared::log_test_action(
            "nested fixture: fresh root A registration complete",
        );
        let fresh_member_a = install_nested_movie_fixture(
            &root_a,
            1,
            "nested_flash_a.dcr",
            include_bytes!("../../tests/fixtures/nested_flash_a.dcr"),
        )?;
        let fresh_start_a = start_child(&root_a, fresh_member_a)?;
        crate::player::testing_shared::log_test_action("nested fixture: fresh child A start begin");
        let (fresh_child_a, fresh_child_owner_a) = crate::player::nested::start_nested_movie_owned(
            root_a.session().clone(),
            fresh_start_a,
        )
        .await
        .map_err(|error| format!("fresh root A nested startup failed: {error}"))?;
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child A start complete id={fresh_child_a}"
        ));
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child A read begin child={fresh_child_a}"
        ));
        if read_fixture(
            root_a.session().clone(),
            fresh_child_a,
            fresh_child_owner_a.clone(),
        )
        .await?
            != 7
        {
            return Err("fresh root A child did not read its authored Flash value".to_owned());
        }
        crate::player::testing_shared::log_test_action(&format!(
            "nested fixture: fresh child A read complete child={fresh_child_a}"
        ));
        wait_for_nested_flash_frame(
            root_a.session().clone(),
            fresh_child_a,
            fresh_child_owner_a.clone(),
            [0xC0, 0x20, 0x20],
        )
        .await?;
        if crate::player::nested::render_nested_players_owned(
            root_a.session(),
            root_a.player_id(),
            root_a.owner(),
        )
        .map_err(|error| format!("fresh root A nested render failed: {error}"))?
            != 1
        {
            return Err("fresh root A child did not compose".to_owned());
        }
        root_a
            .draw_frame()
            .map_err(|error| format!("fresh root A composition draw failed: {error:?}"))?;
        let fresh_pixel_a = read_webgl2_pixel(&root_a_container, 20, 20)?;
        if !pixel_matches(fresh_pixel_a, [0xC0, 0x20, 0x20]) {
            return Err(format!("fresh root A pixel was {fresh_pixel_a:?}"));
        }
        Ok(())
    }

    async fn wait_for_callback_probe_count(
        handle: &crate::BrowserPlayerHandle,
        label: &str,
        count_name: &Symbol,
        expected: i32,
    ) -> Result<(), String> {
        for _ in 0..200 {
            let observed = handle
                .with_context(|context| {
                    context
                        .player
                        .globals
                        .get(count_name)
                        .map(|value| context.player.get_datum(value).clone())
                })
                .map_err(|error| format!("callback owner {label} count read failed: {error:?}"))?
                .ok_or_else(|| format!("callback owner {label} count global was missing"))?;
            match observed {
                Datum::Int(value) if value == expected => return Ok(()),
                Datum::Int(value) if value > expected => {
                    return Err(format!(
                        "callback owner {label} count advanced to {value}, expected {expected}"
                    ));
                }
                Datum::Int(_) => {}
                other => {
                    return Err(format!(
                        "callback owner {label} count had type {}, expected integer",
                        other.type_str(),
                    ));
                }
            }
            async_std::task::sleep(Duration::from_millis(10)).await;
        }
        Err(format!(
            "callback owner {label} did not reach handler count {expected}"
        ))
    }

    async fn assert_authored_callback_count(
        handle: &crate::BrowserPlayerHandle,
        label: &str,
        expected: i32,
    ) -> Result<(), String> {
        for (name, description) in [
            ("callbackWrapperInvocationCount", "wrapper"),
            ("callbackProbeInvocationCount", "probe"),
        ] {
            let value = crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                format!("getVariable(sprite(1), \"_root.{name}\", 1)"),
            )
            .await
            .map_err(|error| {
                format!("callback owner {label} authored {description} count read failed: {error}")
            })?;
            let observed = handle
                .with_context(|context| context.player.get_datum(&value).clone())
                .map_err(|error| {
                    format!("callback owner {label} authored {description} count datum failed: {error:?}")
                })?;
            match observed {
                Datum::Int(value) if value == expected => {}
                Datum::Int(value) => {
                    return Err(format!(
                        "callback owner {label} authored {description} count was integer {value}, expected {expected}"
                    ));
                }
                other => {
                    return Err(format!(
                        "callback owner {label} authored {description} count had type {}, expected integer {expected}",
                        other.type_str(),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn assert_callback_wrapper_type(
        handle: &crate::BrowserPlayerHandle,
        label: &str,
    ) -> Result<(), String> {
        let value = crate::player::eval_lingo_command_owned(
            handle.session().clone(),
            handle.player_id(),
            handle.owner().clone(),
            "getVariable(sprite(1), \"_root.callbackWrapperType\", 1)".to_owned(),
        )
        .await
        .map_err(|error| format!("callback owner {label} wrapper type read failed: {error}"))?;
        let observed = handle
            .with_context(|context| context.player.get_datum(&value).clone())
            .map_err(|error| {
                format!("callback owner {label} wrapper type datum failed: {error:?}")
            })?;
        match observed {
            Datum::String(value) if value == "function" => Ok(()),
            Datum::String(value) => Err(format!(
                "callback owner {label} wrapper type was {value:?}, expected function"
            )),
            other => Err(format!(
                "callback owner {label} wrapper type had type {}, expected string function",
                other.type_str(),
            )),
        }
    }

    async fn store_callback_target_from_probe(
        handle: &crate::BrowserPlayerHandle,
        label: &str,
        count_name: &Symbol,
        probe_object_name: &Symbol,
    ) -> Result<String, String> {
        Self::assert_authored_callback_count(handle, label, 1).await?;
        let target = {
            let mut target = None;
            for _ in 0..200 {
                let observed = handle
                    .with_context(|context| {
                        let count = context
                            .player
                            .globals
                            .get(count_name)
                            .map(|value| context.player.get_datum(value).clone());
                        let object_ref = context.player.globals.get(probe_object_name).cloned();
                        let object = object_ref
                            .as_ref()
                            .map(|value| context.player.get_datum(value).clone());
                        (count, object_ref, object)
                    })
                    .map_err(|error| {
                        format!("callback owner {label} probe state read failed: {error:?}")
                    })?;
                match observed {
                    (Some(Datum::Int(value)), Some(object_ref), Some(Datum::FlashObjectRef(_)))
                        if value == 1 =>
                    {
                        target = Some(object_ref);
                        break;
                    }
                    (Some(Datum::Int(value)), _, _) if value > 1 => {
                        return Err(format!(
                            "callback owner {label} handler count advanced to {value} before target capture"
                        ));
                    }
                    (Some(Datum::Int(_)), _, _) => {}
                    (count, _, object) => {
                        let count_type = count.as_ref().map(Datum::type_str).unwrap_or("missing");
                        let object_type = object.as_ref().map(Datum::type_str).unwrap_or("missing");
                        return Err(format!(
                            "callback owner {label} probe state had unexpected count type={count_type} object type={object_type}"
                        ));
                    }
                }
                async_std::task::sleep(Duration::from_millis(10)).await;
            }
            target.ok_or_else(|| {
                format!("callback owner {label} did not publish the callback target")
            })?
        };
        let expected_owner = handle.owner_identity();
        let expected_generation = handle
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| {
                format!("callback owner {label} probe generation read failed: {error:?}")
            })?
            .ok_or_else(|| format!("callback owner {label} probe generation was not published"))?;
        let target_datum = handle
            .with_context(|context| context.player.get_datum(&target).clone())
            .map_err(|error| {
                format!("callback owner {label} probe target datum failed: {error:?}")
            })?;
        match target_datum {
            Datum::FlashObjectRef(object)
                if object.instance_id == 1
                    && object.owner_key.as_deref() == Some(expected_owner.as_str())
                    && object.instance_generation == Some(expected_generation) => {}
            Datum::FlashObjectRef(object) => {
                return Err(format!(
                    "callback owner {label} probe target lost its owner/sprite/generation binding: expected owner={expected_owner} sprite=1 generation={expected_generation}, observed owner={:?} sprite={} generation={:?}",
                    object.owner_key, object.instance_id, object.instance_generation,
                ));
            }
            other => {
                return Err(format!(
                    "callback owner {label} probe target had type {} instead of FlashObjectRef",
                    other.type_str(),
                ));
            }
        }
        let saved_name = format!("dirplayerSavedCallbackTarget{label}");
        let saved_symbol = handle
            .with_context(|context| context.symbols.intern(&saved_name))
            .map_err(|error| {
                format!("callback owner {label} probe target symbol allocation failed: {error:?}")
            })?;
        handle
            .with_context(|context| {
                context.player.globals.insert(saved_symbol, target.clone());
            })
            .map_err(|error| {
                format!("callback owner {label} probe target global store failed: {error:?}")
            })?;
        let kind = crate::player::eval_lingo_command_owned(
            handle.session().clone(),
            handle.player_id(),
            handle.owner().clone(),
            format!("{saved_name}.kind"),
        )
        .await
        .map_err(|error| {
            format!("callback owner {label} probe target kind read failed: {error}")
        })?;
        if !matches!(
            handle
                .with_context(|context| context.player.get_datum(&kind).clone())
                .map_err(|error| format!("callback owner {label} probe target kind datum failed: {error:?}"))?,
            Datum::String(value) if value == "object"
        ) {
            return Err(format!(
                "callback owner {label} probe target kind was not object"
            ));
        }
        Ok(saved_name)
    }

    /// Exercise the actual SpriteAsync getVariable/setVariable route against
    /// two independent Flash instances that intentionally both use sprite 1.
    /// The first object read occurs before member publication, so its binding
    /// must survive the first exact member association without adopting a new
    /// generation.
    pub async fn test_flash_sprite_variable_owned_fixture(&mut self) -> Result<(), String> {
        let swf = include_bytes!("../../tests/fixtures/flash_lingo_callback_probe.swf").to_vec();
        let mut first = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("SpriteAsync owner A construction failed: {error:?}"))?;
        let mut second = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("SpriteAsync owner B construction failed: {error:?}"))?;
        let (_first_capability, _first_owner_guard) =
            register_browser_handle_flash_owner(&first).await?;
        let (_second_capability, _second_owner_guard) =
            register_browser_handle_flash_owner(&second).await?;
        let harness_owner_key = crate::player::owner_key_string(self.harness_runtime().owner());
        let first_owner_key = first.owner_identity();
        let second_owner_key = second.owner_identity();
        if first_owner_key == second_owner_key
            || first_owner_key == harness_owner_key
            || second_owner_key == harness_owner_key
        {
            return Err(
                "SpriteAsync fixture owners were not isolated from each other and the harness"
                    .to_owned(),
            );
        }

        let first_container = create_public_play_test_container("sprite-variable-a")?;
        let second_container = create_public_play_test_container("sprite-variable-b")?;
        first
            .create_canvas(first_container)
            .map_err(|error| format!("SpriteAsync owner A renderer creation failed: {error:?}"))?;
        second
            .create_canvas(second_container)
            .map_err(|error| format!("SpriteAsync owner B renderer creation failed: {error:?}"))?;
        first
            .set_renderer_backend("Canvas2D".to_owned())
            .map_err(|error| format!("SpriteAsync owner A backend selection failed: {error:?}"))?;
        second
            .set_renderer_backend("Canvas2D".to_owned())
            .map_err(|error| format!("SpriteAsync owner B backend selection failed: {error:?}"))?;
        install_unpublished_flash_channel(&first)?;
        install_unpublished_flash_channel(&second)?;

        let early_first =
            eval_sprite_variable_datum(&first, "sprite(1).getVariable(\"_root\", 0)").await?;
        let early_generation = match &early_first {
            Datum::FlashObjectRef(object) => {
                if object.owner_key.as_deref() != Some(first_owner_key.as_str())
                    || object.instance_id != 1
                    || object.cast_lib != 0
                    || object.cast_member != 0
                {
                    return Err(format!(
                        "owner A early object had unexpected binding: {object:?}"
                    ));
                }
                object.instance_generation.ok_or_else(|| {
                    "owner A early object did not capture an absent-instance generation".to_owned()
                })?
            }
            other => {
                return Err(format!(
                    "owner A early object had type {}",
                    other.type_str()
                ));
            }
        };
        let early_first_name = first
            .with_context(|context| context.symbols.intern("dirplayerEarlySpriteObjectA"))
            .map_err(|error| format!("owner A early object symbol allocation failed: {error:?}"))?;
        let early_first_ref = first
            .with_context(|context| context.player.alloc_datum(early_first.clone()))
            .map_err(|error| format!("owner A early object allocation failed: {error:?}"))?;
        first
            .with_context(|context| {
                context
                    .player
                    .globals
                    .insert(early_first_name, early_first_ref);
            })
            .map_err(|error| format!("owner A early object global store failed: {error:?}"))?;
        let early_second =
            eval_sprite_variable_datum(&second, "sprite(1).getVariable(\"_root\", 0)").await?;
        let early_second_generation = match &early_second {
            Datum::FlashObjectRef(object) => {
                if object.owner_key.as_deref() != Some(second_owner_key.as_str())
                    || object.instance_id != 1
                    || object.cast_lib != 0
                    || object.cast_member != 0
                {
                    return Err(format!(
                        "owner B early object had unexpected binding: {object:?}"
                    ));
                }
                object.instance_generation.ok_or_else(|| {
                    "owner B early object did not capture an absent-instance generation".to_owned()
                })?
            }
            other => {
                return Err(format!(
                    "owner B early object had type {}",
                    other.type_str()
                ));
            }
        };
        let early_second_name = second
            .with_context(|context| context.symbols.intern("dirplayerEarlySpriteObjectB"))
            .map_err(|error| format!("owner B early object symbol allocation failed: {error:?}"))?;
        let early_second_ref = second
            .with_context(|context| context.player.alloc_datum(early_second.clone()))
            .map_err(|error| format!("owner B early object allocation failed: {error:?}"))?;
        second
            .with_context(|context| {
                context
                    .player
                    .globals
                    .insert(early_second_name, early_second_ref);
            })
            .map_err(|error| format!("owner B early object global store failed: {error:?}"))?;

        install_published_sprite_variable_fixture(&first, &swf)?;
        install_published_sprite_variable_fixture(&second, &swf)?;
        for (label, handle) in [("A", &first), ("B", &second)] {
            crate::player::run_movie_init_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
            )
            .await
            .map_err(|error| format!("SpriteAsync owner {label} startup failed: {error}"))?;
            handle.play().map_err(|error| {
                format!("SpriteAsync owner {label} playback start failed: {error:?}")
            })?;
            wait_for_pointer_flash_ready(handle).await?;
        }
        let first_generation = first
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("owner A generation read failed: {error:?}"))?
            .ok_or_else(|| "owner A generation was not published".to_owned())?;
        let second_generation = second
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("owner B generation read failed: {error:?}"))?
            .ok_or_else(|| "owner B generation was not published".to_owned())?;
        if first_generation != early_generation {
            return Err(format!(
                "owner A first member association adopted generation {first_generation} instead of early {early_generation}"
            ));
        }
        if second_generation != early_second_generation {
            return Err(format!(
                "owner B first member association adopted generation {second_generation} instead of early {early_second_generation}"
            ));
        }
        for (label, handle, expression) in [
            ("A", &first, "dirplayerEarlySpriteObjectA.kind"),
            ("B", &second, "dirplayerEarlySpriteObjectB.kind"),
        ] {
            if !matches!(
                eval_sprite_variable_datum(handle, expression).await?,
                Datum::String(value) if value == "object"
            ) {
                return Err(format!(
                    "owner {label} retained prepublication object did not read the published kind"
                ));
            }
        }
        eval_sprite_variable_datum(
            &first,
            "dirplayerEarlySpriteObjectA.dirplayerInvokeCallbackProbe(\"early-root\", 17, 0)",
        )
        .await
        .map_err(|error| format!("owner A retained prepublication method call failed: {error}"))?;
        if !matches!(
            eval_sprite_variable_datum(
                &first,
                "sprite(1).getVariable(\"_root.callbackProbeInvocationCount\", 1)"
            )
            .await?,
            Datum::Int(1)
        ) || !matches!(
            eval_sprite_variable_datum(&first, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "early-root"
        ) || !matches!(
            eval_sprite_variable_datum(
                &first,
                "sprite(1).getVariable(\"_root.callbackProbeNumber\", 1)"
            )
            .await?,
            Datum::Int(17)
        ) || !matches!(
            eval_sprite_variable_datum(
                &first,
                "sprite(1).getVariable(\"_root.callbackProbeObject\", 0)"
            )
            .await?,
            Datum::FlashObjectRef(_)
        ) {
            return Err(
                "owner A retained prepublication method call did not preserve authored arguments"
                    .to_owned(),
            );
        }

        if !matches!(
            eval_sprite_variable_datum(
                &first,
                "sprite(1).getVariable(\"_root.callbackProbeInvocationCount\", 1)"
            )
            .await?,
            Datum::Int(1)
        ) {
            return Err("owner A authored invocation count was not an exact Int(1)".to_owned());
        }
        if !matches!(
            eval_sprite_variable_datum(&first, "sprite(1).getVariable(\"_root.callbackProbeEncoding\", 1)").await?,
            Datum::String(value) if value == "café"
        ) {
            return Err("owner A UTF-8 SpriteAsync value did not round-trip".to_owned());
        }
        eval_sprite_variable_datum(
            &first,
            "sprite(1).setVariable(\"_root.callbackProbeString\", \"owner-a\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&first, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "owner-a"
        ) {
            return Err(
                "owner A SpriteAsync setter did not string-convert and read back".to_owned(),
            );
        }
        eval_sprite_variable_datum(
            &second,
            "sprite(1).setVariable(\"_root.callbackProbeString\", \"owner-b\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&second, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "owner-b"
        ) {
            return Err("owner B SpriteAsync setter did not round-trip independently".to_owned());
        }

        for (label, argument, expected) in [
            ("boolean", "true", 1.0),
            ("strict max", "2147483646", 2147483646.0),
            ("i32 boundary", "2147483647", 2147483647.0),
        ] {
            let expression = format!(
                "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"{label}\" & QUOTE & \",{argument},{{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}}]\")"
            );
            crate::player::eval_lingo_command_owned(
                first.session().clone(),
                first.player_id(),
                first.owner().clone(),
                expression,
            )
            .await
            .map_err(|error| format!("owner A {label} authored value setup failed: {error}"))?;
            let observed = eval_sprite_variable_datum(
                &first,
                "sprite(1).getVariable(\"_root.callbackProbeNumber\", 1)",
            )
            .await?;
            let matches_expected = match (argument, observed) {
                ("2147483647", Datum::Float(value)) => value == expected,
                (_, Datum::Int(value)) => f64::from(value) == expected,
                _ => false,
            };
            if !matches_expected {
                return Err(format!(
                    "owner A {label} value was not the expected strict numeric form"
                ));
            }
        }

        crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"retained\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        )
        .await
        .map_err(|error| format!("owner A authored object setup failed: {error}"))?;
        eval_sprite_variable_datum(
            &first,
            "put sprite(1).getVariable(\"_root.callbackProbeObject\", 0) into retainedSpriteObject",
        )
        .await?;
        let retained = first
            .with_context(|context| {
                let name = context.symbols.intern("retainedSpriteObject");
                context
                    .player
                    .globals
                    .get(&name)
                    .map(|value| context.player.get_datum(value).clone())
            })
            .map_err(|error| format!("owner A retained global read failed: {error:?}"))?
            .ok_or_else(|| "owner A retained global was not assigned".to_owned())?;
        match retained {
            Datum::FlashObjectRef(object)
                if object.owner_key.as_deref() == Some(first_owner_key.as_str())
                    && object.instance_id == 1
                    && object.instance_generation == Some(first_generation) => {}
            other => {
                return Err(format!(
                    "owner A retained object had unexpected datum type {}",
                    other.type_str()
                ));
            }
        }
        eval_sprite_variable_datum(
            &first,
            "sprite(1).setVariable(\"_root.callbackProbeObject\", \"repointed\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&first, "retainedSpriteObject.kind").await?,
            Datum::String(value) if value == "object"
        ) {
            return Err(
                "owner A retained object did not survive source-variable repointing".to_owned(),
            );
        }

        replace_sprite_variable_flash_fixture(&first, &swf)?;
        crate::player::run_movie_init_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
        )
        .await
        .map_err(|error| format!("owner A replacement startup failed: {error}"))?;
        wait_for_pointer_flash_ready(&first).await?;
        let replacement_generation = first
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("owner A replacement generation read failed: {error:?}"))?
            .ok_or_else(|| "owner A replacement generation was not published".to_owned())?;
        if replacement_generation == first_generation {
            return Err("owner A replacement did not rotate the generation".to_owned());
        }
        if eval_sprite_variable_datum(&first, "dirplayerEarlySpriteObjectA.kind")
            .await
            .is_ok()
        {
            return Err("owner A retained prepublication object remained usable after generation replacement".to_owned());
        }
        if eval_sprite_variable_datum(&first, "retainedSpriteObject.kind")
            .await
            .is_ok()
        {
            return Err(
                "owner A retained object remained usable after generation replacement".to_owned(),
            );
        }
        eval_sprite_variable_datum(
            &first,
            "sprite(1).setVariable(\"_root.callbackProbeString\", \"owner-a-replacement\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&first, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "owner-a-replacement"
        ) {
            return Err(
                "owner A replacement SpriteAsync get/set did not use the fresh instance".to_owned(),
            );
        }
        eval_sprite_variable_datum(
            &second,
            "sprite(1).setVariable(\"_root.callbackProbeString\", \"b-after-replacement\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&second, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "b-after-replacement"
        ) {
            return Err("owner B did not survive owner A replacement".to_owned());
        }

        let old_owner = first.owner().clone();
        first
            .reset()
            .map_err(|error| format!("owner A reset failed: {error:?}"))?;
        if old_owner.is_arena_live() {
            return Err("owner A remained live after reset".to_owned());
        }
        let stale_after_reset = crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            old_owner,
            "sprite(1).getVariable(\"_root.callbackProbeString\", 1)".to_owned(),
        )
        .await;
        if stale_after_reset.is_ok() {
            return Err("owner A stale SpriteAsync command was accepted after reset".to_owned());
        }
        let disposed_session = first.session().clone();
        let disposed_player_id = first.player_id();
        let disposed_owner = first.owner().clone();
        drop(first);
        if disposed_owner.is_arena_live() {
            return Err("owner A replacement owner remained live after disposal".to_owned());
        }
        let stale_after_dispose = crate::player::eval_lingo_command_owned(
            disposed_session,
            disposed_player_id,
            disposed_owner,
            "sprite(1).getVariable(\"_root.callbackProbeString\", 1)".to_owned(),
        )
        .await;
        if stale_after_dispose.is_ok() {
            return Err("owner A stale SpriteAsync command was accepted after disposal".to_owned());
        }
        eval_sprite_variable_datum(
            &second,
            "sprite(1).setVariable(\"_root.callbackProbeString\", \"b-after-dispose\")",
        )
        .await?;
        if !matches!(
            eval_sprite_variable_datum(&second, "sprite(1).getVariable(\"_root.callbackProbeString\", 1)").await?,
            Datum::String(value) if value == "b-after-dispose"
        ) {
            return Err("owner B did not survive owner A disposal".to_owned());
        }
        if second_generation
            != second
                .with_context(|context| context.player.flash_instance_generation(1))
                .map_err(|error| format!("owner B generation changed unexpectedly: {error:?}"))?
                .ok_or_else(|| "owner B generation disappeared after owner A disposal".to_owned())?
        {
            return Err("owner B generation changed after owner A disposal".to_owned());
        }
        Ok(())
    }

    pub async fn test_flash_lingo_callback_owned_fixture(&mut self) -> Result<(), String> {
        let mut first = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("callback owner A construction failed: {error:?}"))?;
        let second = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("callback owner B construction failed: {error:?}"))?;
        let (_first_capability, _first_guard) = register_browser_handle_flash_owner(&first).await?;
        let (_second_capability, _second_guard) =
            register_browser_handle_flash_owner(&second).await?;
        let first_container = create_public_play_test_container("callback-owner-a")?;
        let second_container = create_public_play_test_container("callback-owner-b")?;
        let callback_swf = include_bytes!("../../tests/fixtures/flash_lingo_callback_probe.swf");

        install_pointer_flash_fixture(&first, callback_swf)?;
        install_pointer_flash_fixture(&second, callback_swf)?;
        let first_probe = install_callback_script_fixture(&first)?;
        let second_probe = install_callback_script_fixture(&second)?;
        first
            .create_canvas(first_container)
            .map_err(|error| format!("callback owner A renderer creation failed: {error:?}"))?;
        second
            .create_canvas(second_container)
            .map_err(|error| format!("callback owner B renderer creation failed: {error:?}"))?;

        let mut first_saved_target = None;
        for (label, handle, probe) in [
            ("A", &first, first_probe.clone()),
            ("B", &second, second_probe.clone()),
        ] {
            // Register through the string path before the first Flash object
            // publication. This exercises the production reservation/load
            // boundary, including the queued host registration.
            crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                "sprite(1).setCallback(\"_root\", \"dirplayerCallbackProbe\", #onFlashCallback, script(2).new())".to_owned(),
            )
            .await
            .map_err(|error| format!("callback owner {label} prepublication registration failed: {error}"))?;
            Self::assert_callback_wrapper_type(handle, label).await?;
            crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
            )
            .await
            .map_err(|error| format!("callback owner {label} prepublication callback failed: {error}"))?;
            let saved_target =
                Self::store_callback_target_from_probe(handle, label, &probe.0, &probe.3).await?;
            if label == "A" {
                first_saved_target = Some(saved_target.clone());
            }
            crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                format!("sprite(1).setCallback({saved_target}, \"dirplayerCallbackProbe\", #onFlashCallback, script(2).new())"),
            )
            .await
            .map_err(|error| format!("callback owner {label} cache-hit registration failed: {error}"))?;
            crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
            )
            .await
            .map_err(|error| format!("callback owner {label} AVM1 call failed: {error}"))?;
            Self::assert_authored_callback_count(handle, label, 2).await?;
            Self::wait_for_callback_probe_count(handle, label, &probe.0, 2).await?;
            for (path, expected) in [
                ("_root.callbackProbeInvocationCount", "count"),
                ("_root.callbackProbeString", "string"),
                ("_root.callbackProbeNumber", "number"),
            ] {
                let value = crate::player::eval_lingo_command_owned(
                    handle.session().clone(),
                    handle.player_id(),
                    handle.owner().clone(),
                    format!("getVariable(sprite(1), \"{path}\", 1)"),
                )
                .await
                .map_err(|error| {
                    format!("callback owner {label} {expected} probe read failed: {error}")
                })?;
                let observed = handle
                    .with_context(|context| context.player.get_datum(&value).clone())
                    .map_err(|error| {
                        format!("callback owner {label} {expected} probe datum failed: {error:?}")
                    })?;
                match (expected, observed) {
                    ("count", Datum::Int(value)) if value == 2 => {}
                    ("string", Datum::String(value)) if value == "雪だるま☃" => {}
                    ("number", Datum::Int(value)) if value == 7 => {}
                    (_, _) => {
                        return Err(format!(
                            "callback owner {label} {expected} probe had an unexpected datum"
                        ));
                    }
                }
            }
        }

        for (label, handle, (count_name, text_name, number_name, object_name)) in [
            ("A", &first, first_probe.clone()),
            ("B", &second, second_probe.clone()),
        ] {
            let values = handle
                .with_context(|context| {
                    (
                        context
                            .player
                            .get_datum(context.player.globals.get(&count_name).unwrap())
                            .clone(),
                        context
                            .player
                            .get_datum(context.player.globals.get(&text_name).unwrap())
                            .clone(),
                        context
                            .player
                            .get_datum(context.player.globals.get(&number_name).unwrap())
                            .clone(),
                        context
                            .player
                            .get_datum(context.player.globals.get(&object_name).unwrap())
                            .clone(),
                    )
                })
                .map_err(|error| format!("callback owner {label} probe read failed: {error:?}"))?;
            if !matches!(values.0, Datum::Int(2))
                || !matches!(values.1, Datum::String(ref value) if value == "雪だるま☃")
                || !matches!(values.2, Datum::Int(7))
                || !matches!(values.3, Datum::FlashObjectRef(_))
            {
                return Err(format!("callback owner {label} probe values did not match"));
            }
        }

        // The callback's retained object must remain an owner-bound
        // FlashObjectRef. Read a real property through the normal evaluator
        // before replacing the instance generation.
        let first_owner_key = first.owner_identity();
        let old_generation = first
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("callback current-generation read failed: {error:?}"))?
            .ok_or_else(|| "callback Flash generation was not published".to_owned())?;
        let first_saved_target = first_saved_target
            .ok_or_else(|| "callback owner A saved target was not recorded".to_owned())?;
        let retained_object = first
            .with_context(|context| {
                let symbol = context.symbols.intern(&first_saved_target);
                context.player.globals.get(&symbol).cloned()
            })
            .map_err(|error| format!("retained callback object read failed: {error:?}"))?
            .ok_or_else(|| "retained callback object global was missing".to_owned())?;
        let retained_object_datum = first
            .with_context(|context| context.player.get_datum(&retained_object).clone())
            .map_err(|error| format!("retained callback object datum failed: {error:?}"))?;
        let Datum::FlashObjectRef(retained_object_ref) = retained_object_datum else {
            return Err("callback retained object was not a FlashObjectRef".to_owned());
        };
        if retained_object_ref.instance_id != 1
            || retained_object_ref.owner_key.as_deref() != Some(first_owner_key.as_str())
            || retained_object_ref.instance_generation != Some(old_generation)
        {
            return Err(
                "callback retained object lost its owner/sprite/generation binding".to_owned(),
            );
        }
        let retained_kind = crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            format!("{first_saved_target}.kind"),
        )
        .await
        .map_err(|error| format!("retained callback object property read failed: {error}"))?;
        if !matches!(
            first.with_context(|context| context.player.get_datum(&retained_kind).clone())
                .map_err(|error| format!("retained callback kind datum failed: {error:?}"))?,
            Datum::String(value) if value == "object"
        ) {
            return Err("retained callback object kind property did not round-trip".to_owned());
        }

        let window =
            web_sys::window().ok_or_else(|| "callback browser window is unavailable".to_owned())?;
        let trigger = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_triggerLingoCallbackOnScript"),
        )
        .map_err(|_| "stable callback route is unavailable".to_owned())?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "stable callback route is not callable".to_owned())?;
        let trigger_ack = trigger
            .call9(
                &window,
                &JsValue::from_str(&first.owner_identity()),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(old_generation as f64),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(2.0),
                &JsValue::from_str("onFlashCallback"),
                &JsValue::from_str("malformed outer JSON"),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(1.0),
            )
            .map_err(|error| format!("callback stale payload enqueue failed: {error:?}"))?;
        if trigger_ack.as_bool() != Some(true) {
            return Err("stable callback route rejected the valid queued payload".to_owned());
        }
        let old_owner_key = first.owner_identity();
        let old_owner = first.owner().clone();
        first
            .reserve_flash_instance_generation(1.0)
            .map_err(|error| format!("callback generation replacement failed: {error:?}"))?;
        for (label, handle) in [("A", &first), ("B", &second)] {
            crate::player::eval_lingo_command_owned(
                handle.session().clone(),
                handle.player_id(),
                handle.owner().clone(),
                "1 + 1".to_owned(),
            )
            .await
            .map_err(|error| {
                format!("owner {label} did not survive stale callback rejection: {error}")
            })?;
        }
        let stale_count = first
            .with_context(|context| {
                context
                    .player
                    .globals
                    .get(&first_probe.0)
                    .map(|value| context.player.get_datum(value).clone())
            })
            .map_err(|error| format!("owner A stale callback count read failed: {error:?}"))?
            .ok_or_else(|| "owner A stale callback count global was missing".to_owned())?;
        if !matches!(stale_count, Datum::Int(2)) {
            return Err("stale callback changed owner A's Lingo handler count".to_owned());
        }
        if crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            format!("{first_saved_target}.kind"),
        )
        .await
        .is_ok()
        {
            return Err("stale retained callback object still answered a property read".to_owned());
        }
        crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B sibling callback failed: {error}"))?;
        Self::assert_authored_callback_count(&second, "B", 3).await?;
        Self::wait_for_callback_probe_count(&second, "B", &second_probe.0, 3).await?;
        let sibling_count = crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "getVariable(sprite(1), \"_root.callbackProbeInvocationCount\", 1)".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B sibling count read failed: {error}"))?;
        if !matches!(
            second
                .with_context(|context| context.player.get_datum(&sibling_count).clone())
                .map_err(|error| format!("owner B sibling count datum failed: {error:?}"))?,
            Datum::Int(3)
        ) {
            return Err("owner B did not survive owner A stale callback rejection".to_owned());
        }

        unregister_browser_handle_flash_owner(&old_owner_key);
        first
            .reset()
            .map_err(|error| format!("callback owner A reset failed: {error:?}"))?;
        if old_owner.is_arena_live() {
            return Err("disposed callback owner A remained live after reset".to_owned());
        }
        let stale_after_reset = trigger
            .call9(
                &window,
                &JsValue::from_str(&old_owner_key),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(old_generation as f64),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(2.0),
                &JsValue::from_str("onFlashCallback"),
                &JsValue::from_str("[]"),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(1.0),
            )
            .map_err(|error| format!("stale callback route after reset failed: {error:?}"))?;
        if stale_after_reset.as_bool() == Some(true) {
            return Err("disposed callback owner A retained its stable route".to_owned());
        }

        let (_fresh_capability, _fresh_guard) = register_browser_handle_flash_owner(&first).await?;
        install_pointer_flash_fixture(&first, callback_swf)?;
        let fresh_probe = install_callback_script_fixture(&first)?;
        crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            "sprite(1).setCallback(\"_root\", \"dirplayerCallbackProbe\", #onFlashCallback, script(2).new())".to_owned(),
        )
        .await
        .map_err(|error| format!("fresh callback owner A prepublication registration failed: {error}"))?;
        Self::assert_callback_wrapper_type(&first, "AFresh").await?;
        crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        )
        .await
        .map_err(|error| format!("fresh callback owner A prepublication callback failed: {error}"))?;
        let fresh_saved_target = Self::store_callback_target_from_probe(
            &first,
            "AFresh",
            &fresh_probe.0,
            &fresh_probe.3,
        )
        .await?;
        for expression in [
            format!("sprite(1).setCallback({fresh_saved_target}, \"dirplayerCallbackProbe\", #onFlashCallback, script(2).new())"),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        ] {
            crate::player::eval_lingo_command_owned(
                first.session().clone(),
                first.player_id(),
                first.owner().clone(),
                expression,
            )
            .await
            .map_err(|error| format!("fresh callback owner A operation failed: {error}"))?;
        }
        Self::assert_authored_callback_count(&first, "AFresh", 2).await?;
        Self::wait_for_callback_probe_count(&first, "AFresh", &fresh_probe.0, 2).await?;
        let fresh_count = crate::player::eval_lingo_command_owned(
            first.session().clone(),
            first.player_id(),
            first.owner().clone(),
            "getVariable(sprite(1), \"_root.callbackProbeInvocationCount\", 1)".to_owned(),
        )
        .await
        .map_err(|error| format!("fresh owner A authored count read failed: {error}"))?;
        if !matches!(
            first
                .with_context(|context| context.player.get_datum(&fresh_count).clone())
                .map_err(|error| format!("fresh owner A count datum failed: {error:?}"))?,
            Datum::Int(2)
        ) {
            return Err("fresh owner A authored callback count was not exactly two".to_owned());
        }
        let fresh_values = first
            .with_context(|context| {
                [fresh_probe.0, fresh_probe.1, fresh_probe.2, fresh_probe.3].map(|name| {
                    context
                        .player
                        .get_datum(context.player.globals.get(&name).unwrap())
                        .clone()
                })
            })
            .map_err(|error| format!("fresh owner A Lingo probe read failed: {error:?}"))?;
        if !matches!(fresh_values[0], Datum::Int(2))
            || !matches!(&fresh_values[1], Datum::String(value) if value == "雪だるま☃")
            || !matches!(fresh_values[2], Datum::Int(7))
            || !matches!(fresh_values[3], Datum::FlashObjectRef(_))
        {
            return Err(
                "fresh owner A Lingo callback values did not match both deliveries".to_owned(),
            );
        }

        crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B callback after owner A reset failed: {error}"))?;
        Self::assert_authored_callback_count(&second, "B", 4).await?;
        Self::wait_for_callback_probe_count(&second, "B", &second_probe.0, 4).await?;
        let sibling_count_after_reset = crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "getVariable(sprite(1), \"_root.callbackProbeInvocationCount\", 1)".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B count after owner A reset failed: {error}"))?;
        if !matches!(
            second
                .with_context(|context| context
                    .player
                    .get_datum(&sibling_count_after_reset)
                    .clone())
                .map_err(|error| format!("owner B count datum after reset failed: {error:?}"))?,
            Datum::Int(4)
        ) {
            return Err("owner B callback count did not advance after owner A reset".to_owned());
        }
        let sibling_values = second
            .with_context(|context| {
                [
                    second_probe.0.clone(),
                    second_probe.1.clone(),
                    second_probe.2.clone(),
                    second_probe.3.clone(),
                ]
                .map(|name| {
                    context
                        .player
                        .get_datum(context.player.globals.get(&name).unwrap())
                        .clone()
                })
            })
            .map_err(|error| format!("owner B Lingo probe after reset failed: {error:?}"))?;
        if !matches!(sibling_values[0], Datum::Int(4))
            || !matches!(&sibling_values[1], Datum::String(value) if value == "雪だるま☃")
            || !matches!(sibling_values[2], Datum::Int(7))
            || !matches!(sibling_values[3], Datum::FlashObjectRef(_))
        {
            return Err("owner B Lingo callback state did not survive owner A reset".to_owned());
        }

        let fresh_owner_key = first.owner_identity();
        let fresh_generation = first
            .with_context(|context| context.player.flash_instance_generation(1))
            .map_err(|error| format!("fresh owner A generation read failed: {error:?}"))?
            .ok_or_else(|| "fresh owner A generation was absent before disposal".to_owned())?;
        drop(first);
        let disposed_ack = trigger
            .call9(
                &window,
                &JsValue::from_str(&fresh_owner_key),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(fresh_generation as f64),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(2.0),
                &JsValue::from_str("onFlashCallback"),
                &JsValue::from_str("[]"),
                &JsValue::from_f64(1.0),
                &JsValue::from_f64(1.0),
            )
            .map_err(|error| format!("disposed callback route failed: {error:?}"))?;
        if disposed_ack.as_bool() != Some(false) {
            return Err("disposed owner A callback route accepted delivery".to_owned());
        }
        crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "sprite(1).callFunction(\"_root.dirplayerInvokeCallbackProbe\", \"[\" & QUOTE & \"雪だるま☃\" & QUOTE & \",7,{\" & QUOTE & \"kind\" & QUOTE & \":\" & QUOTE & \"object\" & QUOTE & \"}]\")".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B callback after owner A disposal failed: {error}"))?;
        Self::assert_authored_callback_count(&second, "B", 5).await?;
        Self::wait_for_callback_probe_count(&second, "B", &second_probe.0, 5).await?;
        let sibling_count_after_dispose = crate::player::eval_lingo_command_owned(
            second.session().clone(),
            second.player_id(),
            second.owner().clone(),
            "getVariable(sprite(1), \"_root.callbackProbeInvocationCount\", 1)".to_owned(),
        )
        .await
        .map_err(|error| format!("owner B count after owner A disposal failed: {error}"))?;
        if !matches!(
            second
                .with_context(|context| context
                    .player
                    .get_datum(&sibling_count_after_dispose)
                    .clone())
                .map_err(|error| format!("owner B count datum after disposal failed: {error:?}"))?,
            Datum::Int(5)
        ) {
            return Err("owner B callback count did not advance after owner A disposal".to_owned());
        }
        let sibling_values_after_dispose = second
            .with_context(|context| {
                [
                    second_probe.0,
                    second_probe.1,
                    second_probe.2,
                    second_probe.3,
                ]
                .map(|name| {
                    context
                        .player
                        .get_datum(context.player.globals.get(&name).unwrap())
                        .clone()
                })
            })
            .map_err(|error| format!("owner B Lingo probe after disposal failed: {error:?}"))?;
        if !matches!(sibling_values_after_dispose[0], Datum::Int(5))
            || !matches!(&sibling_values_after_dispose[1], Datum::String(value) if value == "雪だるま☃")
            || !matches!(sibling_values_after_dispose[2], Datum::Int(7))
            || !matches!(sibling_values_after_dispose[3], Datum::FlashObjectRef(_))
        {
            return Err("owner B Lingo callback state did not survive owner A disposal".to_owned());
        }
        Ok(())
    }

    /// Exercise the production root startup path with a real embedded SWF.
    /// The Flash member is installed before startup, while no host generation
    /// is manually reserved; the first BindGet must therefore use the
    /// prepared load generation and later access must use that same owner
    /// route.
    pub async fn test_flash_initial_access_before_reservation(&mut self) -> Result<(), String> {
        let swf = include_bytes!("../../tests/fixtures/flash_initial_access.swf").to_vec();
        let read_global_int = |player: &BrowserTestPlayer, name: &str| {
            player
                .harness_runtime()
                .with_context(|context| {
                    let symbol = context.symbols.intern(name);
                    context.player.globals.get(&symbol).and_then(|value| {
                        match context.player.get_datum(value) {
                            Datum::Int(value) => Some(*value),
                            _ => None,
                        }
                    })
                })
                .flatten()
                .ok_or_else(|| format!("global {name} was not an integer"))
        };
        self.harness_runtime().with_context(|context| {
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Flash(FlashMember {
                        data: swf.clone(),
                        reg_point: (0, 0),
                        flash_info: None,
                    }),
                ),
                context.symbols,
            );
            context.player.movie.cast_manager.casts.push(cast);
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            });
            channel.sprite.width = 1;
            channel.sprite.height = 1;
            context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
            context.player.movie.score.invalidate_render_channel_cache();
            context.player.startup_do = Some(
                "put getVariable(sprite(1), \"_root.fixture\", 1) into flashInitialValue"
                    .to_owned(),
            );
        });

        crate::player::run_movie_init_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
        )
        .await
        .map_err(|error| format!("real Flash startup failed: {error}"))?;
        let initial = read_global_int(self, "flashInitialValue")?;
        if initial != 7 {
            return Err(format!("unexpected initial Flash value: {initial}"));
        }

        self.eval("put getVariable(sprite(1), \"_root.fixture\", 1) into flashLaterValue")
            .await
            .map_err(|error| format!("later Flash value read failed: {error:?}"))?;
        let later = read_global_int(self, "flashLaterValue")?;
        if later != 7 {
            return Err(format!("unexpected later Flash value: {later}"));
        }

        // Replace the member after startup. This exercises the lazy first
        // BindGet preparation path and the captured cast-pair/generation fence
        // instead of only rereading the original instance.
        self.harness_runtime().with_context(|context| {
            if let Some(cast) = context.player.movie.cast_manager.casts.first_mut() {
                cast.insert_member(
                    2,
                    CastMember::new(
                        2,
                        CastMemberType::Flash(FlashMember {
                            data: swf.clone(),
                            reg_point: (0, 0),
                            flash_info: None,
                        }),
                    ),
                    context.symbols,
                );
            }
            if let Some(channel) = context.player.movie.score.channels.get_mut(1) {
                channel.sprite.member = Some(CastMemberRef {
                    cast_lib: 1,
                    cast_member: 2,
                });
            }
            context.player.movie.score.invalidate_render_channel_cache();
        });
        self.eval("put getVariable(sprite(1), \"_root.fixture\", 1) into flashReplacementValue")
            .await
            .map_err(|error| format!("replacement Flash value read failed: {error:?}"))?;
        let replacement = read_global_int(self, "flashReplacementValue")?;
        if replacement != 7 {
            return Err(format!("unexpected replacement Flash value: {replacement}"));
        }

        // Retire the owner and install the same authored member on the fresh
        // player. The old capability must be dead before the replacement can
        // publish or answer a Flash access.
        let old_owner = self.harness_runtime().owner().clone();
        self.reset_player().await;
        if old_owner.is_arena_live() {
            return Err("old Flash owner remained live after reset".to_owned());
        }
        self.harness_runtime().with_context(|context| {
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Flash(FlashMember {
                        data: swf.clone(),
                        reg_point: (0, 0),
                        flash_info: None,
                    }),
                ),
                context.symbols,
            );
            context.player.movie.cast_manager.casts.push(cast);
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            });
            channel.sprite.width = 1;
            channel.sprite.height = 1;
            context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
            context.player.movie.score.invalidate_render_channel_cache();
            context.player.startup_do = Some(
                "put getVariable(sprite(1), \"_root.fixture\", 1) into flashResetValue".to_owned(),
            );
        });
        crate::player::run_movie_init_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
        )
        .await
        .map_err(|error| format!("reset replacement Flash startup failed: {error}"))?;
        let reset_value = read_global_int(self, "flashResetValue")?;
        if reset_value != 7 {
            return Err(format!(
                "unexpected reset replacement Flash value: {reset_value}"
            ));
        }
        Ok(())
    }

    /// Exercise the production evaluator's owner-bound Flash request path.
    /// The host functions are replaced only for this test, but Rust reaches
    /// them through `eval_lingo_command_owned`: the first request is an
    /// unbound BindGet and the property read is a generation-qualified Get.
    /// The second host callback replaces the sprite generation before its
    /// response is applied, so the late result must be rejected and a fresh
    /// bind must still work.
    pub async fn test_flash_owned_evaluator_binding(&mut self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let swf = include_bytes!("../../tests/fixtures/flash_initial_access.swf").to_vec();
        self.harness_runtime().with_context(|context| {
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Flash(FlashMember {
                        data: swf.clone(),
                        reg_point: (0, 0),
                        flash_info: None,
                    }),
                ),
                context.symbols,
            );
            context.player.movie.cast_manager.casts.push(cast);
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            });
            context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
            context.player.movie.score.invalidate_render_channel_cache();
        });
        // `dirplayer_registerFlashOwner` loads the production manager lazily.
        // Prime that existing helper before replacing only the two transport
        // globals; this prevents a late bundle import from overwriting the
        // controlled callbacks below.
        let owner = self.harness_runtime().owner().clone();
        let owner_key = {
            let key = owner.key();
            format!("{}:{}:{}", key.session, key.player, key.generation)
        };
        let (pending_cell, flash_binding_state) = self
            .harness_runtime()
            .with_context(|context| {
                (
                    context.player.flash_scripted_access_pending.clone(),
                    context.player.flash_binding_state.clone(),
                )
            })
            .ok_or_else(|| "Flash readiness state is unavailable".to_owned())?;
        let capability = BrowserFlashCapability::new(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            owner.clone(),
            pending_cell.clone(),
            self.command_tx.clone(),
        );
        let helper = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_testFlashOwnerCapability"),
        )
        .map_err(|_| "production FlashOwnerHost helper is unavailable".to_owned())?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "production FlashOwnerHost helper is not callable".to_owned())?;
        let helper_capability: JsValue = capability.into();
        let helper_result = helper
            .call3(
                &window,
                &JsValue::from_str(&owner_key),
                &helper_capability,
                &JsValue::UNDEFINED,
            )
            .map_err(|error| format!("FlashOwnerHost priming failed: {error:?}"))?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| "FlashOwnerHost priming did not return a Promise".to_owned())?;
        JsFuture::from(helper_result)
            .await
            .map_err(|error| format!("FlashOwnerHost priming rejected: {error:?}"))?;
        Self::next_frame().await;

        let route_names = [
            "dirplayer_ruffleGetVariableOwnedForBinding",
            "dirplayer_ruffleGetVariableOwnedAtGeneration",
        ];
        let originals = route_names
            .iter()
            .map(|name| {
                js_sys::Reflect::get(&window, &JsValue::from_str(name))
                    .map(|value| ((*name).to_owned(), value))
                    .map_err(|error| format!("could not capture {name}: {error:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let _routes = FlashOwnedRouteGuard {
            window: window.clone(),
            entries: originals,
        };

        let route_capability = BrowserFlashCapability::new(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            owner.clone(),
            pending_cell.clone(),
            self.command_tx.clone(),
        );
        let assertion_capability = BrowserFlashCapability::new(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            owner.clone(),
            pending_cell,
            self.command_tx.clone(),
        );
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let route_error = Rc::new(RefCell::new(None::<String>));
        let replace_on_get = Rc::new(Cell::new(false));
        let replacement_generation = Rc::new(Cell::new(0.0));
        let bound_generation = Rc::new(Cell::new(None::<f64>));
        let successful_get_generation = Rc::new(Cell::new(None::<f64>));

        let bind_calls = calls.clone();
        let bind_error = route_error.clone();
        let bind_generation = replacement_generation.clone();
        let bind_bound_generation = bound_generation.clone();
        let bind_binding = flash_binding_state;
        let bind_owner_key = owner_key.clone();
        let bind_response = Closure::wrap(Box::new(
            move |requested_owner: JsValue,
                  sprite: JsValue,
                  path: JsValue,
                  return_as_object: JsValue|
                  -> JsValue {
                let owner_matches =
                    requested_owner.as_string().as_deref() == Some(bind_owner_key.as_str());
                let sprite_matches = sprite.as_f64() == Some(1.0);
                let path = path.as_string().unwrap_or_default();
                let object_mode = return_as_object.as_bool().unwrap_or(false);
                let current_generation = bind_binding
                    .borrow()
                    .generations
                    .get(&1)
                    .copied()
                    .map(|generation| generation as f64);
                let Some(current_generation) = current_generation else {
                    *bind_error.borrow_mut() =
                        Some("BindGet did not observe a current sprite generation".to_owned());
                    return flash_owned_test_error(
                        "invalid-generation",
                        "BindGet did not observe a current sprite generation",
                    );
                };
                bind_generation.set(current_generation);
                bind_bound_generation.set(Some(current_generation));
                bind_calls
                    .borrow_mut()
                    .push(format!("bind:{path}:{current_generation}"));
                if !owner_matches || !sprite_matches || !object_mode {
                    *bind_error.borrow_mut() = Some(format!(
                        "invalid BindGet arguments owner={owner_matches} sprite={sprite_matches} object={object_mode}"
                    ));
                    return flash_owned_test_error(
                        "invalid-arguments",
                        "invalid BindGet arguments",
                    );
                }
                let value = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &value,
                    &JsValue::from_str("__dirplayer_stored_path"),
                    &JsValue::from_str("_root.fixture"),
                );
                flash_owned_test_response(current_generation, value.into())
            },
        )
            as Box<dyn FnMut(JsValue, JsValue, JsValue, JsValue) -> JsValue>);

        let get_calls = calls.clone();
        let get_error = route_error.clone();
        let get_capability = route_capability;
        let get_generation = replacement_generation.clone();
        let get_replace = replace_on_get.clone();
        let get_successful_generation = successful_get_generation.clone();
        let get_owner_key = owner_key.clone();
        let get_response = Closure::wrap(Box::new(
            move |requested_owner: JsValue,
                  sprite: JsValue,
                  generation: JsValue,
                  path: JsValue,
                  return_as_object: JsValue|
                  -> JsValue {
                let requested_generation = generation.as_f64().unwrap_or(f64::NAN);
                let owner_matches =
                    requested_owner.as_string().as_deref() == Some(get_owner_key.as_str());
                let sprite_matches = sprite.as_f64() == Some(1.0);
                let path = path.as_string().unwrap_or_default();
                get_calls
                    .borrow_mut()
                    .push(format!("get:{path}:{requested_generation}"));
                if !owner_matches || !sprite_matches || !return_as_object.as_bool().unwrap_or(false)
                {
                    *get_error.borrow_mut() =
                        Some("invalid generation-qualified Get arguments".to_owned());
                    return flash_owned_test_error(
                        "invalid-arguments",
                        "invalid generation-qualified Get arguments",
                    );
                }
                if !get_replace.get() {
                    get_successful_generation.set(Some(requested_generation));
                }
                if get_replace.get() {
                    get_replace.set(false);
                    let invalidated = get_capability
                        .invalidate_flash_instance_generation(1.0, requested_generation)
                        .unwrap_or(false);
                    let fresh = get_capability
                        .reserve_flash_instance_generation(1.0)
                        .unwrap_or(0.0);
                    get_generation.set(fresh);
                    if !invalidated || fresh <= requested_generation {
                        *get_error.borrow_mut() = Some(format!(
                            "replacement did not advance generation (invalidated={invalidated}, old={requested_generation}, fresh={fresh})"
                        ));
                    }
                }
                flash_owned_test_response(
                    requested_generation,
                    JsValue::from_str("stale-or-current-value"),
                )
            },
        )
            as Box<dyn FnMut(JsValue, JsValue, JsValue, JsValue, JsValue) -> JsValue>);

        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("dirplayer_ruffleGetVariableOwnedForBinding"),
            bind_response.as_ref(),
        )
        .map_err(|error| format!("could not install BindGet fixture route: {error:?}"))?;
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("dirplayer_ruffleGetVariableOwnedAtGeneration"),
            get_response.as_ref(),
        )
        .map_err(|error| format!("could not install generation Get fixture route: {error:?}"))?;

        self.eval("put getVariable(sprite(1), \"_root.fixture\", 0) into bound")
            .await
            .map_err(|error| format!("BindGet evaluator turn failed: {error:?}"))?;
        let initial_generation = bound_generation
            .get()
            .ok_or_else(|| "BindGet did not capture a current sprite generation".to_owned())?;
        let current_value = self
            .eval_datum("bound.value")
            .await
            .map_err(|error| format!("generation-qualified Get failed: {error:?}"))?;
        if !matches!(
            &current_value,
            crate::director::static_datum::StaticDatum::String(value)
                if value == "stale-or-current-value"
        ) {
            return Err(format!(
                "unexpected generation-qualified Get value: {current_value:?}"
            ));
        }
        if let Some(error) = route_error.borrow_mut().take() {
            return Err(error);
        }
        if successful_get_generation.get() != Some(initial_generation) {
            return Err(format!(
                "generation-qualified Get used {:?}, expected {initial_generation}",
                successful_get_generation.get()
            ));
        }
        let calls_before_replacement = calls.borrow().clone();
        if calls_before_replacement.len() != 2
            || !calls_before_replacement[0].starts_with("bind:")
            || !calls_before_replacement[1].starts_with("get:")
            || !calls_before_replacement[1].ends_with(&format!(":{initial_generation}"))
        {
            return Err(format!(
                "unexpected BindGet/Get transport sequence: {calls_before_replacement:?}"
            ));
        }

        replace_on_get.set(true);
        let stale_result = self.eval_datum("bound.value").await;
        if stale_result.is_ok() {
            return Err("stale generation-qualified Get completed successfully".to_owned());
        }
        let fresh_generation = replacement_generation.get();
        if fresh_generation <= initial_generation
            || !assertion_capability
                .is_flash_instance_generation_current(1.0, fresh_generation)
                .map_err(|error| format!("fresh generation query failed: {error:?}"))?
        {
            return Err("replacement generation was not left current".to_owned());
        }
        if let Some(error) = route_error.borrow_mut().take() {
            return Err(error);
        }

        self.eval("put getVariable(sprite(1), \"_root.fixture\", 0) into replacement")
            .await
            .map_err(|error| format!("replacement BindGet failed: {error:?}"))?;
        let replacement_value = self
            .eval_datum("replacement.value")
            .await
            .map_err(|error| format!("replacement Get failed: {error:?}"))?;
        if !matches!(
            &replacement_value,
            crate::director::static_datum::StaticDatum::String(value)
                if value == "stale-or-current-value"
        ) {
            return Err(format!(
                "unexpected replacement Get value: {replacement_value:?}"
            ));
        }
        if let Some(error) = route_error.borrow_mut().take() {
            return Err(error);
        }
        if successful_get_generation.get() != Some(fresh_generation) {
            return Err(format!(
                "replacement Get used {:?}, expected {fresh_generation}",
                successful_get_generation.get()
            ));
        }
        let calls_after_replacement = calls.borrow().clone();
        if calls_after_replacement.len() != 5
            || calls_after_replacement
                .iter()
                .filter(|call| call.starts_with("bind:"))
                .count()
                != 2
            || calls_after_replacement
                .iter()
                .filter(|call| call.starts_with("get:"))
                .count()
                != 3
        {
            return Err(format!(
                "unexpected replacement transport sequence: {calls_after_replacement:?}"
            ));
        }
        // Restore the production bridge before the callback closures are
        // released, so a reentrant browser task can never observe a dropped
        // fixture closure behind the restored global.
        drop(_routes);
        Ok(())
    }

    async fn dispatch_sysmenu_global(
        &self,
        name: &str,
        args: Vec<crate::player::DatumRef>,
    ) -> Result<crate::player::DatumRef, String> {
        let symbol = self
            .harness_runtime()
            .with_context(|mut context| context.symbols.intern(name))
            .ok_or_else(|| "SysMenu test player is stale before dispatch".to_owned())?;
        let session = self.harness_runtime().session();
        let player_id = self.harness_runtime().player_id();
        let dispatch = session
            .borrow_mut()
            .dispatch_global(player_id, &symbol, &args)
            .map_err(|error| error.message)?;
        let intent = match dispatch {
            crate::player::driver::GlobalDispatch::PendingRequest {
                request: crate::player::driver::InternalVmRequest::XtraPending(intent),
                ..
            } => intent,
            crate::player::driver::GlobalDispatch::SyncResult(result) => {
                return result.map_err(|error| error.message);
            }
            _ => {
                return Err(format!(
                    "SysMenu global {name} did not produce a typed host intent"
                ));
            }
        };
        crate::player::xtra::manager::execute_pending_intent(&session, player_id, intent)
            .await
            .map_err(|error| error.message)
    }

    async fn dispatch_budapi_global(
        &self,
        name: &str,
        args: Vec<crate::player::DatumRef>,
    ) -> Result<crate::player::DatumRef, String> {
        let symbol = self
            .harness_runtime()
            .with_context(|mut context| context.symbols.intern(name))
            .ok_or_else(|| "BudAPI test player is stale before dispatch".to_owned())?;
        let session = self.harness_runtime().session();
        let player_id = self.harness_runtime().player_id();
        let dispatch = session
            .borrow_mut()
            .dispatch_global(player_id, &symbol, &args)
            .map_err(|error| error.message)?;
        let intent = match dispatch {
            crate::player::driver::GlobalDispatch::PendingRequest {
                request: crate::player::driver::InternalVmRequest::XtraPending(intent),
                ..
            } => intent,
            crate::player::driver::GlobalDispatch::SyncResult(result) => {
                return result.map_err(|error| error.message);
            }
            _ => {
                return Err(format!(
                    "BudAPI global {name} did not produce a typed host intent"
                ));
            }
        };
        crate::player::xtra::manager::execute_pending_intent(&session, player_id, intent)
            .await
            .map_err(|error| error.message)
    }

    /// Run BudAPI through the real global-dispatch and browser host boundary.
    /// The mocked alert re-enters reset, proving the host call is outside the
    /// VM borrow and the post-host owner fence rejects the late result.
    pub async fn test_budapi_host_effects(&mut self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let original_alert = js_sys::Reflect::get(&window, &JsValue::from_str("alert"))
            .map_err(|_| "browser alert is unavailable".to_owned())?;
        let session = self.harness_runtime().session();
        let player_id = self.harness_runtime().player_id();
        let owner = self.harness_runtime().owner().clone();
        let reset_window = window.clone();
        let reset_closure = Closure::wrap(Box::new(move || {
            let succeeded = session
                .borrow_mut()
                .reset_player_owned(player_id, &owner)
                .is_ok();
            let _ = js_sys::Reflect::set(
                &reset_window,
                &JsValue::from_str("__budapiResetObserved"),
                &JsValue::from_bool(succeeded),
            );
        }) as Box<dyn FnMut()>);
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__budapiResetDuringAlert"),
            reset_closure.as_ref(),
        )
        .map_err(|_| "could not install BudAPI reset hook".to_owned())?;
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__budapiResetObserved"),
            &JsValue::FALSE,
        )
        .map_err(|_| "could not initialize BudAPI reset marker".to_owned())?;

        let alert_window = window.clone();
        let alert_closure = Closure::wrap(Box::new(move |_value: JsValue| -> JsValue {
            if let Ok(callback) = js_sys::Reflect::get(
                &alert_window,
                &JsValue::from_str("__budapiResetDuringAlert"),
            ) {
                if let Ok(callback) = callback.dyn_into::<js_sys::Function>() {
                    let _ = callback.call0(&alert_window);
                }
            }
            JsValue::UNDEFINED
        }) as Box<dyn FnMut(JsValue) -> JsValue>);
        js_sys::Reflect::set(&window, &JsValue::from_str("alert"), alert_closure.as_ref())
            .map_err(|_| "could not install BudAPI alert hook".to_owned())?;

        let args = self
            .harness_runtime()
            .with_context(|mut context| {
                vec![
                    context
                        .player
                        .alloc_datum(crate::director::lingo::datum::Datum::String(
                            "BudAPI fixture".to_owned(),
                        )),
                ]
            })
            .ok_or_else(|| "BudAPI player disappeared before dispatch".to_owned())?;
        let result = self.dispatch_budapi_global("baMsgBox", args).await;
        let reset_observed =
            js_sys::Reflect::get(&window, &JsValue::from_str("__budapiResetObserved"))
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);

        let _ = js_sys::Reflect::set(&window, &JsValue::from_str("alert"), &original_alert);
        let _ = js_sys::Reflect::delete_property(
            &window,
            &JsValue::from_str("__budapiResetDuringAlert"),
        );
        let _ =
            js_sys::Reflect::delete_property(&window, &JsValue::from_str("__budapiResetObserved"));
        if !self.harness_runtime().owner_valid() {
            self.reset_player().await;
        }
        if result.is_ok() {
            return Err("BudAPI message box completed after owner reset".to_owned());
        }
        if !reset_observed {
            return Err("BudAPI alert did not re-enter the owner reset path".to_owned());
        }

        // Exercise a rejected clipboard promise while the host callback resets
        // the owner.  The executor must catch the rejection, then reject the
        // stale completion rather than mutating the replacement state.
        let navigator = window.navigator();
        let original_clipboard =
            js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("clipboard"))
                .map_err(|_| "could not inspect browser clipboard capability".to_owned())?;
        let mut clipboard_replaced = false;
        let clipboard = if original_clipboard.is_null() || original_clipboard.is_undefined() {
            let fake = js_sys::Object::new();
            let define = js_sys::Function::new_with_args(
                "target, value",
                "Object.defineProperty(target, 'clipboard', { configurable: true, writable: true, value: value });",
            );
            define
                .call2(&JsValue::UNDEFINED, navigator.as_ref(), &fake)
                .map_err(|_| "browser clipboard capability cannot be masked".to_owned())?;
            clipboard_replaced = true;
            fake.into()
        } else {
            original_clipboard.clone()
        };
        let original_write = js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText"))
            .map_err(|_| "could not inspect browser clipboard writer".to_owned())?;
        let mut clipboard_reset: Option<Closure<dyn FnMut()>> = None;
        let mut rejection_write = None;
        let throwing_write =
            js_sys::Function::new_no_args("throw new Error('clipboard fixture throw')");
        if !js_sys::Reflect::set(&clipboard, &JsValue::from_str("writeText"), &throwing_write)
            .map_err(|_| "could not install throwing clipboard writer".to_owned())?
        {
            return Err("browser clipboard writer cannot be masked".to_owned());
        }
        let throwing_copy_args = self
            .harness_runtime()
            .with_context(|mut context| {
                vec![
                    context
                        .player
                        .alloc_datum(crate::director::lingo::datum::Datum::String(
                            "synchronous clipboard throw".to_owned(),
                        )),
                ]
            })
            .ok_or_else(|| {
                "BudAPI player disappeared before synchronous clipboard dispatch".to_owned()
            })?;
        if self
            .dispatch_budapi_global("baCopyText", throwing_copy_args)
            .await
            .is_err()
        {
            return Err("synchronous clipboard throw escaped the owner fallback".to_owned());
        }

        let reset_session = self.harness_runtime().session();
        let reset_player_id = self.harness_runtime().player_id();
        let reset_owner = self.harness_runtime().owner().clone();
        let reset_window = window.clone();
        let reset_closure = Closure::wrap(Box::new(move || {
            let succeeded = reset_session
                .borrow_mut()
                .reset_player_owned(reset_player_id, &reset_owner)
                .is_ok();
            let _ = js_sys::Reflect::set(
                &reset_window,
                &JsValue::from_str("__budapiClipboardResetObserved"),
                &JsValue::from_bool(succeeded),
            );
        }) as Box<dyn FnMut()>);
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__budapiResetDuringClipboard"),
            reset_closure.as_ref(),
        )
        .map_err(|_| "could not install BudAPI clipboard reset hook".to_owned())?;
        clipboard_reset = Some(reset_closure);
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__budapiClipboardResetObserved"),
            &JsValue::FALSE,
        )
        .map_err(|_| "could not initialize BudAPI clipboard reset marker".to_owned())?;
        let rejection_window = window.clone();
        let rejection = Closure::wrap(Box::new(move |_value: JsValue| -> js_sys::Promise {
            if let Ok(callback) = js_sys::Reflect::get(
                &rejection_window,
                &JsValue::from_str("__budapiResetDuringClipboard"),
            ) {
                if let Ok(callback) = callback.dyn_into::<js_sys::Function>() {
                    let _ = callback.call0(&rejection_window);
                }
            }
            js_sys::Promise::reject(&JsValue::from_str("clipboard fixture rejection"))
        }) as Box<dyn FnMut(JsValue) -> js_sys::Promise>);
        if !js_sys::Reflect::set(
            &clipboard,
            &JsValue::from_str("writeText"),
            rejection.as_ref(),
        )
        .map_err(|_| "could not install rejected clipboard writer".to_owned())?
        {
            return Err("browser clipboard writer cannot install rejection fixture".to_owned());
        }
        rejection_write = Some(rejection);
        {
            let rejected_copy_args =
                self.harness_runtime()
                    .with_context(|mut context| {
                        vec![context.player.alloc_datum(
                            crate::director::lingo::datum::Datum::String(
                                "rejected clipboard".to_owned(),
                            ),
                        )]
                    })
                    .ok_or_else(|| {
                        "BudAPI player disappeared before rejected clipboard dispatch".to_owned()
                    })?;
            let rejected_copy = self
                .dispatch_budapi_global("baCopyText", rejected_copy_args)
                .await;
            let clipboard_reset_observed = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__budapiClipboardResetObserved"),
            )
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
            let _ =
                js_sys::Reflect::set(&clipboard, &JsValue::from_str("writeText"), &original_write);
            drop(rejection_write);
            drop(clipboard_reset);
            let _ = js_sys::Reflect::delete_property(
                &window,
                &JsValue::from_str("__budapiResetDuringClipboard"),
            );
            let _ = js_sys::Reflect::delete_property(
                &window,
                &JsValue::from_str("__budapiClipboardResetObserved"),
            );
            if rejected_copy.is_ok() || !clipboard_reset_observed {
                return Err("rejected clipboard completion crossed the retired owner".to_owned());
            }
            self.reset_player().await;
        }
        if clipboard_replaced {
            let restore = js_sys::Function::new_with_args(
                "target, value",
                "Object.defineProperty(target, 'clipboard', { configurable: true, writable: true, value: value });",
            );
            let _ = restore.call2(&JsValue::UNDEFINED, navigator.as_ref(), &original_clipboard);
        }

        // With the real clipboard method restored (or absent), copy/paste
        // must still work through the owner-local fallback state.
        let copy_args = self
            .harness_runtime()
            .with_context(|mut context| {
                vec![
                    context
                        .player
                        .alloc_datum(crate::director::lingo::datum::Datum::String(
                            "BudAPI clipboard fixture".to_owned(),
                        )),
                ]
            })
            .ok_or_else(|| "BudAPI player disappeared before clipboard dispatch".to_owned())?;
        self.dispatch_budapi_global("baCopyText", copy_args)
            .await
            .map_err(|error| format!("BudAPI clipboard write failed: {error}"))?;
        let pasted = self
            .dispatch_budapi_global("baPasteText", Vec::new())
            .await
            .map_err(|error| format!("BudAPI clipboard read failed: {error}"))?;
        let pasted_text = self
            .harness_runtime()
            .with_context(|context| {
                context
                    .player
                    .allocator
                    .try_get_datum(&pasted)
                    .and_then(|datum| match datum {
                        crate::director::lingo::datum::Datum::String(value) => Some(value.clone()),
                        _ => None,
                    })
            })
            .flatten()
            .ok_or_else(|| "BudAPI clipboard read returned a non-string".to_owned())?;
        if pasted_text != "BudAPI clipboard fixture" {
            return Err(format!(
                "unexpected BudAPI clipboard value: {pasted_text:?}"
            ));
        }
        Ok(())
    }

    /// Run SysMenu through the production global-dispatch and owner-bound host
    /// executor. The browser hooks capture the real console/alert effects, and
    /// the alert callback synchronously resets the captured player so the
    /// executor's post-host owner fence is exercised by a real browser call.
    pub async fn test_sysmenu_host_effects(&mut self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let console = js_sys::Reflect::get(&window, &JsValue::from_str("console"))
            .map_err(|_| "browser console is unavailable".to_owned())?;
        let original_log = js_sys::Reflect::get(&console, &JsValue::from_str("log"))
            .map_err(|_| "browser console.log is unavailable".to_owned())?;
        let original_alert = js_sys::Reflect::get(&window, &JsValue::from_str("alert"))
            .map_err(|_| "browser alert is unavailable".to_owned())?;
        let events = js_sys::Array::new();

        let log_events = events.clone();
        let log_closure = Closure::wrap(Box::new(move |value: JsValue| {
            if let Some(message) = value.as_string() {
                log_events.push(&JsValue::from_str(&format!("print:{message}")));
            }
        }) as Box<dyn FnMut(JsValue)>);
        js_sys::Reflect::set(&console, &JsValue::from_str("log"), log_closure.as_ref())
            .map_err(|_| "could not install SysMenu console hook".to_owned())?;

        let reset_session = self.harness_runtime().session();
        let reset_player_id = self.harness_runtime().player_id();
        let reset_owner = self.harness_runtime().owner().clone();
        let reset_window = window.clone();
        let reset_closure = Closure::wrap(Box::new(move || {
            let succeeded = reset_session
                .borrow_mut()
                .reset_player_owned(reset_player_id, &reset_owner)
                .is_ok();
            let _ = js_sys::Reflect::set(
                &reset_window,
                &JsValue::from_str("__sysmenuResetObserved"),
                &JsValue::from_bool(succeeded),
            );
        }) as Box<dyn FnMut()>);
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__sysmenuResetDuringAlert"),
            reset_closure.as_ref(),
        )
        .map_err(|_| "could not install SysMenu reset hook".to_owned())?;
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__sysmenuResetObserved"),
            &JsValue::FALSE,
        )
        .map_err(|_| "could not initialize SysMenu reset marker".to_owned())?;

        let alert_events = events.clone();
        let alert_window = window.clone();
        let alert_closure = Closure::wrap(Box::new(move |value: JsValue| -> JsValue {
            let message = value.as_string().unwrap_or_default();
            alert_events.push(&JsValue::from_str(&format!("messagebox:{message}")));
            if let Ok(callback) = js_sys::Reflect::get(
                &alert_window,
                &JsValue::from_str("__sysmenuResetDuringAlert"),
            ) {
                if let Ok(callback) = callback.dyn_into::<js_sys::Function>() {
                    let _ = callback.call0(&alert_window);
                }
            }
            JsValue::UNDEFINED
        }) as Box<dyn FnMut(JsValue) -> JsValue>);
        js_sys::Reflect::set(&window, &JsValue::from_str("alert"), alert_closure.as_ref())
            .map_err(|_| "could not install SysMenu alert hook".to_owned())?;

        let result = async {
            let print_args =
                self.harness_runtime()
                    .with_context(|mut context| {
                        vec![context.player.alloc_datum(
                            crate::director::lingo::datum::Datum::String(
                                "print fixture".to_owned(),
                            ),
                        )]
                    })
                    .ok_or_else(|| "SysMenu player disappeared before print".to_owned())?;
            self.dispatch_sysmenu_global("sysMenuPrintMsg", print_args)
                .await
                .map_err(|error| format!("SysMenu print failed: {error}"))?;

            let message_box_args = self
                .harness_runtime()
                .with_context(|mut context| {
                    vec![
                        context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::String(
                                "message fixture".to_owned(),
                            )),
                        context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::String(
                                "caption fixture".to_owned(),
                            )),
                        context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::Int(1)),
                    ]
                })
                .ok_or_else(|| "SysMenu player disappeared before message box".to_owned())?;
            let late_result = self
                .dispatch_sysmenu_global("sysMenuMessageBox", message_box_args)
                .await;
            if late_result.is_ok() {
                return Err("SysMenu message box completed after reset".to_owned());
            }
            let reset_observed =
                js_sys::Reflect::get(&window, &JsValue::from_str("__sysmenuResetObserved"))
                    .ok()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
            if !reset_observed {
                return Err("SysMenu alert did not re-enter the owner reset path".to_owned());
            }
            let saw = |prefix: &str| {
                (0..events.length()).any(|index| {
                    events
                        .get(index)
                        .as_string()
                        .is_some_and(|event| event.starts_with(prefix))
                })
            };
            if !saw("print:[SysMenu] print fixture") {
                return Err("SysMenu print did not reach browser console.log".to_owned());
            }
            if !saw("messagebox:caption fixture\n\nmessage fixture") {
                return Err("SysMenu message box did not reach browser alert".to_owned());
            }
            Ok(())
        }
        .await;

        let _ = js_sys::Reflect::set(&console, &JsValue::from_str("log"), &original_log);
        let _ = js_sys::Reflect::set(&window, &JsValue::from_str("alert"), &original_alert);
        let _ = js_sys::Reflect::delete_property(
            &window,
            &JsValue::from_str("__sysmenuResetDuringAlert"),
        );
        let _ =
            js_sys::Reflect::delete_property(&window, &JsValue::from_str("__sysmenuResetObserved"));

        // The callback intentionally rotated the owner. Reinstall the harness
        // player so subsequent browser tests retain their normal lifecycle.
        if !self.harness_runtime().owner_valid() {
            self.reset_player().await;
        }
        result
    }

    /// Fully tear down the current dirplayer and allocate a fresh one via
    /// `DirPlayer::new()`. Called both by `BrowserTestPlayer::new()` (per-test
    /// setup) and by `load_movie()` (per-movie setup) so every movie load
    /// starts from a clean state — no leaked scopes, globals, timeouts, or
    /// cached sprite textures from a previously loaded movie.
    /// Call the JS bridge's `resolveAndLoadMovieXtras()` (exposed on `window`
    /// by the runner template) and await it. No-op when the hook is absent.
    fn install_playback_effect_fixture(
        handle: &crate::BrowserPlayerHandle,
    ) -> Result<PlaybackEffectMarkers, String> {
        handle
            .with_context(|context| {
                let prepare_movie = Symbol::builtin(BuiltInSymbol::PrepareMovie);
                let prepare_frame = Symbol::builtin(BuiltInSymbol::PrepareFrame);
                let enter_frame = Symbol::builtin(BuiltInSymbol::EnterFrame);
                let start_movie = Symbol::builtin(BuiltInSymbol::StartMovie);
                let stop_movie = Symbol::builtin(BuiltInSymbol::StopMovie);
                let end_sprite = Symbol::builtin(BuiltInSymbol::EndSprite);
                let init = context.symbols.intern("browserPlaybackInit");
                let frame = context.symbols.intern("browserPlaybackFrame");
                let stop_marker = context.symbols.intern("browserPlaybackStopMovie");
                let end_after_stop = context.symbols.intern("browserPlaybackEndSpriteAfterStop");
                // Index zero is the event symbol; the remaining entries are
                // the globals addressed by the bytecode operands below.
                let names = vec![
                    stop_movie.clone(),
                    init.clone(),
                    frame.clone(),
                    stop_marker.clone(),
                    end_after_stop.clone(),
                ];
                for marker in [&init, &frame, &stop_marker, &end_after_stop] {
                    let value = context
                        .player
                        .allocator
                        .alloc_datum(Datum::Int(0), &mut context.player.bitmap_manager)
                        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))?;
                    context.player.globals.insert(marker.clone(), value);
                }

                let chunk = || ScriptChunk {
                    script_number: 1,
                    literals: vec![],
                    handlers: vec![],
                    property_name_ids: vec![],
                    property_defaults: HashMap::new(),
                };
                let mut movie_handlers = FxHashMap::default();
                movie_handlers.insert(
                    prepare_movie.clone(),
                    playback_counter_handler(1, names.len()),
                );
                movie_handlers.insert(start_movie, playback_counter_handler(1, names.len()));
                movie_handlers.insert(prepare_frame, playback_counter_handler(2, names.len()));
                movie_handlers.insert(enter_frame, playback_counter_handler(2, names.len()));
                movie_handlers.insert(stop_movie.clone(), playback_counter_handler(3, names.len()));
                let movie_ref = CastMemberRef {
                    cast_lib: 1,
                    cast_member: 2,
                };
                let behavior_ref = CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                };
                let movie_script = Rc::new(Script {
                    member_ref: movie_ref,
                    name: "browser-playback-movie".to_owned(),
                    chunk: chunk(),
                    script_type: ScriptType::Movie,
                    handlers: movie_handlers,
                    handler_names_raw: vec![
                        "prepareMovie".to_owned(),
                        "startMovie".to_owned(),
                        "prepareFrame".to_owned(),
                        "enterFrame".to_owned(),
                        "stopMovie".to_owned(),
                    ],
                    handler_names: vec![
                        prepare_movie,
                        Symbol::builtin(BuiltInSymbol::StartMovie),
                        Symbol::builtin(BuiltInSymbol::PrepareFrame),
                        Symbol::builtin(BuiltInSymbol::EnterFrame),
                        stop_movie,
                    ],
                    properties: RefCell::new(FxHashMap::default()),
                });
                let behavior_script = Rc::new(Script {
                    member_ref: behavior_ref.clone(),
                    name: "browser-playback-behavior".to_owned(),
                    chunk: chunk(),
                    script_type: ScriptType::Score,
                    handlers: FxHashMap::from_iter([(
                        end_sprite.clone(),
                        playback_end_sprite_handler(3, 4, names.len()),
                    )]),
                    handler_names_raw: vec!["endSprite".to_owned()],
                    handler_names: vec![end_sprite],
                    properties: RefCell::new(FxHashMap::default()),
                });
                let mut cast = CastLib::test_external(1, 0);
                cast.name_symbols = Rc::from(names);
                cast.scripts.insert(1, behavior_script);
                cast.scripts.insert(2, movie_script);
                context.player.movie.cast_manager.casts.push(cast);

                let instance = context
                    .player
                    .allocator
                    .alloc_script_instance(ScriptInstance {
                        instance_id: 1,
                        script: behavior_ref,
                        ancestor: None,
                        properties: FxHashMap::default(),
                        begin_sprite_called: false,
                    });
                let mut channel = SpriteChannel::new(1);
                channel.sprite.entered = true;
                channel.sprite.script_instance_list = vec![instance];
                context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
                context.player.movie.score.invalidate_render_channel_cache();
                Ok::<_, wasm_bindgen::JsValue>(PlaybackEffectMarkers {
                    init,
                    frame,
                    stop_movie: stop_marker,
                    end_sprite_after_stop: end_after_stop,
                })
            })
            .and_then(|result| result)
            .map_err(|error| format!("playback effect fixture setup failed: {error:?}"))
    }

    /// Exercise the public handle playback boundary without a movie asset.
    /// The assertions cover the part that was previously untestable: one
    /// owner gets one cancellable loop, stop wakes it, replay claims a fresh
    /// epoch, and reset cannot cancel or clear the other handle's loop.
    pub async fn test_browser_handle_public_play(&self) -> Result<(), String> {
        let mut first = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("first handle construction failed: {error:?}"))?;
        let second = crate::BrowserPlayerHandle::new()
            .map_err(|error| format!("second handle construction failed: {error:?}"))?;
        let first_owner = first.owner.clone();
        let second_owner = second.owner.clone();
        let first_container = create_public_play_test_container("first")?;
        let second_container = create_public_play_test_container("second")?;
        first
            .create_canvas(first_container)
            .map_err(|error| format!("first renderer creation failed: {error:?}"))?;
        second
            .create_canvas(second_container)
            .map_err(|error| format!("second renderer creation failed: {error:?}"))?;
        let first_markers = Self::install_playback_effect_fixture(&first)?;
        let second_markers = Self::install_playback_effect_fixture(&second)?;

        first
            .play()
            .map_err(|error| format!("first play failed: {error:?}"))?;
        first
            .play()
            .map_err(|error| format!("duplicate first play failed: {error:?}"))?;
        second
            .play()
            .map_err(|error| format!("second play failed: {error:?}"))?;
        if !first
            .session
            .borrow()
            .playback_loop_active(first.player_id, &first_owner)
            || !second
                .session
                .borrow()
                .playback_loop_active(second.player_id, &second_owner)
        {
            return Err("public play did not install both owner loops".into());
        }

        let mut first_effects = None;
        for _ in 0..2000 {
            async_std::task::sleep(Duration::from_millis(1)).await;
            let observed_first = first
                .session
                .borrow_mut()
                .with_player(first.player_id, |context| {
                    (
                        context.player.is_playing,
                        context.player.last_initialized_frame,
                        context.player.movie.current_frame,
                        context.player.playback_init_count,
                        context.player.playback_frame_count,
                        context.player.playback_stop_count,
                    )
                })
                .ok_or_else(|| "first owner disappeared during initial playback".to_string())?;
            let observed_second = second
                .session
                .borrow_mut()
                .with_player(second.player_id, |context| {
                    (
                        context.player.is_playing,
                        context.player.last_initialized_frame,
                    )
                })
                .ok_or_else(|| "second owner disappeared during initial playback".to_string())?;
            if observed_first.0
                && observed_first.1.is_some()
                && observed_second.0
                && observed_second.1.is_some()
                && read_playback_marker(&first, &first_markers.init)? == 2
                && read_playback_marker(&first, &first_markers.frame)? > 0
                && read_playback_marker(&second, &second_markers.init)? > 0
            {
                first_effects = Some(observed_first);
                break;
            }
        }
        let first_effects = first_effects.ok_or_else(|| {
            "initial playback did not complete both owner initializations".to_string()
        })?;
        if !first_effects.0 || first_effects.1.is_none() {
            return Err(format!(
                "initial playback did not complete owned init: playing={}, initialized_frame={:?}",
                first_effects.0, first_effects.1
            ));
        }
        if read_playback_marker(&first, &first_markers.init)? != 2
            || read_playback_marker(&first, &first_markers.frame)? == 0
            || read_playback_marker(&second, &second_markers.init)? == 0
        {
            return Err(
                "owned init/frame handlers did not update the expected owner globals".into(),
            );
        }
        if read_playback_marker(&first, &first_markers.stop_movie)? != 0
            || read_playback_marker(&first, &first_markers.end_sprite_after_stop)? != 0
        {
            return Err("stop lifecycle markers were set before stop()".into());
        }

        first
            .stop()
            .map_err(|error| format!("first stop failed: {error:?}"))?;
        let stopped = first
            .session
            .borrow_mut()
            .with_player(first.player_id, |context| context.player.is_playing)
            .ok_or_else(|| "first owner disappeared after stop".to_string())?;
        if stopped {
            return Err("stop returned while the first owner was still playing".into());
        }
        // Queue the replay before yielding so the stopped epoch must retain
        // the request until its StopMovie/endSprite cleanup completes.
        first
            .play()
            .map_err(|error| format!("immediate first replay failed: {error:?}"))?;
        let mut stop_lifecycle_markers = None;
        for _ in 0..2000 {
            async_std::task::sleep(Duration::from_millis(1)).await;
            let stop_marker = read_playback_marker(&first, &first_markers.stop_movie)?;
            let end_marker = read_playback_marker(&first, &first_markers.end_sprite_after_stop)?;
            if end_marker != 0 {
                stop_lifecycle_markers = Some((stop_marker, end_marker));
                break;
            }
        }
        let (stop_marker, end_marker) = stop_lifecycle_markers.ok_or_else(|| {
            "StopMovie/endSprite handlers did not complete within the playback wait bound"
                .to_owned()
        })?;
        if stop_marker == 0 || end_marker != stop_marker {
            return Err(
                "StopMovie/endSprite handlers did not observe the real lifecycle order".into(),
            );
        }
        if read_playback_marker(&second, &second_markers.stop_movie)? != 0 {
            return Err("stopping the first owner mutated the second owner lifecycle".into());
        }
        // The second owner remains live while the first owner completes its
        // queued stop-cleanup/replay handoff.
        let mut second_survived = false;
        for _ in 0..2000 {
            async_std::task::sleep(Duration::from_millis(1)).await;
            if second
                .session
                .borrow()
                .playback_loop_active(second.player_id, &second_owner)
            {
                second_survived = true;
                break;
            }
        }
        if !second_survived {
            return Err("stopping first handle affected second owner".into());
        }

        let mut replay_effects = None;
        for _ in 0..2000 {
            async_std::task::sleep(Duration::from_millis(1)).await;
            let active = first
                .session
                .borrow()
                .playback_loop_active(first.player_id, &first.owner);
            let observed = first
                .session
                .borrow_mut()
                .with_player(first.player_id, |context| {
                    (
                        context.player.is_playing,
                        context.player.last_initialized_frame,
                        context.player.playback_init_count,
                        context.player.playback_frame_count,
                        context.player.playback_stop_count,
                    )
                })
                .ok_or_else(|| "first owner disappeared during replay".to_string())?;
            if active
                && observed.0
                && observed.1 == first_effects.1
                && observed.2 == first_effects.3
                && observed.3 > first_effects.4
                && observed.4 > first_effects.5
            {
                replay_effects = Some(observed);
                break;
            }
        }
        let replay_effects = replay_effects.ok_or_else(|| {
            "immediate replay did not claim a progressing replacement epoch".to_owned()
        })?;
        if !replay_effects.0 || replay_effects.1 != first_effects.1 {
            return Err(format!(
                "replay did not restore playback without reinitializing: playing={}, initialized_frame={:?}, before={:?}",
                replay_effects.0, replay_effects.1, first_effects.1
            ));
        }
        if replay_effects.2 != first_effects.3 {
            return Err(format!(
                "replay reinitialized the movie: init_count={} before={}",
                replay_effects.2, first_effects.3
            ));
        }
        if replay_effects.3 <= first_effects.4 {
            return Err(format!(
                "replay did not advance a frame: frame_count={} before={}",
                replay_effects.3, first_effects.4
            ));
        }
        // The initialization marker is deliberately stable at two handler
        // invocations (prepareMovie + startMovie). Frame progress is checked
        // by the production frame counter above; this proves replay did not
        // run initialization again.
        if read_playback_marker(&first, &first_markers.init)? != 2 {
            return Err("replay reran the synthetic initialization handlers".into());
        }
        if replay_effects.4 <= first_effects.5 {
            return Err(format!(
                "stop lifecycle did not run: stop_count={} before={}",
                replay_effects.4, first_effects.5
            ));
        }
        first
            .reset()
            .map_err(|error| format!("first reset failed: {error:?}"))?;
        if first_owner.is_arena_live()
            || first
                .session
                .borrow()
                .playback_loop_active(first.player_id, &first_owner)
        {
            return Err("reset left stale playback ownership alive".into());
        }
        if !second
            .session
            .borrow()
            .playback_loop_active(second.player_id, &second_owner)
        {
            return Err("reset of first handle affected second owner".into());
        }
        second
            .stop()
            .map_err(|error| format!("second stop failed: {error:?}"))?;
        Ok(())
    }

    async fn resolve_movie_xtras() {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(hook) = js_sys::Reflect::get(
            &window,
            &JsValue::from_str("dirplayer_resolveAndLoadMovieXtras"),
        ) else {
            return;
        };
        let Ok(func) = hook.dyn_into::<js_sys::Function>() else {
            return;
        };
        let Ok(ret) = func.call0(&window) else { return };
        if let Ok(promise) = ret.dyn_into::<js_sys::Promise>() {
            let _ = JsFuture::from(promise).await;
        }
    }

    async fn reset_player(&mut self) {
        self.reset_player_with(true).await
    }

    fn owner_key(&self) -> String {
        let key = self.runtime.owner().key();
        format!("{}:{}:{}", key.session, key.player, key.generation)
    }

    fn register_flash_owner(&mut self) -> Option<js_sys::Promise> {
        let flash_scripted_access_pending = self
            .runtime
            .session()
            .borrow_mut()
            .with_player(self.runtime.player_id(), |context| {
                context.player.flash_scripted_access_pending.clone()
            })
            .expect("browser test player must expose Flash readiness state");
        let capability = BrowserFlashCapability::new(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
            flash_scripted_access_pending.clone(),
            self.command_tx.clone(),
        );
        // The JS callback registration owns its own wasm-bindgen wrapper. Keep
        // a second, capability-equivalent Rust value in the harness so the
        // callback's JS lifetime is independent of the Rust test field; both
        // values share the same session/owner handles and BrowserFlashCapability
        // is non-owning, so dropping either wrapper cannot retire the player.
        let js_capability = BrowserFlashCapability::new(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
            flash_scripted_access_pending,
            self.command_tx.clone(),
        );
        let owner_key = capability.owner_identity();
        let mut registration_ready = None;
        if let Some(window) = web_sys::window() {
            if let Ok(value) =
                js_sys::Reflect::get(&window, &JsValue::from_str("dirplayer_registerFlashOwner"))
            {
                if let Ok(function) = value.dyn_into::<js_sys::Function>() {
                    let js_capability: JsValue = js_capability.into();
                    registration_ready = function
                        .call2(&window, &JsValue::from_str(&owner_key), &js_capability)
                        .ok()
                        .and_then(|value| value.dyn_into::<js_sys::Promise>().ok());
                }
            }
        }
        self.flash_capability = Some(capability);
        registration_ready
    }

    fn unregister_flash_owner(&mut self, owner_key: &str) {
        if let Some(window) = web_sys::window() {
            if let Ok(value) = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("dirplayer_unregisterFlashOwner"),
            ) {
                if let Ok(function) = value.dyn_into::<js_sys::Function>() {
                    let _ = function.call1(&window, &JsValue::from_str(owner_key));
                }
            }
        }
        self.flash_capability = None;
    }

    /// `preserve_external_params` — carry the current player's external params
    /// onto the fresh one. TRUE for the per-movie reset inside `load_movie`:
    /// tests call `cfg.apply_external_params()` BEFORE `load_movie`, so
    /// discarding them there would leave the new player without the
    /// credentials/args the test configured. FALSE at the per-test boundary
    /// (`BrowserTestPlayer::new`), so a movie's params cannot reach the next
    /// test — see the note there.
    async fn reset_player_with(&mut self, preserve_external_params: bool) {
        let preserved_external_params = if preserve_external_params {
            self.harness_runtime()
                .with_context(|context| context.player.external_params.clone())
                .unwrap_or_default()
        } else {
            Default::default()
        };
        // The projector's `--do` / `--doBefore` / `--go` payloads travel with
        // the external params, and for the same reason: a test calls
        // `cfg.apply_startup_do()` BEFORE `load_movie`, and this reset happens
        // INSIDE it. Dropping them here left `startup_do` unset by the time the
        // movie-init sequence looked for it, so the seeded member kept the
        // placeholder the .dcr shipped with and the wrapper redirected nowhere.
        let preserved_startup = if preserve_external_params {
            self.harness_runtime()
                .with_context(|context| {
                    (
                        context.player.startup_do.clone(),
                        context.player.startup_do_before.clone(),
                        context.player.startup_go,
                    )
                })
                .unwrap_or_default()
        } else {
            Default::default()
        };

        // Stop the current movie and clear all timeouts before resetting.
        // Stop every playing sound too: the old player is about to be dropped,
        // but dropping an `Rc<AudioBufferSourceNode>` does NOT halt the node —
        // the AudioContext keeps it running until it ends, so a looping sound
        // from the previous movie would keep playing over the next test.
        self.harness_runtime().with_context(|context| {
            context.player.stop();
            context.player.sound_manager.stop_all();
            context.player.timeout_manager.clear();
            context.player.bitmap_manager.clear_movie_bitmaps();
        });
        crate::js_api::JsApi::dispatch_clear_timeouts();
        // Tear down every Ruffle/Flash instance from the previous movie so its
        // per-frame capture RAF loop, Ruffle player, and SWF audio don't leak
        // across the movie switch (the per-sprite unload path only fires for
        // sprites the frame actually changed).
        let key = self.runtime.owner().key();
        let owner_key = format!("{}:{}:{}", key.session, key.player, key.generation);
        crate::js_api::JsApi::dispatch_flash_reset_all(&owner_key);
        self.unregister_flash_owner(&owner_key);
        // Retire the owner before yielding so no in-flight handler can resume
        // against the replacement player. Keep the original RAF drain: these
        // yields let stale browser tasks observe retirement and exit without
        // advancing Director simulation during teardown.
        let had_active_handler = self
            .harness_runtime()
            .with_context(|context| context.player.handler_stack_depth != 0)
            .unwrap_or(false);
        self.runtime.retire_current();
        if had_active_handler {
            for _ in 0..120 {
                Self::next_frame().await;
            }
        }
        for _ in 0..4 {
            Self::next_frame().await;
        }

        // Dispose the test-owned renderer before replacing the player. This
        // removes its canvas/listeners and stops its owner-checked RAF loop;
        // no process-global renderer is shared across movie resets.
        crate::rendering::dispose_renderer_state(&self.renderer);
        self.renderer = crate::rendering::new_renderer_state();

        let (tx, rx) = async_std::channel::unbounded();
        assert!(self.runtime.install_player(tx.clone()));
        self.command_tx = tx;
        self.runtime
            .session()
            .borrow_mut()
            .bind_renderer_state(self.runtime.player_id(), &self.renderer);
        let command_session = self.runtime.session();
        let command_player_id = self.runtime.player_id();
        let command_owner = self.runtime.owner().clone();
        crate::player::spawn_player_local(async move {
            crate::player::commands::run_command_loop(
                rx,
                command_session,
                command_player_id,
                command_owner,
            )
            .await;
        });
        if let Some(registration_ready) = self.register_flash_owner() {
            let _ = JsFuture::from(registration_ready).await;
        }

        // Init logger (normally done by init_player which we skip in test mode)
        let _ = console_log::init_with_level(log::Level::Warn);

        // Restore external_params on the freshly created player so any
        // `cfg.apply_external_params()` call the test made before `load_movie`
        // is preserved across the reset.
        if !preserved_external_params.is_empty() {
            self.harness_runtime().with_context(|context| {
                context.player.external_params = preserved_external_params;
            });
        }
        let (startup_do, startup_do_before, startup_go) = preserved_startup;
        if startup_do.is_some() || startup_do_before.is_some() || startup_go.is_some() {
            self.harness_runtime().with_context(|context| {
                context.player.startup_do = startup_do;
                context.player.startup_do_before = startup_do_before;
                context.player.startup_go = startup_go;
            });
        }

        // Load the system font (required for text rendering). Decode happens
        // outside the session borrow; installation remains bound to this
        // runtime's captured owner generation.
        let font_session = self.runtime.session();
        let font_player_id = self.runtime.player_id();
        let font_owner = self.runtime.owner().clone();
        let _ = crate::player::font::player_load_system_font_owned(
            font_session,
            font_player_id,
            font_owner,
            "/assets/charmap-system.png".to_owned(),
        )
        .await;
    }

    /// Wait for the next animation frame.
    async fn next_frame() {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window()
                .unwrap()
                .request_animation_frame(&resolve)
                .unwrap();
        });
        let _ = JsFuture::from(promise).await;
    }

    /// Sleep for the given number of milliseconds.
    async fn sleep_ms(ms: u32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32)
                .unwrap();
        });
        let _ = JsFuture::from(promise).await;
    }

    /// Ensure a renderer exists, creating one if needed.
    fn ensure_renderer(&self) {
        let window = web_sys::window().expect("browser window is required");
        let document = window.document().expect("browser document is required");
        let container = document
            .get_element_by_id("stage_canvas_container")
            .expect("stage container is required")
            .dyn_into::<web_sys::HtmlElement>()
            .expect("stage container must be an HTML element");
        if crate::rendering::renderer_is_created(&self.renderer) {
            return;
        }
        crate::rendering::player_create_canvas_for_handle(
            &self.renderer,
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
            &container,
        )
        .unwrap_or_else(|error| panic!("test renderer creation failed: {:?}", error));
    }

    /// Fetch and register an external Xtra plugin .wasm. Call this from a
    /// test BEFORE `load_movie` so plugin-using Lingo (`new(xtra "...")`,
    /// `the xtraList`, instance handlers) finds the xtra registered.
    /// Returns the registered xtra name.
    pub async fn load_external_xtra(&self, url: &str) -> Result<String, String> {
        crate::player::xtra::external::load_for_test(url).await
    }

    /// Establish one real browser Multiuser connection through the same
    /// prepare/execute/install path used by production Xtra dispatch. The
    /// socket executor runs outside the session borrow; only the instance
    /// allocation and final installation use short owner-checked borrows.
    async fn connect_multiuser(
        &self,
        base_url: &str,
        label: &str,
    ) -> Result<MultiuserSocketProbe, String> {
        let url = web_sys::Url::new(&format!("{}/{}", base_url.trim_end_matches('/'), label))
            .map_err(|_| "invalid Multiuser test WebSocket URL".to_owned())?;
        let host = url.hostname();
        let port = if url.port().is_empty() {
            if url.protocol() == "wss:" { 443 } else { 80 }
        } else {
            url.port()
                .parse::<i32>()
                .map_err(|_| "invalid Multiuser test port".to_owned())?
        };
        let path = {
            let mut path = url.pathname();
            let search = url.search();
            if !search.is_empty() {
                path.push_str(&search);
            }
            path
        };
        let (owner, instance_id, generation) = self
            .harness_runtime()
            .with_context(|context| {
                let owner = context.player.owner.clone();
                let instance_id = context
                    .player
                    .xtra_manager_state
                    .multiuser
                    .create_instance(&Vec::new());
                let generation = context
                    .player
                    .xtra_manager_state
                    .multiuser
                    .instance_generation(instance_id)
                    .expect("new Multiuser instance must have a generation");
                (owner, instance_id, generation)
            })
            .ok_or_else(|| "Multiuser test player is stale".to_owned())?;
        let request = crate::player::xtra::manager::MultiuserConnectRequest {
            username: label.to_owned(),
            password: String::new(),
            host,
            port,
            movie_id: "browser-lifecycle".to_owned(),
            // Text mode lets the fixture send a deterministic text message
            // back to B, which the production Multiuser event command pumps
            // into B's message queue.
            mode: 1,
            encryption_key: String::new(),
            websocket_path: Some(path),
            websocket_ssl: Some(url.protocol() == "wss:"),
        };
        let intent = crate::player::xtra::manager::XtraPendingIntent::MultiuserConnect {
            owner: owner.clone(),
            instance_id,
            generation,
            request,
        };
        let session = self.harness_runtime().session();
        let player_id = self.harness_runtime().player_id();
        crate::player::xtra::manager::execute_pending_intent(&session, player_id, intent)
            .await
            .map_err(|error| error.message)?;
        let sender = self
            .harness_runtime()
            .with_context(|context| {
                context
                    .player
                    .xtra_manager_state
                    .multiuser
                    .instances
                    .get(&instance_id)
                    .and_then(|instance| instance.socket_tx.clone())
            })
            .flatten()
            .ok_or_else(|| "Multiuser install did not retain its socket sender".to_owned())?;
        Ok(MultiuserSocketProbe {
            sender,
            owner,
            instance_id,
            generation,
        })
    }

    fn break_multiuser_connection(&self, instance_id: u32) -> Result<(), String> {
        self.harness_runtime()
            .with_context(|mut context| {
                context.player.with_xtra_manager_state(|state, player| {
                    state.multiuser.call_instance_handler_explicit(
                        player,
                        context.symbols,
                        instance_id,
                        "breakConnection",
                        &[],
                    )
                })
            })
            .ok_or_else(|| "Multiuser test player is stale".to_owned())?
            .map(|_| ())
            .map_err(|error| error.message)
    }

    async fn inject_stale_multiuser_event(
        &self,
        probe: &MultiuserSocketProbe,
    ) -> Result<(), String> {
        let (future, completer) = ManualFuture::new();
        self.command_tx
            .send(crate::player::PlayerVMExecutionItem {
                command: PlayerVMCommand::MultiuserSocketEvent {
                    owner: probe.owner.clone(),
                    instance_id: probe.instance_id,
                    generation: probe.generation,
                    event: crate::player::xtra::multiuser::MultiuserSocketEvent::Message(
                        b"stale-generation".to_vec(),
                    ),
                },
                completer: Some(completer),
            })
            .await
            .map_err(|_| "Multiuser command loop stopped".to_owned())?;
        future.await.map(|_| ()).map_err(|error| error.message)
    }

    async fn multiuser_state(base_url: &str) -> Result<MultiuserServerState, String> {
        let http_url = base_url
            .strip_prefix("ws://")
            .map(|rest| format!("http://{rest}/state"))
            .or_else(|| {
                base_url
                    .strip_prefix("wss://")
                    .map(|rest| format!("https://{rest}/state"))
            })
            .ok_or_else(|| "invalid Multiuser test state URL".to_owned())?;
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&http_url))
            .await
            .map_err(|_| "Multiuser test state request failed".to_owned())?;
        let response: web_sys::Response = response
            .dyn_into()
            .map_err(|_| "Multiuser test state response was not a Response".to_owned())?;
        let body = wasm_bindgen_futures::JsFuture::from(
            response
                .text()
                .map_err(|_| "Multiuser test state body failed".to_owned())?,
        )
        .await
        .map_err(|_| "Multiuser test state body failed".to_owned())?;
        let body = body
            .as_string()
            .ok_or_else(|| "Multiuser test state was not text".to_owned())?;
        let value = js_sys::JSON::parse(&body)
            .map_err(|_| "Multiuser test state was not JSON".to_owned())?;
        let bool_field = |name: &str| -> Result<bool, String> {
            js_sys::Reflect::get(&value, &JsValue::from_str(name))
                .map_err(|_| format!("Multiuser state is missing {name}"))?
                .as_bool()
                .ok_or_else(|| format!("Multiuser state field {name} was not boolean"))
        };
        Ok(MultiuserServerState {
            opened_a: bool_field("openedA")?,
            opened_b: bool_field("openedB")?,
            closed_a: bool_field("closedA")?,
            received_stale_a: bool_field("receivedStaleA")?,
            received_survival_b: bool_field("receivedSurvivalB")?,
        })
    }

    async fn wait_for_multiuser_state(
        base_url: &str,
        predicate: impl Fn(&MultiuserServerState) -> bool,
    ) -> Result<MultiuserServerState, String> {
        for _ in 0..40 {
            let state = Self::multiuser_state(base_url).await?;
            if predicate(&state) {
                return Ok(state);
            }
            Self::sleep_ms(50).await;
        }
        Err("timed out waiting for Multiuser server state".to_owned())
    }

    fn has_multiuser_text(&self, instance_id: u32, expected: &str) -> bool {
        self.harness_runtime()
            .with_context(|context| {
                context
                    .player
                    .xtra_manager_state
                    .multiuser
                    .instances
                    .get(&instance_id)
                    .is_some_and(|instance| {
                        instance.message_queue.iter().any(|message| {
                            matches!(
                                &message.content,
                                crate::director::static_datum::StaticDatum::String(value)
                                    if value == expected
                            )
                        })
                    })
            })
            .unwrap_or(false)
    }

    async fn wait_for_multiuser_text(
        &self,
        instance_id: u32,
        expected: &str,
    ) -> Result<(), String> {
        for _ in 0..40 {
            if self.has_multiuser_text(instance_id, expected) {
                return Ok(());
            }
            Self::sleep_ms(50).await;
        }
        Err(format!(
            "timed out waiting for Multiuser message {expected}"
        ))
    }

    /// Browser-only lifecycle fixture used by the e2e harness. It keeps two
    /// owners alive, retires A through the production reset path, and checks
    /// that B remains connected while A's retained sender cannot write after
    /// its resource is closed. A second A connection then receives an event
    /// stamped with its previous generation; the owner-bound command path must
    /// reject it without changing the replacement instance.
    pub async fn test_multiuser_socket_lifecycle(&mut self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let value = js_sys::Reflect::get(&window, &JsValue::from_str("__multiuserTestWsBase"))
            .map_err(|_| "Multiuser test server URL is unavailable".to_owned())?;
        let base_url = value
            .as_string()
            .ok_or_else(|| "Multiuser test server URL is not text".to_owned())?;

        let first = self.connect_multiuser(&base_url, "A").await?;
        let mut second = BrowserTestPlayer::new().await;
        let second_socket = second.connect_multiuser(&base_url, "B").await?;
        Self::wait_for_multiuser_state(&base_url, |state| state.opened_a && state.opened_b).await?;
        self.wait_for_multiuser_text(first.instance_id, "A-server-message")
            .await?;
        second
            .wait_for_multiuser_text(second_socket.instance_id, "B-server-message")
            .await?;

        // This is the actual owner reset lifecycle, including resource
        // collection and outside-borrow teardown. No test-side drain is used.
        self.reset_player().await;
        // A closed sender is also a valid cancellation result. If the clone
        // still reaches the pump, the server state below must show no stale
        // bytes after the resource has been detached.
        let _ = first.sender.try_send(b"A-stale-after-reset".to_vec());
        second_socket
            .sender
            .try_send(b"B-survives-A-reset".to_vec())
            .map_err(|_| "B sender stopped when A reset".to_owned())?;
        second
            .wait_for_multiuser_text(second_socket.instance_id, "B-after-A-reset")
            .await?;

        // Recreate A, advance its instance generation, and submit an old
        // callback through the live owner command queue. The production event
        // guard must reject it rather than append a stale message.
        let replacement = self.connect_multiuser(&base_url, "A-replacement").await?;
        self.break_multiuser_connection(replacement.instance_id)?;
        self.inject_stale_multiuser_event(&replacement).await?;
        if self.has_multiuser_text(replacement.instance_id, "stale-generation") {
            return Err("stale-generation event reached replacement instance".to_owned());
        }

        let state = Self::wait_for_multiuser_state(&base_url, |state| {
            state.closed_a && state.received_survival_b
        })
        .await?;
        if state.received_stale_a {
            return Err("retired A sender reached the server".to_owned());
        }
        Ok(())
    }

    /// Fetch a small text response through the owner-bound FileIO openFile
    /// request and read it back through the real NetManager completion path.
    /// The fixture is served by the existing loopback Playwright server, so no
    /// movie asset or second browser service is required.
    pub async fn test_fileio_open_remote(&self) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let value = js_sys::Reflect::get(&window, &JsValue::from_str("__fileIoTestHttpBase"))
            .map_err(|_| "FileIO fixture URL is unavailable".to_owned())?;
        let base = value
            .as_string()
            .ok_or_else(|| "FileIO fixture URL is not text".to_owned())?;
        let base_url = format!("{}/", base.trim_end_matches('/'));
        let expected_url = format!("{}fileio.txt", base_url);
        let (owner, instance_id, receiver, file_name, mode) = self
            .harness_runtime()
            .with_context(|mut context| {
                context.player.net_manager.set_base_path(
                    url::Url::parse(&base_url)
                        .map_err(|_| "FileIO fixture URL is invalid".to_owned())?,
                );
                let owner = context.player.owner.clone();
                let instance_id = context
                    .player
                    .with_xtra_manager_state(|state, _| state.fileio.create_instance_explicit(&[]))
                    .map_err(|error| error.message)?;
                let receiver =
                    context
                        .player
                        .alloc_datum(crate::director::lingo::datum::Datum::XtraInstance(
                            "FileIO".to_owned(),
                            instance_id,
                        ));
                let file_name =
                    context
                        .player
                        .alloc_datum(crate::director::lingo::datum::Datum::String(
                            "fileio.txt".to_owned(),
                        ));
                let mode = context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::Int(1));
                Ok::<_, String>((owner, instance_id, receiver, file_name, mode))
            })
            .ok_or_else(|| "FileIO test player is stale".to_owned())??;
        let pending = self
            .harness_runtime()
            .with_context(|mut context| {
                crate::player::xtra::manager::call_instance_handler_pending_explicit(
                    context.player,
                    context.symbols,
                    "FileIO",
                    &receiver,
                    "openFile",
                    &[file_name, mode],
                )
            })
            .ok_or_else(|| "FileIO test player was retired before openFile".to_owned())?
            .map_err(|error| error.message)?;
        let intent = match pending {
            crate::player::xtra::manager::XtraPendingOrValue::Pending(intent) => intent,
            crate::player::xtra::manager::XtraPendingOrValue::Value(_) => {
                return Err("remote FileIO open unexpectedly completed synchronously".to_owned());
            }
        };
        if !intent.owner().same_identity(&owner) {
            return Err("FileIO request lost its owner capability".to_owned());
        }
        if let crate::player::xtra::manager::XtraPendingIntent::FileIoOpen(request) = &intent {
            if request.prepared.task.resolved_url.to_string() != expected_url {
                return Err(format!(
                    "FileIO request resolved to {}, expected {}",
                    request.prepared.task.resolved_url, expected_url
                ));
            }
        }
        crate::player::xtra::manager::execute_pending_intent(
            &self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            intent,
        )
        .await
        .map_err(|error| error.message)?;
        let result = self
            .harness_runtime()
            .with_context(|mut context| {
                crate::player::xtra::manager::call_instance_handler_explicit(
                    context.player,
                    context.symbols,
                    "FileIO",
                    &receiver,
                    "readFile",
                    &[],
                )
            })
            .ok_or_else(|| "FileIO test player was retired before readFile".to_owned())?
            .map_err(|error| error.message)?;
        let text = self
            .harness_runtime()
            .with_context(|context| {
                context
                    .player
                    .allocator
                    .try_get_datum(&result)
                    .and_then(|datum| match datum {
                        crate::director::lingo::datum::Datum::String(value) => Some(value.clone()),
                        _ => None,
                    })
            })
            .flatten()
            .ok_or_else(|| "FileIO readFile did not return text".to_owned())?;
        if text != "owner-bound fileio fixture\n" {
            return Err(format!("unexpected FileIO fixture text: {text:?}"));
        }
        Ok(())
    }
}

impl TestHarness for BrowserTestPlayer {
    fn harness_runtime(&self) -> &HarnessRuntime {
        &self.runtime
    }

    fn asset_path(&self, relative: &str) -> String {
        format!("/assets/{}", relative)
    }

    async fn init_movie(&mut self) {
        crate::player::testing_shared::log_test_action("Init movie");
        self.harness_runtime().with_context(|context| {
            context.player.is_playing = false;
        });
        crate::player::run_movie_init_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("movie initialization failed: {}", error));
    }

    async fn load_movie(&mut self, url: &str) {
        crate::player::testing_shared::log_test_action(&format!("Load: {}", url));
        let full_url = if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            let origin = web_sys::window().unwrap().location().origin().unwrap();
            format!("{}{}", origin, url)
        };

        // Allocate a brand-new DirPlayer via DirPlayer::new() so every movie
        // load starts from a clean slate — no scopes, globals, timeouts,
        // datum allocations, or cached sprite textures from a prior movie.
        self.reset_player().await;

        crate::player::load_movie_from_url_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
            full_url,
        )
        .await
        .unwrap_or_else(|error| panic!("movie load failed: {}", error));

        // Resolve the movie's XTRl declarations against the registry and load
        // the matching wasm plugins (Groove, BobbaXtra, …). The dev host does
        // this in `LoadMovie`; without it a movie that needs an external Xtra
        // dies on its first plugin call ("No built-in handler: ...").
        Self::resolve_movie_xtras().await;

        // Initialize the renderer now that the stage size is known
        self.ensure_renderer();
    }

    async fn step_frame(&mut self) -> bool {
        // Yield to the browser so host callbacks and the owned command loop can
        // make progress, then advance this owned player exactly once. The wasm
        // test entrypoint intentionally does not start init_player's global
        // loops, so RAF alone would leave playback permanently stationary.
        Self::next_frame().await;
        crate::player::fire_pending_timeouts_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("timeout dispatch failed: {}", error));
        let (is_playing, _) = crate::player::run_single_frame_owned(
            self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner().clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("frame execution failed: {}", error));
        // Fail fast: surface any Lingo script errors before the next wait/step.
        if let Some(err) = Self::take_script_error() {
            crate::player::testing_shared::log_test_action(&format!("Script error: {}", err));
            panic!("Script error: {}", err);
        }
        is_playing
    }

    // Override input methods to dispatch through the command channel
    // so they're processed by the command loop at the right time,
    // avoiding concurrent access with the frame loop.

    async fn click(&mut self, x: i32, y: i32) {
        crate::player::testing_shared::log_test_action(&format!("Click ({}, {})", x, y));
        self.harness_runtime().with_context(|context| {
            context.player.mouse_loc = (x, y);
            context.player.movie.mouse_down = true;
        });
        let _ = self
            .harness_runtime()
            .dispatch(PlayerVMCommand::MouseDown((x, y)))
            .await;
        self.step_frame().await;
        self.harness_runtime().with_context(|context| {
            context.player.mouse_loc = (x, y);
            context.player.movie.mouse_down = false;
        });
        let _ = self
            .harness_runtime()
            .dispatch(PlayerVMCommand::MouseUp((x, y)))
            .await;
    }

    async fn key_down(&mut self, key: &str, code: u16) {
        self.harness_runtime().with_context(|context| {
            context
                .player
                .keyboard_manager
                .key_down(key.to_string(), code);
        });
        let _ = self
            .harness_runtime()
            .dispatch(PlayerVMCommand::KeyDown(key.to_string(), code))
            .await;
    }

    async fn key_up(&mut self, key: &str, code: u16) {
        self.harness_runtime().with_context(|context| {
            context.player.keyboard_manager.key_up(key, code);
        });
        let _ = self
            .harness_runtime()
            .dispatch(PlayerVMCommand::KeyUp(key.to_string(), code))
            .await;
    }

    fn snapshot_stage(&self) -> SnapshotOutput {
        self.ensure_renderer();
        let draw_result = crate::rendering::draw_frame_owned(
            &self.renderer,
            &self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner(),
        );
        if let Err(error) = draw_result {
            panic!("stage render failed: {}", error);
        }

        Self::canvas_to_snapshot(&self.renderer)
    }

    async fn snapshot_sprite_isolated(
        &self,
        query: impl Into<SpriteQuery>,
    ) -> Result<SnapshotOutput, String> {
        self.ensure_renderer();

        let query = query.into();
        let sprite_num = self
            .find_sprite(&query)
            .ok_or_else(|| format!("No sprite with {} found", query))?;
        let (l, t, r, b) = self.sprite_rect(sprite_num).await?;

        let draw_result = crate::rendering::draw_sprite_isolated_owned(
            &self.renderer,
            &self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner(),
            sprite_num as i16,
        );
        draw_result.map_err(|error| error.to_string())?;

        let full = Self::canvas_to_snapshot(&self.renderer);
        // Restore the full frame so subsequent renders aren't broken
        let restore_result = crate::rendering::draw_frame_owned(
            &self.renderer,
            &self.harness_runtime().session(),
            self.harness_runtime().player_id(),
            self.harness_runtime().owner(),
        );
        restore_result.map_err(|error| error.to_string())?;

        Ok(full.crop(l, t, r, b))
    }
}

impl Drop for BrowserTestPlayer {
    fn drop(&mut self) {
        crate::rendering::dispose_renderer_state(&self.renderer);
        let owner_key = self.owner_key();
        self.unregister_flash_owner(&owner_key);
        self.runtime.retire_current();
    }
}

impl BrowserTestPlayer {
    /// Remove and return the first pending Lingo script error, if any.
    fn take_script_error() -> Option<String> {
        let window = web_sys::window()?;
        let val = js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("__scriptErrors"))
            .ok()?;
        let arr: js_sys::Array = wasm_bindgen::JsCast::dyn_into(val).ok()?;
        if arr.length() == 0 {
            return None;
        }
        arr.shift().as_string()
    }

    /// Capture the current canvas contents as a base64 PNG snapshot.
    fn canvas_to_snapshot(state: &crate::rendering::RendererStateHandle) -> SnapshotOutput {
        let data_url = crate::rendering::canvas_data_url_for_handle(state)
            .unwrap_or_else(|error| panic!("renderer snapshot failed: {:?}", error));

        let base64 = data_url
            .strip_prefix("data:image/png;base64,")
            .unwrap_or(&data_url)
            .to_string();
        SnapshotOutput::Base64Png(base64)
    }
}
