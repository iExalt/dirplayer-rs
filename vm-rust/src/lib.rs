#![allow(static_mut_ref)]
pub mod cursor;
pub mod io;
pub mod js_api;
pub mod player;
pub mod rendering;
pub mod rendering_gpu;
pub mod utils;

use async_std::{channel::{unbounded, Receiver, Sender}, task::spawn_local};
use log::{debug, warn};
use manual_future::ManualFuture;
use std::sync::atomic::{AtomicU64, Ordering};
use js_api::JsApi;
use num::ToPrimitive;
use utils::set_panic_hook;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[macro_use]
extern crate pest_derive;

pub mod director;

use player::{
    cast_lib::{cast_member_ref, CastMemberRef},
    cast_member::CastMemberType,
    commands::{player_dispatch, PlayerVMCommand},
    datum_ref::DatumId,
    init_player, reserve_player_mut, reserve_player_ref,
    score::get_sprite_at,
    ownership::OwnerToken,
    session::{ExecutionContext, PlayerId, RuntimeSession, RuntimeSessionHandle},
    symbols::symbol_table::SymbolOwner,
    PlayerVMExecutionItem,
    PLAYER_OPT,
};

use crate::{player::symbols::symbol::Symbol};

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
}

static NEXT_BROWSER_SESSION: AtomicU64 = AtomicU64::new(1);

/// Explicit browser-side capability for one player and its session-owned
/// symbol table. Host code keeps this handle and passes it to stateful entry
/// points; no entry point discovers a player through the legacy global slots.
#[wasm_bindgen]
pub struct BrowserPlayerHandle {
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    command_tx: Sender<PlayerVMExecutionItem>,
    renderer: rendering::RendererStateHandle,
    /// Kept only until the owner-bound command loop takes it.  The loop is
    /// installed by the frontend executor once the explicit command API is
    /// linked; retaining the receiver here prevents a constructor from
    /// silently dropping the queue on an incomplete host integration.
    command_rx: Option<Receiver<PlayerVMExecutionItem>>,
}

/// Non-owning Flash callback capability for browser test harnesses. It carries
/// the existing session/player/owner capability but never removes the player
/// when dropped; the harness controls retirement explicitly.
#[wasm_bindgen]
pub struct BrowserFlashCapability {
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    command_tx: Sender<PlayerVMExecutionItem>,
}

impl BrowserFlashCapability {
    pub(crate) fn new(
        session: RuntimeSessionHandle,
        player_id: PlayerId,
        owner: OwnerToken,
        command_tx: Sender<PlayerVMExecutionItem>,
    ) -> Self {
        Self { session, player_id, owner, command_tx }
    }

    fn with_context<R>(
        &self,
        mut callback: impl FnOnce(&mut ExecutionContext<'_>) -> Result<R, JsValue>,
    ) -> Result<R, JsValue> {
        let mut session = self.session.try_borrow_mut().map_err(|_| {
            JsValue::from_str("browser flash capability session is already borrowed")
        })?;
        let owner = self.owner.clone();
        session
            .with_player(self.player_id, |mut context| {
                if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                    return Err(JsValue::from_str("browser flash capability is stale"));
                }
                callback(&mut context)
            })
            .ok_or_else(|| JsValue::from_str("browser flash player is not installed"))
            .and_then(|result| result)
    }

    fn enqueue_raw(&self, command: PlayerVMCommand) -> Result<bool, JsValue> {
        if !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser flash capability is stale"));
        }
        self.command_tx
            .try_send(PlayerVMExecutionItem { command, completer: None })
            .map_err(|_| JsValue::from_str("browser player command loop stopped"))?;
        Ok(true)
    }
}

#[wasm_bindgen]
impl BrowserFlashCapability {
    pub fn owner_identity(&self) -> String {
        let key = self.owner.key();
        format!("{}:{}:{}", key.session, key.player, key.generation)
    }

    pub fn update_flash_frame(
        &self,
        sprite_num: i32,
        width: u32,
        height: u32,
        rgba_data: &[u8],
    ) -> Result<(), JsValue> {
        self.with_context(|context| {
            update_flash_frame_for_player(context.player, context.symbols, sprite_num, width, height, rgba_data)
        })
    }

    pub fn trigger_lingo_callback_on_script(
        &self,
        cast_lib: i32,
        cast_member: i32,
        handler_name: String,
        args_json: String,
        flash_cast_lib: i32,
        flash_cast_member: i32,
    ) -> Result<bool, JsValue> {
        self.enqueue_raw(PlayerVMCommand::TriggerLingoCallbackOnScriptRaw {
            cast_lib,
            cast_member,
            handler_name,
            args_json,
            flash_cast_lib,
            flash_cast_member,
        })
    }

    pub fn local_connection_send(
        &self,
        connection_name: String,
        method_name: String,
        args_json: String,
    ) -> Result<bool, JsValue> {
        self.enqueue_raw(PlayerVMCommand::TriggerLocalConnectionCallbackRaw {
            connection_name,
            method_name,
            args_json,
        })
    }

    pub fn dispatch_flash_event(
        &self,
        cast_lib: i32,
        cast_member: i32,
        body: String,
    ) -> Result<bool, JsValue> {
        self.enqueue_raw(PlayerVMCommand::DispatchFlashEventRaw { cast_lib, cast_member, body })
    }

    pub async fn dispatch_flash_lingo(&self, body: String) -> Result<bool, JsValue> {
        let trimmed = body.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(false);
        }
        crate::player::eval_lingo_command_owned(
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
            trimmed,
        )
        .await
        .map(|_| true)
        .map_err(|error| JsValue::from_str(&error.message))
    }
}

impl BrowserPlayerHandle {
    async fn dispatch_command(&self, command: PlayerVMCommand) -> Result<player::datum_ref::DatumRef, JsValue> {
        if !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser player handle is stale"));
        }
        let (future, completer) = ManualFuture::new();
        self.command_tx
            .send(PlayerVMExecutionItem { command, completer: Some(completer) })
            .await
            .map_err(|_| JsValue::from_str("browser player command loop stopped"))?;
        future.await.map_err(|error| JsValue::from_str(&error.message))
    }

    /// Start exactly one loop for the currently captured owner.  The loop
    /// receives the session and capability explicitly; using the legacy
    /// process-global command loop here would let a stale browser handle
    /// reach a replacement player with the same numeric id.
    fn start_command_loop(&mut self) {
        let Some(receiver) = self.command_rx.take() else {
            return;
        };
        let session = self.session.clone();
        let player_id = self.player_id;
        let owner = self.owner.clone();
        spawn_local(async move {
            crate::player::commands::run_command_loop(receiver, session, player_id, owner).await;
        });
    }

    fn with_context<R>(
        &self,
        callback: impl FnOnce(&mut ExecutionContext<'_>) -> R,
    ) -> Result<R, JsValue> {
        let mut session = self.session.try_borrow_mut().map_err(|_| {
            JsValue::from_str("browser player session is already borrowed")
        })?;
        let current_owner = session
            .with_player(self.player_id, |context| context.player.owner.clone())
            .ok_or_else(|| JsValue::from_str("browser player is not installed"))?;
        if !current_owner.same_identity(&self.owner) || !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser player handle is stale"));
        }
        session
            .with_player(self.player_id, |mut context| callback(&mut context))
            .ok_or_else(|| JsValue::from_str("browser player is not installed"))
    }

    pub(crate) fn session(&self) -> &RuntimeSessionHandle {
        &self.session
    }

    pub(crate) fn player_id(&self) -> PlayerId {
        self.player_id
    }

    pub(crate) fn owner(&self) -> &OwnerToken {
        &self.owner
    }
}

