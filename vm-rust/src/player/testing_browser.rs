use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use manual_future::ManualFuture;

use crate::player::{
    cast_lib::CastMemberRef,
    commands::PlayerVMCommand,
    geometry::IntRect,
    score::SpriteChannel,
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

/// Browser test harness with an owned command and frame scheduler.
/// The wasm test entrypoint skips the production global loops, so each RAF
/// step explicitly drives the supplied session's timeout and frame turns.
pub struct BrowserTestPlayer {
    runtime: HarnessRuntime,
    renderer: crate::rendering::RendererStateHandle,
    command_tx: async_std::channel::Sender<crate::player::PlayerVMExecutionItem>,
    flash_capability: Option<BrowserFlashCapability>,
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
            ).await;
        });
        let renderer = crate::rendering::new_renderer_state();
        runtime.session().borrow_mut().bind_renderer_state(runtime.player_id(), &renderer);
        let mut harness = BrowserTestPlayer { runtime, renderer, command_tx, flash_capability: None };
        // `preserve_external_params: false` — this is the per-TEST boundary, and
        // one movie's external params are never right for the next movie.
        harness.reset_player_with(false).await;
        harness
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
                context.player.movie.score.channels =
                    vec![SpriteChannel::new(0), lower, top];
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
                return result.map_err(|error| error.message)
            }
            _ => return Err(format!("SysMenu global {name} did not produce a typed host intent")),
        };
        crate::player::xtra::manager::execute_pending_intent(&session, player_id, intent)
            .await
            .map_err(|error| error.message)
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
        js_sys::Reflect::set(
            &console,
            &JsValue::from_str("log"),
            log_closure.as_ref(),
        )
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
        js_sys::Reflect::set(
            &window,
            &JsValue::from_str("alert"),
            alert_closure.as_ref(),
        )
        .map_err(|_| "could not install SysMenu alert hook".to_owned())?;

        let result = async {
            let print_args = self
                .harness_runtime()
                .with_context(|mut context| {
                    vec![context.player.alloc_datum(
                        crate::director::lingo::datum::Datum::String("print fixture".to_owned()),
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
                        context.player.alloc_datum(
                            crate::director::lingo::datum::Datum::String("message fixture".to_owned()),
                        ),
                        context.player.alloc_datum(
                            crate::director::lingo::datum::Datum::String("caption fixture".to_owned()),
                        ),
                        context.player.alloc_datum(crate::director::lingo::datum::Datum::Int(1)),
                    ]
                })
                .ok_or_else(|| "SysMenu player disappeared before message box".to_owned())?;
            let late_result = self
                .dispatch_sysmenu_global("sysMenuMessageBox", message_box_args)
                .await;
            if late_result.is_ok() {
                return Err("SysMenu message box completed after reset".to_owned());
            }
            let reset_observed = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__sysmenuResetObserved"),
            )
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

        let _ = js_sys::Reflect::set(
            &console,
            &JsValue::from_str("log"),
            &original_log,
        );
        let _ = js_sys::Reflect::set(
            &window,
            &JsValue::from_str("alert"),
            &original_alert,
        );
        let _ = js_sys::Reflect::delete_property(
            &window,
            &JsValue::from_str("__sysmenuResetDuringAlert"),
        );
        let _ = js_sys::Reflect::delete_property(
            &window,
            &JsValue::from_str("__sysmenuResetObserved"),
        );

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
    async fn resolve_movie_xtras() {
        let Some(window) = web_sys::window() else { return };
        let Ok(hook) = js_sys::Reflect::get(&window, &JsValue::from_str("dirplayer_resolveAndLoadMovieXtras")) else { return };
        let Ok(func) = hook.dyn_into::<js_sys::Function>() else { return };
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

    fn register_flash_owner(&mut self) {
        let capability = BrowserFlashCapability::new(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
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
            self.command_tx.clone(),
        );
        let owner_key = capability.owner_identity();
        if let Some(window) = web_sys::window() {
            if let Ok(value) = js_sys::Reflect::get(&window, &JsValue::from_str("dirplayer_registerFlashOwner")) {
                if let Ok(function) = value.dyn_into::<js_sys::Function>() {
                    let js_capability: JsValue = js_capability.into();
                    let _ = function.call2(&window, &JsValue::from_str(&owner_key), &js_capability);
                }
            }
        }
        self.flash_capability = Some(capability);
    }

    fn unregister_flash_owner(&mut self, owner_key: &str) {
        if let Some(window) = web_sys::window() {
            if let Ok(value) = js_sys::Reflect::get(&window, &JsValue::from_str("dirplayer_unregisterFlashOwner")) {
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
                    (context.player.startup_do.clone(), context.player.startup_do_before.clone(), context.player.startup_go)
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
        // JS-Lingo runtimes live in a thread_local, so dropping the old player
        // below does NOT free them — clear them explicitly.
        crate::player::js_lingo_loader::clear_all_runtimes();

        // Retire the owner before yielding so no in-flight handler can resume
        // against the replacement player. Keep the original RAF drain: these
        // yields let stale browser tasks observe retirement and exit without
        // advancing Director simulation during teardown.
        let had_active_handler = self.harness_runtime()
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
        self.runtime.session().borrow_mut().bind_renderer_state(self.runtime.player_id(), &self.renderer);
        let command_session = self.runtime.session();
        let command_player_id = self.runtime.player_id();
        let command_owner = self.runtime.owner().clone();
        crate::player::spawn_player_local(async move {
            crate::player::commands::run_command_loop(rx, command_session, command_player_id, command_owner).await;
        });
        self.register_flash_owner();

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

        // Load the system font (required for text rendering)
        crate::player::font::player_load_system_font("/assets/charmap-system.png").await;
    }

    /// Wait for the next animation frame.
    async fn next_frame() {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window().unwrap()
                .request_animation_frame(&resolve)
                .unwrap();
        });
        let _ = JsFuture::from(promise).await;
    }

    /// Sleep for the given number of milliseconds.
    async fn sleep_ms(ms: u32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window().unwrap()
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
        ).unwrap_or_else(|error| panic!("test renderer creation failed: {:?}", error));
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
    async fn connect_multiuser(&self, base_url: &str, label: &str) -> Result<MultiuserSocketProbe, String> {
        let url = web_sys::Url::new(&format!("{}/{}", base_url.trim_end_matches('/'), label))
            .map_err(|_| "invalid Multiuser test WebSocket URL".to_owned())?;
        let host = url.hostname();
        let port = if url.port().is_empty() {
            if url.protocol() == "wss:" { 443 } else { 80 }
        } else {
            url.port().parse::<i32>().map_err(|_| "invalid Multiuser test port".to_owned())?
        };
        let path = {
            let mut path = url.pathname();
            let search = url.search();
            if !search.is_empty() { path.push_str(&search); }
            path
        };
        let (owner, instance_id, generation) = self
            .harness_runtime()
            .with_context(|context| {
                let owner = context.player.owner.clone();
                let instance_id = context.player.xtra_manager_state.multiuser.create_instance(&Vec::new());
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
        Ok(MultiuserSocketProbe { sender, owner, instance_id, generation })
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

    async fn inject_stale_multiuser_event(&self, probe: &MultiuserSocketProbe) -> Result<(), String> {
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
            .or_else(|| base_url.strip_prefix("wss://").map(|rest| format!("https://{rest}/state")))
            .ok_or_else(|| "invalid Multiuser test state URL".to_owned())?;
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&http_url))
            .await
            .map_err(|_| "Multiuser test state request failed".to_owned())?;
        let response: web_sys::Response = response
            .dyn_into()
            .map_err(|_| "Multiuser test state response was not a Response".to_owned())?;
        let body = wasm_bindgen_futures::JsFuture::from(
            response.text().map_err(|_| "Multiuser test state body failed".to_owned())?,
        )
        .await
        .map_err(|_| "Multiuser test state body failed".to_owned())?;
        let body = body.as_string().ok_or_else(|| "Multiuser test state was not text".to_owned())?;
        let value = js_sys::JSON::parse(&body).map_err(|_| "Multiuser test state was not JSON".to_owned())?;
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

    async fn wait_for_multiuser_text(&self, instance_id: u32, expected: &str) -> Result<(), String> {
        for _ in 0..40 {
            if self.has_multiuser_text(instance_id, expected) {
                return Ok(());
            }
            Self::sleep_ms(50).await;
        }
        Err(format!("timed out waiting for Multiuser message {expected}"))
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
        let base_url = value.as_string().ok_or_else(|| "Multiuser test server URL is not text".to_owned())?;

        let first = self.connect_multiuser(&base_url, "A").await?;
        let mut second = BrowserTestPlayer::new().await;
        let second_socket = second.connect_multiuser(&base_url, "B").await?;
        Self::wait_for_multiuser_state(&base_url, |state| state.opened_a && state.opened_b).await?;
        self.wait_for_multiuser_text(first.instance_id, "A-server-message").await?;
        second.wait_for_multiuser_text(second_socket.instance_id, "B-server-message").await?;

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
        }).await?;
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
        let base = value.as_string().ok_or_else(|| "FileIO fixture URL is not text".to_owned())?;
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
                let receiver = context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::XtraInstance(
                        "FileIO".to_owned(),
                        instance_id,
                    ));
                let file_name = context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::String("fileio.txt".to_owned()));
                let mode = context.player.alloc_datum(crate::director::lingo::datum::Datum::Int(1));
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
                return Err("remote FileIO open unexpectedly completed synchronously".to_owned())
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
    fn harness_runtime(&self) -> &HarnessRuntime { &self.runtime }

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
        let _ = self.harness_runtime().dispatch(PlayerVMCommand::MouseDown((x, y))).await;
        self.step_frame().await;
        self.harness_runtime().with_context(|context| {
            context.player.mouse_loc = (x, y);
            context.player.movie.mouse_down = false;
        });
        let _ = self.harness_runtime().dispatch(PlayerVMCommand::MouseUp((x, y))).await;
    }

    async fn key_down(&mut self, key: &str, code: u16) {
        self.harness_runtime().with_context(|context| {
            context.player.keyboard_manager.key_down(key.to_string(), code);
        });
        let _ = self.harness_runtime().dispatch(PlayerVMCommand::KeyDown(key.to_string(), code)).await;
    }

    async fn key_up(&mut self, key: &str, code: u16) {
        self.harness_runtime().with_context(|context| {
            context.player.keyboard_manager.key_up(key, code);
        });
        let _ = self.harness_runtime().dispatch(PlayerVMCommand::KeyUp(key.to_string(), code)).await;
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

    async fn snapshot_sprite_isolated(&self, query: impl Into<SpriteQuery>) -> Result<SnapshotOutput, String> {
        self.ensure_renderer();

        let query = query.into();
        let sprite_num = self.find_sprite(&query)
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
        let val = js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("__scriptErrors")).ok()?;
        let arr: js_sys::Array = wasm_bindgen::JsCast::dyn_into(val).ok()?;
        if arr.length() == 0 { return None; }
        arr.shift().as_string()
    }

    /// Capture the current canvas contents as a base64 PNG snapshot.
    fn canvas_to_snapshot(state: &crate::rendering::RendererStateHandle) -> SnapshotOutput {
        let data_url = crate::rendering::canvas_data_url_for_handle(state)
            .unwrap_or_else(|error| panic!("renderer snapshot failed: {:?}", error));

        let base64 = data_url.strip_prefix("data:image/png;base64,")
            .unwrap_or(&data_url)
            .to_string();
        SnapshotOutput::Base64Png(base64)
    }
}
