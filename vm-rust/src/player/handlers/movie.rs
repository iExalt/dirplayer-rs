use log::{debug, warn, error};
use wasm_bindgen::prelude::wasm_bindgen;
use crate::{
    director::lingo::datum::{Datum, DatumType},
    player::{
        compare::validate_direct_symbol_fields, ownership::OwnerToken, session::{ExecutionContext, PlayerId, RuntimeSessionHandle},
        DatumRef, MovieFrameTarget, Score, ScriptError, ScriptErrorCode, ScriptInstanceRef, cast_lib::{CastMemberRef, INVALID_CAST_MEMBER_REF, NULL_CAST_MEMBER_REF}, datum_formatting::format_datum, events::{
            dispatch_event_to_all_behaviors, player_dispatch_event_beginsprite, player_invoke_static_event, player_invoke_targeted_event, player_wait_available
        }, get_score_sprite_mut, handlers::datum_handlers::player_call_datum_handler, reserve_player_mut, reserve_player_mut_async, reserve_player_ref, score::{concrete_sprite_hit_test, get_sprite_at}, symbols::{builtin::BuiltInSymbol, symbol::Symbol}
    },
    utils::log_i,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MovieAsyncKind {
    Do,
    Go,
    Play,
    UpdateStage { now_ms: f64 },
    Nothing,
    FrameUpdate { now_ms: f64 },
    /// Initialization phases are split around StartMovie. Director runs the
    /// frame preparation work first, then StartMovie, then EnterFrame/ExitFrame.
    InitPrepareFrame { now_ms: f64 },
    InitEnterFrame,
    InitExitFrame,
    InitEnterExitFrame,
}

/// Owned movie work handed from global dispatch to the session executor.
/// Driver/evaluator layers attach their own completion capability; this
/// payload carries only the player lifetime fence and validated arguments.
#[derive(Clone)]
pub(crate) struct MovieAsyncRequest {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) kind: MovieAsyncKind,
    pub(crate) args: Vec<DatumRef>,
}

pub struct MovieHandlers {}

#[wasm_bindgen(js_name = "dirplayer_rufflePlay")]
extern "C" {
    fn movie_ruffle_play(sprite_num: i32);
}