#[wasm_bindgen]
impl BrowserPlayerHandle {
    /// Create a new isolated browser player and capture its exact owner
    /// capability. A replacement player receives a different owner even when
    /// it reuses the same numeric id.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<BrowserPlayerHandle, JsValue> {
        let session_key = NEXT_BROWSER_SESSION.fetch_add(1, Ordering::Relaxed);
        let session = RuntimeSession::new(SymbolOwner {
            session: session_key,
            generation: 1,
        })
        .into_handle();
        let (command_tx, command_rx) = unbounded();
        let player_id = 1;
        if !session
            .borrow_mut()
            .add_player(player_id, command_tx.clone())
        {
            return Err(JsValue::from_str("failed to install browser player"));
        }
        let owner = session
            .borrow_mut()
            .with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(|| JsValue::from_str("failed to capture browser player owner"))?;
        let renderer = rendering::new_renderer_state();
        session.borrow_mut().bind_renderer_state(player_id, &renderer);
        let mut handle = Self {
            session,
            player_id,
            owner,
            command_tx,
            renderer,
            command_rx: Some(command_rx),
        };
        handle.start_command_loop();
        Ok(handle)
    }

    #[wasm_bindgen(getter)]
    pub fn player_id_value(&self) -> u32 {
        self.player_id
    }

    /// Stable owner capability identifier used only to route browser events;
    /// it is never accepted as authority by the runtime.
    pub fn owner_identity(&self) -> String {
        let key = self.owner.key();
        format!("{}:{}:{}", key.session, key.player, key.generation)
    }

    /// Transfer the queue receiver to the owner-bound frontend executor.
    /// This is one-shot for each installed player; callers must start the
    /// explicit `(session, player_id, owner)` loop before dispatching queued
    /// commands.  Keeping this transfer explicit prevents an accidental
    /// fallback to the legacy process-global command loop.
    pub(crate) fn take_command_receiver(&mut self) -> Option<Receiver<PlayerVMExecutionItem>> {
        self.command_rx.take()
    }

    pub fn play(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.play())
    }

    pub fn stop(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.stop())
    }

    /// Create and start the renderer owned by this browser player.  Renderer
    /// state is kept with the handle so another provider cannot replace this
    /// player's canvas or draw loop.
    pub fn create_canvas(&self, container: web_sys::HtmlElement) -> Result<(), JsValue> {
        rendering::player_create_canvas_for_handle(
            &self.renderer,
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
            &container,
        )
    }

    pub fn set_renderer_backend(&self, backend: String) -> Result<(), JsValue> {
        rendering::player_set_renderer_backend_for_handle(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
            &backend,
        )
    }

    pub fn renderer_backend(&self) -> Result<String, JsValue> {
        rendering::renderer_backend_for_handle(&self.renderer)
    }

    pub fn rebind_renderer_owner(&self) -> Result<(), JsValue> {
        rendering::rebind_renderer_owner(
            &self.renderer,
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
        )
        .map_err(|error| JsValue::from_str(&error.message))
    }

    pub fn draw_frame(&self) -> Result<bool, JsValue> {
        rendering::draw_frame_owned(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
        )
        .map_err(|error| JsValue::from_str(&error.message))
    }

    pub fn draw_frame_at_end(&self) -> Result<bool, JsValue> {
        rendering::draw_frame_at_end_owned(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
        )
        .map_err(|error| JsValue::from_str(&error.message))
    }

    pub fn set_preview_parent(
        &self,
        parent: Option<web_sys::HtmlElement>,
    ) -> Result<(), JsValue> {
        rendering::set_preview_parent_for_handle(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
            parent,
        )
    }

    pub fn set_preview_member_ref(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        rendering::set_preview_member_ref_for_handle(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
            CastMemberRef { cast_lib, cast_member },
        )
    }

    pub fn set_preview_font_size(&self, size: u16) -> Result<(), JsValue> {
        rendering::set_preview_font_size_for_handle(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
            size,
        )
    }

    pub fn set_debug_selected_channel(&self, channel: i16) -> Result<(), JsValue> {
        rendering::set_debug_selected_channel_for_handle(
            &self.renderer,
            &self.session,
            self.player_id,
            &self.owner,
            channel,
        )?;
        self.with_context(|context| {
            context
                .player
                .queue_player_notification(crate::player::cast_lib::PlayerNotificationKind::ChannelChanged(channel));
        })?;
        JsApi::dispatch_player_notifications(self.session.clone(), self.player_id)?;
        Ok(())
    }

    /// Read back handle-owned debug state for browser isolation checks.
    pub fn preview_state(&self) -> Result<JsValue, JsValue> {
        self.with_context(|_| ())?;
        let (member_ref, font_size, selected_channel) =
            rendering::preview_state_for_handle(&self.renderer)?;
        let state = js_sys::Object::new();
        let member = js_sys::Array::new();
        if let Some(member_ref) = member_ref {
            member.push(&JsValue::from_f64(member_ref.cast_lib as f64));
            member.push(&JsValue::from_f64(member_ref.cast_member as f64));
        }
        js_sys::Reflect::set(&state, &JsValue::from_str("member"), &member)?;
        js_sys::Reflect::set(
            &state,
            &JsValue::from_str("fontSize"),
            &font_size.map(|value| JsValue::from_f64(value as f64)).unwrap_or(JsValue::UNDEFINED),
        )?;
        js_sys::Reflect::set(
            &state,
            &JsValue::from_str("selectedChannel"),
            &selected_channel
                .map(|value| JsValue::from_f64(value as f64))
                .unwrap_or(JsValue::UNDEFINED),
        )?;
        Ok(state.into())
    }

    pub fn set_pfr_font_enabled(&self, enabled: bool) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.font_manager.pfr_enabled = enabled;
            context.player.font_manager.font_cache.clear();
        })?;
        Ok(())
    }

    pub fn pfr_font_enabled(&self) -> Result<bool, JsValue> {
        self.with_context(|context| context.player.font_manager.pfr_enabled)
    }

    pub async fn print_member_bitmap_hex(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.dispatch_command(PlayerVMCommand::PrintMemberBitmapHex(CastMemberRef { cast_lib, cast_member }))
            .await
            .map(|_| ())
    }

    pub fn print_member_sound_hex(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.with_context(|context| {
            print_member_sound_hex_for_player(context.player, cast_lib, cast_member);
        })
    }

    pub async fn play_member_sound(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.dispatch_command(PlayerVMCommand::PlayMemberSound(CastMemberRef { cast_lib, cast_member }))
            .await
            .map(|_| ())
    }

    pub fn update_flash_frame(
        &self,
        sprite_num: i32,
        width: u32,
        height: u32,
        rgba_data: &[u8],
    ) -> Result<(), JsValue> {
        self.with_context(|context| {
            update_flash_frame_for_player(context.player, context.symbols, sprite_num, width, height, rgba_data)
        })?
    }

    pub fn trigger_lingo_callback_on_script(
        &self,
        cast_lib: i32,
        cast_member: i32,
        handler_name: String,
        args_json: String,
        flash_cast_lib: i32,
        flash_cast_member: i32,
    ) -> Result<bool, JsValue> {
        let args_value = js_sys::JSON::parse(&args_json)
            .map_err(|_| JsValue::from_str("invalid Flash callback JSON"))?;
        self.with_context(|context| -> Result<bool, JsValue> {
            if !js_sys::Array::is_array(&args_value) {
                return Err(JsValue::from_str("Flash callback arguments are not an array"));
            }
            let array = js_sys::Array::from(&args_value);
            let mut args = vec![context.player.alloc_datum(director::lingo::datum::Datum::Void)];
            for item in array.iter() {
                args.push(js_value_to_datum_ref_for_context(
                    &item,
                    context.player,
                    context.symbols,
                    flash_cast_lib,
                    flash_cast_member,
                ));
            }
            context
                .player
                .queue_tx
                .try_send(PlayerVMExecutionItem {
                    command: PlayerVMCommand::TriggerLingoCallbackOnScript {
                        cast_lib,
                        cast_member,
                        handler_name: context.symbols.intern(&handler_name),
                        args,
                    },
                    completer: None,
                })
                .map_err(|_| JsValue::from_str("browser player command loop stopped"))?;
            Ok(true)
        }).and_then(|result| result)
    }

    pub fn local_connection_send(
        &self,
        connection_name: String,
        method_name: String,
        args_json: String,
    ) -> Result<bool, JsValue> {
        let args_value = js_sys::JSON::parse(&args_json)
            .map_err(|_| JsValue::from_str("invalid LocalConnection JSON"))?;
        self.with_context(|context| -> Result<bool, JsValue> {
            let Some(lc_path) = context.player.flash_lc_connections.get(&connection_name).cloned() else {
                return Ok(false);
            };
            let Some((handler_name, target)) = context
                .player
                .flash_lc_callbacks
                .get(&(lc_path, method_name))
                .cloned()
            else {
                return Ok(false);
            };
            if !js_sys::Array::is_array(&args_value) {
                return Err(JsValue::from_str("LocalConnection arguments are not an array"));
            }
            let array = js_sys::Array::from(&args_value);
            let mut args = vec![context.player.alloc_datum(director::lingo::datum::Datum::Void)];
            for item in array.iter() {
                args.push(js_value_to_datum_ref_for_context(
                    &item,
                    context.player,
                    context.symbols,
                    1,
                    1,
                ));
            }
            context
                .player
                .queue_tx
                .try_send(PlayerVMExecutionItem {
                    command: PlayerVMCommand::TriggerLocalConnectionCallback {
                        target,
                        handler_name,
                        args,
                    },
                    completer: None,
                })
                .map_err(|_| JsValue::from_str("browser player command loop stopped"))?;
            Ok(true)
        }).and_then(|result| result)
    }

    pub fn dispatch_flash_event(
        &self,
        cast_lib: i32,
        cast_member: i32,
        body: String,
    ) -> Result<bool, JsValue> {
        let Some((handler_name, raw_args)) = parse_flash_event_body(&body) else {
            return Ok(false);
        };
        self.with_context(|context| -> Result<bool, JsValue> {
            use director::lingo::datum::Datum;
            let args = raw_args
                .into_iter()
                .map(|token| {
                    let datum = if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
                        Datum::String(token[1..token.len() - 1].to_owned())
                    } else if let Some(symbol) = token.strip_prefix('#') {
                        Datum::Symbol(context.symbols.intern(symbol))
                    } else if let Ok(number) = token.parse::<i32>() {
                        Datum::Int(number)
                    } else if let Ok(number) = token.parse::<f64>() {
                        Datum::Float(number)
                    } else {
                        Datum::String(token)
                    };
                    context.player.alloc_datum(datum)
                })
                .collect();
            context
                .player
                .queue_tx
                .try_send(PlayerVMExecutionItem {
                    command: PlayerVMCommand::DispatchFlashEvent {
                        cast_lib,
                        cast_member,
                        handler_name: context.symbols.intern(&handler_name),
                        args,
                    },
                    completer: None,
                })
                .map_err(|_| JsValue::from_str("browser player command loop stopped"))?;
            Ok(true)
        }).and_then(|result| result)
    }

    pub async fn dispatch_flash_lingo(&self, body: String) -> Result<bool, JsValue> {
        let trimmed = body.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(false);
        }
        crate::player::eval_lingo_command_owned(
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
            trimmed,
        )
        .await
        .map(|_| true)
        .map_err(|error| JsValue::from_str(&error.message))
    }

    pub fn reset(&mut self) -> Result<(), JsValue> {
        // Reset is an ownership boundary, but DirPlayer::reset deliberately
        // preserves the loaded movie and host configuration while replacing
        // the allocator epoch.  Validate the captured capability before
        // changing the queue so a failed/stale reset leaves the live player
        // untouched.
        let mut session = self
            .session
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("browser player session is already borrowed"))?;
        let current_owner = session
            .with_player(self.player_id, |context| context.player.owner.clone())
            .ok_or_else(|| JsValue::from_str("browser player is not installed"))?;
        if !current_owner.same_identity(&self.owner) || !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser player handle is stale"));
        }

        let (command_tx, command_rx) = unbounded();
        let owner = session
            .reset_player_owned(self.player_id, &self.owner)
            .map_err(|error| JsValue::from_str(&error.message))?;
        let teardowns = session.take_host_teardowns();
        let queue_result = session
            .with_player(self.player_id, |context| {
                // Install the replacement queue only after the session reset
                // has validated and retired the captured owner.
                context.player.queue_tx = command_tx.clone();
            });
        if queue_result.is_none() {
            drop(session);
            drop(teardowns);
            return Err(JsValue::from_str("browser player is not installed"));
        }
        drop(session);
        drop(teardowns);
        // Close the old sender only after reset has succeeded and the player
        // has received its replacement queue.
        self.command_tx.close();
        self.owner = owner;
        self.command_tx = command_tx;
        self.command_rx = Some(command_rx);
        rendering::rebind_renderer_owner(
            &self.renderer,
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
        )
        .map_err(|error| JsValue::from_str(&error.message))?;
        self.start_command_loop();
        JsApi::dispatch_player_notifications(self.session.clone(), self.player_id)?;
        Ok(())
    }

    /// Set launch parameters on this player without consulting the legacy
    /// global player slot.  Values are prepared before the short session
    /// borrow ends, and invalid URLs are reported to the host unchanged.
    pub fn set_external_params(&self, params: js_sys::Object) -> Result<(), JsValue> {
        let mut external_params = indexmap::IndexMap::new();
        for key in js_sys::Object::keys(&params).iter() {
            let key_str = key
                .as_string()
                .ok_or_else(|| JsValue::from_str("external parameter key is not a string"))?;
            let value = js_sys::Reflect::get(&params, &key)
                .map_err(|_| JsValue::from_str("failed to read external parameter"))?
                .as_string()
                .ok_or_else(|| JsValue::from_str("external parameter value is not a string"))?;
            external_params.insert(key_str, value);
        }
        self.with_context(|context| {
            context.player.external_params = external_params;
            crate::player::stage::apply_stage_draw_rect(context.player);
            let (width, height) = crate::player::stage::stage_canvas_dims(context.player);
            JsApi::dispatch_stage_size_changed(width, height, context.player.center_stage);
        })?;
        Ok(())
    }

    pub fn set_base_path(&self, path: String) -> Result<(), JsValue> {
        let url = url::Url::parse(&path)
            .map_err(|error| JsValue::from_str(&format!("invalid base path URL '{}': {}", path, error)))?;
        self.with_context(|context| context.player.net_manager.set_base_path(url))
    }

    pub fn set_startup_do(&self, code: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.startup_do = (!code.is_empty()).then_some(code);
        })?;
        Ok(())
    }

    pub fn set_startup_do_before(&self, code: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.startup_do_before = (!code.is_empty()).then_some(code);
        })?;
        Ok(())
    }

    pub fn set_startup_go(&self, frame: u32) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.startup_go = (frame != 0).then_some(frame);
        })
    }

    pub fn set_movie_path_override(&self, path: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.movie_path_override = (!path.is_empty()).then_some(path);
        })
    }

    pub fn set_movie_path_label(&self, path: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.movie_path_label = (!path.is_empty()).then_some(path);
        })
    }

    pub fn set_stage_size(&self, width: u32, height: u32) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.stage_size = (width, height);
            crate::player::stage::apply_stage_draw_rect(context.player);
            let (width, height) = crate::player::stage::stage_canvas_dims(context.player);
            JsApi::dispatch_stage_size_changed(width, height, context.player.center_stage);
        })
    }

    /// Read the per-player stage size for owner-isolation checks and browser
    /// host diagnostics. The value is read through the captured session and
    /// therefore cannot observe a replacement player with the same id.
    pub fn stage_size(&self) -> Result<js_sys::Array, JsValue> {
        self.with_context(|context| {
            let (width, height) = context.player.stage_size;
            js_sys::Array::of2(&JsValue::from(width), &JsValue::from(height))
        })
    }

    fn input_movie_loc(&self, x: f64, y: f64) -> Result<(i32, i32), JsValue> {
        self.with_context(|context| {
            if context.player.wants_pointer_lock {
                return context.player.mouse_loc;
            }
            let (mx, my) = crate::player::stage::canvas_to_movie_coords(context.player, x, y);
            (mx.to_i32().unwrap_or(0), my.to_i32().unwrap_or(0))
        })
    }

    /// Route a left-button/move command to the topmost linked movie after the
    /// parent has received it.  Pointer lock deliberately keeps input on the
    /// parent movie, matching the existing host behavior.
    fn route_nested_pointer(
        &self,
        loc: (i32, i32),
        command: PlayerVMCommand,
    ) -> Result<(), JsValue> {
        if self.with_context(|context| context.player.wants_pointer_lock)? {
            return Ok(());
        }
        let Some((member_ref, channel_num)) = crate::player::nested::nested_hit_target_owned(
            &self.session,
            self.player_id,
            &self.owner,
            loc.0,
            loc.1,
        ) else {
            return Ok(());
        };
        crate::player::nested::enqueue_nested_pointer_command_owned(
            &self.session,
            self.player_id,
            &self.owner,
            member_ref,
            channel_num,
            loc.0,
            loc.1,
            command,
        )
        .map(|_| ())
        .map_err(|error| JsValue::from_str(&error.message))
    }

    fn route_nested_key(&self, key: &str, code: u16, is_down: bool) -> Result<(), JsValue> {
        crate::player::nested::enqueue_nested_key_command_owned(
            &self.session,
            self.player_id,
            &self.owner,
            key,
            code,
            is_down,
        )
        .map(|_| ())
        .map_err(|error| JsValue::from_str(&error.message))
    }

    pub fn mouse_down(&self, x: f64, y: f64) -> Result<(), JsValue> {
        let loc = self.input_movie_loc(x, y)?;
        self.with_context(|context| {
            context.player.mouse_loc = loc;
            context.player.movie.mouse_down = true;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::MouseDown(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        self.route_nested_pointer(loc, PlayerVMCommand::MouseDown(loc))?;
        Ok(())
    }

    pub fn mouse_up(&self, x: f64, y: f64) -> Result<(), JsValue> {
        let loc = self.input_movie_loc(x, y)?;
        self.with_context(|context| {
            context.player.mouse_loc = loc;
            context.player.movie.mouse_down = false;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::MouseUp(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        self.route_nested_pointer(loc, PlayerVMCommand::MouseUp(loc))?;
        Ok(())
    }

    pub fn right_mouse_down(&self, x: f64, y: f64) -> Result<(), JsValue> {
        let loc = self.input_movie_loc(x, y)?;
        self.with_context(|context| {
            context.player.mouse_loc = loc;
            context.player.movie.right_mouse_down = true;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::RightMouseDown(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        Ok(())
    }

    pub fn right_mouse_up(&self, x: f64, y: f64) -> Result<(), JsValue> {
        let loc = self.input_movie_loc(x, y)?;
        self.with_context(|context| {
            context.player.mouse_loc = loc;
            context.player.movie.right_mouse_down = false;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::RightMouseUp(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        Ok(())
    }

    pub fn mouse_move(&self, x: f64, y: f64) -> Result<(), JsValue> {
        let loc = self.input_movie_loc(x, y)?;
        self.with_context(|context| {
            context.player.mouse_loc = loc;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::MouseMove(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        self.route_nested_pointer(loc, PlayerVMCommand::MouseMove(loc))?;
        Ok(())
    }

    pub fn mouse_move_delta(&self, dx: f64, dy: f64) -> Result<(), JsValue> {
        let dx = dx.to_i32().unwrap_or(0);
        let dy = dy.to_i32().unwrap_or(0);
        self.with_context(|context| {
            context.player.mouse_loc.0 += dx;
            context.player.mouse_loc.1 += dy;
            let loc = context.player.mouse_loc;
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::MouseMove(loc),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        let loc = self.with_context(|context| context.player.mouse_loc)?;
        self.route_nested_pointer(loc, PlayerVMCommand::MouseMove(loc))?;
        Ok(())
    }

    pub fn key_down(&self, key: String, code: u16) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.keyboard_manager.key_down(key.clone(), code);
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::KeyDown(key.clone(), code),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        self.route_nested_key(&key, code, true)?;
        Ok(())
    }

    pub fn key_up(&self, key: String, code: u16) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.keyboard_manager.key_up(&key, code);
            context.player.queue_tx.try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::KeyUp(key.clone(), code),
                completer: None,
            }).map_err(|_| JsValue::from_str("browser player command loop stopped"))
        })?;
        self.route_nested_key(&key, code, false)?;
        Ok(())
    }

    pub fn wants_pointer_lock(&self) -> Result<bool, JsValue> {
        self.with_context(|context| context.player.wants_pointer_lock)
    }

    pub fn set_picking_mode(&self, enabled: bool) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.picking_mode = enabled;
        })
    }

    pub fn get_sprite_at(&self, x: f64, y: f64) -> Result<i32, JsValue> {
        self.with_context(|context| {
            let (mx, my) = crate::player::stage::canvas_to_movie_coords(context.player, x, y);
            crate::player::score::get_sprite_at(context.player, mx as i32, my as i32, false)
                .map(|n| n as i32)
                .unwrap_or(0)
        })
    }

    pub fn is_sprite_editable_field(&self, sprite_id: i32) -> Result<bool, JsValue> {
        self.with_context(|context| {
            let sprite = context.player.movie.score.get_sprite(sprite_id as i16);
            let member = sprite
                .and_then(|sprite| sprite.member.as_ref())
                .and_then(|member| context.player.movie.cast_manager.find_member_by_ref(member));
            member.is_some_and(|member| matches!(
                &member.member_type,
                CastMemberType::Field(field) if field.editable
            ) || matches!(
                &member.member_type,
                CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable)
            ))
        })
    }

    fn focused_member_editable(context: &ExecutionContext<'_>) -> Option<i16> {
        let sprite_id = context.player.keyboard_focus_sprite;
        if sprite_id < 0 { return None; }
        let sprite_id = sprite_id as i16;
        let member_ref = context.player.movie.score.get_sprite(sprite_id)?.member.as_ref()?;
        let member = context.player.movie.cast_manager.find_member_by_ref(member_ref)?;
        let editable = matches!(&member.member_type, CastMemberType::Field(field) if field.editable)
            || matches!(&member.member_type, CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable));
        editable.then_some(sprite_id)
    }

    pub fn is_field_focused(&self) -> Result<bool, JsValue> {
        self.with_context(|context| Self::focused_member_editable(context).is_some())
    }

    pub fn get_focused_field_selected_text(&self) -> Result<String, JsValue> {
        self.with_context(|context| {
            let Some(sprite_id) = Self::focused_member_editable(context) else { return String::new(); };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.as_ref()) else { return String::new(); };
            let Some(member) = context.player.movie.cast_manager.find_member_by_ref(member_ref) else { return String::new(); };
            let (text, start, end) = match &member.member_type {
                CastMemberType::Field(field) if field.editable => (&field.text, field.sel_start, field.sel_end),
                CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable) => (&text.text, text.sel_start, text.sel_end),
                _ => return String::new(),
            };
            let len = text.len() as i32;
            // Preserve both endpoints before ordering them. Selection can be
            // reversed when the user extends backwards from its anchor.
            let first = start.clamp(0, len);
            let second = end.clamp(0, len);
            let mut lo_b = first.min(second) as usize;
            let mut hi_b = first.max(second) as usize;
            while lo_b < text.len() && !text.is_char_boundary(lo_b) { lo_b += 1; }
            while hi_b < text.len() && !text.is_char_boundary(hi_b) { hi_b += 1; }
            text[lo_b..hi_b].to_owned()
        })
    }

    pub fn field_set_caret_at(&self, sprite_id: i32, canvas_x: f64, canvas_y: f64, extend: bool) -> Result<bool, JsValue> {
        let mode = if extend {
            crate::player::keyboard_events::CaretAtMode::ExtendToAnchor
        } else {
            crate::player::keyboard_events::CaretAtMode::SetAndAnchor
        };
        self.with_context(|context| {
            let (x, y) = crate::player::stage::canvas_to_movie_coords(context.player, canvas_x, canvas_y);
            crate::player::keyboard_events::set_caret_at_screen_for_player(context.player, sprite_id as i16, x as i32, y as i32, mode)
        })
    }

    pub fn field_drag_extend_to(&self, sprite_id: i32, canvas_x: f64, canvas_y: f64) -> Result<bool, JsValue> {
        self.with_context(|context| {
            let (x, y) = crate::player::stage::canvas_to_movie_coords(context.player, canvas_x, canvas_y);
            crate::player::keyboard_events::set_caret_at_screen_for_player(context.player, sprite_id as i16, x as i32, y as i32, crate::player::keyboard_events::CaretAtMode::DragExtend)
        })
    }

    pub fn field_select_word_at(&self, sprite_id: i32, canvas_x: f64, canvas_y: f64) -> Result<bool, JsValue> {
        self.with_context(|context| {
            let (x, y) = crate::player::stage::canvas_to_movie_coords(context.player, canvas_x, canvas_y);
            crate::player::keyboard_events::set_caret_at_screen_for_player(context.player, sprite_id as i16, x as i32, y as i32, crate::player::keyboard_events::CaretAtMode::SelectWord)
        })
    }

    pub fn field_select_line_at(&self, sprite_id: i32, canvas_x: f64, canvas_y: f64) -> Result<bool, JsValue> {
        self.with_context(|context| {
            let (x, y) = crate::player::stage::canvas_to_movie_coords(context.player, canvas_x, canvas_y);
            crate::player::keyboard_events::set_caret_at_screen_for_player(context.player, sprite_id as i16, x as i32, y as i32, crate::player::keyboard_events::CaretAtMode::SelectLine)
        })
    }

    pub fn field_select_all(&self) -> Result<(), JsValue> {
        self.with_context(|context| {
            let Some(sprite_id) = Self::focused_member_editable(context) else { return; };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.clone()) else { return; };
            let Some(member) = context.player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else { return; };
            let (len, start, end, anchor) = match &mut member.member_type {
                CastMemberType::Field(field) if field.editable => (field.text.len() as i32, &mut field.sel_start, &mut field.sel_end, &mut field.sel_anchor),
                CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable) => (text.text.len() as i32, &mut text.sel_start, &mut text.sel_end, &mut text.sel_anchor),
                _ => return,
            };
            *start = 0; *end = len; *anchor = 0;
            context.player.text_selection_start = 0;
            context.player.text_selection_end = len.max(0) as u16;
        })
    }

    pub fn delete_focused_field_selection(&self) -> Result<(), JsValue> {
        self.with_context(|context| {
            let Some(sprite_id) = Self::focused_member_editable(context) else { return; };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.clone()) else { return; };
            let Some(member) = context.player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else { return; };
            let (text, start, end, anchor) = match &mut member.member_type {
                CastMemberType::Field(field) if field.editable => (&mut field.text, &mut field.sel_start, &mut field.sel_end, &mut field.sel_anchor),
                CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable) => (&mut text.text, &mut text.sel_start, &mut text.sel_end, &mut text.sel_anchor),
                _ => return,
            };
            crate::player::keyboard_events::apply_text_insertion(text, start, end, anchor, "");
            context.player.text_selection_start = (*start).max(0) as u16;
            context.player.text_selection_end = (*end).max(0) as u16;
        })
    }

    pub fn set_clipboard_mirror(&self, text: String) -> Result<(), JsValue> {
        self.with_context(|context| context.player.clipboard_mirror = text)
    }

    pub fn paste_text_into_focused_field(&self, text: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            let Some(sprite_id) = Self::focused_member_editable(context) else { return; };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.clone()) else { return; };
            let Some(member) = context.player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else { return; };
            let (target, start, end, anchor) = match &mut member.member_type {
                CastMemberType::Field(field) if field.editable => (&mut field.text, &mut field.sel_start, &mut field.sel_end, &mut field.sel_anchor),
                CastMemberType::Text(text_member) if text_member.info.as_ref().is_some_and(|info| info.editable) => (&mut text_member.text, &mut text_member.sel_start, &mut text_member.sel_end, &mut text_member.sel_anchor),
                _ => return,
            };
            crate::player::keyboard_events::apply_text_insertion(target, start, end, anchor, &text);
            context.player.text_selection_start = (*start).max(0) as u16;
            context.player.text_selection_end = (*end).max(0) as u16;
        })
    }

    pub fn ime_composition_start(&self) -> Result<(), JsValue> {
        self.with_context(|context| {
            let Some(sprite_id) = Self::focused_member_editable(context) else { return; };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.clone()) else { return; };
            let Some(member) = context.player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else { return; };
            let (target, start, end, anchor) = match &mut member.member_type {
                CastMemberType::Field(field) if field.editable => (&mut field.text, &mut field.sel_start, &mut field.sel_end, &mut field.sel_anchor),
                CastMemberType::Text(text) if text.info.as_ref().is_some_and(|info| info.editable) => (&mut text.text, &mut text.sel_start, &mut text.sel_end, &mut text.sel_anchor),
                _ => return,
            };
            if *start != *end { crate::player::keyboard_events::apply_text_insertion(target, start, end, anchor, ""); }
            let pos = (*start).max(0);
            context.player.ime_composition = Some((pos, pos));
            context.player.text_selection_start = pos as u16;
            context.player.text_selection_end = pos as u16;
        })
    }

    pub fn ime_composition_update(&self, text: String) -> Result<(), JsValue> {
        self.with_context(|context| {
            let Some((start, end)) = context.player.ime_composition else { return; };
            let Some(sprite_id) = Self::focused_member_editable(context) else { return; };
            let Some(member_ref) = context.player.movie.score.get_sprite(sprite_id).and_then(|s| s.member.clone()) else { return; };
            let Some(member) = context.player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else { return; };
            let (target, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
                CastMemberType::Field(field) if field.editable => (&mut field.text, &mut field.sel_start, &mut field.sel_end, &mut field.sel_anchor),
                CastMemberType::Text(text_member) if text_member.info.as_ref().is_some_and(|info| info.editable) => (&mut text_member.text, &mut text_member.sel_start, &mut text_member.sel_end, &mut text_member.sel_anchor),
                _ => return,
            };
            let len = target.len() as i32;
            let lo = start.clamp(0, len) as usize;
            let hi = end.clamp(0, len).max(lo as i32) as usize;
            if !target.is_char_boundary(lo) || !target.is_char_boundary(hi) { return; }
            target.replace_range(lo..hi, &text);
            let new_end = lo as i32 + text.len() as i32;
            *sel_start = new_end; *sel_end = new_end; *sel_anchor = new_end;
            context.player.ime_composition = Some((start, new_end));
            context.player.text_selection_start = new_end.max(0) as u16;
            context.player.text_selection_end = new_end.max(0) as u16;
        })
    }

    pub fn ime_composition_end(&self, text: String) -> Result<(), JsValue> {
        self.ime_composition_update(text)?;
        self.with_context(|context| context.player.ime_composition = None)
    }

    /// Queue an asynchronous load on this handle's owner-bound command loop.
    /// The receiver is deliberately not serviced by the legacy global loop;
    /// the frontend executor must attach the captured session/player/owner
    /// before calling this method.
    pub async fn load_movie_file(&self, path: String, autoplay: bool) -> Result<(), JsValue> {
        self.dispatch_command(PlayerVMCommand::LoadMovieFromFile(path, autoplay)).await.map(|_| ())
    }

    pub async fn set_system_font_path(&self, path: String) -> Result<(), JsValue> {
        self.dispatch_command(PlayerVMCommand::SetSystemFontPath(path)).await.map(|_| ())
    }

    pub async fn provide_net_task_data(&self, task_id: u32, data: Vec<u8>) -> Result<(), JsValue> {
        let shared = self.with_context(|context| std::sync::Arc::clone(&context.player.net_manager.shared_state))?;
        shared.lock().await.fulfill_task(task_id, Ok(data)).await;
        Ok(())
    }

    pub async fn provide_net_task_error(&self, task_id: u32) -> Result<(), JsValue> {
        let shared = self.with_context(|context| std::sync::Arc::clone(&context.player.net_manager.shared_state))?;
        shared.lock().await.fulfill_task(task_id, Err(4)).await;
        Ok(())
    }

    pub fn mcp_list_scripts(&self, cast_lib: i32, limit: i32, offset: i32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_list_scripts(
                context.player,
                context.symbols,
                (cast_lib >= 0).then_some(cast_lib),
                (limit >= 0).then_some(limit as usize),
                (offset >= 0).then_some(offset as usize),
            )
        })
    }

    pub fn mcp_get_script(&self, cast_lib: i32, cast_member: i32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_get_script(context.player, context.symbols, cast_lib, cast_member)
        })
    }

    pub fn mcp_disassemble_handler(
        &self,
        cast_lib: i32,
        cast_member: i32,
        handler_name: String,
    ) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_disassemble_handler(
                context.player,
                context.symbols,
                cast_lib,
                cast_member,
                &handler_name,
            )
        })
    }

    pub fn mcp_decompile_handler(
        &self,
        cast_lib: i32,
        cast_member: i32,
        handler_name: String,
    ) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_decompile_handler(
                context.player,
                context.symbols,
                cast_lib,
                cast_member,
                &handler_name,
            )
        })
    }

    pub fn mcp_get_call_stack(&self, depth: i32, include_locals: bool) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_get_call_stack(
                context.player,
                context.symbols,
                (depth >= 0).then_some(depth as usize),
                include_locals,
            )
        })
    }

    pub fn mcp_get_globals(&self) -> Result<String, JsValue> {
        self.with_context(|context| player::mcp::mcp_get_globals(context.player, context.symbols))
    }

    pub fn mcp_get_locals(&self, scope_index: i32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_get_locals(
                context.player,
                context.symbols,
                (scope_index >= 0).then_some(scope_index as usize),
            )
        })
    }

    pub fn mcp_inspect_datum(&self, datum_id: u32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_inspect_datum(context.player, context.symbols, datum_id as usize)
        })
    }

    pub fn mcp_inspect_cast_member(&self, cast_lib: i32, cast_member: i32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_inspect_cast_member(
                context.player,
                context.symbols,
                cast_lib,
                cast_member,
            )
        })
    }

    pub fn mcp_get_console_output(&self, last_n_lines: usize) -> Result<String, JsValue> {
        self.with_context(|context| context.player.console.read_tail(last_n_lines))
    }

    pub fn mcp_get_context(&self) -> Result<String, JsValue> {
        self.with_context(|context| player::mcp::mcp_get_context(context.player))
    }

    pub fn mcp_get_execution_state(&self) -> Result<String, JsValue> {
        self.with_context(|context| player::mcp::mcp_get_execution_state(context.player))
    }

    pub fn mcp_list_cast_libs(&self) -> Result<String, JsValue> {
        self.with_context(|context| player::mcp::mcp_list_cast_libs(context.player))
    }

    pub fn mcp_list_cast_members(&self, cast_lib: i32) -> Result<String, JsValue> {
        self.with_context(|context| {
            player::mcp::mcp_list_cast_members(
                context.player,
                (cast_lib >= 0).then_some(cast_lib),
            )
        })
    }

    pub fn mcp_list_breakpoints(&self) -> Result<String, JsValue> {
        self.with_context(|context| player::mcp::mcp_list_breakpoints(context.player))
    }

    /// Evaluate a Lingo expression in this handle's owner-bound runtime and
    /// format the result with the same authoritative symbol table.
    pub async fn mcp_eval_lingo(&self, code: String) -> Result<String, JsValue> {
        let result = player::eval_lingo_command_owned(
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
            code,
        )
        .await;
        self.with_context(|context| {
            player::mcp::mcp_format_eval_result(context.player, context.symbols, result)
        })
    }

    /// Evaluate a debugger command without re-entering the legacy global
    /// player slot. Errors are reported through this same owner context.
    pub async fn eval_command(&self, command: String) -> Result<(), JsValue> {
        JsApi::dispatch_debug_message(&command);
        let result = player::eval_lingo_command_owned(
            self.session.clone(),
            self.player_id,
            self.owner.clone(),
            command,
        )
        .await;
        if let Err(error) = result {
            self.with_context(|context| {
                JsApi::dispatch_script_error(context.player, &error);
            })?;
        }
        Ok(())
    }

    pub fn add_breakpoint(&self, script_name: String, handler_name: String, bytecode_index: usize) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.breakpoint_manager.add_breakpoint(script_name, handler_name, bytecode_index);
        })
    }

    pub fn toggle_breakpoint(&self, script_name: String, handler_name: String, bytecode_index: usize) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.breakpoint_manager.toggle_breakpoint(
                script_name,
                handler_name,
                bytecode_index,
            );
        })
    }

    pub fn remove_breakpoint(&self, script_name: String, handler_name: String, bytecode_index: usize) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.breakpoint_manager.remove_breakpoint(script_name, handler_name, bytecode_index);
        })
    }

    pub fn resume_breakpoint(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.resume_breakpoint())
    }

    pub fn step_into(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.step_into())
    }

    pub fn step_over(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.step_over())
    }

    pub fn step_out(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.step_out())
    }

    pub fn step_over_line(&self, skip_bytecode_indices: Vec<usize>) -> Result<(), JsValue> {
        self.with_context(|context| context.player.step_over_line(skip_bytecode_indices))
    }

    pub fn step_into_line(&self, skip_bytecode_indices: Vec<usize>) -> Result<(), JsValue> {
        self.with_context(|context| context.player.step_into_line(skip_bytecode_indices))
    }

    pub fn trigger_timeout(&self, name: String) -> Result<(), JsValue> {
        if !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser player handle is stale"));
        }
        self.command_tx
            .try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::TimeoutTriggered(name),
                completer: None,
            })
            .map_err(|_| JsValue::from_str("browser player command loop stopped"))
    }

    pub fn get_breakpoints(&self) -> Result<js_sys::Array, JsValue> {
        self.with_context(|context| JsApi::get_breakpoint_list(context.player).into_iter().collect())
    }

    pub fn request_datum(&self, datum_id: u32) -> Result<(), JsValue> {
        self.with_context(|context| {
            if let Some(datum_ref) = context.player.allocator.get_datum_ref(datum_id as DatumId) {
                JsApi::dispatch_datum_snapshot(&datum_ref, context.symbols, context.player);
            }
        })
    }

    pub fn request_script_instance_snapshot(&self, script_instance_id: u32) -> Result<(), JsValue> {
        self.with_context(|context| -> Result<(), JsValue> {
            let instance_ref = if script_instance_id == 0 {
                None
            } else {
                Some(context.player.allocator.get_script_instance_ref(script_instance_id)
                    .ok_or_else(|| JsValue::from_str("script instance is not live"))?)
            };
            JsApi::dispatch_script_instance_snapshot(instance_ref, context.symbols, context.player);
            Ok(())
        }).and_then(|result| result)
    }

    pub fn clear_debug_messages(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.debug_datum_refs.clear())
    }

    pub fn set_eval_scope_index(&self, index: i32) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.eval_scope_index = if index >= 0 { Some(index as u32) } else { None };
        })
    }

    pub fn trigger_alert_hook(&self) -> Result<(), JsValue> {
        if !self.owner.is_arena_live() {
            return Err(JsValue::from_str("browser player handle is stale"));
        }
        self.command_tx
            .try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::TriggerAlertHook,
                completer: None,
            })
            .map_err(|_| JsValue::from_str("browser player command loop stopped"))
    }

    pub fn get_cast_chunk_list(&self, cast_number: u32) -> Result<JsValue, JsValue> {
        self.with_context(|context| JsApi::get_cast_chunk_list_for(context.player, cast_number).into())
    }

    pub fn get_movie_top_level_chunks(&self) -> Result<JsValue, JsValue> {
        self.with_context(|context| JsApi::get_movie_top_level_chunks(context.player).into())
    }

    pub fn get_chunk_bytes(&self, cast_number: u32, chunk_id: u32) -> Result<Option<Vec<u8>>, JsValue> {
        self.with_context(|context| JsApi::get_chunk_bytes(context.player, cast_number, chunk_id))
    }

    pub fn get_parsed_chunk(&self, cast_number: u32, chunk_id: u32) -> Result<JsValue, JsValue> {
        self.with_context(|context| JsApi::get_parsed_chunk(context.player, context.symbols, cast_number, chunk_id).into())
    }

    pub fn subscribe_to_member(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.with_context(|context| {
            let member_ref = cast_member_ref(cast_lib, cast_member);
            if !context.player.subscribed_member_refs.contains(&member_ref) {
                context.player.subscribed_member_refs.push(member_ref.clone());
            }
            JsApi::dispatch_cast_member_changed(member_ref, context.symbols, context.player);
        })
    }

    pub fn unsubscribe_from_member(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.with_context(|context| {
            let member_ref = cast_member_ref(cast_lib, cast_member);
            context.player.subscribed_member_refs.retain(|item| item != &member_ref);
        })
    }

    pub fn subscribe_to_channel_names(&self) -> Result<(), JsValue> {
        let (names, owner_key) = self.with_context(|context| {
            context.player.is_subscribed_to_channel_names = true;
            JsApi::channel_names_snapshot_for_player(context.player)
        })?;
        JsApi::dispatch_channel_names_snapshot(names, &owner_key);
        Ok(())
    }

    pub fn unsubscribe_from_channel_names(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.is_subscribed_to_channel_names = false)
    }

    pub fn subscribe_to_score(&self) -> Result<(), JsValue> {
        let snapshot = self.with_context(|context| {
            context.player.is_subscribed_to_score = true;
            JsApi::score_snapshot_for_player(context.player)
        })?;
        if let Some((snapshot, owner_key)) = snapshot {
            JsApi::dispatch_score_snapshot(snapshot, &owner_key);
        }
        Ok(())
    }

    pub fn unsubscribe_from_score(&self) -> Result<(), JsValue> {
        self.with_context(|context| context.player.is_subscribed_to_score = false)
    }

    /// Read subscription flags for owner-isolation checks without exposing the
    /// session or its mutable player state to JavaScript.
    pub fn subscription_state(&self) -> Result<JsValue, JsValue> {
        self.with_context(|context| -> Result<JsValue, JsValue> {
            let state = js_sys::Object::new();
            js_sys::Reflect::set(
                &state,
                &JsValue::from_str("score"),
                &JsValue::from_bool(context.player.is_subscribed_to_score),
            )?;
            js_sys::Reflect::set(
                &state,
                &JsValue::from_str("channelNames"),
                &JsValue::from_bool(context.player.is_subscribed_to_channel_names),
            )?;
            Ok(state.into())
        })?
    }

    pub fn subscribe_to_cast_member_list(&self, cast_number: u32) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.subscribed_cast_member_lists.insert(cast_number);
            JsApi::dispatch_cast_member_list_changed(cast_number);
        })
    }

    pub fn unsubscribe_from_cast_member_list(&self, cast_number: u32) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.subscribed_cast_member_lists.remove(&cast_number);
        })
    }

    pub fn list_w3d_members(&self) -> Result<String, JsValue> {
        self.with_context(|context| {
            let mut result = String::new();
            for (lib_idx, cast) in context.player.movie.cast_manager.casts.iter().enumerate() {
                for (_, member) in cast.members.iter() {
                    if member.member_type.as_shockwave3d().is_some() {
                        result.push_str(&format!(
                            "castLib {}  member {} \"{}\"  (call handle.export_w3d_obj({}, {}) to download)\n",
                            lib_idx + 1,
                            member.number,
                            member.name,
                            lib_idx + 1,
                            member.number,
                        ));
                    }
                }
            }
            if result.is_empty() {
                result.push_str("No Shockwave3D members found.");
            }
            result
        })
    }

    pub fn export_w3d_raw(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.with_context(|context| {
            let member_ref = CastMemberRef { cast_lib, cast_member };
            let Some(member) = context.player.movie.cast_manager.find_member_by_ref(&member_ref) else {
                return Err(JsValue::from_str("W3D member not found"));
            };
            let Some(w3d) = member.member_type.as_shockwave3d() else {
                return Err(JsValue::from_str("member is not Shockwave3D"));
            };
            let magic = [0x49u8, 0x46, 0x58, 0x00];
            let Some(offset) = (0..w3d.w3d_data.len().min(256))
                .find(|&index| index + 4 <= w3d.w3d_data.len() && w3d.w3d_data[index..index + 4] == magic)
            else {
                return Err(JsValue::from_str("W3D member has no IFX payload"));
            };
            trigger_browser_download(
                &format!("member_{}_{}.w3d", cast_lib, cast_member),
                &w3d.w3d_data[offset..],
                "application/octet-stream",
            );
            Ok(())
        })?
    }

    pub fn export_w3d_obj(&self, cast_lib: i32, cast_member: i32) -> Result<(), JsValue> {
        self.with_context(|context| export_w3d_obj_for_player(context.player, context.symbols, cast_lib, cast_member))?
    }

    /// Drop host-side external-Xtra slots for this exact owner generation.
    pub fn dispose_external_xtra_host(&self) {
        crate::player::xtra::external::dispose_external_host(&self.owner_identity());
    }

    pub fn external_xtra_host_dispatch(
        &self,
        op_id: u32,
        args: &[u8],
    ) -> Result<Vec<u8>, JsValue> {
        if matches!(op_id, 5 | 6 | 7) {
            let decoded = xtra_sdk::wire::decode_args(args)
                .map_err(|error| JsValue::from_str(&format!("bad args: {}", error)))?;
            let request = self.with_context(|context| {
                crate::player::xtra::external::prepare_nested_host_request(
                    context.player,
                    op_id,
                    &decoded,
                )
            })?.map_err(|error| JsValue::from_str(&error))?;
            let response = crate::player::xtra::external::execute_request(&request)
                .map_err(|error| JsValue::from_str(&error.message))?
                .ok_or_else(|| JsValue::from_str("external Xtra host dispatch returned no response"))?;
            let still_current = self
                .with_context(|context| {
                    request.owner.same_identity(&context.player.owner)
                        && request.owner.is_arena_live()
                        && context.player.owner.is_arena_live()
                })
                .unwrap_or(false);
            if !still_current {
                return Err(JsValue::from_str("external Xtra host dispatch owner was retired"));
            }
            return Ok(response.bytes);
        }
        self.with_context(|context| {
            crate::player::xtra::external::host_call_dispatch(
                context.player,
                context.symbols,
                op_id,
                args,
            )
        })
    }

    pub fn register_external_xtra(&self, name: &str) -> Result<(), JsValue> {
        self.with_context(|context| {
            context.player.xtra_manager_state.external.register(name);
        })
    }

    pub fn complete_external_xtra_load(
        &self,
        name: &str,
        capability: &str,
        success: bool,
    ) -> Result<(), JsValue> {
        let mut parts = capability.split(':');
        let state_id = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| JsValue::from_str("invalid external Xtra load capability"))?;
        let request_id = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| JsValue::from_str("invalid external Xtra load capability"))?;
        if parts.next().is_some() {
            return Err(JsValue::from_str("invalid external Xtra load capability"));
        }
        self.with_context(|context| {
            context.player.xtra_manager_state.external.complete_load(
                &context.player.owner,
                state_id,
                request_id,
                name,
                success,
            );
        })
    }

    /// Return a bridge snapshot using the session's authoritative symbols.
    pub fn datum_snapshot(&self, datum_id: u32) -> Result<js_sys::Object, JsValue> {
        self.with_context(|context| -> Result<js_sys::Object, JsValue> {
            let datum_ref = context
                .player
                .allocator
                .get_datum_ref(datum_id as usize)
                .ok_or_else(|| JsValue::from_str("datum id is not live"))?;
            crate::player::datum_formatting::format_concrete_datum(
                context.player.get_datum(&datum_ref),
                context.symbols,
                context.player,
            )
            .map_err(|error| JsValue::from_str(&error.message))?;
            Ok(js_api::datum_to_js_bridge_with_symbols(
                &datum_ref,
                context.symbols,
                context.player,
            ))
        })
        .and_then(|result| result)
    }
}