impl MovieHandlers {
    pub(crate) fn prepare_movie_async(
        runtime: &mut ExecutionContext<'_>,
        player_id: PlayerId,
        kind: MovieAsyncKind,
        args: &[DatumRef],
    ) -> Result<MovieAsyncRequest, ScriptError> {
        if !runtime.player.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "movie request owner is retired".to_owned(),
            ));
        }
        if args.len() > 0 {
            for arg in args {
                Self::checked_datum(runtime, arg)?;
            }
        }
        if kind == MovieAsyncKind::Do {
            let code = args
                .first()
                .map(|arg| Self::checked_datum(runtime, arg)?.string_value(runtime.symbols))
                .transpose()?
                .unwrap_or_default();
            if code.trim().is_empty() {
                return Ok(MovieAsyncRequest {
                    player_id,
                    owner: runtime.player.owner.clone(),
                    kind,
                    args: Vec::new(),
                });
            }
        }
        Ok(MovieAsyncRequest {
            player_id,
            owner: runtime.player.owner.clone(),
            kind,
            args: args.to_vec(),
        })
    }

    fn checked_datum<'a>(runtime: &'a ExecutionContext<'_>, datum_ref: &DatumRef) -> Result<&'a Datum, ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => runtime.player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
                ScriptError::new_code(ScriptErrorCode::InvalidReference, format!("invalid datum reference {datum_ref}"))
            })?,
        };
        validate_direct_symbol_fields(datum, runtime.symbols)?;
        Ok(datum)
    }

    pub fn puppet_tempo(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let tempo = Self::checked_datum(runtime, &args[0])?.int_value()? as u32;
        runtime.player.movie.puppet_tempo = tempo;
        Ok(DatumRef::Void)
    }

    pub fn script(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let identifier = Self::checked_datum(runtime, &args[0])?.clone();
        let formatted_id = format_datum(&args[0], runtime.symbols, runtime.player)?;

        let member_ref = match identifier {
            Datum::String(script_name) => Ok(runtime.player
                .movie
                .cast_manager
                .find_member_ref_by_name(&script_name)),
            Datum::Int(script_num) => Ok(runtime.player
                .movie
                .cast_manager
                .find_member_ref_by_number(script_num as u32)),
            Datum::CastMember(cast_member_ref) => Ok(Some(cast_member_ref.clone())),
            _ => Err(ScriptError::new(format!(
                "Invalid identifier for script: {}",
                formatted_id
            ))), // TODO
        }?;
        let script = member_ref
            .to_owned()
            .and_then(|r| runtime.player.movie.cast_manager.get_script_by_ref(&r));

        match script {
            Some(_) => Ok(runtime.player.alloc_datum(Datum::ScriptRef(member_ref.unwrap()))),
            None => Err(ScriptError::new(format!(
                "Script not found {}",
                formatted_id
            ))),
        }
    }

    pub fn member(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.len() > 2 {
            return Err(ScriptError::new(
                "Too many arguments for member".to_string(),
            ));
        }
        let member_name_or_num_ref = args.get(0).ok_or_else(|| ScriptError::new("member requires an identifier".to_string()))?;
        let member_name_or_num = Self::checked_datum(runtime, member_name_or_num_ref)?.clone();
        if let Datum::CastMember(_) = &member_name_or_num {
            return Ok(member_name_or_num_ref.clone());
        }
        if let Some(cast_ref) = args.get(1) {
            Self::checked_datum(runtime, cast_ref)?;
        }
        let player = &mut *runtime.player;
        {
            let cast_name_or_num = args.get(1).map(|x| player.get_datum(x));
            let member = player.movie.cast_manager.find_member_ref_by_identifiers(
                runtime.symbols,
                &member_name_or_num,
                cast_name_or_num,
                &player.allocator,
            )?;
            if let Some(member) = member {
                Ok(player.alloc_datum(Datum::CastMember(member.to_owned())))
            } else if let Some(cast_datum) = cast_name_or_num {
                // Director returns a valid ref for member(N, castLib) even if the
                // member slot is empty (used by getDynamicSlot pattern).
                // Only for positive member numbers — negative/zero are invalid refs.
                let cast_num = match cast_datum {
                    Datum::String(s) => player.movie.cast_manager.get_cast_by_name(s)
                        .map(|c| c.number as i32),
                    Datum::Int(n) => Some(*n),
                    Datum::CastLib(n) => Some(*n as i32),
                    _ => None,
                };
                if let (Some(cast_num), Ok(member_num)) = (cast_num, member_name_or_num.int_value()) {
                    if member_num > 0 {
                        Ok(player.alloc_datum(Datum::CastMember(CastMemberRef {
                            cast_lib: cast_num,
                            cast_member: member_num,
                        })))
                    } else if member_num == 0 && cast_num == 0 {
                        // `member(0, 0)` — BOTH args zero — is Director's NULL
                        // member reference; every other non-positive form keeps
                        // the invalid sentinel it had before, so the
                        // getDynamicSlot pattern above is untouched.
                        //
                        // It is the value an EMPTY sprite channel reports for
                        // its member, so it must equal NULL_CAST_MEMBER_REF.
                        // Returning the invalid sentinel made every empty
                        // channel look occupied to the standard score scan.
                        //
                        // Merlin's Revenge 3 snapshots a screen with
                        //   repeat with i = 1 to the lastChannel
                        //     if sprite(i).member <> member(0, 0) then
                        //       ... thelist.append(nMark)
                        // With lastChannel at 1005 that produced ~1005 marks
                        // instead of a few dozen, and drawing the screen then
                        // requested 1010 sprites from a 999-sprite pool,
                        // exhausting it on the first screen. Everything
                        // downstream got VOID for its sprite.
                        Ok(player.alloc_datum(Datum::CastMember(NULL_CAST_MEMBER_REF)))
                    } else {
                        Ok(player.alloc_datum(Datum::CastMember(INVALID_CAST_MEMBER_REF)))
                    }
                } else {
                    Ok(player.alloc_datum(Datum::CastMember(INVALID_CAST_MEMBER_REF)))
                }
            } else {
                // `member(N)` with no cast lib and no existing member: Director
                // returns a valid BY-NUMBER reference to member N of the default
                // cast even when that slot is empty — `member(19)` IS member 19,
                // allocated or not (11.5 Scripting Dictionary: a numbered
                // reference addresses a slot; it does not require the slot to be
                // filled). Without this the ref was INVALID (-1,-1), so
                // `new(#bitmap, member(19))` couldn't target slot 19 (it fell
                // back to the first free slot) and `member(19).image = ...`
                // errored. Tetris reserves empty slots 10-14 / 19-21 and fills
                // them at runtime with exactly that pattern. A failed name
                // lookup or a non-positive number stays invalid.
                let numeric = match &member_name_or_num {
                    Datum::Int(n) => Some(*n),
                    Datum::Float(f) => Some(*f as i32),
                    _ => None,
                };
                match numeric {
                    Some(n) if n > 0 => {
                        // High 16 bits may encode the cast lib (slot-number
                        // form); a bare member number has cast_lib 0 → default
                        // to the first cast (mirrors member_ref_from_slot_number).
                        let cast_lib = (n >> 16) as i32;
                        let cast_member = (n & 0xFFFF) as i32;
                        let cast_lib = if cast_lib == 0 { 1 } else { cast_lib };
                        Ok(player.alloc_datum(Datum::CastMember(CastMemberRef {
                            cast_lib,
                            cast_member,
                        })))
                    }
                    _ => Ok(player.alloc_datum(Datum::CastMember(INVALID_CAST_MEMBER_REF))),
                }
            }
        }
    }

    /// Compatibility entry point for legacy manager callers.  The actual go
    /// transition is owner-bound and shared with global dispatch; retaining a
    /// second ambient implementation here would bypass its lifecycle checks.
    pub async fn go(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let session = crate::player::retained_session_handle()
            .ok_or_else(|| ScriptError::new("runtime session is not initialized".to_owned()))?;
        let player_id = crate::player::active_player_id() as PlayerId;
        let request = session
            .borrow_mut()
            .with_player(player_id, |mut context| {
                Self::prepare_movie_async(&mut context, player_id, MovieAsyncKind::Go, args)
            })
            .ok_or_else(cancelled_scope_error)??;
        debug!("legacy go entry routed through owner-bound movie executor for player {}", player_id);
        execute_movie_async(session, request).await
    }

    pub fn puppet_sprite(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let sprite_number = Self::checked_datum(runtime, &args[0])?.int_value()?;
        let is_puppet = Self::checked_datum(runtime, &args[1])?.int_value()? == 1;
        let player = &mut *runtime.player;
        {

            if !is_puppet {
                // Director defers reverting an unpuppeted sprite to the Score:
                // the sprite keeps its current member/position/appearance until
                // the NEXT frame update (begin_sprites / per-frame delta) reloads
                // the channel from the score. Clearing the visual state here is
                // wrong — it destroys a sprite the script is still reading in the
                // same handler.
                //
                // BrickOut's newLevel does, per brick channel:
                //     makeStage()                  -- set the memberNum (scene)
                //     puppetSprite(i, 0)           -- unpuppet
                //     puppetSprite(i, 1)           -- re-puppet
                //     add(vListRect, the rect of sprite i)
                // With the old clear, puppetSprite(i,0) wiped the member/loc the
                // score+makeStage had established, so `the rect` read (0,0,0,0)
                // and brick collision never fired. Preserving the visual state
                // (and letting the score re-drive it on the next frame if it is
                // NOT re-puppeted) matches Director's deferred reversion.
                //
                // Only the puppet flag and behavior lifecycle are cleared:
                // `entered = false` lets the next begin_sprites re-enter the span
                // and reload from the score (the deferred revert) when the sprite
                // stays unpuppeted; if it is re-puppeted first, it keeps its data.
                let sprite = player.movie.score.get_sprite_mut(sprite_number as i16);
                sprite.puppet = false;
                sprite.entered = false;
                sprite.exited = false;
                sprite.script_instance_list.clear();
                // Mark for revert on the NEXT frame tick. If the sprite is not
                // re-puppeted / re-membered before then it reverts to the Score
                // (a reset to empty for a pure-puppet channel with no span). This
                // matches Director's deferred revert AND keeps BrickOut working
                // (it re-puppets in the same handler, clearing the flag), while
                // fixing Coke Studios' WallItems which unpuppet furniture WITHOUT
                // setting visible=0 and relied on the old immediate wipe to clear.
                sprite.pending_unpuppet_revert = true;
                player.movie.score.invalidate_render_channel_cache();
                player.refresh_stage_behavior_channel_cache_entry(sprite_number as i16);
                player.invalidate_active_stage_filmloop_cache();
                return Ok(DatumRef::Void);
            }

            let sprite = player.movie.score.get_sprite_mut(sprite_number as i16);
            sprite.puppet = is_puppet;
            // Re-puppeted before the deferred revert ran → keep its data.
            sprite.pending_unpuppet_revert = false;
            player.movie.score.invalidate_render_channel_cache();
            player.refresh_stage_behavior_channel_cache_entry(sprite_number as i16);
            player.invalidate_active_stage_filmloop_cache();
            Ok(DatumRef::Void)
        }
    }

    pub fn sprite(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let sprite_datum = Self::checked_datum(runtime, &args[0])?.clone();
        let player = &mut *runtime.player;
        {
            // "nameOrNum Required. A string or integer that specifies the name
            // or index position of the sprite." (Director 11.5 Scripting
            // Dictionary, `sprite()`). A string names a span — the Property
            // Inspector's sprite name, stored in the score alongside the
            // span's behaviors.
            //
            // Without this a name fell through to `int_value()`, whose String
            // arm is `s.parse().unwrap_or(0)`, so every named lookup silently
            // became sprite 0 — no error, just a reference to nothing. Age of
            // Speed 2 builds its whole menu through
            // `script("OffGame").new(8, 6, 15, "off_game_sprite", 10)` and then
            // `getVariable(sprite(pFlashSprite), "_level0", 0)`, so the Flash
            // handle came back VOID, `StartOffGame` was never delivered to the
            // SWF, and the movie sat on its menu frame with a blank stage.
            let datum = &sprite_datum;
            if let Datum::String(name) = datum {
                let name = name.clone();
                if let Some(number) = player.movie.score.find_sprite_number_by_name(&name) {
                    return Ok(player.alloc_datum(Datum::SpriteRef(number)));
                }
                // Documented miss behaviour: scriptExecutionStyle 10 (the
                // default for D10+ movies, which is where named sprites appear)
                // returns VOID; style 9 returns sprite 1.
                return Ok(DatumRef::Void);
            }
            let sprite_number = datum.int_value()?;
            Ok(player.alloc_datum(Datum::SpriteRef(sprite_number as i16)))
        }
    }

    pub fn external_param_count(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let count = runtime.player.external_params.len() as i32;
        Ok(runtime.player.alloc_datum(Datum::Int(count)))
    }

    pub fn external_param_name(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let datum = Self::checked_datum(runtime, &args[0])?.clone();
        let player = &mut *runtime.player;
        {

            // Case 1: argument is a string (lookup by name, case-insensitive)
            if let Ok(key) = datum.string_value(runtime.symbols) {
                if player
                    .external_params
                    .keys()
                    .any(|k| k.to_lowercase() == key.to_lowercase())
                {
                    return Ok(player.alloc_datum(Datum::String(key)));
                } else {
                    return Ok(player.alloc_datum(Datum::Void));
                }
            }

            // Case 2: argument is an integer (index)
            if let Ok(index) = datum.int_value() {
                if index > 0 && (index as usize) <= player.external_params.len() {
                    if let Some((key, _)) = player.external_params.iter().nth(index as usize - 1) {
                        return Ok(player.alloc_datum(Datum::String(key.clone())));
                    }
                }
                return Ok(player.alloc_datum(Datum::Void));
            }

            // Invalid argument type
            log_i("external_param_name(): invalid argument type, returning Void");
            Ok(player.alloc_datum(Datum::Void))
        }
    }

    pub fn external_param_value(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let datum = Self::checked_datum(runtime, &args[0])?.clone();
        let player = &mut *runtime.player;
        {

            // Case 1: argument is a string (lookup by name)
            if let Ok(key) = datum.string_value(runtime.symbols) {
                if let Some((_k, value)) = player
                    .external_params
                    .iter()
                    .find(|(k, _)| k.to_lowercase() == key.to_lowercase())
                {
                    return Ok(player.alloc_datum(Datum::String(value.clone())));
                } else {
                    return Ok(player.alloc_datum(Datum::Void));
                }
            }

            // Case 2: argument is an integer (index)
            if let Ok(index) = datum.int_value() {
                if index > 0 && (index as usize) <= player.external_params.len() {
                    if let Some((_key, value)) =
                        player.external_params.iter().nth(index as usize - 1)
                    {
                        return Ok(player.alloc_datum(Datum::String(value.clone())));
                    }
                }
                return Ok(player.alloc_datum(Datum::Void));
            }

            // Invalid type
            log_i(&format!(
                "external_param_value(): invalid argument type, returning Void"
            ));
            Ok(player.alloc_datum(Datum::Void))
        }
    }

    /// `stopEvent()` — Director 11.5 Scripting Dictionary, Movie method:
    /// "prevents scripts from passing an event message to subsequent locations
    /// in the message hierarchy… Neither subsequent scripts nor other behaviors
    /// on the sprite receive the event if it is stopped in this manner."
    ///
    /// The event-dispatch loops read (and clear) this flag; see
    /// `player_invoke_event_to_instances` / `player_invoke_static_event`.
    pub fn stop_event(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.player.event_stopped = true;
        Ok(DatumRef::Void)
    }

    pub fn get_pref(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let pref_name = Self::checked_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(value) = runtime.player.native_preference(&pref_name).map(str::to_owned) {
            return Ok(runtime.player.alloc_datum(Datum::String(value)));
        }
        #[cfg(target_arch = "wasm32")]
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten());
        #[cfg(target_arch = "wasm32")]
        if let Some(storage) = storage {
            let key = format!("dirplayer_pref_{}", pref_name);
            if let Ok(Some(value)) = storage.get_item(&key) {
                return Ok(runtime.player.alloc_datum(Datum::String(value)));
            }
        }
        Ok(DatumRef::Void)
    }

    pub fn set_pref(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let pref_name = Self::checked_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
        let pref_value = Self::checked_datum(runtime, &args[1])?.string_value(runtime.symbols)?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            runtime
                .player
                .set_native_preference(pref_name, pref_value);
            return Ok(DatumRef::Void);
        }
        #[cfg(target_arch = "wasm32")]
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten());
        #[cfg(target_arch = "wasm32")]
        if let Some(storage) = storage {
            let key = format!("dirplayer_pref_{}", pref_name);
            let _ = storage.set_item(&key, &pref_value);
        }
        Ok(DatumRef::Void)
    }

    pub fn go_to_net_page(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Ok(DatumRef::Void);
        }
        // LeechProtectionRemovalHelp `disableGoToNetPage` — swallow the call so
        // a movie's leech check can't bounce the player to the original site.
        if runtime.player.env_overrides.disable_goto_net_page {
            return Ok(DatumRef::Void);
        }
        let url = Self::checked_datum(runtime, &args[0])?.string_value(runtime.symbols)?;
        let target = if let Some(target_ref) = args.get(1) {
            Self::checked_datum(runtime, target_ref)?
                .string_value(runtime.symbols)
                .unwrap_or_else(|_| "_blank".to_string())
        } else {
            "_blank".to_string()
        };

        if let Some(code) = url.strip_prefix("javascript:") {
            // Defer via setTimeout(...,0) so the current WASM call stack
            // fully unwinds before the host-page JS runs. Synchronous
            // callbacks from the host back into WASM (e.g. openMixer
            // touching a wasm_bindgen closure) otherwise trip the
            // "closure invoked recursively or after being dropped" guard.
            let escaped = code.replace('\\', "\\\\").replace('\'', "\\'");
            let wrapper = format!(
                "setTimeout(function(){{try{{eval('{}');}}catch(e){{console.warn('gotoNetPage eval:',e);}}}},0);",
                escaped
            );
            if let Err(e) = js_sys::eval(&wrapper) {
                log::warn!("gotoNetPage: schedule eval failed: {:?}", e);
            }
        } else if let Some(window) = web_sys::window() {
            if let Err(e) = window.open_with_url_and_target(&url, &target) {
                log::warn!("gotoNetPage: window.open failed: {:?}", e);
            }
        }
        Ok(DatumRef::Void)
    }

    pub fn go_to_net_movie(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        // LeechProtectionRemovalHelp `disableGoToNetMovie` — see
        // `go_to_net_page`. Note this pins the CURRENT movie in place, so a
        // movie that legitimately navigates by `gotoNetMovie` will stop
        // advancing once the movie's setup script calls this.
        if runtime.player.env_overrides.disable_goto_net_movie {
            return Ok(DatumRef::Void);
        }
        let raw_url = Self::checked_datum(runtime, &args[0])?.string_value(runtime.symbols)?;

        // Parse URL and extract #fragment marker
        let (fetch_url, target) = if let Some(hash_pos) = raw_url.find('#') {
            let url_part = raw_url[..hash_pos].to_string();
            let fragment = raw_url[hash_pos + 1..].to_string();
            let target = if fragment.is_empty() {
                MovieFrameTarget::Default
            } else {
                MovieFrameTarget::Label(fragment)
            };
            (url_part, target)
        } else {
            (raw_url, MovieFrameTarget::Default)
        };

        // Start the network fetch (non-blocking)
        let task_id = runtime.player.net_manager.preload_net_thing(fetch_url.clone());

        // Store the pending operation (replaces any previous pending one, cancelling it)
        runtime.player.pending_goto_net_movie = Some((task_id, target));

        Ok(runtime.player.alloc_datum(Datum::Int(task_id as i32)))
    }

    /// `pass` — "passes an event message to the next location in the message
    /// hierarchy and enables execution of more than one handler for a given
    /// event ... The pass command branches to the next location AS SOON AS THE
    /// COMMAND RUNS. Any Lingo that follows the pass command in the handler
    /// does not run." (Director 11.5 Scripting Dictionary, `pass`).
    ///
    /// The second half matters as much as the first: movies use `pass` as an
    /// early exit. NabiscoWorld Mini-Golf guards its per-frame hole logic with
    ///
    ///   on enterFrame
    ///     if voidp(ballState) then
    ///       pass()
    ///     end if
    ///     gameLogic()
    ///
    /// and gameLogic opens with `getAt(ballState, player)`. Setting the flag
    /// without stopping the handler ran gameLogic anyway and raised on exactly
    /// the VOID the guard existed to avoid.
    pub fn pass(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let player = &mut *runtime.player;
        {
            let scope_ref = player.current_scope_ref();
            let scope = player.scopes.get_mut(scope_ref).unwrap();
            scope.passed = true;
            scope.stop_requested = true;
            Ok(DatumRef::Void)
        }
    }

    pub async fn execute_frame_update() -> Result<(), ScriptError> {
        let session = crate::player::retained_session_handle()
            .ok_or_else(|| ScriptError::new("runtime session is not initialized".to_owned()))?;
        let player_id = crate::player::active_player_id() as PlayerId;
        let owner = session.borrow_mut().with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(cancelled_scope_error)?;
        execute_frame_update_owned(
            session,
            player_id,
            owner,
            crate::player::testing_shared::now_ms().max(0.0),
        ).await.map(|_| ())
    }

    /// Owner-bound updateStage entrypoint. The caller supplies the frame
    /// timestamp captured at its host boundary; the animation phase never
    /// samples a clock after it starts.
    pub async fn update_stage_at(
        _args: &Vec<DatumRef>,
        now_ms: f64,
    ) -> Result<DatumRef, ScriptError> {
        let session = crate::player::retained_session_handle()
            .ok_or_else(|| ScriptError::new("runtime session is not initialized".to_owned()))?;
        let player_id = crate::player::active_player_id() as PlayerId;
        let owner = session.borrow_mut().with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(cancelled_scope_error)?;
        execute_movie_async(session, MovieAsyncRequest {
            player_id,
            owner,
            kind: MovieAsyncKind::UpdateStage { now_ms },
            args: Vec::new(),
        }).await
    }

    pub async fn update_stage(_args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Self::update_stage_at(_args, crate::player::testing_shared::now_ms().max(0.0)).await
    }

    pub async fn nothing_async(_args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let session = crate::player::retained_session_handle()
            .ok_or_else(|| ScriptError::new("runtime session is not initialized".to_owned()))?;
        let player_id = crate::player::active_player_id() as PlayerId;
        let owner = session.borrow_mut().with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(cancelled_scope_error)?;
        execute_movie_async(session, MovieAsyncRequest {
            player_id,
            owner,
            kind: MovieAsyncKind::Nothing,
            args: Vec::new(),
        }).await
    }

    pub fn rollover(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if let Some(arg) = args.first() {
            Self::checked_datum(runtime, arg)?;
        }
        let player = &mut *runtime.player;
        {
            if !args.is_empty() {
                // rollOver(spriteNum) - returns TRUE if the mouse is over the specified sprite
                let sprite_num = player.get_datum(&args[0]).int_value()?;
                let sprite = player.movie.score.get_sprite(sprite_num as i16);
                if let Some(sprite) = sprite {
                    let hit = concrete_sprite_hit_test(player, sprite, player.mouse_loc.0, player.mouse_loc.1);
                    let result = if hit { 1 } else { 0 };
                    Ok(player.alloc_datum(Datum::Int(result)))
                } else {
                    Ok(player.alloc_datum(Datum::Int(0)))
                }
            } else {
                // the rollOver - returns the sprite number under the mouse
                let sprite = get_sprite_at(player, player.mouse_loc.0, player.mouse_loc.1, false);
                Ok(player.alloc_datum(Datum::Int(sprite.unwrap_or(0) as i32)))
            }
        }
    }

    pub fn puppet_sound(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "puppetSound requires at least 1 argument".to_string(),
            ));
        }
        
        reserve_player_mut(|player| {
            // If only one argument, use channel 1 by default
            let (channel_num, member_ref) = if args.len() == 1 {
                (1, args[0].clone())
            } else {
                // Director's puppetSound disambiguates its two arguments by type
                // rather than by strict position: the sound cast member may be
                // named by a string, and the channel is always an integer. The
                // documented order is `puppetSound whichChannel, whichCastMember`,
                // but many movies (e.g. BrickOut) write the member-first idiom
                // `puppetSound "Intro", 2`. When the first argument is a string
                // it is the member name and the second is the channel; otherwise
                // fall back to the documented channel-first order.
                if matches!(player.get_datum(&args[0]), Datum::String(_)) {
                    let channel = player.get_datum(&args[1]).int_value()?;
                    (channel, args[0].clone())
                } else {
                    let channel = player.get_datum(&args[0]).int_value()?;
                    (channel, args[1].clone())
                }
            };

            // `puppetSound channel, 0` (and the single-arg `puppetSound 0`)
            // turns off sound puppeting and STOPS the channel — it is not a
            // request to play cast member 0. Director 4 movies use this idiom
            // heavily. Routing it through the play path just fails to resolve a
            // member and logs a spurious error, so handle it as a stop here.
            let member_datum = player.get_datum(&member_ref);
            if matches!(&member_datum, Datum::Int(n) if *n <= 0) {
                player.sound_stop(channel_num)?;
                return Ok(DatumRef::Void);
            }

            player.puppet_sound(channel_num, member_ref)?;
            Ok(DatumRef::Void)
        })
    }

    pub fn delay(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            let ticks = player.get_datum(&args[0]).int_value()?;
            // Only set delay if not already delaying (prevent reset on every enterFrame)
            if ticks > 0 && player.delay_until.is_none() {
                let delay_ms = (ticks as f64) * (1000.0 / 60.0);
                player.delay_until = Some(
                    chrono::Local::now() + chrono::Duration::milliseconds(delay_ms as i64),
                );
            }
            Ok(DatumRef::Void)
        })
    }

    pub fn halt(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            // Stop movie playback
            player.movie.current_frame = 1;
            player.is_playing = false;
            Ok(DatumRef::Void)
        })
    }
}