impl Drop for BrowserPlayerHandle {
    fn drop(&mut self) {
        // Close before removing the player: DirPlayer and queued work retain
        // sender clones, so dropping this handle alone would otherwise leave
        // the receiver task alive and keep the session graph reachable.
        self.command_tx.close();
        self.dispose_external_xtra_host();
        rendering::dispose_renderer_state(&self.renderer);
        self.owner.mark_arena_dead();
        if let Ok(mut session) = self.session.try_borrow_mut() {
            session.unbind_renderer_state(self.player_id);
            let _ = session.remove_player(self.player_id);
            let teardowns = session.take_host_teardowns();
            drop(session);
            drop(teardowns);
        }
    }
}

#[wasm_bindgen]
pub fn set_external_params(params: js_sys::Object) {
    // IndexMap: `Object::keys` yields the object's own string keys in
    // insertion order, and Director's indexed `externalParamName(n)` /
    // `externalParamValue(n)` expose that order to the movie.
    let mut external_params = indexmap::IndexMap::new();
    let keys = js_sys::Object::keys(&params);
    for key in keys.iter() {
        let key_str = key.as_string().unwrap();
        let value = js_sys::Reflect::get(&params, &key)
            .unwrap()
            .as_string()
            .unwrap();
        external_params.insert(key_str, value);
    }

    player_dispatch(PlayerVMCommand::SetExternalParams(external_params));
}