fn cancelled_scope_error() -> ScriptError {
    crate::player::cancelled_scope_error()
}

/// Execute a movie request after the evaluator has released its session borrow.
/// All values crossing this boundary are owned handles; every reacquisition
/// checks the captured owner before mutating the player.
pub(crate) async fn execute_movie_async(
    session: RuntimeSessionHandle,
    request: MovieAsyncRequest,
) -> Result<DatumRef, ScriptError> {
    let player_id = request.player_id;
    let owner = request.owner.clone();
    ensure_movie_owner(&session, player_id, &owner)?;
    match request.kind {
        MovieAsyncKind::Do => {
            let source = session.borrow_mut().with_player(player_id, |context| {
                let Some(value) = request.args.first() else { return Ok(String::new()) };
                MovieHandlers::checked_datum(&context, value)?;
                context.player.get_datum(value).string_value(context.symbols)
            }).ok_or_else(cancelled_scope_error)??;
            if source.trim().is_empty() || source.trim().eq_ignore_ascii_case("nothing") {
                return Ok(DatumRef::Void);
            }
            // The command evaluator owns all child and host continuations. It
            // is awaited only after the session borrow used to read `source`.
            Box::pin(crate::player::eval_lingo_command_owned(session, player_id, owner, source)).await
        }
        MovieAsyncKind::Go => execute_go_owned(session, player_id, owner, request.args).await,
        MovieAsyncKind::Play => execute_play_owned(session, player_id, owner, request.args).await,
        MovieAsyncKind::UpdateStage { now_ms } => {
            let should_yield = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.draw_hold_since_ms = None;
                Ok(context.player.is_yield_safe()
                    || context.player.command_handler_yielding
                    || context.player.in_mouse_command
                    || context.player.movie.mouse_down)
            }).ok_or_else(cancelled_scope_error)??;

            // updateStage is also the animation pump for Director's busy-wait
            // input handlers.  Run one owner-bound frame update at movie tempo
            // while the normal frame loop is suspended, then redraw after all
            // session borrows have ended.
            let run_frame_anim = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                if context.player.is_in_frame_update {
                    return Ok(false);
                }
                let busy = context.player.command_handler_yielding
                    || (context.player.in_mouse_command && context.player.movie.mouse_down);
                if !busy {
                    return Ok(false);
                }
                let now = now_ms;
                let interval = 1000.0 / context.player.current_frame_tempo.max(1) as f64;
                if now - context.player.last_kb_loop_frame_ms >= interval {
                    context.player.last_kb_loop_frame_ms = now;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }).ok_or_else(cancelled_scope_error)??;
            if run_frame_anim {
                execute_frame_update_owned(
                    session.clone(), player_id, owner.clone(), now_ms,
                ).await?;
            }
            let result = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.stage_dirty = true;
                Ok(())
            }).ok_or_else(cancelled_scope_error)??;
            crate::rendering::draw_frame_for_owner(&session, player_id, &owner)?;
            let yield_now = should_yield && session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                let now = now_ms;
                if now - context.player.last_update_stage_yield_ms >= 8.0 {
                    context.player.last_update_stage_yield_ms = now;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }).ok_or_else(cancelled_scope_error)??;
            if yield_now {
                async_std::task::sleep(std::time::Duration::from_millis(2)).await;
            }
            Ok(DatumRef::Void)
        }
        MovieAsyncKind::Nothing => execute_nothing_owned(&session, player_id, &owner).await,
        MovieAsyncKind::FrameUpdate { now_ms } => execute_frame_update_owned(
            session,
            player_id,
            owner,
            now_ms,
        ).await,
        MovieAsyncKind::InitPrepareFrame { now_ms } => {
            execute_frame_prepare_owned(
                session,
                player_id,
                owner,
                now_ms,
            ).await.map(|_| DatumRef::Void)
        }
        MovieAsyncKind::InitEnterFrame => {
            execute_enter_frame_owned(session, player_id, owner).await.map(|_| DatumRef::Void)
        }
        MovieAsyncKind::InitExitFrame => {
            execute_exit_frame_owned(session, player_id, owner).await.map(|_| DatumRef::Void)
        }
        MovieAsyncKind::InitEnterExitFrame => {
            execute_enter_exit_frame_owned(session, player_id, owner).await.map(|_| DatumRef::Void)
        }
    }
}

fn ensure_movie_owner(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
) -> Result<(), ScriptError> {
    let valid = session.borrow_mut().with_player(player_id, |context| {
        owner.same_identity(&context.player.owner) && owner.is_arena_live()
    }).unwrap_or(false);
    valid.then_some(()).ok_or_else(cancelled_scope_error)
}

async fn execute_go_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    args: Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    #[derive(Clone)]
    enum GoTarget {
        Default,
        Frame(u32),
        Label(String),
    }

    let (target, path) = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        let target = match args.first() {
            None => GoTarget::Default,
            Some(value) => match MovieHandlers::checked_datum(&context, value)? {
                Datum::Int(frame) => GoTarget::Frame(*frame as u32),
                Datum::String(label) => GoTarget::Label(label.clone()),
                Datum::Symbol(symbol) => GoTarget::Label(
                    context.symbols.display(symbol).map_err(|_| ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "foreign movie frame label".to_owned(),
                    ))?.to_owned(),
                ),
                _ => return Err(ScriptError::new(
                    "Unsupported or invalid frame label passed to go()".to_owned(),
                )),
            },
        };
        let path = args.get(1).map(|value| {
            MovieHandlers::checked_datum(&context, value)?.string_value(context.symbols)
        }).transpose()?;
        Ok((target, path))
    }).ok_or_else(cancelled_scope_error)??;

    if let Some(mut path) = path {
        if !path.contains('.') {
            let extension = session.borrow_mut().with_player(player_id, |context| {
                context.player.movie.file_name.split('.').last().unwrap_or("dcr").to_owned()
            }).ok_or_else(cancelled_scope_error)?;
            path.push('.');
            path.push_str(&extension);
        }
        // The owned loader preserves the target label until the new movie is
        // mounted; resolve it against the new score below, never the old one.
        crate::player::load_movie_from_file_owned(
            session.clone(), player_id, owner.clone(), path,
        ).await?;
    }

    let result = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        let current = context.player.movie.current_frame;
        let frame = match &target {
            GoTarget::Default => context.player.movie.score.frame_labels.iter().rev()
                .find(|entry| entry.frame_num <= current as i32)
                .map(|entry| entry.frame_num as u32).unwrap_or(1),
            GoTarget::Frame(frame) => *frame,
            GoTarget::Label(label) => context.player.movie.score.frame_labels.iter()
                .find(|entry| entry.label.eq_ignore_ascii_case(label))
                .map(|entry| entry.frame_num as u32).unwrap_or(current),
        };
        if args.is_empty() {
            // Bare go() is gotoLoop: it only schedules the nearest marker (or
            // frame one). The frame pump owns the transition and the command
            // does not force playback or mark a same-frame go.
            context.player.next_frame = Some(frame);
            return Ok(DatumRef::Void);
        }
        if frame == current {
            context.player.go_same_frame = true;
        } else {
            // A later different-frame Go supersedes a retained same-frame
            // marker from an earlier callback; do not let the canonical
            // boundary consume the stale marker and discard this target.
            context.player.go_same_frame = false;
            context.player.next_frame = Some(frame);
            context.player.has_frame_changed_in_go = true;
            context.player.go_direction = if frame > current { 2 } else { 1 };
        }
        Ok(DatumRef::Void)
    }).ok_or_else(cancelled_scope_error)??;
    Ok(result)
}