#[wasm_bindgen]
pub fn set_base_path(path: String) {
    player_dispatch(PlayerVMCommand::SetBasePath(path));
}

/// The Shockwave projector's `--do` launch argument: Lingo evaluated once,
/// immediately before the launched movie's `prepareMovie`.
///
/// Archived titles are often launched through a wrapper movie that the
/// projector has to seed first — Agent Free Ride's Flashpoint entry is
/// `"…/wrapper_silentbaystudios.dcr" --do "member('gameUrl').text = '…'"`,
/// and the wrapper cannot redirect without it because the member ships empty.
///
/// Call before `load_movie_*`. Single-quoted strings in the payload are
/// accepted as the launcher writes them (see `normalize_startup_do_quotes`).
#[wasm_bindgen]
pub fn set_startup_do(code: String) {
    player_dispatch(PlayerVMCommand::SetStartupDo(code));
}

/// The projector's `--doBefore`: Lingo evaluated once BEFORE the movie loads.
#[wasm_bindgen]
pub fn set_startup_do_before(code: String) {
    player_dispatch(PlayerVMCommand::SetStartupDoBefore(code));
}

/// The projector's `--go N`: jump to frame `N` once the movie has started.
/// Pass 0 to clear.
#[wasm_bindgen]
pub fn set_startup_go(frame: u32) {
    player_dispatch(PlayerVMCommand::SetStartupGo(frame));
}

#[wasm_bindgen]
pub fn set_movie_path_override(path: String) {
    player_dispatch(PlayerVMCommand::SetMoviePathOverride(path));
}

/// Like `set_movie_path_override` but does NOT register the path with the
/// net manager for URL rewriting. The given path becomes what `the
/// moviePath` / `the movieName` return; URLs the script builds from it
/// (e.g. `postNetText(the moviePath & "x.aspx")`) go out unchanged for
/// the JS-side fetch interceptor / proxy to handle.
///
/// Use this when you've already wired up host-based proxying on the
/// JS/dev-server side (see `flashPlayerManager.ts::applyFetchRewrite`)
/// and you only want to advertise a "real" path to the movie without
/// dirplayer rewriting any URLs that result.
#[wasm_bindgen]
pub fn set_movie_path_label(path: String) {
    player_dispatch(PlayerVMCommand::SetMoviePathLabel(path));
}

#[wasm_bindgen]
pub fn set_system_font_path(path: String) {
    player_dispatch(PlayerVMCommand::SetSystemFontPath(path));
}

#[wasm_bindgen]
pub async fn load_movie_file(path: String, autoplay: bool) {
    player_dispatch(PlayerVMCommand::LoadMovieFromFile(path, autoplay));
}

// Player control commands bypass the command queue to allow stopping/resetting
// while a breakpoint is active.

#[wasm_bindgen]
pub fn play() {
    reserve_player_mut(|player| {
        player.play();
    });
}

#[wasm_bindgen]
pub fn stop() {
    reserve_player_mut(|player| {
        player.stop();
    });
}

#[wasm_bindgen]
pub fn reset() {
    reserve_player_mut(|player| {
        player.reset();
    });
}

/// Datum-arena census: live count per datum type, plus Int pool/refcount
/// detail. Callable from the console as `dirplayer_datumStats()` while a movie
/// is running, so a long Lingo loop that is growing the heap can be attributed
/// to what it is actually allocating instead of guessed at from a CPU profile.
#[wasm_bindgen(js_name = "dirplayer_datumStats")]
pub fn datum_stats() -> String {
    crate::player::reserve_player_ref(|player| player.allocator.datum_type_stats())
}

// Debug commands bypass the command queue to avoid deadlocks when a breakpoint
// is hit during command processing. These operations are safe to call directly
// because they only modify player state synchronously.

#[wasm_bindgen]
pub fn add_breakpoint(script_name: String, handler_name: String, bytecode_index: usize) {
    reserve_player_mut(|player| {
        player.breakpoint_manager.add_breakpoint(
            script_name,
            handler_name,
            bytecode_index,
        );
    });
}

/// Read the current breakpoint list.
///
/// The UI otherwise only ever learns breakpoints from the
/// `onBreakpointListChanged` push, so a view that mounts after the push — or a
/// session where the push never happened — shows none of them. This gives it a
/// way to ask.
#[wasm_bindgen]
pub fn get_breakpoints() -> js_sys::Array {
    reserve_player_ref(|player| {
        JsApi::get_breakpoint_list(player).into_iter().collect()
    })
}

#[wasm_bindgen]
pub fn remove_breakpoint(script_name: String, handler_name: String, bytecode_index: usize) {
    reserve_player_mut(|player| {
        player.breakpoint_manager.remove_breakpoint(
            script_name,
            handler_name,
            bytecode_index,
        );
    });
}

#[wasm_bindgen]
pub fn toggle_breakpoint(script_name: String, handler_name: String, bytecode_index: usize) {
    reserve_player_mut(|player| {
        player.breakpoint_manager.toggle_breakpoint(
            script_name,
            handler_name,
            bytecode_index,
        );
    });
}

#[wasm_bindgen]
pub fn resume_breakpoint() {
    reserve_player_mut(|player| {
        player.resume_breakpoint();
    });
}