async fn execute_play_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    args: Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    if args.len() >= 2 {
        let restart = session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let name = context.player.get_datum(&args[1]).string_value(context.symbols).unwrap_or_default();
            fn base(value: &str) -> String {
                value.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(value)
                    .split('.').next().unwrap_or(value).to_ascii_lowercase()
            }
            let current = context.player.movie.file_name.clone();
            let retained = context.player.movie_reload_data.is_some();
            Ok(retained && (name.is_empty() || current.is_empty() || base(&name) == base(&current)))
        }).ok_or_else(cancelled_scope_error)??;
        if restart {
            let result = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.pending_restart = true;
                Ok(DatumRef::Void)
            }).ok_or_else(cancelled_scope_error)??;
            Ok(result)
        } else {
            execute_go_owned(session, player_id, owner, args).await
        }
    } else {
        let flash_sprite = session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let Some(sprite_ref) = args.first() else { return Ok(None) };
            let datum = MovieHandlers::checked_datum(&context, sprite_ref)?;
            let sprite_num = match datum {
                Datum::SpriteRef(number) => *number,
                Datum::Int(number) => *number as i16,
                _ => return Ok(None),
            };
            let Some(sprite) = context.player.movie.score.get_sprite(sprite_num) else { return Ok(None) };
            let Some(member_ref) = sprite.member.as_ref() else { return Ok(None) };
            let Some(member) = context.player.movie.cast_manager.find_member_by_ref(member_ref) else { return Ok(None) };
            if matches!(member.member_type, crate::player::cast_member::CastMemberType::Flash(_)) {
                context.player.movie.score.get_sprite_mut(sprite_num).flash_asserted_frame = None;
                Ok(Some(sprite_num as i32))
            } else {
                Ok(None)
            }
        }).ok_or_else(cancelled_scope_error)??;
        if let Some(sprite_num) = flash_sprite {
            // Keep this host call outside the RuntimeSession borrow.  The old
            // global play path invokes the same per-sprite Ruffle operation.
            movie_ruffle_play(sprite_num);
        }
        Ok(DatumRef::Void)
    }
}

async fn execute_nothing_owned(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
) -> Result<DatumRef, ScriptError> {
    // Use the shared cross-platform wall clock. The owned MovieAsync path
    // also runs in native child tests, where js_sys::Date is unavailable.
    let now = crate::player::testing_shared::now_ms();
    let yield_now = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        if !context.player.in_frame_script {
            context.player.nothing_call_count = 0;
            return Ok(false);
        }
        context.player.nothing_call_count += 1;
        Ok(context.player.nothing_call_count >= 50
            && now - context.player.last_nothing_yield_ms >= 16.0)
    }).ok_or_else(cancelled_scope_error)??;
    if yield_now {
        session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.nothing_call_count = 0;
            context.player.last_nothing_yield_ms = now;
            Ok(())
        }).ok_or_else(cancelled_scope_error)??;
        async_std::task::sleep(std::time::Duration::from_millis(2)).await;
        session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(())
        }).ok_or_else(cancelled_scope_error)??;
    }
    Ok(DatumRef::Void)
}

async fn await_script_action_scope(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
    receiver: Option<ScriptInstanceRef>,
    handler: crate::player::script::ScriptHandlerRef,
    args: &[DatumRef],
) -> Result<crate::player::scope::ScopeResult, ScriptError> {
    ensure_movie_owner(session, player_id, owner)?;
    let turn = crate::player::player_call_script_handler_turn_in_session_sync(
        &mut session.borrow_mut(), player_id, receiver, handler, &args.to_vec());
    match turn {
        crate::player::ScriptHandlerTurn::Complete(result) => result,
        crate::player::ScriptHandlerTurn::Waiting => {
            let (sender, receiver) = async_std::channel::bounded(1);
            session.borrow_mut().retain_pending_command(crate::player::driver::PendingCommand {
                player_id,
                owner: owner.clone(),
                action: None,
                started: false,
                ticket: None,
                completer: None,
                event_sender: Some(sender),
                score_continuation: None,
                child_completion: None,
                eval_child: None,
                eval_sender: None,
            });
            match receiver.recv().await {
                Ok(result) => result,
                Err(_) => Err(cancelled_scope_error()),
            }
        }
        crate::player::ScriptHandlerTurn::Pending(action) => {
            let (sender, receiver) = async_std::channel::bounded(1);
            let ticket = action.ticket().clone();
            session.borrow_mut().retain_pending_command(crate::player::driver::PendingCommand {
                player_id,
                owner: owner.clone(),
                action: Some(action),
                started: false,
                ticket: Some(ticket),
                completer: None,
                event_sender: Some(sender),
                score_continuation: None,
                child_completion: None,
                eval_child: None,
                eval_sender: None,
            });
            match receiver.recv().await {
                Ok(result) => result,
                Err(_) => Err(cancelled_scope_error()),
            }
        }
    }
}

async fn await_script_action(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
    receiver: Option<ScriptInstanceRef>,
    handler: crate::player::script::ScriptHandlerRef,
    args: &[DatumRef],
) -> Result<(), ScriptError> {
    await_script_action_scope(session, player_id, owner, receiver, handler, args)
        .await.map(|_| ())
}

async fn execute_frame_callback(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
    callback: crate::player::events::W3dCallbackRequest,
) -> Result<(), ScriptError> {
    crate::player::events::dispatch_w3d_callback_owned(
        session.clone(),
        player_id,
        owner.clone(),
        callback,
    )
    .await
}