#[wasm_bindgen]
pub fn step_into() {
    reserve_player_mut(|player| {
        player.step_into();
    });
}

#[wasm_bindgen]
pub fn step_over() {
    reserve_player_mut(|player| {
        player.step_over();
    });
}

#[wasm_bindgen]
pub fn step_out() {
    reserve_player_mut(|player| {
        player.step_out();
    });
}

#[wasm_bindgen]
pub fn step_over_line(skip_bytecode_indices: Vec<usize>) {
    reserve_player_mut(|player| {
        player.step_over_line(skip_bytecode_indices);
    });
}

#[wasm_bindgen]
pub fn step_into_line(skip_bytecode_indices: Vec<usize>) {
    reserve_player_mut(|player| {
        player.step_into_line(skip_bytecode_indices);
    });
}

/// Run a synthetic Lingo bytecode-throughput benchmark in the live (browser)
/// interpreter and return a human-readable report (ops/sec, ns/op). Call from
/// the DevTools console after a movie has loaded to measure real WASM per-op
/// cost. Requires an initialized player.
/// Turn the register-IR execution path on or off at runtime, so a movie can be
/// A/B'd in one session instead of one 6-minute wasm build per data point.
/// Returns the value now in effect.
#[wasm_bindgen]
pub fn set_ir_enabled(enabled: bool) -> bool {
    player::reserve_player_mut(|player| {
        player.ir_enabled = enabled;
        player.ir_enabled
    })
}

#[wasm_bindgen]
pub fn bench_bytecode_throughput() -> String {
    player::run_bytecode_benchmark()
}

/// Begin recording a speedscope profile of Lingo VM execution (handlers +
/// bytecode ops). Clears any previously recorded events.
#[wasm_bindgen]
pub fn start_profiling_recording() {
    player::profiling::start_recording();
}

/// Stop recording. Buffered events stay available for `export_profiling_speedscope`.
#[wasm_bindgen]
pub fn stop_profiling_recording() {
    player::profiling::stop_recording();
}

/// True while a profiling recording is active.
#[wasm_bindgen]
pub fn is_profiling_recording() -> bool {
    player::profiling::is_recording()
}

/// Discard all buffered profiling events.
#[wasm_bindgen]
pub fn clear_profiling_recording() {
    player::profiling::clear_recording();
}

/// Serialise the recorded events to a speedscope "evented" profile JSON string,
/// ready to be saved as a `.speedscope.json` file and opened in speedscope.
#[wasm_bindgen]
pub fn export_profiling_speedscope() -> String {
    player::profiling::export_speedscope_json()
}

#[wasm_bindgen]
pub fn set_break_on_error(enabled: bool) {
    reserve_player_mut(|player| {
        player.break_on_error = enabled;
    });
}

#[wasm_bindgen]
pub fn get_break_on_error() -> bool {
    reserve_player_ref(|player| player.break_on_error)
}

/// Returns the trace log file path and content as a JS object { path, content },
/// or null if no trace log file is set or empty.
#[wasm_bindgen]
pub fn get_trace_log() -> JsValue {
    let (path, data) = reserve_player_ref(|player| {
        let path = player.movie.trace_log_file.clone();
        if path.is_empty() {
            return (String::new(), Vec::new());
        }
        let data = player
            .xtra_manager_state
            .fileio
            .virtual_fs
            .get(&path)
            .cloned()
            .unwrap_or_default();
        (path, data)
    });

    if path.is_empty() || data.is_empty() {
        return JsValue::NULL;
    }

    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &"path".into(), &path.into());
    let content = String::from_utf8_lossy(&data);
    let _ = js_sys::Reflect::set(&obj, &"content".into(), &content.as_ref().into());
    obj.into()
}

#[wasm_bindgen]
pub fn set_stage_size(width: u32, height: u32) {
    player_dispatch(PlayerVMCommand::SetStageSize(width, height));
}

#[wasm_bindgen]
pub fn trigger_timeout(name: &str) {
    player_dispatch(PlayerVMCommand::TimeoutTriggered(name.to_string()));
}

// ── Interpreter instrumentation ─────────────────────────────────────────
//
// Off by default; the browser e2e harness turns it on for the whole suite so
// the opcode histogram is sampled across every movie rather than one title.
// See `player::interp_stats`.

#[wasm_bindgen]
pub fn set_interp_stats_enabled(enabled: bool) {
    crate::player::interp_stats::set_enabled(enabled);
}

#[wasm_bindgen]
pub fn reset_interp_stats() {
    crate::player::interp_stats::reset();
}

#[wasm_bindgen]
pub fn get_interp_stats_report() -> String {
    crate::player::interp_stats::report()
}

// ── External Xtra plugin host surface ───────────────────────────────────
//
// JS calls these after fetching and instantiating a plugin .wasm. The
// loader and bridge live in `dirplayer-js-api`; the four host
// environments (dev / polyfill / extension / Electron) share them.

/// Returns the currently-loaded movie's declared xtra dependencies
/// (parsed from its XTRl chunk). Each entry is a `js_sys::Object` with
/// `filename` (always present) and `displayName` (may be empty if the
/// movie's entry only had a filename).
///
/// JS-side hosts call this right after `load_movie_file` to resolve
/// each declared xtra against the host's name->URL registry, fetching
/// any plugins that aren't loaded yet.
///
/// Returns an empty array if no movie is loaded or the movie has no
/// XTRl chunk (older Director versions, lightweight movies).
#[wasm_bindgen]
pub fn movie_required_xtras() -> js_sys::Array {
    use crate::player::PLAYER_OPT;
    let result = js_sys::Array::new();
    unsafe {
        if let Some(player) = PLAYER_OPT.as_ref() {
            if let Some(file) = player.movie.file.as_ref() {
                if let Some(xtra_list) = file.xtra_list.as_ref() {
                    for decl in &xtra_list.entries {
                        let obj = js_sys::Object::new();
                        let _ = js_sys::Reflect::set(
                            &obj,
                            &"filename".into(),
                            &decl.filename.as_str().into(),
                        );
                        let _ = js_sys::Reflect::set(
                            &obj,
                            &"displayName".into(),
                            &decl
                                .display_name
                                .as_deref()
                                .unwrap_or("")
                                .into(),
                        );
                        result.push(&obj);
                    }
                }
            }
            // The 3D Groove engine is provided by an external plugin, but Groove
            // movies call its commands as bare globals (`InitGroove()`), never
            // `new(xtra "Groove")` — so nothing triggers an on-demand load. When
            // the movie carries any `.3GM` model (a `Groove3gm` cast member),
            // surface a synthetic "Groove" dependency so the JS resolver loads
            // the plugin from the registry before Lingo runs. (The name may also
            // appear in XTRl above; the JS side de-dupes by normalized key.)
            if movie_has_groove3gm(player) {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"filename".into(), &"Groove".into());
                let _ = js_sys::Reflect::set(&obj, &"displayName".into(), &"Groove".into());
                result.push(&obj);
            }
        }
    }
    result
}

/// True if any cast member of the loaded movie is a `.3GM` Groove model.
fn movie_has_groove3gm(player: &crate::player::DirPlayer) -> bool {
    use crate::player::cast_member::CastMemberType;
    player.movie.cast_manager.casts.iter().any(|cast| {
        cast.members
            .values()
            .any(|m| matches!(m.member_type, CastMemberType::Groove3gm(_)))
    })
}

#[wasm_bindgen]
pub fn player_print_member_bitmap_hex(cast_lib: i32, cast_member: i32) {
    player_dispatch(PlayerVMCommand::PrintMemberBitmapHex(CastMemberRef {
        cast_lib,
        cast_member,
    }));
}

/// Dev UI sound preview: dump a sound member's decoded header fields plus the
/// first 256 raw bytes (hex + ASCII) to the browser console. Lets a sound that
/// "doesn't play" be identified by its real format magic (RIFF/WAV, FORM/AIFF,
/// ID3 or 0xFF Ex = MP3, otherwise raw PCM) versus what the member metadata
/// claims. Read-only, so it resolves the member synchronously.
#[wasm_bindgen]
pub fn player_print_member_sound_hex(cast_lib: i32, cast_member: i32) {
    reserve_player_ref(|player| print_member_sound_hex_for_player(player, cast_lib, cast_member));
}

fn print_member_sound_hex_for_player(
    player: &crate::player::DirPlayer,
    cast_lib: i32,
    cast_member: i32,
) {
    use crate::player::cast_member::CastMemberType;
        let member_ref = CastMemberRef { cast_lib, cast_member };
        let Some(member) = player.movie.cast_manager.find_member_by_ref(&member_ref) else {
            web_sys::console::warn_1(
                &format!("[sound-preview] member {}:{} not found", cast_lib, cast_member).into(),
            );
            return;
        };
        let CastMemberType::Sound(snd) = &member.member_type else {
            web_sys::console::warn_1(
                &format!(
                    "[sound-preview] member {}:{} '{}' is not a sound member",
                    cast_lib, cast_member, member.name
                )
                .into(),
            );
            return;
        };
        let data = snd.sound.data();
        let info = &snd.info;
        let codec = snd.sound.codec();
        let big_endian = snd.sound.big_endian_data();
        let magic = if data.len() >= 4 && &data[0..4] == b"RIFF" {
            "RIFF/WAV"
        } else if data.len() >= 4 && &data[0..4] == b"FORM" {
            "AIFF (FORM)"
        } else if data.len() >= 3 && &data[0..3] == b"ID3" {
            "MP3 (ID3 tag)"
        } else if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
            "MP3 frame (FF Ex)"
        } else {
            "raw / PCM?"
        };
        web_sys::console::log_1(
            &format!(
                "🎧 SOUND {}:{} '{}' — {} Hz, {} ch, {}-bit, samples={}, dur={}ms, loop={}, codec='{}', big_endian={} — data={} bytes — magic={}",
                cast_lib, cast_member, member.name,
                info.sample_rate, info.channels, info.sample_size, info.sample_count,
                info.duration, info.loop_enabled, codec, big_endian,
                data.len(), magic
            )
            .into(),
        );
        let n = data.len().min(256);
        let mut hex = String::with_capacity(n * 3 + n / 16);
        let mut ascii = String::with_capacity(n + n / 16);
        for (i, &b) in data[..n].iter().enumerate() {
            hex.push_str(&format!("{:02X} ", b));
            ascii.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
            if (i + 1) % 16 == 0 {
                hex.push('\n');
                ascii.push('\n');
            }
        }
        web_sys::console::log_1(&format!("🎧 first {} bytes (hex):\n{}", n, hex).into());
        web_sys::console::log_1(&format!("🎧 first {} bytes (ascii):\n{}", n, ascii).into());
}

/// Dev UI sound preview: play a sound member on channel 1 via the real
/// puppetSound path. Routed through the command queue so playback starts at a
/// safe point; the triggering button click satisfies the audio-gesture gate.
#[wasm_bindgen]
pub fn player_play_member_sound(cast_lib: i32, cast_member: i32) {
    player_dispatch(PlayerVMCommand::PlayMemberSound(CastMemberRef {
        cast_lib,
        cast_member,
    }));
}

/// Dump every authored child sprite inside a filmloop member to the browser
/// console. Lingo can't reach a filmloop's child sprites directly (the
/// member.media is opaque), so this exposes the parsed score state
/// dirplayer-rs already holds in memory. Resolves keyframes the same way
/// `render_filmloop_from_channel_data` does (most-recent frame_idx per
/// channel up to current_frame). Call from the JS console:
///   `vm.player_print_filmloop_sprites(2, 145)` for the spiderweb filmloop.
#[wasm_bindgen]
pub fn player_print_filmloop_sprites(cast_lib: i32, cast_member: i32) {
    use crate::player::{cast_member::CastMemberType, reserve_player_ref};
    use crate::player::score::get_channel_number_from_index;
    reserve_player_ref(|player| {
        let member_ref = CastMemberRef { cast_lib, cast_member };
        let Some(member) = player.movie.cast_manager.find_member_by_ref(&member_ref) else {
            web_sys::console::warn_1(&format!(
                "[filmloop-dump] member {}:{} not found", cast_lib, cast_member
            ).into());
            return;
        };
        let CastMemberType::FilmLoop(film) = &member.member_type else {
            web_sys::console::warn_1(&format!(
                "[filmloop-dump] member {}:{} ('{}') is not a filmloop",
                cast_lib, cast_member, member.name
            ).into());
            return;
        };

        let frame = film.current_frame;
        let frame_idx_target = frame.saturating_sub(1);
        let init_data = &film.score.channel_initialization_data;

        // Resolve active sprite per channel: most-recent frame_idx <= target
        let mut latest: std::collections::HashMap<u16, (u32, &crate::director::chunks::score::ScoreFrameChannelData)> =
            std::collections::HashMap::new();
        for (frame_idx, channel_idx, data) in init_data.iter() {
            if *channel_idx < 6 { continue; }
            if data.cast_member == 0 { continue; }
            if *frame_idx > frame_idx_target { continue; }
            latest
                .entry(*channel_idx)
                .and_modify(|(f, d)| if *frame_idx > *f { *f = *frame_idx; *d = data; })
                .or_insert((*frame_idx, data));
        }
        let mut entries: Vec<_> = latest.into_iter().collect();
        entries.sort_by_key(|(ch, _)| *ch);

        web_sys::console::warn_1(&format!(
            "[filmloop-dump] member {}:{} '{}' frame={} init_data_entries={} active_channels={}",
            cast_lib, cast_member, member.name,
            frame, init_data.len(), entries.len()
        ).into());

        // Raw dump of ALL channel_init_data entries (no frame/filter), so we
        // can see channels that activate later, "reverse ink" companions, etc.
        let mut all_raw: Vec<_> = init_data.iter().collect();
        all_raw.sort_by_key(|(f, c, _)| (*c, *f));
        for (f, c, d) in all_raw.iter().take(80) {
            let ilib = if d.cast_lib == 65535 { cast_lib } else { d.cast_lib as i32 };
            let nm = player.movie.cast_manager
                .find_filmloop_inner_member(&CastMemberRef { cast_lib: ilib, cast_member: d.cast_member as i32 })
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "<no_member>".into());
            web_sys::console::warn_1(&format!(
                "[filmloop-raw]   f={} ch={} ink={} blend={} member=({},{}) '{}' size={}x{}",
                f, c, d.ink, d.blend, ilib, d.cast_member, nm, d.width, d.height
            ).into());
        }

        for (channel_idx, (frame_idx, data)) in entries {
            let channel_num = get_channel_number_from_index(channel_idx as u32);
            let resolved_lib = if data.cast_lib == 65535 { cast_lib } else { data.cast_lib as i32 };
            let sprite_ref = CastMemberRef { cast_lib: resolved_lib, cast_member: data.cast_member as i32 };
            let inner = player.movie.cast_manager.find_filmloop_inner_member(&sprite_ref);
            let (mname, mtype, bm_info) = match inner {
                Some(m) => {
                    let info = if let CastMemberType::Bitmap(bm) = &m.member_type {
                        let bmp = player.bitmap_manager.get_bitmap(bm.image_ref);
                        match bmp {
                            Some(b) => format!(" bm={}x{} bd={} obd={} use_alpha={} pal={:?}",
                                b.width, b.height, b.bit_depth, b.original_bit_depth,
                                b.use_alpha, b.palette_ref),
                            None => " bm=<no_data>".into(),
                        }
                    } else {
                        String::new()
                    };
                    (m.name.clone(), format!("{:?}", m.member_type.member_type_id()), info)
                }
                None => ("<not found>".into(), "<none>".into(), String::new()),
            };
            web_sys::console::warn_1(&format!(
                "[filmloop-dump]   ch={} ink={} blend={} member=({},{}) '{}' [{}]{} pos=({},{}) size={}x{} flipH={} flipV={} fore/back=({}/{}) color_flag={} kf_frame={}",
                channel_num,
                data.ink,
                data.blend,
                resolved_lib, data.cast_member,
                mname, mtype, bm_info,
                data.pos_x, data.pos_y,
                data.width, data.height,
                data.flip_h(), data.flip_v(),
                data.fore_color, data.back_color,
                data.color_flag,
                frame_idx
            ).into());

            // If the child is itself a filmloop, dump one level deeper so we
            // can see what bitmaps it ultimately contains.
            if let Some(m) = inner {
                if let CastMemberType::FilmLoop(inner_film) = &m.member_type {
                    let inner_target = inner_film.current_frame.saturating_sub(1);
                    let mut inner_latest: std::collections::HashMap<u16, (u32, &crate::director::chunks::score::ScoreFrameChannelData)> =
                        std::collections::HashMap::new();
                    for (f, c, d) in inner_film.score.channel_initialization_data.iter() {
                        if *c < 6 || d.cast_member == 0 || *f > inner_target { continue; }
                        inner_latest
                            .entry(*c)
                            .and_modify(|(ef, ed)| if *f > *ef { *ef = *f; *ed = d; })
                            .or_insert((*f, d));
                    }
                    let mut inner_entries: Vec<_> = inner_latest.into_iter().collect();
                    inner_entries.sort_by_key(|(c, _)| *c);
                    for (ic, (ifr, id)) in inner_entries {
                        let icn = get_channel_number_from_index(ic as u32);
                        let ilib = if id.cast_lib == 65535 { resolved_lib } else { id.cast_lib as i32 };
                        let inested_ref = CastMemberRef { cast_lib: ilib, cast_member: id.cast_member as i32 };
                        let (inn_name, inn_type) = match player.movie.cast_manager.find_filmloop_inner_member(&inested_ref) {
                            Some(im) => (im.name.clone(), format!("{:?}", im.member_type.member_type_id())),
                            None => ("<not found>".into(), "<none>".into()),
                        };
                        web_sys::console::warn_1(&format!(
                            "[filmloop-dump]     >> nested ch={} ink={} member=({},{}) '{}' [{}] size={}x{} fore/back=({}/{}) color_flag={} kf_frame={}",
                            icn, id.ink, ilib, id.cast_member, inn_name, inn_type,
                            id.width, id.height, id.fore_color, id.back_color, id.color_flag, ifr
                        ).into());
                    }
                }
            }
        }
    });
}