async fn execute_actor_list_stepframe(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
) -> Result<(), ScriptError> {
    let entered = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        if context.player.in_step_frame {
            return Ok(false);
        }
        context.player.in_step_frame = true;
        Ok(true)
    }).ok_or_else(cancelled_scope_error)??;
    if !entered {
        return Ok(());
    }

    let (snapshot, mut active_ids, mut generation) = session.borrow_mut()
        .with_player(player_id, |context| context.player.actor_list_stepframe_snapshot())
        .ok_or_else(cancelled_scope_error)?;
    let result = async {
        for (index, actor_ref) in snapshot.iter().enumerate() {
            if !active_ids.contains(&actor_ref.unwrap()) {
                continue;
            }
            // actorList entries are normally ScriptInstanceRefs. Resolve the
            // handler immediately before each callback so a preceding actor
            // may mutate the list or its script handlers without making this
            // frame use stale handler metadata.
            let instance = session.borrow_mut().with_player(player_id, |context| {
                match context.player.get_datum(actor_ref) {
                    Datum::ScriptInstanceRef(instance) => Some(instance.clone()),
                    _ => None,
                }
            }).ok_or_else(cancelled_scope_error)?;
            if let Some(instance) = instance {
                let handler = session.borrow_mut().with_player(player_id, |context| {
                    crate::player::handlers::datum_handlers::script_instance::ScriptInstanceUtils::get_script_instance_handler(
                        Symbol::builtin(BuiltInSymbol::StepFrame), &instance, context.player)
                }).ok_or_else(cancelled_scope_error)??;
                if let Some(handler) = handler {
                    await_script_action(session, player_id, owner, Some(instance), handler, &[]).await
                        .map_err(|error| {
                            error!("stepFrame[{}] error: {}", index, error.message);
                            error
                    })?;
                }
            } else {
                let dispatch = session
                    .borrow_mut()
                    .with_player(player_id, |mut context| {
                        crate::player::handlers::datum_handlers::player_call_datum_handler(
                            &mut context,
                            actor_ref,
                            Symbol::builtin(BuiltInSymbol::StepFrame),
                            &Vec::new(),
                        )
                    })
                    .ok_or_else(cancelled_scope_error)?;
                match dispatch {
                    crate::player::handlers::datum_handlers::DatumDispatch::Sync(result) => {
                        result.map(|_| ())?;
                    }
                    crate::player::handlers::datum_handlers::DatumDispatch::Child {
                        receiver,
                        handler_ref,
                        args,
                        ..
                    } => {
                        await_script_action(
                            session,
                            player_id,
                            owner,
                            receiver,
                            handler_ref,
                            &args,
                        )
                        .await?;
                    }
                    crate::player::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
                        receiver,
                        handler_ref,
                        args,
                        completion,
                    } => {
                        let scope = await_script_action_scope(
                            session,
                            player_id,
                            owner,
                            receiver,
                            handler_ref,
                            &args,
                        )
                        .await?;
                        session.borrow_mut().apply_child_completion(
                            player_id,
                            completion,
                            scope.return_value,
                        )?;
                    }
                    crate::player::handlers::datum_handlers::DatumDispatch::Pending {
                        request,
                        ..
                    } => {
                        // Await the real evaluator request before refreshing
                        // the actor generation. A pending StepFrame can
                        // mutate actorList, and advancing the snapshot before
                        // its completion loses that mutation and reorders the
                        // next actor.
                        crate::player::eval::invoke_request_owned(
                            session.clone(),
                            player_id,
                            owner.clone(),
                            request,
                        )
                        .await
                        .map(|_| ())?;
                    }
                }
            }
            let refreshed = session.borrow_mut().with_player(player_id, |context| {
                if context.player.actor_list_generation != generation {
                    Some(context.player.actor_list_active_ids())
                } else {
                    None
                }
            }).ok_or_else(cancelled_scope_error)?;
            if let Some((next_ids, next_generation)) = refreshed {
                active_ids = next_ids;
                generation = next_generation;
            }
        }
        Ok::<(), ScriptError>(())
    }.await;
    let clear_result = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return false;
        }
        context.player.in_step_frame = false;
        true
    });
    if !clear_result.unwrap_or(false) {
        return Err(cancelled_scope_error());
    }
    result.map(|_| ())
}

/// Run the part of a frame that precedes StartMovie/EnterFrame: actorList
/// stepFrame, PrepareFrame, W3D timer/animation/particle/collision callbacks.
/// The caller owns `is_in_frame_update`; this phase deliberately leaves it set
/// so initialization can place StartMovie between preparation and EnterFrame.
async fn execute_frame_prepare_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    now_ms: f64,
) -> Result<(), ScriptError> {
    execute_actor_list_stepframe(&session, player_id, &owner).await?;
    let previous_prepare = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        let previous = context.player.in_prepare_frame;
        context.player.in_prepare_frame = true;
        context.player.drain_allocator_reclaims();
        Ok::<bool, ScriptError>(previous)
    }).ok_or_else(cancelled_scope_error)??;
    let mut phase_guard = FramePhaseFlagGuard::new(
        session.clone(), player_id, owner.clone(), FramePhaseFlag::Prepare, previous_prepare,
    );
    let result = async {
        dispatch_timeout_event_owned(
            &session,
            player_id,
            &owner,
            BuiltInSymbol::PrepareFrame,
            Vec::new(),
        ).await?;
        crate::player::events::player_invoke_global_event_owned(
            session.clone(), player_id, owner.clone(),
            Symbol::builtin(BuiltInSymbol::PrepareFrame), Vec::new()).await?;
        let timer_callbacks = session.borrow_mut().with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            crate::player::events::prepare_w3d_timer_events(&mut context, now_ms)
        }).ok_or_else(cancelled_scope_error)??;
        for callback in timer_callbacks {
            execute_frame_callback(&session, player_id, &owner, callback).await?;
        }
        let (animation_dt, particle_dt) = session
            .borrow_mut()
            .w3d_deltas(player_id, &owner, now_ms)?;
        let callbacks = session.borrow_mut().with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            crate::player::events::tick_w3d_animations(&mut context, animation_dt)?;
            crate::player::events::tick_w3d_particles(&mut context, particle_dt)?;
            let dirty = crate::player::events::W3dDirtyTransformInput::new(
                owner.clone(), context.player.w3d_dirty_transform_ids.clone());
            let mut callbacks = Vec::new();
            callbacks.extend(crate::player::events::prepare_w3d_collision_callbacks(&mut context, &dirty)?);
            callbacks.extend(crate::player::events::prepare_physx_collision_callbacks(&mut context)?);
            context.player.w3d_dirty_transform_ids.clear();
            Ok::<Vec<crate::player::events::W3dCallbackRequest>, ScriptError>(callbacks)
        }).ok_or_else(cancelled_scope_error)??;
        for callback in callbacks {
            execute_frame_callback(&session, player_id, &owner, callback).await?;
        }
        Ok::<(), ScriptError>(())
    }.await;
    if result.is_ok() {
        phase_guard.clear_now()?;
    }
    result
}

#[derive(Clone, Copy)]
enum FramePhaseFlag {
    Prepare,
    Enter,
    FrameUpdate,
}

struct FramePhaseFlagGuard {
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    phase: FramePhaseFlag,
    previous: bool,
    armed: bool,
}

impl FramePhaseFlagGuard {
    fn new(
        session: RuntimeSessionHandle,
        player_id: PlayerId,
        owner: OwnerToken,
        phase: FramePhaseFlag,
        previous: bool,
    ) -> Self {
        Self { session, player_id, owner, phase, previous, armed: true }
    }

    fn clear_now(&mut self) -> Result<(), ScriptError> {
        let cleared = self.session.borrow_mut().with_player(self.player_id, |context| {
            if !self.owner.same_identity(&context.player.owner) || !self.owner.is_arena_live() {
                return false;
            }
            match self.phase {
                FramePhaseFlag::Prepare => context.player.in_prepare_frame = self.previous,
                FramePhaseFlag::Enter => context.player.in_enter_frame = self.previous,
                FramePhaseFlag::FrameUpdate => context.player.is_in_frame_update = self.previous,
            }
            true
        }).unwrap_or(false);
        self.armed = false;
        cleared.then_some(()).ok_or_else(cancelled_scope_error)
    }
}