/// Resolve a canvas-space mouse event to movie coordinates — EXCEPT during FPS
/// mouse-look (pointer lock). While the movie wants pointer lock it drives the
/// camera from `mouse_move_delta` and recenters `mouse_loc` every frame; the
/// browser also freezes the cursor at the lock-engage point, so a click's absolute
/// position is meaningless and would slam `mouse_loc` away from that center — a
/// one-frame delta spike that jolts the camera on EVERY click (left or right).
/// In that mode keep the existing delta-managed `mouse_loc` so the click only
/// registers the button, not a phantom move.
fn mouse_event_loc(x: f64, y: f64) -> (i32, i32) {
    reserve_player_ref(|p| {
        if p.wants_pointer_lock {
            p.mouse_loc
        } else {
            // Invert the stage auto-scale so mouseH/mouseV land in movie coordinates,
            // matching where sprites live in Lingo-facing state. No-op when scale=1.
            let (mx, my) = crate::player::stage::canvas_to_movie_coords(p, x, y);
            (mx.to_i32().unwrap(), my.to_i32().unwrap())
        }
    })
}

#[wasm_bindgen]
pub fn mouse_down(x: f64, y: f64) {
    let (ix, iy) = mouse_event_loc(x, y);
    reserve_player_mut(|player| {
        player.mouse_loc = (ix, iy);
        player.movie.mouse_down = true;
    });
    player_dispatch(PlayerVMCommand::MouseDown((ix, iy)));
    forward_mouse_to_nested(0, ix, iy);
}

#[wasm_bindgen]
pub fn mouse_up(x: f64, y: f64) {
    let (ix, iy) = mouse_event_loc(x, y);
    reserve_player_mut(|player| {
        player.mouse_loc = (ix, iy);
        player.movie.mouse_down = false;
    });
    player_dispatch(PlayerVMCommand::MouseUp((ix, iy)));
    forward_mouse_to_nested(1, ix, iy);
}

/// Forward a mouse event to a nested `#movie` sub-player when the cursor is over
/// its sprite: find the on-stage #movie sprite whose rect contains `(ix,iy)`, map
/// the point into the sub-player's stage rect, set the sub's `mouse_loc`, and
/// dispatch the event to the sub's own command queue so its behaviours
/// (`checkMouse` / `on mouseDown`) respond. `kind`: 0=down, 1=up, 2=move.
fn forward_mouse_to_nested(kind: u8, ix: i32, iy: i32) {
    use crate::player::{
        nested_player_id, reserve_player_mut, reserve_player_ref, ACTIVE_PLAYER_ID, NESTED_PLAYERS,
    };
    let target = reserve_player_ref(|host| {
        for channel in &host.movie.score.channels {
            let Some(member_ref) = channel.sprite.member.as_ref() else {
                continue;
            };
            let Some(id) = nested_player_id(member_ref) else {
                continue;
            };
            if !channel.sprite.visible {
                continue;
            }
            let rect = crate::player::score::get_concrete_sprite_rect(host, &channel.sprite);
            if ix >= rect.left && ix < rect.right && iy >= rect.top && iy < rect.bottom {
                let sub_rect = unsafe {
                    NESTED_PLAYERS
                        .get(id - 1)
                        .and_then(|o| o.as_ref())
                        .map(|s| s.movie.rect.clone())
                };
                if let Some(sr) = sub_rect {
                    let sw = rect.width().max(1);
                    let sh = rect.height().max(1);
                    // The sub renders 0-origin (the composite uses ortho at the
                    // sub's WIDTH/HEIGHT, ignoring movie.rect's left/top — g349's
                    // rect origin is (20,20) but its sprites live at 0..600). Map
                    // the click to that same 0-origin space; adding sr.left/top
                    // shifted every click ~20px and made small buttons (Restart /
                    // Quit) miss.
                    let sx = (ix - rect.left) * sr.width() / sw;
                    let sy = (iy - rect.top) * sr.height() / sh;
                    return Some((id, sx, sy));
                }
            }
        }
        None
    });
    let Some((id, sx, sy)) = target else {
        return;
    };
    // Update the sub-player's mouse STATE synchronously (so polling handlers like
    // the DGS `checkMouse` see `the mouseLoc`/`the mouseDown` immediately) AND
    // dispatch the event into the sub's own command queue so its sprite
    // behaviours fire — the Restart button is `on mouseDown me` and Quit is
    // `on mouseUp me`, which only run on an actual event, not state polling.
    // player_dispatch routes to the ACTIVE player's queue while ACTIVE_PLAYER_ID
    // = id; the sub's command loop resolves instance ids against the sub's own
    // allocator (de-globalization is in place, so the earlier cross-boundary
    // concern no longer applies).
    let prev = unsafe { ACTIVE_PLAYER_ID };
    unsafe {
        ACTIVE_PLAYER_ID = id;
    }
    reserve_player_mut(|sub| {
        sub.mouse_loc = (sx, sy);
        match kind {
            0 => sub.movie.mouse_down = true,
            1 => sub.movie.mouse_down = false,
            _ => {}
        }
    });
    match kind {
        0 => player_dispatch(PlayerVMCommand::MouseDown((sx, sy))),
        1 => player_dispatch(PlayerVMCommand::MouseUp((sx, sy))),
        // MouseMove drives the sub's rollover events (mouseEnter / mouseWithin /
        // mouseLeave) — dispatched in the MouseMove command handler by hover
        // hit-testing. Without this the sub never fires mouseWithin, so
        // hold-to-scroll behaviours ("scrolldisplay arrow bhv") highlight but
        // never scroll.
        2 => player_dispatch(PlayerVMCommand::MouseMove((sx, sy))),
        _ => {}
    }
    unsafe {
        ACTIVE_PLAYER_ID = prev;
    }
}

#[wasm_bindgen]
pub fn mouse_move(x: f64, y: f64) {
    // During FPS mouse-look the camera is driven by `mouse_move_delta` and the movie
    // recenters `mouse_loc` each frame. An absolute move here — e.g. right after
    // pointer lock exits on Esc, before the movie disables mouse-look, when the cursor
    // reappears far from center — would inject a large one-frame delta and snap the
    // view ("camera tilts up on focus loss"). Ignore absolute moves while mouse-look
    // is active; the delta path owns the camera.
    if reserve_player_ref(|p| p.wants_pointer_lock) {
        return;
    }
    let (mx, my) = reserve_player_ref(|p| crate::player::stage::canvas_to_movie_coords(p, x, y));
    let (ix, iy) = (mx.to_i32().unwrap(), my.to_i32().unwrap());
    reserve_player_mut(|player| {
        player.mouse_loc = (ix, iy);
    });
    player_dispatch(PlayerVMCommand::MouseMove((ix, iy)));
    forward_mouse_to_nested(2, ix, iy);
}

/// Right-mouse-button down. Tracked via a separate flag because Director
/// scripts use `the rightMouseDown` to gate right-drag behaviour. Position
/// is updated alongside so `the mouseLoc` reflects the click point.
#[wasm_bindgen]
pub fn right_mouse_down(x: f64, y: f64) {
    let (ix, iy) = mouse_event_loc(x, y);
    reserve_player_mut(|player| {
        player.mouse_loc = (ix, iy);
        player.movie.right_mouse_down = true;
    });
    player_dispatch(PlayerVMCommand::RightMouseDown((ix, iy)));
}

#[wasm_bindgen]
pub fn right_mouse_up(x: f64, y: f64) {
    let (ix, iy) = mouse_event_loc(x, y);
    reserve_player_mut(|player| {
        player.mouse_loc = (ix, iy);
        player.movie.right_mouse_down = false;
    });
    player_dispatch(PlayerVMCommand::RightMouseUp((ix, iy)));
}

/// Check if the game wants pointer lock (for FPS mouse look)
#[wasm_bindgen]
pub fn wants_pointer_lock() -> bool {
    reserve_player_ref(|player| player.wants_pointer_lock)
}

/// Mouse move with delta values (for pointer lock mode).
/// The delta is added to the current mouse_loc (which the game resets to center each
/// frame), so `the mouseH` tracks pointer-lock movementX. X must be ADDED, not
/// subtracted: `the mouseH` increases to the right (screen coords), so moving the
/// mouse right (movementX > 0) must increase mouseH → the movie yaws right. The
/// previous `-= dx` inverted horizontal look (move left → turn right).
#[wasm_bindgen]
pub fn mouse_move_delta(dx: f64, dy: f64) {
    reserve_player_mut(|player| {
        player.mouse_loc.0 += dx.to_i32().unwrap();
        player.mouse_loc.1 += dy.to_i32().unwrap();
    });
    let (x, y) = reserve_player_ref(|player| player.mouse_loc);
    player_dispatch(PlayerVMCommand::MouseMove((x, y)));
}

#[wasm_bindgen]
pub fn key_down(key: String, code: u16) {
    // Update keyboard state immediately so keyPressed() reflects
    // real state even during long-running script handlers
    reserve_player_mut(|player| {
        player.keyboard_manager.key_down(key.clone(), code);
    });
    forward_key_to_nested(true, &key, code);
    player_dispatch(PlayerVMCommand::KeyDown(key, code));
}

#[wasm_bindgen]
pub fn key_up(key: String, code: u16) {
    // Update keyboard state immediately so keyPressed() reflects
    // real state even during long-running script handlers
    reserve_player_mut(|player| {
        player.keyboard_manager.key_up(&key, code);
    });
    forward_key_to_nested(false, &key, code);
    player_dispatch(PlayerVMCommand::KeyUp(key, code));
}

/// Mirror the keyboard state into every on-stage nested `#movie` sub-player so
/// its polling `keyPressed(code)` (e.g. g349's arrow-key list scrolling) sees
/// the same keys as the host. State-only, like `forward_mouse_to_nested` — no
/// event dispatch across the player boundary (that resolves instance ids against
/// the wrong allocator). Keyboard has no cursor position, so it goes to all
/// visible nested sub-players (typically one active game).
fn forward_key_to_nested(is_down: bool, key: &str, code: u16) {
    use crate::player::{nested_player_id, reserve_player_mut, reserve_player_ref, ACTIVE_PLAYER_ID};
    let ids = reserve_player_ref(|host| {
        let mut ids: Vec<usize> = Vec::new();
        for channel in &host.movie.score.channels {
            if !channel.sprite.visible {
                continue;
            }
            if let Some(id) = channel.sprite.member.as_ref().and_then(nested_player_id) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    });
    for id in ids {
        let prev = unsafe { ACTIVE_PLAYER_ID };
        unsafe {
            ACTIVE_PLAYER_ID = id;
        }
        reserve_player_mut(|sub| {
            if is_down {
                sub.keyboard_manager.key_down(key.to_string(), code);
            } else {
                sub.keyboard_manager.key_up(key, code);
            }
        });
        unsafe {
            ACTIVE_PLAYER_ID = prev;
        }
    }
}

// Picking mode commands bypass the command queue for synchronous access.

#[wasm_bindgen]
pub fn player_set_picking_mode(enabled: bool) {
    reserve_player_mut(|player| {
        player.picking_mode = enabled;
    });
}

#[wasm_bindgen]
pub fn player_get_sprite_at(x: f64, y: f64) -> i32 {
    reserve_player_ref(|player| {
        let (mx, my) = crate::player::stage::canvas_to_movie_coords(player, x, y);
        get_sprite_at(player, mx as i32, my as i32, false)
            .map(|n| n as i32)
            .unwrap_or(0)
    })
}

/// Check if a sprite is an editable Field or Text member (for mobile keyboard
/// focus and to gate caret/selection events from JS).
#[wasm_bindgen]
pub fn is_sprite_editable_field(sprite_id: i32) -> bool {
    reserve_player_ref(|player| {
        let sprite = player.movie.score.get_sprite(sprite_id as i16);
        let member = sprite
            .and_then(|s| s.member.as_ref())
            .and_then(|m| player.movie.cast_manager.find_member_by_ref(m));
        member.map_or(false, |m| match &m.member_type {
            CastMemberType::Field(f) => f.editable,
            CastMemberType::Text(t) => t.info.as_ref().map_or(false, |i| i.editable),
            _ => false,
        })
    })
}

/// Place the caret in an editable Field/Text at the given canvas coordinates.
/// `extend` mirrors a shift-click: extends from the existing anchor instead
/// of collapsing the selection.
#[wasm_bindgen]
pub fn field_set_caret_at(sprite_id: i32, canvas_x: f64, canvas_y: f64, extend: bool) {
    let mode = if extend {
        crate::player::keyboard_events::CaretAtMode::ExtendToAnchor
    } else {
        crate::player::keyboard_events::CaretAtMode::SetAndAnchor
    };
    let (mx, my) = reserve_player_ref(|player| {
        crate::player::stage::canvas_to_movie_coords(player, canvas_x, canvas_y)
    });
    crate::player::keyboard_events::set_caret_at_screen(
        sprite_id as i16, mx as i32, my as i32, mode,
    );
}

/// Continuation of a click-drag in an editable Field/Text — extends sel_end
/// to the position under the current pointer, sel_anchor unchanged.
#[wasm_bindgen]
pub fn field_drag_extend_to(sprite_id: i32, canvas_x: f64, canvas_y: f64) {
    let (mx, my) = reserve_player_ref(|player| {
        crate::player::stage::canvas_to_movie_coords(player, canvas_x, canvas_y)
    });
    crate::player::keyboard_events::set_caret_at_screen(
        sprite_id as i16, mx as i32, my as i32,
        crate::player::keyboard_events::CaretAtMode::DragExtend,
    );
}

/// Double-click word selection at the given canvas coordinates.
#[wasm_bindgen]
pub fn field_select_word_at(sprite_id: i32, canvas_x: f64, canvas_y: f64) {
    let (mx, my) = reserve_player_ref(|player| {
        crate::player::stage::canvas_to_movie_coords(player, canvas_x, canvas_y)
    });
    crate::player::keyboard_events::set_caret_at_screen(
        sprite_id as i16, mx as i32, my as i32,
        crate::player::keyboard_events::CaretAtMode::SelectWord,
    );
}

/// Triple-click line selection at the given canvas coordinates.
#[wasm_bindgen]
pub fn field_select_line_at(sprite_id: i32, canvas_x: f64, canvas_y: f64) {
    let (mx, my) = reserve_player_ref(|player| {
        crate::player::stage::canvas_to_movie_coords(player, canvas_x, canvas_y)
    });
    crate::player::keyboard_events::set_caret_at_screen(
        sprite_id as i16, mx as i32, my as i32,
        crate::player::keyboard_events::CaretAtMode::SelectLine,
    );
}

/// Whether a Field/Text sprite currently holds keyboard focus. Used by
/// frontend clipboard listeners to decide whether to intercept copy/paste.
#[wasm_bindgen]
pub fn is_field_focused() -> bool {
    reserve_player_ref(|player| {
        if player.keyboard_focus_sprite < 0 { return false; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let member = sprite
            .and_then(|s| s.member.as_ref())
            .and_then(|m| player.movie.cast_manager.find_member_by_ref(m));
        member.map_or(false, |m| matches!(&m.member_type,
            CastMemberType::Field(f) if f.editable
        ) || matches!(&m.member_type,
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable)
        ))
    })
}

/// Selected text from the focused editable Field/Text. Empty string if no
/// focus, no selection, or non-editable. Synchronous so it can be called
/// from a copy/cut event handler.
#[wasm_bindgen]
pub fn get_focused_field_selected_text() -> String {
    reserve_player_ref(|player| {
        if player.keyboard_focus_sprite < 0 { return String::new(); }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let member = sprite
            .and_then(|s| s.member.as_ref())
            .and_then(|m| player.movie.cast_manager.find_member_by_ref(m));
        let Some(member) = member else { return String::new() };
        let (text, lo, hi) = match &member.member_type {
            CastMemberType::Field(f) if f.editable => {
                (&f.text, f.sel_start, f.sel_end)
            }
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => {
                (&t.text, t.sel_start, t.sel_end)
            }
            _ => return String::new(),
        };
        let len = text.len() as i32;
        let lo = lo.clamp(0, len).min(hi.clamp(0, len));
        let hi = lo.max(hi.clamp(0, len));
        // Snap to char boundaries before slicing. Caret indices are byte
        // positions into a UTF-8 string; if any prior path produced one
        // landing inside a multi-byte sequence (e.g. mid-ß), a raw slice
        // would panic.
        let mut lo_b = lo as usize;
        let mut hi_b = hi as usize;
        while lo_b < text.len() && !text.is_char_boundary(lo_b) { lo_b += 1; }
        while hi_b < text.len() && !text.is_char_boundary(hi_b) { hi_b += 1; }
        text[lo_b..hi_b].to_string()
    })
}

/// Select all text in the focused editable member (Cmd/Ctrl+A).
#[wasm_bindgen]
pub fn field_select_all() {
    reserve_player_mut(|player| {
        if player.keyboard_focus_sprite < 0 { return; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let Some(member_ref) = sprite.and_then(|s| s.member.clone()) else { return };
        let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else {
            return;
        };
        let (len, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
            CastMemberType::Field(f) if f.editable => {
                (f.text.len() as i32, &mut f.sel_start, &mut f.sel_end, &mut f.sel_anchor)
            }
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => {
                (t.text.len() as i32, &mut t.sel_start, &mut t.sel_end, &mut t.sel_anchor)
            }
            _ => return,
        };
        *sel_start = 0;
        *sel_end = len;
        *sel_anchor = 0;
        player.text_selection_start = 0;
        player.text_selection_end = len.max(0) as u16;
    });
}

/// Delete the current selection in the focused editable member. Used by cut.
#[wasm_bindgen]
pub fn delete_focused_field_selection() {
    reserve_player_mut(|player| {
        if player.keyboard_focus_sprite < 0 { return; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let Some(member_ref) = sprite.and_then(|s| s.member.clone()) else { return };
        let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else {
            return;
        };
        let (text, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
            CastMemberType::Field(f) if f.editable => (
                &mut f.text, &mut f.sel_start, &mut f.sel_end, &mut f.sel_anchor,
            ),
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => (
                &mut t.text, &mut t.sel_start, &mut t.sel_end, &mut t.sel_anchor,
            ),
            _ => return,
        };
        crate::player::keyboard_events::apply_text_insertion(text, sel_start, sel_end, sel_anchor, "");
        let s = *sel_start;
        let e = *sel_end;
        player.text_selection_start = s.max(0) as u16;
        player.text_selection_end = e.max(0) as u16;
    });
}

/// Update the VM-side mirror of the OS clipboard so Lingo `the clipBoard`
/// reads back what was last copied via the JS gesture event. The OS clipboard
/// itself remains the source of truth; this is purely so scripts can observe
/// what the user just copied/cut.
#[wasm_bindgen]
pub fn set_clipboard_mirror(text: String) {
    reserve_player_mut(|player| {
        player.clipboard_mirror = text;
    });
}

/// IME composition started — record the byte offset where the provisional
/// composition will begin (= current caret position, with any selection
/// already replaced). No-op if no editable member has focus.
#[wasm_bindgen]
pub fn ime_composition_start() {
    reserve_player_mut(|player| {
        if player.keyboard_focus_sprite < 0 { return; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let Some(member_ref) = sprite.and_then(|s| s.member.clone()) else { return };
        let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else {
            return;
        };
        let (text, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
            CastMemberType::Field(f) if f.editable => (
                &mut f.text, &mut f.sel_start, &mut f.sel_end, &mut f.sel_anchor,
            ),
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => (
                &mut t.text, &mut t.sel_start, &mut t.sel_end, &mut t.sel_anchor,
            ),
            _ => return,
        };
        // Collapse any existing selection so the composition replaces it cleanly.
        if *sel_start != *sel_end {
            crate::player::keyboard_events::apply_text_insertion(text, sel_start, sel_end, sel_anchor, "");
        }
        let pos = (*sel_start).max(0);
        player.ime_composition = Some((pos, pos));
        player.text_selection_start = pos.max(0) as u16;
        player.text_selection_end = pos.max(0) as u16;
    });
}

/// IME composition update — replace the current provisional run with `text`.
/// Caret advances to the end of the new provisional text. No-op if no
/// composition is active.
#[wasm_bindgen]
pub fn ime_composition_update(text: String) {
    reserve_player_mut(|player| {
        let Some((start, end)) = player.ime_composition else { return };
        if player.keyboard_focus_sprite < 0 { return; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let Some(member_ref) = sprite.and_then(|s| s.member.clone()) else { return };
        let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else {
            return;
        };
        let (text_buf, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
            CastMemberType::Field(f) if f.editable => (
                &mut f.text, &mut f.sel_start, &mut f.sel_end, &mut f.sel_anchor,
            ),
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => (
                &mut t.text, &mut t.sel_start, &mut t.sel_end, &mut t.sel_anchor,
            ),
            _ => return,
        };
        let len = text_buf.len() as i32;
        let lo = start.clamp(0, len) as usize;
        let hi = end.clamp(0, len) as usize;
        let hi = hi.max(lo);
        text_buf.replace_range(lo..hi, &text);
        let new_end = lo as i32 + text.len() as i32;
        *sel_start = new_end;
        *sel_end = new_end;
        *sel_anchor = new_end;
        player.ime_composition = Some((start, new_end));
        player.text_selection_start = new_end.max(0) as u16;
        player.text_selection_end = new_end.max(0) as u16;
    });
}

/// IME composition committed — `text` is the final string. Same replacement
/// as update, then clears composition state. No-op if no composition is active.
#[wasm_bindgen]
pub fn ime_composition_end(text: String) {
    ime_composition_update(text);
    reserve_player_mut(|player| {
        player.ime_composition = None;
    });
}

/// Insert text at the focused editable member's caret/selection. Used by
/// paste and IME commit.
#[wasm_bindgen]
pub fn paste_text_into_focused_field(text: String) {
    reserve_player_mut(|player| {
        if player.keyboard_focus_sprite < 0 { return; }
        let sprite_id = player.keyboard_focus_sprite as i16;
        let sprite = player.movie.score.get_sprite(sprite_id);
        let Some(member_ref) = sprite.and_then(|s| s.member.clone()) else { return };
        let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) else {
            return;
        };
        let (target, sel_start, sel_end, sel_anchor) = match &mut member.member_type {
            CastMemberType::Field(f) if f.editable => (
                &mut f.text, &mut f.sel_start, &mut f.sel_end, &mut f.sel_anchor,
            ),
            CastMemberType::Text(t) if t.info.as_ref().map_or(false, |i| i.editable) => (
                &mut t.text, &mut t.sel_start, &mut t.sel_end, &mut t.sel_anchor,
            ),
            _ => return,
        };
        crate::player::keyboard_events::apply_text_insertion(target, sel_start, sel_end, sel_anchor, &text);
        let s = *sel_start;
        let e = *sel_end;
        player.text_selection_start = s.max(0) as u16;
        player.text_selection_end = e.max(0) as u16;
    });
}

// Inspector commands bypass the command queue to allow inspecting state
// while a breakpoint is active.

#[wasm_bindgen]
pub fn get_cast_chunk_list(cast_number: u32) -> JsValue {
    reserve_player_ref(|player| {
        JsApi::get_cast_chunk_list_for(player, cast_number).into()
    })
}

#[wasm_bindgen]
pub fn get_movie_top_level_chunks() -> JsValue {
    reserve_player_ref(|player| {
        JsApi::get_movie_top_level_chunks(player).into()
    })
}

#[wasm_bindgen]
pub fn get_chunk_bytes(cast_number: u32, chunk_id: u32) -> Option<Vec<u8>> {
    reserve_player_ref(|player| {
        JsApi::get_chunk_bytes(player, cast_number, chunk_id)
    })
}

#[wasm_bindgen]
pub fn clear_debug_messages() {
    reserve_player_mut(|player| {
        player.debug_datum_refs.clear();
    });
}

#[wasm_bindgen]
pub fn set_eval_scope_index(index: i32) {
    reserve_player_mut(|player| {
        player.eval_scope_index = if index >= 0 { Some(index as u32) } else { None };
    });
}

#[wasm_bindgen]
pub fn unsubscribe_from_member(cast_lib: i32, cast_member: i32) {
    let member_ref = cast_member_ref(cast_lib, cast_member);
    reserve_player_mut(|player| {
        player.subscribed_member_refs.retain(|x| x != &member_ref);
    });
}

#[wasm_bindgen]
pub fn trigger_alert_hook() {
    player_dispatch(PlayerVMCommand::TriggerAlertHook);
}

#[wasm_bindgen]
pub fn subscribe_to_channel_names() {
    crate::player::spawn_player_local(async {
        let player = unsafe { crate::player::player_mut() };

        player.is_subscribed_to_channel_names = true;
        // One message for all of them — see dispatch_all_channel_names.
        JsApi::dispatch_all_channel_names(player);
    });
}

/// The score inspector is showing: start pushing score snapshots, and send the
/// current one right away. Nothing is pushed while unsubscribed — a full score
/// snapshot is tens of thousands of objects and ordinary playback needs none of
/// it.
#[wasm_bindgen]
pub fn subscribe_to_score() {
    crate::player::spawn_player_local(async {
        let player = unsafe { crate::player::player_mut() };
        player.is_subscribed_to_score = true;
        JsApi::dispatch_score_changed();
    });
}

#[wasm_bindgen]
pub fn unsubscribe_from_score() {
    crate::player::spawn_player_local(async {
        let player = unsafe { crate::player::player_mut() };
        player.is_subscribed_to_score = false;
    });
}

/// Same deal per cast library: only the ones the cast inspector has expanded
/// get their member lists serialized.
#[wasm_bindgen]
pub fn subscribe_to_cast_member_list(cast_number: u32) {
    crate::player::spawn_player_local(async move {
        let player = unsafe { crate::player::player_mut() };
        player.subscribed_cast_member_lists.insert(cast_number);
        JsApi::dispatch_cast_member_list_changed(cast_number);
    });
}

#[wasm_bindgen]
pub fn unsubscribe_from_cast_member_list(cast_number: u32) {
    crate::player::spawn_player_local(async move {
        let player = unsafe { crate::player::player_mut() };
        player.subscribed_cast_member_lists.remove(&cast_number);
    });
}

#[wasm_bindgen]
pub fn unsubscribe_from_channel_names() {
    crate::player::spawn_player_local(async {
        let player = unsafe { crate::player::player_mut() };

        player.is_subscribed_to_channel_names = false;
    });
}

#[wasm_bindgen]
pub fn provide_net_task_data(task_id: u32, data: Vec<u8>) {
    // Directly fulfill the task without going through the command queue to avoid deadlock
    // This is safe because we only access the shared state which is behind a mutex
    crate::player::spawn_player_local(async move {
        let shared_state_arc =
            reserve_player_ref(|player| std::sync::Arc::clone(&player.net_manager.shared_state));
        let result = Ok(data);
        let mut shared_state = shared_state_arc.lock().await;
        shared_state.fulfill_task(task_id, result).await;
    });
}

#[wasm_bindgen]
pub fn provide_net_task_error(task_id: u32) {
    crate::player::spawn_player_local(async move {
        let shared_state_arc =
            reserve_player_ref(|player| std::sync::Arc::clone(&player.net_manager.shared_state));
        let result: player::net_task::NetResult = Err(4);
        let mut shared_state = shared_state_arc.lock().await;
        shared_state.fulfill_task(task_id, result).await;
    });
}

/// Receive a rendered Flash frame from JavaScript (Ruffle) and store it as a
/// per-sprite bitmap. Each Flash sprite has its own Ruffle player instance
/// (so multiple sprites that share a single Flash cast member can display
/// different frames at the same time — e.g. storyscramble's 3 story tiles
/// pinned to poster frames 2/4/6 of one shared SWF). The renderer reads
pub(crate) fn parse_flash_event_body(body: &str) -> Option<(String, Vec<String>)> {
    let trimmed = body.trim();
    let mut tokens = trimmed.split_whitespace();
    let handler = tokens.next()?.trim_start_matches('#').to_owned();
    let rest = trimmed[trimmed.find(char::is_whitespace).unwrap_or(trimmed.len())..].trim();
    let mut raw_args = Vec::new();
    if !rest.is_empty() {
        let mut current = String::new();
        let mut in_quotes = false;
        for ch in rest.chars() {
            match ch {
                '"' => {
                    in_quotes = !in_quotes;
                    current.push(ch);
                }
                ',' if !in_quotes => {
                    raw_args.push(current.trim().to_owned());
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
        raw_args.push(current.trim().to_owned());
        if raw_args.len() == 1 && !raw_args[0].starts_with('"') {
            raw_args = rest.split_whitespace().map(str::to_owned).collect();
        }
    }
    Some((handler, raw_args))
}

fn update_flash_frame_for_player(
    player: &mut player::DirPlayer,
    symbols: &mut player::symbols::symbol_table::SymbolTable,
    sprite_num: i32,
    width: u32,
    height: u32,
    rgba_data: &[u8],
) -> Result<(), JsValue> {
    use player::bitmap::bitmap::{get_system_default_palette, Bitmap, PaletteRef};

    let expected_len = (width * height * 4) as usize;
    if rgba_data.len() != expected_len {
        return Err(JsValue::from_str("Flash frame pixel length does not match dimensions"));
    }
    let mut bitmap = Bitmap::new(
        width as u16,
        height as u16,
        32,
        32,
        8,
        PaletteRef::BuiltIn(get_system_default_palette()),
    );
    bitmap.data = rgba_data.to_vec();
    bitmap.use_alpha = true;

    // A negative synthetic sprite number identifies an off-screen W3D texture.
    // Resolve its texture symbol through this player's authoritative table so
    // a callback cannot mutate a replacement session's symbol graph.
    let key = sprite_num as i16;
    if let Some((member_ref, texture_name)) = player.flash_texture_targets.get(&key).cloned() {
        let texture_symbol = symbols.intern(&texture_name);
        let mut texture_data = Vec::with_capacity(8 + rgba_data.len());
        texture_data.extend_from_slice(&width.to_le_bytes());
        texture_data.extend_from_slice(&height.to_le_bytes());
        texture_data.extend_from_slice(rgba_data);
        if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
            if let Some(w3d) = member.member_type.as_shockwave3d_mut() {
                if let Some(scene) = w3d.scene_mut() {
                    let changed = scene
                        .texture_images
                        .get(&texture_symbol)
                        .map_or(true, |old| old.len() != texture_data.len());
                    scene.texture_images.insert(texture_symbol, texture_data);
                    if changed {
                        scene.texture_content_version += 1;
                    }
                }
            }
        }
        return Ok(());
    }

    if let Some(&existing_ref) = player.flash_frame_buffers.get(&key) {
        player.bitmap_manager.replace_bitmap(existing_ref, bitmap);
    } else {
        let bitmap_ref = player.bitmap_manager.add_bitmap(bitmap);
        player.flash_frame_buffers.insert(key, bitmap_ref);
    }
    Ok(())
}

fn js_value_to_datum_ref_for_context(
    item: &JsValue,
    player: &mut player::DirPlayer,
    symbols: &mut player::symbols::symbol_table::SymbolTable,
    flash_cast_lib: i32,
    flash_cast_member: i32,
) -> player::datum_ref::DatumRef {
    use director::lingo::datum::{Datum, DatumType, FlashObjectRef};

    if item.is_null() || item.is_undefined() {
        return player.alloc_datum(Datum::Void);
    }
    if let Some(value) = item.as_string() {
        return player.alloc_datum(Datum::String(value));
    }
    if let Some(value) = item.as_f64() {
        let datum = if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
            Datum::Int(value as i32)
        } else {
            Datum::Float(value)
        };
        return player.alloc_datum(datum);
    }
    if let Some(value) = item.as_bool() {
        return player.alloc_datum(Datum::Int(if value { 1 } else { 0 }));
    }
    if js_sys::Array::is_array(item) {
        let array = js_sys::Array::from(item);
        let mut values = std::collections::VecDeque::new();
        for value in array.iter() {
            values.push_back(js_value_to_datum_ref_for_context(
                &value,
                player,
                symbols,
                flash_cast_lib,
                flash_cast_member,
            ));
        }
        return player.alloc_datum(Datum::List(DatumType::XmlChildNodes, values, false));
    }
    if item.is_object() {
        let object = js_sys::Object::from(item.clone());
        if let Ok(stored_path) = js_sys::Reflect::get(&object, &JsValue::from_str("__dirplayer_stored_path")) {
            if let Some(path) = stored_path.as_string() {
                return player.alloc_datum(Datum::FlashObjectRef(
                    FlashObjectRef::from_path_with_member(&path, flash_cast_lib, flash_cast_member),
                ));
            }
        }
        let mut properties = std::collections::VecDeque::new();
        let entries = js_sys::Object::entries(&object);
        let mut flash_type = None;
        for entry in entries.iter() {
            let pair = js_sys::Array::from(&entry);
            let key = pair.get(0).as_string().unwrap_or_default();
            let value = pair.get(1);
            if key == "#type" {
                flash_type = value.as_string();
                continue;
            }
            let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern(&key)));
            let value_ref = js_value_to_datum_ref_for_context(
                &value,
                player,
                symbols,
                flash_cast_lib,
                flash_cast_member,
            );
            properties.push_back((key_ref, value_ref));
        }
        if let Some(value) = flash_type {
            let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern("#type")));
            let value_ref = player.alloc_datum(Datum::String(value));
            properties.push_front((key_ref, value_ref));
        }
        return player.alloc_datum(Datum::PropList(properties, false));
    }
    player.alloc_datum(Datum::Void)
}

/// Check if WebGL2 is supported in the browser
#[wasm_bindgen]
pub fn is_webgl2_supported() -> bool {
    rendering_gpu::is_webgl2_supported()
}

/// Set glyph rendering preference for text/field members.
/// Values: "auto" (default), "bitmap" (PFR atlas), "native" (Canvas2D fillText),
///         "outline" (force outline rasterization, skip PFR bitmap strikes — needs clear_font_cache)
#[wasm_bindgen]
pub fn set_glyph_preference(mode: &str) {
    use player::font::{GlyphPreference, set_glyph_preference as set_pref};
    let pref = match mode.to_lowercase().as_str() {
        "bitmap" => GlyphPreference::Bitmap,
        "native" => GlyphPreference::Native,
        "outline" => GlyphPreference::Outline,
        _ => GlyphPreference::Auto,
    };
    set_pref(pref);
}

/// Get the current glyph rendering preference.
#[wasm_bindgen]
pub fn get_glyph_preference() -> String {
    use player::font::{GlyphPreference, get_glyph_preference as get_pref};
    match get_pref() {
        GlyphPreference::Auto => "auto".to_string(),
        GlyphPreference::Bitmap => "bitmap".to_string(),
        GlyphPreference::Native => "native".to_string(),
        GlyphPreference::Outline => "outline".to_string(),
    }
}

/// Clear the font cache so fonts will be re-rasterized on next use.
/// Call this after set_glyph_preference("outline") to see the effect.
#[wasm_bindgen]
pub fn clear_font_cache() {
    reserve_player_mut(|player| {
        let count = player.font_manager.font_cache.len();
        player.font_manager.font_cache.clear();
        player.font_manager.fonts.clear();
        player.font_manager.font_by_id.clear();
        player.font_manager.font_counter = 0;
        debug!("[clear_font_cache] Cleared {} cached fonts. Reload movie to re-rasterize.", count);
    });
}

/// Get the current renderer backend name
#[wasm_bindgen]
pub fn get_renderer_backend() -> String {
    use rendering_gpu::Renderer;
    rendering::with_renderer_mut(|renderer_lock| {
        if let Some(renderer) = renderer_lock {
            renderer.backend_name().to_string()
        } else {
            "none".to_string()
        }
    })
}

/// Download raw W3D/IFX data for external testing
fn export_w3d_obj_for_player(
    player: &player::DirPlayer,
    symbols: &player::symbols::symbol_table::SymbolTable,
    cast_lib: i32,
    cast_member: i32,
) -> Result<(), JsValue> {
    let member_ref = CastMemberRef { cast_lib, cast_member };
    let member = player
        .movie
        .cast_manager
        .find_member_by_ref(&member_ref)
        .ok_or_else(|| JsValue::from_str(&format!("Member {}:{} not found", cast_lib, cast_member)))?;
    let w3d = member
        .member_type
        .as_shockwave3d()
        .ok_or_else(|| JsValue::from_str("Not a Shockwave3D member"))?;
    let scene = w3d
        .parsed_scene
        .as_ref()
        .ok_or_else(|| JsValue::from_str("No parsed 3D scene"))?;
    let name = if member.name.is_empty() {
        format!("member_{}_{}", cast_lib, cast_member)
    } else {
        member.name.replace(' ', "_")
    };
    let mtl_filename = format!("{}.mtl", name);
    let obj_data = scene
        .export_obj_with_mtl(&mtl_filename, symbols)
        .map_err(|error| JsValue::from_str(&error))?;
    let mtl_data = scene
        .export_mtl(&mtl_filename, symbols)
        .map_err(|error| JsValue::from_str(&error))?;
    let glb_data = crate::director::chunks::w3d::gltf_export::export_glb(scene, symbols)
        .map_err(|error| JsValue::from_str(&error))?;
    let obj_filename = format!("{}.obj", name);
    let glb_filename = format!("{}.glb", name);
    let zip_data = build_zip_with_glb(
        &obj_filename,
        obj_data.as_bytes(),
        &mtl_filename,
        mtl_data.as_bytes(),
        &glb_filename,
        &glb_data,
        &scene.texture_images,
        symbols,
    );
    trigger_browser_download(&format!("{}.zip", name), &zip_data, "application/zip");
    Ok(())
}

/// Build a minimal uncompressed ZIP file containing OBJ + MTL + textures
fn build_zip_with_glb(
    obj_name: &str, obj_data: &[u8],
    mtl_name: &str, mtl_data: &[u8],
    glb_name: &str, glb_data: &[u8],
    textures: &std::collections::HashMap<crate::player::symbols::symbol::Symbol, Vec<u8>>,
    symbols: &player::symbols::symbol_table::SymbolTable,
) -> Vec<u8> {
    let mut files: Vec<(String, &[u8])> = Vec::new();
    files.push((obj_name.to_string(), obj_data));
    files.push((mtl_name.to_string(), mtl_data));
    files.push((glb_name.to_string(), glb_data));

    for (tex_name, image_data) in textures {
        let tex_name = symbols.display(tex_name).unwrap_or("foreign-texture");
        let ext = if image_data.len() >= 2 && image_data[0] == 0xFF && image_data[1] == 0xD8 {
            "jpg"
        } else if image_data.len() >= 2 && image_data[0] == 0x89 && image_data[1] == 0x50 {
            "png"
        } else {
            "bin"
        };
        files.push((format!("{}.{}", tex_name, ext), image_data));
    }

    build_zip_files(&files)
}

/// Build a minimal uncompressed (stored) ZIP from a list of (name, bytes).
fn build_zip_files(files: &[(String, &[u8])]) -> Vec<u8> {
    let mut zip = Vec::new();
    let mut central_dir = Vec::new();
    let mut offsets: Vec<u32> = Vec::new();

    // Write local file headers + data
    for (name, data) in files {
        offsets.push(zip.len() as u32);
        let name_bytes = name.as_bytes();
        let crc = crc32(data);

        // Local file header (0x04034b50)
        zip.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes());  // flags
        zip.extend_from_slice(&0u16.to_le_bytes());  // compression (0=stored)
        zip.extend_from_slice(&0u16.to_le_bytes());  // mod time
        zip.extend_from_slice(&0u16.to_le_bytes());  // mod date
        zip.extend_from_slice(&crc.to_le_bytes());   // crc32
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed size
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes()); // name length
        zip.extend_from_slice(&0u16.to_le_bytes());  // extra length
        zip.extend_from_slice(name_bytes);
        zip.extend_from_slice(data);
    }

    // Write central directory
    let cd_offset = zip.len() as u32;
    for (i, (name, data)) in files.iter().enumerate() {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);

        central_dir.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // signature
        central_dir.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central_dir.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // flags
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // compression
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // mod time
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // mod date
        central_dir.extend_from_slice(&crc.to_le_bytes());   // crc32
        central_dir.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed size
        central_dir.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed size
        central_dir.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes()); // name length
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // extra length
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // comment length
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // disk number
        central_dir.extend_from_slice(&0u16.to_le_bytes());  // internal attrs
        central_dir.extend_from_slice(&0u32.to_le_bytes());  // external attrs
        central_dir.extend_from_slice(&offsets[i].to_le_bytes()); // local header offset
        central_dir.extend_from_slice(name_bytes);
    }

    zip.extend_from_slice(&central_dir);

    // End of central directory
    zip.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // signature
    zip.extend_from_slice(&0u16.to_le_bytes());  // disk number
    zip.extend_from_slice(&0u16.to_le_bytes());  // cd disk number
    zip.extend_from_slice(&(files.len() as u16).to_le_bytes()); // entries on disk
    zip.extend_from_slice(&(files.len() as u16).to_le_bytes()); // total entries
    zip.extend_from_slice(&(central_dir.len() as u32).to_le_bytes()); // cd size
    zip.extend_from_slice(&cd_offset.to_le_bytes()); // cd offset
    zip.extend_from_slice(&0u16.to_le_bytes());  // comment length

    zip
}