impl Drop for FramePhaseFlagGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Owned phase futures never suspend while holding the session borrow.
        // Drop therefore performs a synchronous owner-checked cleanup. A
        // failed borrow here would violate that invariant; do not silently
        // leave a replacement owner with a stale phase flag.
        let mut session = self.session.borrow_mut();
        let _ = session.with_player(self.player_id, |context| {
            if !self.owner.same_identity(&context.player.owner) || !self.owner.is_arena_live() {
                return;
            }
            match self.phase {
                FramePhaseFlag::Prepare => context.player.in_prepare_frame = self.previous,
                FramePhaseFlag::Enter => context.player.in_enter_frame = self.previous,
                FramePhaseFlag::FrameUpdate => context.player.is_in_frame_update = self.previous,
            }
        });
    }
}

async fn execute_enter_exit_frame_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    execute_enter_frame_owned(session.clone(), player_id, owner.clone()).await?;
    execute_exit_frame_owned(session, player_id, owner).await
}

async fn execute_enter_frame_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let previous_enter = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        let previous = context.player.in_enter_frame;
        context.player.in_enter_frame = true;
        Ok::<bool, ScriptError>(previous)
    }).ok_or_else(cancelled_scope_error)??;
    let mut phase_guard = FramePhaseFlagGuard::new(
        session.clone(), player_id, owner.clone(), FramePhaseFlag::Enter, previous_enter,
    );
    let result = crate::player::events::player_invoke_global_event_owned(
        session.clone(), player_id, owner.clone(),
        Symbol::builtin(BuiltInSymbol::EnterFrame), Vec::new()).await;
    if result.is_ok() {
        session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.stage_dirty = true;
            Ok(())
        }).ok_or_else(cancelled_scope_error)??;
        phase_guard.clear_now()?;
    }
    result.map(|_| ())
}

async fn execute_exit_frame_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    crate::player::events::player_invoke_global_event_owned(
        session, player_id, owner, Symbol::builtin(BuiltInSymbol::ExitFrame), Vec::new()).await?;
    Ok(())
}

async fn execute_frame_update_owned(
    session: RuntimeSessionHandle,
    player_id: PlayerId,
    owner: OwnerToken,
    now_ms: f64,
) -> Result<DatumRef, ScriptError> {
    let proceed = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        if context.player.is_in_frame_update || context.player.has_player_frame_changed {
            return Ok(false);
        }
        context.player.is_in_frame_update = true;
        context.player.movie.score.apply_tween_modifiers(context.player.movie.current_frame);
        Ok(true)
    }).ok_or_else(cancelled_scope_error)??;
    if !proceed {
        return Ok(DatumRef::Void);
    }

    let mut frame_guard = FramePhaseFlagGuard::new(
        session.clone(),
        player_id,
        owner.clone(),
        FramePhaseFlag::FrameUpdate,
        false,
    );

    let result = async {
        execute_frame_prepare_owned(session.clone(), player_id, owner.clone(), now_ms).await?;
        let pending_movie_init = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                Ok::<bool, ScriptError>(context.player.pending_movie_init)
            })
            .ok_or_else(cancelled_scope_error)??;
        if pending_movie_init {
            return Ok::<(), ScriptError>(());
        }
        execute_enter_exit_frame_owned(session.clone(), player_id, owner.clone()).await?;
        crate::rendering::draw_frame_for_owner(&session, player_id, &owner)?;
        Ok::<(), ScriptError>(())
    }.await;
    frame_guard.clear_now()?;
    result.map(|_| DatumRef::Void)
}

/// Dispatch a system event to the timeout targets belonging to one player.
/// The legacy helper discovers a process-global player and therefore cannot
/// be used by an owned movie turn.  Snapshot the target handles under a short
/// session borrow, then resolve each handler immediately before invoking it.
/// This keeps child and deferred host work in the owner-bound session while
/// preserving timeout ordering and Abort/error handling.
async fn dispatch_timeout_event_owned(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
    handler: BuiltInSymbol,
    args: Vec<DatumRef>,
) -> Result<(), ScriptError> {
    let targets = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context
                .player
                .timeout_manager
                .timeouts
                .values()
                .filter(|timeout| timeout.is_scheduled)
                .map(|timeout| timeout.target_ref.clone())
                .collect::<Vec<_>>())
        })
        .ok_or_else(cancelled_scope_error)??;

    for target in targets {
        let dispatch = session
            .borrow_mut()
            .with_player(player_id, |mut context| {
                crate::player::handlers::datum_handlers::player_call_datum_handler(
                    &mut context,
                    &target,
                    Symbol::builtin(handler),
                    &args,
                )
            })
            .ok_or_else(cancelled_scope_error)?;
        let result = match dispatch {
            crate::player::handlers::datum_handlers::DatumDispatch::Sync(result) => result,
            crate::player::handlers::datum_handlers::DatumDispatch::Child {
                receiver,
                handler_ref,
                args,
                ..
            } => await_script_action(
                session,
                player_id,
                owner,
                receiver,
                handler_ref,
                &args,
            )
            .await
            .map(|_| DatumRef::Void),
            crate::player::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
                receiver,
                handler_ref,
                args,
                completion,
            } => {
                let scope = await_script_action_scope(
                    session,
                    player_id,
                    owner,
                    receiver,
                    handler_ref,
                    &args,
                )
                .await?;
                session
                    .borrow_mut()
                    .apply_child_completion(player_id, completion, scope.return_value)?;
                Ok(DatumRef::Void)
            }
            crate::player::handlers::datum_handlers::DatumDispatch::Pending { request, reason } => {
                session
                    .borrow_mut()
                    .retain_deferred_request(player_id, request, reason);
                Ok(DatumRef::Void)
            }
        };
        if let Err(error) = result {
            if error.code == ScriptErrorCode::Abort {
                return Err(error);
            }
            if error.code != ScriptErrorCode::HandlerNotFound {
                log::warn!("timeout system event {:?} error: {}", handler, error.message);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_existing_cast_member_does_not_consume_unused_cast_argument() {
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 901,
                generation: 1,
            },
        );
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let mut foreign_symbols = crate::player::symbols::symbol_table::SymbolTable::with_owner(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 902,
                generation: 1,
            },
        );
        let foreign_symbol = foreign_symbols.intern("unusedCast");

        let result = session
            .with_player(1, |mut runtime| {
                let existing = runtime.player.alloc_datum(Datum::CastMember(CastMemberRef {
                    cast_lib: 1,
                    cast_member: 7,
                }));
                let foreign_arg = runtime.player.alloc_datum(Datum::Symbol(foreign_symbol));
                MovieHandlers::member(&mut runtime, &vec![existing.clone(), foreign_arg])
                    .map(|returned| returned == existing)
            })
            .unwrap();

        assert!(matches!(result, Ok(true)));
    }
}