/// Simple CRC32 (used for ZIP file entries)
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn trigger_browser_download(filename: &str, data: &[u8], mime_type: &str) {
    use js_sys::{Array, Uint8Array};
    use wasm_bindgen::JsCast;

    let uint8_array = Uint8Array::new_with_length(data.len() as u32);
    uint8_array.copy_from(data);

    let array = Array::new();
    array.push(&uint8_array.buffer());

    let mut options = web_sys::BlobPropertyBag::new();
    options.type_(mime_type);

    let blob = match web_sys::Blob::new_with_buffer_source_sequence_and_options(&array, &options) {
        Ok(b) => b,
        Err(_) => return,
    };

    let url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(u) => u,
        Err(_) => return,
    };

    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let a = match document.create_element("a") {
        Ok(el) => el,
        Err(_) => return,
    };

    let _ = a.set_attribute("href", &url);
    let _ = a.set_attribute("download", filename);
    let _ = a.set_attribute("style", "display:none");

    let body = match document.body() {
        Some(b) => b,
        None => return,
    };
    let _ = body.append_child(&a);

    if let Some(html_el) = a.dyn_ref::<web_sys::HtmlElement>() {
        html_el.click();
    }

    let _ = body.remove_child(&a);
    let _ = web_sys::Url::revoke_object_url(&url);
}

// ============================================================================
// MCP (Model Context Protocol) functions for VM debugging
// These functions return JSON strings and are used by the MCP server
// ============================================================================

/// Set whether PFR font rasterization is enabled
#[wasm_bindgen]
pub fn set_pfr_font_enabled(enabled: bool) {
    reserve_player_mut(|player| {
        player.font_manager.pfr_enabled = enabled;
        player.font_manager.font_cache.clear();
    });
}

/// Get whether PFR font rasterization is enabled
#[wasm_bindgen]
pub fn get_pfr_font_enabled() -> bool {
    reserve_player_ref(|player| player.font_manager.pfr_enabled)
}

#[wasm_bindgen(start)]
pub fn start() {
    set_panic_hook();
    // In test mode, BrowserTestPlayer::new() handles initialization
    // with fresh state for each test. Skip init_player() here to avoid
    // spawning command/event loops that interfere with the test harness.
    #[cfg(target_arch = "wasm32")]
    {
        let is_test = web_sys::window()
            .and_then(|w| js_sys::Reflect::get(&w, &"__dirplayerTestMode".into()).ok())
            .map_or(false, |v| v.is_truthy());
        if is_test {
            return;
        }
    }
    init_player();
}
