use std::{cell::RefCell, collections::{HashMap, VecDeque}, future::Future, pin::Pin, rc::Rc};

use indexmap::IndexMap;

use async_std::channel::Receiver;
use async_std::task::sleep;
use chrono::Local;
use futures::{select, stream::{FuturesUnordered, StreamExt}, FutureExt};
use log::{warn, debug};
use manual_future::{ManualFuture, ManualFutureCompleter};
use url::Url;

use crate::{
    console_warn,
    director::lingo::datum::{Datum, DatumType, FlashObjectRef, TimeoutRef},
    js_api::JsApi,
    player::{call_datum_handler_active, active_player_id, retained_session_handle, PLAYER_OPT, symbols::{builtin::BuiltInSymbol, symbol::Symbol}},
    utils::ToHexString,
};

use super::{
    allocator::ScriptInstanceAllocatorTrait,
    cast_lib::CastMemberRef,
    cast_member::CastMemberType,
    datum_ref::DatumRef,
    events::{
        player_dispatch_callback_event, player_dispatch_event_to_sprite,
        player_dispatch_movie_callback, player_wait_available,
        player_dispatch_event_to_sprite_targeted, player_invoke_frame_and_movie_scripts,
    },
    font::player_load_system_font_owned,
    keyboard_events::{player_key_down, player_key_up},
    
    player_alloc_datum, player_call_script_handler, player_call_script_handler_turn_in_session_sync, player_dispatch_global_event,
    player_is_playing, reserve_player_mut, reserve_player_ref,
    Score,
    score::{
        concrete_sprite_hit_test, get_concrete_sprite_rect, get_sprite_at, get_sprites_at,
        sprite_has_handler,
    },
    script_ref::ScriptInstanceRef,
    PlayerVMExecutionItem, ScriptError, ScriptReceiver, ScriptHandlerTurn, PLAYER_TX,
    session::RuntimeSession,
};

/// Result of driving one owner-bound pending action.  `Retain` is used for
/// work whose completion must come from the real host (network, JavaScript,
/// or debugger); the action and its ticket remain attached to the command.
/// It is deliberately distinct from a script error or `Void` result.
enum PendingActionExecution {
    Complete(crate::player::driver::ActionCompletion),
    Retain(crate::player::driver::PendingAction),
}

/// Result of running one command turn. A pending callback owns both the
/// driver's action and the caller completer until the host resumes it.
pub(crate) enum CommandTurn {
    /// The retained evaluator advanced independently; no caller result is
    /// produced by this pump tick.
    Continue,
    Complete(
        Result<DatumRef, ScriptError>,
        Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
    ),
    Waiting(crate::player::driver::PendingCommand),
    Pending(crate::player::driver::PendingCommand),
}

enum OwnerSchedulerEvent {
    Eval(bool),
    Actions(Vec<CommandTurn>, bool),
}

#[allow(dead_code)]
pub enum PlayerVMCommand {
    LoadMovieFromFile(String, bool),
    SetExternalParams(IndexMap<String, String>),
    SetBasePath(String),
    SetStartupDo(String),
    SetStartupDoBefore(String),
    SetStartupGo(u32),
    SetMoviePathOverride(String),
    SetMoviePathLabel(String),
    SetSystemFontPath(String),
    SetStageSize(u32, u32),
    TimeoutTriggered(TimeoutRef),
    PrintMemberBitmapHex(CastMemberRef),
    /// Dev UI sound preview: play a sound member through channel 1 (the real
    /// puppetSound path) so it exercises the same decode/playback code a movie
    /// would. Triggered by a user gesture so the AudioContext can resume.
    PlayMemberSound(CastMemberRef),
    MouseDown((i32, i32)),
    MouseUp((i32, i32)),
    MouseMove((i32, i32)),
    RightMouseDown((i32, i32)),
    RightMouseUp((i32, i32)),
    KeyDown(String, u16),
    KeyUp(String, u16),
    TriggerAlertHook,
    // Flash-to-Lingo callback mechanism
    TriggerFlashCallback {
        sprite_num: i32,
        handler_name: Symbol,
        args: Vec<DatumRef>,
    },
    TriggerLingoCallbackOnScript {
        cast_lib: i32,
        cast_member: i32,
        handler_name: Symbol,
        args: Vec<DatumRef>,
    },
    /// Raw Flash callback payload. Browser host callbacks enqueue this form
    /// without re-entering the VM; the owner-bound command turn converts it
    /// after it has acquired the live player and symbol table.
    TriggerLingoCallbackOnScriptRaw {
        cast_lib: i32,
        cast_member: i32,
        handler_name: String,
        args_json: String,
        flash_cast_lib: i32,
        flash_cast_member: i32,
    },
    /// Flash `LocalConnection.send` → the Lingo handler registered via
    /// `setCallback` on a Director-created LocalConnection, dispatched to the
    /// EXACT target instance (so `me` in `on myOnStatus me, ...` is correct).
    TriggerLocalConnectionCallback {
        target: ScriptInstanceRef,
        handler_name: String,
        args: Vec<DatumRef>,
    },
    TriggerLocalConnectionCallbackRaw {
        connection_name: String,
        method_name: String,
        args_json: String,
    },
    SetLingoScriptProperty {
        cast_lib: i32,
        cast_member: i32,
        prop_name: Symbol,
        value: DatumRef,
    },
    /// Dispatch a Flash `getURL("event: …")` body into Director's event chain.
    /// Mirrors the Flash Asset Xtra: `send <handler> [args...]` becomes a global
    /// event invocation that walks active sprite behaviors → frame script → movie
    /// scripts, so any `on <handler>` in scope picks it up.
    DispatchFlashEvent {
        cast_lib: i32,
        cast_member: i32,
        handler_name: Symbol,
        args: Vec<DatumRef>,
    },
    DispatchFlashEventRaw {
        cast_lib: i32,
        cast_member: i32,
        body: String,
    },
    /// Wake the owner-bound command loop after a host submits a completion.
    /// The payload itself stays in `RuntimeSession` until the loop consumes it.
    PumpPending,
    /// Owner/generation-qualified WebSocket events.  Browser closures enqueue
    /// this command and never borrow the VM directly.
    MultiuserSocketEvent {
        owner: super::ownership::OwnerToken,
        instance_id: u32,
        generation: u64,
        event: super::xtra::multiuser::MultiuserSocketEvent,
    },
    TriggerXtraCallback {
        owner: super::ownership::OwnerToken,
        target: DatumRef,
        handler_name: Symbol,
        args: Vec<DatumRef>,
    },
}

pub fn _format_player_cmd(command: &PlayerVMCommand) -> String {
    match command {
        PlayerVMCommand::LoadMovieFromFile(path, autoplay) => format!("LoadMovieFromFile({}, {})", path, autoplay),
        PlayerVMCommand::SetExternalParams(params) => {
            format!("SetExternalParams({:?})", params.keys().collect::<Vec<_>>())
        }
        PlayerVMCommand::SetBasePath(path) => format!("SetBasePath({})", path),
        PlayerVMCommand::SetStartupDo(code) => format!("SetStartupDo({})", code),
        PlayerVMCommand::SetStartupDoBefore(code) => format!("SetStartupDoBefore({})", code),
        PlayerVMCommand::SetStartupGo(frame) => format!("SetStartupGo({})", frame),
        PlayerVMCommand::SetMoviePathOverride(path) => format!("SetMoviePathOverride({})", path),
        PlayerVMCommand::SetMoviePathLabel(path) => format!("SetMoviePathLabel({})", path),
        PlayerVMCommand::SetSystemFontPath(path) => format!("SetSystemFontPath({})", path),
        PlayerVMCommand::SetStageSize(width, height) => {
            format!("SetStageSize({}, {})", width, height)
        }
        PlayerVMCommand::TimeoutTriggered(timeout_ref) => {
            format!("TimeoutTriggered({})", timeout_ref)
        }
        PlayerVMCommand::PrintMemberBitmapHex(..) => "PrintMemberBitmapHex(..)".to_string(),
        PlayerVMCommand::PlayMemberSound(..) => "PlayMemberSound(..)".to_string(),
        PlayerVMCommand::MouseDown((x, y)) => format!("MouseDown({}, {})", x, y),
        PlayerVMCommand::MouseUp((x, y)) => format!("MouseUp({}, {})", x, y),
        PlayerVMCommand::MouseMove((x, y)) => format!("MouseMove({}, {})", x, y),
        PlayerVMCommand::RightMouseDown((x, y)) => format!("RightMouseDown({}, {})", x, y),
        PlayerVMCommand::RightMouseUp((x, y)) => format!("RightMouseUp({}, {})", x, y),
        PlayerVMCommand::KeyDown(key, ..) => format!("KeyDown({})", key),
        PlayerVMCommand::KeyUp(key, ..) => format!("KeyUp({})", key),
        PlayerVMCommand::TriggerAlertHook => "TriggerAlertHook".to_string(),
        PlayerVMCommand::TriggerFlashCallback { sprite_num, handler_name, .. } => {
            format!("TriggerFlashCallback(sprite: {}, handler: {:?})", sprite_num, handler_name)
        }
        PlayerVMCommand::TriggerLingoCallbackOnScript { cast_lib, cast_member, handler_name, .. } => {
            format!("TriggerLingoCallbackOnScript(cast_lib: {}, cast_member: {}, handler: {:?})", cast_lib, cast_member, handler_name)
        }
        PlayerVMCommand::TriggerLingoCallbackOnScriptRaw { cast_lib, cast_member, handler_name, .. } => {
            format!("TriggerLingoCallbackOnScriptRaw(cast_lib: {}, cast_member: {}, handler: {:?})", cast_lib, cast_member, handler_name)
        }
        PlayerVMCommand::TriggerLocalConnectionCallback { handler_name, .. } => {
            format!("TriggerLocalConnectionCallback(handler: {:?})", handler_name)
        }
        PlayerVMCommand::TriggerLocalConnectionCallbackRaw { connection_name, method_name, .. } => {
            format!("TriggerLocalConnectionCallbackRaw(connection: {:?}, method: {:?})", connection_name, method_name)
        }
        PlayerVMCommand::SetLingoScriptProperty { cast_lib, cast_member, prop_name, .. } => {
            format!("SetLingoScriptProperty(cast_lib: {}, cast_member: {}, prop: {:?})", cast_lib, cast_member, prop_name)
        }
        PlayerVMCommand::DispatchFlashEvent { cast_lib, cast_member, handler_name, .. } => {
            format!("DispatchFlashEvent(cast_lib: {}, cast_member: {}, handler: {:?})", cast_lib, cast_member, handler_name)
        }
        PlayerVMCommand::DispatchFlashEventRaw { cast_lib, cast_member, .. } => {
            format!("DispatchFlashEventRaw(cast_lib: {}, cast_member: {})", cast_lib, cast_member)
        }
        PlayerVMCommand::PumpPending => "PumpPending".to_string(),
        PlayerVMCommand::MultiuserSocketEvent { instance_id, generation, .. } => {
            format!("MultiuserSocketEvent(instance: {}, generation: {})", instance_id, generation)
        }
        PlayerVMCommand::TriggerXtraCallback { handler_name, .. } => {
            format!("TriggerXtraCallback(handler: {:?})", handler_name)
        }
    }
}

/// Handle a mouse-down that landed on a `#scroll` field's scrollbar.
///
/// Returns true when the click was consumed. Director's field scrollbar is
/// chrome owned by the player, not the movie: it scrolls the field and the click
/// does not reach the sprite's behaviours or move the caret.
///
/// Click zones follow the standard scrollbar contract — the arrow boxes step by
/// one line, the trough above/below the lift pages by the visible height
/// (Director's `pageHeight`), and pressing the lift starts a drag. `scrollTop`
/// is in pixels per the 11.5 Scripting Dictionary ("the distance, in pixels,
/// from the top of a field cast member to the top of the field that is currently
/// visible in the scrolling box"), so all three move it in pixels.
fn handle_field_scrollbar_mouse_down(
    player: &mut crate::player::DirPlayer,
    x: i32,
    y: i32,
) -> bool {
    use crate::player::score::{find_field_scrollbar_at, get_concrete_sprite_rect};

    let Some((sprite_number, sb)) = find_field_scrollbar_at(player, x, y) else {
        return false;
    };
    let Some(sprite) = player.movie.score.get_sprite(sprite_number) else {
        return false;
    };
    let rect = get_concrete_sprite_rect(player, sprite);
    let ly = y - rect.top;

    let member_ref = sprite.member.clone();
    let scroll_top = member_ref
        .as_ref()
        .and_then(|r| player.movie.cast_manager.find_member_by_ref(r))
        .and_then(|m| match &m.member_type {
            CastMemberType::Field(f) => Some(f.scroll_top as i32),
            _ => None,
        })
        .unwrap_or(0);

    // One "line" for the arrow steps. Use the font size rather than a fixed
    // constant so a large-text field steps by a visible amount.
    let line_h = member_ref
        .as_ref()
        .and_then(|r| player.movie.cast_manager.find_member_by_ref(r))
        .and_then(|m| match &m.member_type {
            CastMemberType::Field(f) => Some(if f.font_size > 0 { f.font_size as i32 } else { 12 }),
            _ => None,
        })
        .unwrap_or(12);

    let thumb = sb.thumb(scroll_top);
    let new_scroll = if ly < sb.y0 + sb.arrow_h {
        scroll_top - line_h
    } else if ly >= sb.y1 - sb.arrow_h {
        scroll_top + line_h
    } else if let Some((ty0, ty1)) = thumb {
        if ly < ty0 {
            scroll_top - sb.page_height
        } else if ly >= ty1 {
            scroll_top + sb.page_height
        } else {
            // On the lift: start a drag, remembering where it was grabbed so it
            // doesn't jump under the pointer.
            player.field_scroll_drag = Some((sprite_number, ly - ty0));
            scroll_top
        }
    } else {
        scroll_top
    };

    set_field_scroll_top(player, sprite_number, new_scroll.clamp(0, sb.max_scroll));
    true
}

/// Write a field's `scrollTop` and invalidate so the next frame re-rasterizes.
fn set_field_scroll_top(player: &mut crate::player::DirPlayer, sprite_number: i16, scroll_top: i32) {
    let member_ref = player
        .movie
        .score
        .get_sprite(sprite_number)
        .and_then(|s| s.member.clone());
    let Some(member_ref) = member_ref else { return };
    if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
        if let CastMemberType::Field(field) = &mut member.member_type {
            let new = scroll_top.max(0) as u16;
            if field.scroll_top != new {
                field.scroll_top = new;
                player.movie.score.invalidate_render_channel_cache();
            }
        }
    }
}

/// Continue an in-progress lift drag. Called from the mouse-move path.
pub fn update_field_scrollbar_drag(player: &mut crate::player::DirPlayer, y: i32) {
    use crate::player::score::{get_concrete_sprite_rect, get_field_scrollbar};

    let Some((sprite_number, grab_offset)) = player.field_scroll_drag else {
        return;
    };
    let Some(sprite) = player.movie.score.get_sprite(sprite_number) else {
        player.field_scroll_drag = None;
        return;
    };
    let Some(sb) = get_field_scrollbar(player, sprite) else {
        player.field_scroll_drag = None;
        return;
    };
    let rect = get_concrete_sprite_rect(player, sprite);
    let ly = y - rect.top;
    let scroll = sb.scroll_for_thumb_top(ly - grab_offset);
    set_field_scroll_top(player, sprite_number, scroll.clamp(0, sb.max_scroll));
}

/// Check if a movie callback script (mouseDownScript, mouseUpScript, etc.)
/// contains actual executable content. Comments like "--nothing" are stored
/// but don't block event propagation in Director.
fn has_executable_callback(callback: &Option<ScriptReceiver>) -> bool {
    match callback {
        Some(ScriptReceiver::ScriptText(text)) => {
            let trimmed = text.trim();
            !trimmed.is_empty() && !trimmed.starts_with("--")
        }
        Some(_) => true, // ScriptInstance or Script refs are always executable
        None => false,
    }
}

pub async fn run_command_loop(
    rx: Receiver<PlayerVMExecutionItem>,
    session_handle: Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: super::ownership::OwnerToken,
) {
    warn!("Starting command loop");
    let mut deferred_items = VecDeque::new();
    let mut drive_pending = false;

    let abort_queued = |item: PlayerVMExecutionItem| {
        if let Some(completer) = item.completer {
            async_std::task::spawn_local(async move {
                completer.complete(Err(super::cancelled_scope_error())).await;
            });
        }
    };
    let mut abort_buffered = |deferred: &mut VecDeque<PlayerVMExecutionItem>| {
        while let Some(item) = deferred.pop_front() {
            abort_queued(item);
        }
        while let Ok(item) = rx.try_recv() {
            abort_queued(item);
        }
    };

    while !rx.is_closed() {
        // Validate the captured owner before touching pending actions or
        // draining allocator/error state. A reset may reuse the numeric
        // player id while this old loop is still being scheduled.
        if !owner_is_current(&session_handle, player_id, &owner) {
            warn!("Command loop stopped (owner replaced or reset)");
            abort_buffered(&mut deferred_items);
            retire_stale_nested_owner(&session_handle, player_id, &owner);
            return;
        }
        // Host completions wake this loop with `PumpPending`; consume and
        // resume every matching owner/ticket before accepting new work.
        let resumed = pump_pending_commands(&session_handle, player_id);
        for turn in resumed {
            drive_pending |= matches!(&turn, CommandTurn::Waiting(_) | CommandTurn::Pending(_));
            finish_command_turn_no_wake(turn, &session_handle, player_id, &owner).await;
        }
        if !owner_is_current(&session_handle, player_id, &owner) {
            warn!("Command loop stopped (owner replaced or reset)");
            abort_buffered(&mut deferred_items);
            retire_stale_nested_owner(&session_handle, player_id, &owner);
            return;
        }
        if drive_pending || session_handle.borrow().has_ready_pending_commands(player_id) {
            drive_pending_owner(&session_handle, player_id, &owner).await;
            drive_pending = false;
        }
        if resume_completed_score_defaults(&session_handle).await {
            drive_pending = true;
        }
        let blocked = session_handle.borrow().has_pending_commands(player_id);
        let item = if !blocked {
            match deferred_items.pop_front().or_else(|| rx.try_recv().ok()) {
                Some(item) => item,
                None => match rx.recv().await {
                    Ok(item) => item,
                    Err(_) => {
                        retire_stale_nested_owner(&session_handle, player_id, &owner);
                        break;
                    } // Channel closed (sender dropped)
                },
            }
        } else {
            match rx.recv().await {
                Ok(item) => item,
                Err(_) => {
                    retire_stale_nested_owner(&session_handle, player_id, &owner);
                    break;
                } // Channel closed (sender dropped)
            }
        };
        if !owner_is_current(&session_handle, player_id, &owner) {
            warn!("Command loop stopped after recv (owner replaced or reset)");
            abort_buffered(&mut deferred_items);
            abort_queued(item);
            retire_stale_nested_owner(&session_handle, player_id, &owner);
            return;
        }
        if blocked && !matches!(item.command, PlayerVMCommand::PumpPending) {
            // Preserve command ordering while the current command owns a
            // suspended driver. It is safe to queue the item because its
            // completer remains attached until the earlier action resumes.
            deferred_items.push_back(item);
            continue;
        }
        if matches!(item.command, PlayerVMCommand::PumpPending) {
            drive_pending_owner(&session_handle, player_id, &owner).await;
            continue;
        }
        // A command can enter movie/frame initialization and suspend on a
        // driver action of its own.  Running it inline would prevent this
        // loop from servicing the retained action, so drive the command in a
        // local task while this owner continues pumping its queue.
        let result = run_command_with_pending_pump(
            item.command,
            item.completer,
            session_handle.clone(),
            player_id,
            owner.clone(),
        ).await;
        let produced_pending = matches!(&result, CommandTurn::Waiting(_) | CommandTurn::Pending(_));
        // A command is an ownership boundary: temporaries created during the
        // async dispatch may have queued final drops under their owner token.
        // Drain before completing the caller so deferred garbage cannot wait
        // for an unrelated future allocation.
        finish_command_turn(result, &session_handle, player_id, &owner).await;
        drive_pending = produced_pending;
    }
    abort_buffered(&mut deferred_items);
    retire_stale_nested_owner(&session_handle, player_id, &owner);
    warn!("Command loop stopped!")
}

/// Execute a setup callback after the session borrow has ended. The JS
/// loader owns its registration payload and returns the exact driver
/// completion, so setup suspension is never converted to a synthetic value.
pub(crate) async fn execute_setup_pending_action(
    session: super::session::RuntimeSessionHandle,
    action: super::driver::PendingAction,
) -> Option<super::driver::ActionCompletion> {
    match action {
        super::driver::PendingAction::Setup(request) => Some(
            super::js_lingo_loader::execute_setup_callback(session, request),
        ),
        _ => None,
    }
}

/// Run one command while continuing to service the same owner's retained
/// driver actions.  This is deliberately owner-scoped: a pending completion
/// for another player is left in the session for that player's loop.  The
/// short timer is a fallback for producers which enqueue a pending action
/// without a queue wake; host completions still wake the normal loop through
/// `PumpPending` and are consumed by `pump_pending_commands` below.
async fn run_command_with_pending_pump(
    command: PlayerVMCommand,
    completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
    session_handle: Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: super::ownership::OwnerToken,
) -> CommandTurn {
    // Keep the command future in this task.  A detached child could retain a
    // caller completer after owner replacement; polling the pinned future
    // here lets cancellation drop the whole continuation together.
    let command_done = run_player_command(
        command,
        None,
        session_handle.clone(),
        player_id,
        owner.clone(),
    )
    .fuse();
    futures::pin_mut!(command_done);
    let mut caller_completer = completer;
    let mut drive_pending = false;
    let mut work: FuturesUnordered<Pin<Box<dyn Future<Output = OwnerSchedulerEvent>>>> =
        FuturesUnordered::new();
    loop {
        if !owner_is_current(&session_handle, player_id, &owner) {
            return CommandTurn::Complete(
                Err(super::cancelled_scope_error()),
                caller_completer,
            );
        }

        // Consume completed driver actions before admitting new work. The
        // evaluator queue is polled concurrently below because a movie or
        // script action may issue an evaluator child while this command is
        // still suspended.
        let resumed = pump_pending_commands(&session_handle, player_id);
        for turn in resumed {
            drive_pending |= matches!(&turn, CommandTurn::Waiting(_) | CommandTurn::Pending(_));
            finish_command_turn_no_wake(turn, &session_handle, player_id, &owner).await;
        }
        // Keep action futures in a persistent set. Recreating a future inside
        // the loop and then losing a timer/select race would drop its exact
        // host completion capability and strand the continuation.
        if session_handle.borrow().has_pending_eval_requests(player_id) {
            work.push(Box::pin(async {
                OwnerSchedulerEvent::Eval(pump_pending_eval_requests(&session_handle, player_id).await)
            }));
        }
        if drive_pending || session_handle.borrow().has_ready_pending_commands(player_id) {
            // This flag admits the action once. A retained external action
            // sets it again only when a completion is observed; leaving it
            // set while its future is in `work` would schedule duplicates.
            drive_pending = false;
            work.push(Box::pin(async {
                let result = execute_pending_commands(&session_handle, player_id, &owner).await;
                OwnerSchedulerEvent::Actions(result.0, result.1)
            }));
        }

        // Do not hold a RefCell borrow across the select. A producer may add
        // a pending action while the command future is being polled.
        if work.is_empty() {
            let tick = sleep(std::time::Duration::from_millis(1)).fuse();
            futures::pin_mut!(tick);
            select! {
                result = command_done => {
                    return attach_caller_completer(result, caller_completer.take());
                }
                _ = tick => {}
            }
        } else {
            let next = work.next().fuse();
            futures::pin_mut!(next);
            select! {
                result = command_done => {
                    return attach_caller_completer(result, caller_completer.take());
                }
                event = next => {
                    match event {
                        Some(OwnerSchedulerEvent::Eval(advanced)) => drive_pending |= advanced,
                        Some(OwnerSchedulerEvent::Actions(turns, again)) => {
                            drive_pending = again;
                            for turn in turns {
                                finish_command_turn_no_wake(turn, &session_handle, player_id, &owner).await;
                            }
                        }
                        None => {}
                    }
                }
                _ = sleep(std::time::Duration::from_millis(1)).fuse() => {}
            }
        }
    }
}

/// Drain one idle owner's retained work with persistent action futures. This
/// is also used for `PumpPending`, so an external wake cannot synchronously
/// await a movie/evaluator action that needs the same owner loop to service a
/// nested child.
pub(crate) async fn drive_pending_owner(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) {
    let mut action_ready = false;
    let mut work: FuturesUnordered<Pin<Box<dyn Future<Output = OwnerSchedulerEvent>>>> =
        FuturesUnordered::new();
    loop {
        if !owner_is_current(session, player_id, owner) {
            return;
        }
        let resumed = pump_pending_commands(session, player_id);
        for turn in resumed {
            action_ready |= matches!(&turn, CommandTurn::Waiting(_) | CommandTurn::Pending(_));
            finish_command_turn_no_wake(turn, session, player_id, owner).await;
        }
        if session.borrow().has_pending_eval_requests(player_id) {
            work.push(Box::pin(async {
                OwnerSchedulerEvent::Eval(pump_pending_eval_requests(session, player_id).await)
            }));
        }
        if action_ready || session.borrow().has_ready_pending_commands(player_id) {
            action_ready = false;
            work.push(Box::pin(async {
                let result = execute_pending_commands(session, player_id, owner).await;
                OwnerSchedulerEvent::Actions(result.0, result.1)
            }));
        }
        if work.is_empty() {
            return;
        }
        let next = work.next().fuse();
        futures::pin_mut!(next);
        let tick = sleep(std::time::Duration::from_millis(1)).fuse();
        futures::pin_mut!(tick);
        select! {
            event = next => match event {
                Some(OwnerSchedulerEvent::Eval(advanced)) => action_ready |= advanced,
                Some(OwnerSchedulerEvent::Actions(turns, again)) => {
                    action_ready = again;
                    for turn in turns {
                        finish_command_turn_no_wake(turn, session, player_id, owner).await;
                    }
                }
                None => {}
            },
            _ = tick => {}
        }
    }
}

fn attach_caller_completer(
    result: CommandTurn,
    completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
) -> CommandTurn {
    match result {
        CommandTurn::Continue => CommandTurn::Continue,
        CommandTurn::Complete(result, inner) => CommandTurn::Complete(result, inner.or(completer)),
        CommandTurn::Waiting(mut pending) => {
            if pending.completer.is_none() {
                pending.completer = completer;
            }
            CommandTurn::Waiting(pending)
        }
        CommandTurn::Pending(mut pending) => {
            if pending.completer.is_none() {
                pending.completer = completer;
            }
            CommandTurn::Pending(pending)
        }
    }
}

/// Deliver completed score-initialization child results after the driver turn
/// has released its session borrow. The continuation performs its own owner
/// check before touching defaults or reapplying authored parameters.
async fn resume_completed_score_defaults(
    session_handle: &Rc<RefCell<RuntimeSession>>,
) -> bool {
    let completed = session_handle.borrow_mut().take_completed_score_defaults();
    let mut queued = false;
    for (player_id, continuation, result) in completed {
        let outcome = Score::resume_behavior_defaults_continuation(
            session_handle.clone(),
            player_id,
            continuation,
            result,
        )
        .await;
        if let Err(error) = outcome {
            if error.code != super::ScriptErrorCode::Abort {
                let _ = session_handle.borrow_mut().with_player(player_id, |context| {
                    context.player.on_script_error_with_symbols(&error, Some(context.symbols));
                });
            }
        }
        queued |= session_handle.borrow().has_pending_commands(player_id);
    }
    queued
}

fn owner_is_current(
    session_handle: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) -> bool {
    let (matches, backpressured) = session_handle
        .borrow_mut()
        .with_player(player_id, |context| {
            (
                owner.same_identity(&context.player.owner) && owner.is_arena_live(),
                context.player.host_event_backpressure.is_some(),
            )
        })
        .unwrap_or((false, false));
    if matches && backpressured {
        session_handle
            .borrow_mut()
            .cancel_host_backpressured_owner(player_id, owner);
        return false;
    }
    matches
}

/// Retire a stale nested child without consulting any ambient player.  The
/// numeric id is insufficient because a reset can immediately install a new
/// player there; RuntimeSession checks both the nested registry capability and
/// the live player's owner identity.  Host resources are drained only after
/// the short mutable borrow has ended.
fn retire_stale_nested_owner(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) {
    let retired = session
        .try_borrow_mut()
        .map(|mut runtime| runtime.retire_nested_child_if_identity(player_id, owner))
        .unwrap_or(false);
    if retired {
        drain_host_teardowns(session);
    }
}

async fn finish_command_turn(
    result: CommandTurn,
    session_handle: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) {
    if !owner_is_current(session_handle, player_id, owner) {
        // Do not drain or report against a replacement player. Completing a
        // stale caller future is safe because it does not mutate VM state.
        if let CommandTurn::Complete(_, Some(completer)) = result {
            completer
                .complete(Err(super::cancelled_scope_error()))
                .await;
        }
        return;
    }
    session_handle.borrow_mut().with_player(player_id, |context| {
        context.player.drain_allocator_reclaims();
    });
    match result {
            CommandTurn::Continue => {}
            CommandTurn::Complete(Ok(result), completer) => {
                if let Some(completer) = completer {
                    completer.complete(Ok(result)).await;
                }
            }
            CommandTurn::Complete(Err(err), completer) => {
                if err.code == super::ScriptErrorCode::Abort {
                    // abort is a normal control flow mechanism, not an error
                    if let Some(completer) = completer {
                        completer.complete(Ok(DatumRef::Void)).await;
                    }
                } else {
                    // TODO ignore error if it's a CancelledException
                    // TODO print stack trace
                    session_handle.borrow_mut().with_player(player_id, |context| {
                        context.player.on_script_error_with_symbols(&err, Some(context.symbols))
                    });
                    if let Some(completer) = completer {
                        completer.complete(Err(err)).await;
                    }
                }
            }
            CommandTurn::Waiting(pending) | CommandTurn::Pending(pending) => {
                session_handle.borrow_mut().retain_pending_command(pending);
            }
    }
    let _ = JsApi::dispatch_player_notifications(session_handle.clone(), player_id);
    drain_host_teardowns(session_handle);
}

/// Drop retired host resources only after the session borrow used to extract
/// them has ended. Host cleanup may synchronously re-enter the same session.
pub(crate) fn drain_host_teardowns(session: &Rc<RefCell<RuntimeSession>>) {
    let teardowns = session.borrow_mut().take_host_teardowns();
    drop(teardowns);
}

/// Finish a turn produced while the owner pump already removed its pending
/// command. Requeueing with a wake would enqueue another `PumpPending` for
/// the same action on every polling tick, so only newly arrived turns use the
/// waking variant above.
async fn finish_command_turn_no_wake(
    result: CommandTurn,
    session_handle: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) {
    if !owner_is_current(session_handle, player_id, owner) {
        if let CommandTurn::Complete(_, Some(completer)) = result {
            completer.complete(Err(super::cancelled_scope_error())).await;
        }
        return;
    }
    match result {
        CommandTurn::Continue => {}
        CommandTurn::Waiting(pending) | CommandTurn::Pending(pending) => {
            session_handle.borrow_mut().requeue_pending_command(pending);
        }
        complete => finish_command_turn(complete, session_handle, player_id, owner).await,
    }
    let _ = JsApi::dispatch_player_notifications(session_handle.clone(), player_id);
}

/// Execute one evaluator-owned external request after the session borrow has
/// ended. Any preparation, finish, or fallback redispatch reacquires only a
/// short owner-checked borrow.
fn finish_eval_external_error(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: &crate::player::eval::EvalAction,
    owner: &super::ownership::OwnerToken,
    error: ScriptError,
) -> super::session::EvalRequestTurn {
    let turn = session
        .borrow_mut()
        .resume_eval(id, action, owner, Err(error));
    super::session::EvalRequestTurn::Evaluator(turn)
}

fn eval_action_is_current(
    session: &Rc<RefCell<RuntimeSession>>,
    id: &crate::player::eval::EvalId,
    action: &crate::player::eval::EvalAction,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) -> bool {
    session
        .borrow_mut()
        .eval_action_anchor(id, action)
        .is_some_and(|(current_player, current_owner)| {
            current_player == player_id && current_owner.same_identity(owner)
        })
}

pub(crate) async fn execute_eval_external_request(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    player_id: u32,
    owner: super::ownership::OwnerToken,
    request: super::xtra::external::ExternalXtraRequest,
) -> super::session::EvalRequestTurn {
    execute_eval_external_request_with(session, id, action, player_id, owner, request, |request| {
        super::xtra::external::execute_request(request)
    })
    .await
}

/// Testable form of the external evaluator boundary. The executor is called
/// only after the session borrow has ended; production supplies the JS/Xtra
/// bridge and tests can prove synchronous callback re-entry is borrow-safe.
pub(crate) async fn execute_eval_external_request_with<F>(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    player_id: u32,
    owner: super::ownership::OwnerToken,
    request: super::xtra::external::ExternalXtraRequest,
    execute_host: F,
) -> super::session::EvalRequestTurn
where
    F: Fn(&super::xtra::external::ExternalXtraRequest)
        -> Result<Option<super::xtra::external::ExternalXtraResponse>, ScriptError>,
{
    if !owner_is_current(session, player_id, &owner)
        || !request.owner.same_identity(&owner)
        || !request.owner.is_arena_live()
        || !eval_action_is_current(session, &id, &action, player_id, &owner)
    {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    }

    // The host call is deliberately made without a RuntimeSession borrow.
    let response = match execute_host(&request) {
        Ok(Some(response)) => response,
        Ok(None) => {
            let super::xtra::external::ExternalXtraOperation::ProbeStatic {
                fallback_name,
                fallback_args,
                ..
            } = &request.operation else {
                return finish_eval_external_error(
                    session,
                    id,
                    &action,
                    &owner,
                    ScriptError::new("external Xtra did not claim the evaluator request".to_owned()),
                );
            };
            if !eval_action_is_current(session, &id, &action, player_id, &owner) {
                return finish_eval_external_error(
                    session,
                    id,
                    &action,
                    &owner,
                    super::cancelled_scope_error(),
                );
            }
            let dispatch = session.borrow_mut().dispatch_global_after_external_probe(
                player_id,
                fallback_name,
                fallback_args,
            );
            return match dispatch {
                Ok(dispatch) => session.borrow_mut().execute_eval_global_dispatch(
                    id,
                    action,
                    fallback_name.clone(),
                    fallback_args.clone(),
                    dispatch,
                ),
                Err(error) => finish_eval_external_error(session, id, &action, &owner, error),
            };
        }
        Err(error) => {
            return finish_eval_external_error(session, id, &action, &owner, error);
        }
    };

    let request = if let super::xtra::external::ExternalXtraOperation::ProbeStatic {
        handler,
        raw_args,
        ..
    } = &request.operation
    {
        let selected = response.xtra_name.clone();
        if !eval_action_is_current(session, &id, &action, player_id, &owner) {
            return finish_eval_external_error(
                session,
                id,
                &action,
                &owner,
                super::cancelled_scope_error(),
            );
        }
        let prepared = session.borrow_mut().with_player(player_id, |context| {
            if !request.owner.same_identity(&context.player.owner)
                || !request.owner.is_arena_live()
            {
                return Err(super::cancelled_scope_error());
            }
            super::xtra::external::prepare_static_request(
                context.player,
                context.symbols,
                &selected,
                handler,
                raw_args,
            )?
            .ok_or_else(|| ScriptError::new("probed external Xtra was unloaded".to_owned()))
        });
        let prepared = match prepared {
            Some(Ok(prepared)) => prepared,
            Some(Err(error)) => {
                return finish_eval_external_error(session, id, &action, &owner, error);
            }
            None => {
                return finish_eval_external_error(
                    session,
                    id,
                    &action,
                    &owner,
                    super::cancelled_scope_error(),
                );
            }
        };
        if !eval_action_is_current(session, &id, &action, player_id, &owner) {
            return finish_eval_external_error(
                session,
                id,
                &action,
                &owner,
                super::cancelled_scope_error(),
            );
        }
        // The second host call is also outside the RuntimeSession borrow.
        match execute_host(&prepared) {
            Ok(Some(response)) => {
                return finish_eval_external_request(
                    session,
                    id,
                    action,
                    player_id,
                    owner,
                    prepared,
                    response,
                );
            }
            Ok(None) => {
                return finish_eval_external_error(
                    session,
                    id,
                    &action,
                    &owner,
                    ScriptError::new("selected external Xtra declined during dispatch".to_owned()),
                );
            }
            Err(error) => {
                return finish_eval_external_error(session, id, &action, &owner, error);
            }
        }
    } else {
        request
    };

    finish_eval_external_request(
        session,
        id,
        action,
        player_id,
        owner,
        request,
        response,
    )
}

fn finish_eval_external_request(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    player_id: u32,
    owner: super::ownership::OwnerToken,
    request: super::xtra::external::ExternalXtraRequest,
    response: super::xtra::external::ExternalXtraResponse,
) -> super::session::EvalRequestTurn {
    if !owner_is_current(session, player_id, &owner)
        || !eval_action_is_current(session, &id, &action, player_id, &owner)
    {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    }
    let result = session.borrow_mut().with_player(player_id, |context| {
        if !request.owner.same_identity(&context.player.owner)
            || !request.owner.is_arena_live()
        {
            return Err(super::cancelled_scope_error());
        }
        super::xtra::external::finish_request(
            context.player,
            context.symbols,
            &request,
            &response,
        )
    });
    let result = result.unwrap_or_else(|| Err(super::cancelled_scope_error()));
    let turn = session
        .borrow_mut()
        .resume_eval(id, &action, &owner, result);
    super::session::EvalRequestTurn::Evaluator(turn)
}

/// Execute an evaluator-owned on-demand Xtra load without holding the
/// RuntimeSession borrow across the browser callback. The load capability is
/// consumed only after validating its exact owner and evaluator action; its
/// retained continuation is then handed back to the ordinary external
/// request path so create/static/instance completion keeps one finish route.
pub(crate) async fn execute_eval_external_load_request(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    player_id: u32,
    owner: super::ownership::OwnerToken,
    request: super::xtra::external::ExternalXtraLoadRequest,
) -> super::session::EvalRequestTurn {
    if !owner_is_current(session, player_id, &owner)
        || !request.owner.same_identity(&owner)
        || !request.owner.is_arena_live()
        || !eval_action_is_current(session, &id, &action, player_id, &owner)
    {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    }
    let receiver = session.borrow_mut().with_player(player_id, |context| {
        if !request.owner.same_identity(&context.player.owner)
            || !request.owner.is_arena_live()
        {
            return None;
        }
        context.player.xtra_manager_state.external.take_load_waiter(&request)
    });
    let Some(Some(receiver)) = receiver else {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    };
    if request.notify_host {
        super::xtra::external::start_load_request(&request);
    }
    if let Err(error) = receiver.await.unwrap_or_else(|_| {
        Err(ScriptError::new("external Xtra load waiter was cancelled".to_owned()))
    }) {
        return finish_eval_external_error(session, id, &action, &owner, error);
    }
    if !owner_is_current(session, player_id, &owner)
        || !eval_action_is_current(session, &id, &action, player_id, &owner)
    {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    }
    let continuation = session.borrow_mut().with_player(player_id, |context| {
        if !request.owner.same_identity(&context.player.owner)
            || !request.owner.is_arena_live()
        {
            return None;
        }
        context.player.xtra_manager_state.external.take_load_continuation(&request)
    });
    let Some(Some(continuation)) = continuation else {
        return finish_eval_external_error(
            session,
            id,
            &action,
            &owner,
            super::cancelled_scope_error(),
        );
    };
    super::session::EvalRequestTurn::ExternalXtra(continuation)
}

/// Execute a prepared built-in network intent after the VM/session borrow has
/// ended.  The request is capability-bound: validating the owner and instance
/// generation happens before any transport side effect, and again in the
/// transport-specific implementation before applying a result.  Native builds
/// report the browser-only transport as unsupported rather than retaining a
/// request forever or manufacturing a successful value.
pub(crate) async fn execute_xtra_pending_request(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    intent: super::xtra::manager::XtraPendingIntent,
) -> Result<DatumRef, ScriptError> {
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&intent)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(super::cancelled_scope_error()),
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if matches!(
            &intent,
            super::xtra::manager::XtraPendingIntent::FileIoOpen(_)
                | super::xtra::manager::XtraPendingIntent::SysMenu(_)
                | super::xtra::manager::XtraPendingIntent::BudApi(_)
                | super::xtra::manager::XtraPendingIntent::OpenUrl(_)
        ) {
            return super::xtra::manager::execute_pending_intent(session, player_id, intent).await;
        }
        let name = match intent {
            super::xtra::manager::XtraPendingIntent::MultiuserConnect { .. } => "Multiuser connect",
            super::xtra::manager::XtraPendingIntent::MultiuserSend { .. } => "Multiuser send",
            super::xtra::manager::XtraPendingIntent::CurlExec { .. } => "Curl execAsync",
            super::xtra::manager::XtraPendingIntent::FileIoOpen(_) => unreachable!("handled above"),
            super::xtra::manager::XtraPendingIntent::SysMenu(_) => "SysMenu host effect",
            super::xtra::manager::XtraPendingIntent::BudApi(_) => "BudAPI host effect",
            super::xtra::manager::XtraPendingIntent::OpenUrl(_) => "OpenURL host effect",
        };
        return Err(ScriptError::new(format!(
            "{} requires the browser transport executor",
            name
        )));
    }
    #[cfg(target_arch = "wasm32")]
    {
        // The browser executor is deliberately entered from this owned
        // boundary.  The transport modules enqueue callbacks with the same
        // owner/generation fence; no RefCell borrow crosses their await.
        return super::xtra::manager::execute_pending_intent(session, player_id, intent).await;
    }
}

async fn finish_pending_eval_request_turn(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    owner: super::ownership::OwnerToken,
    request_player: u32,
    sender: async_std::channel::Sender<Result<DatumRef, ScriptError>>,
    turn: super::session::EvalRequestTurn,
) -> bool {
    match turn {
        super::session::EvalRequestTurn::ExternalXtra(request) => {
            let next = execute_eval_external_request(
                session,
                id.clone(),
                action.clone(),
                request_player,
                owner.clone(),
                request,
            )
            .await;
            Box::pin(finish_pending_eval_request_turn(
                session,
                id,
                action,
                owner,
                request_player,
                sender,
                next,
            ))
            .await
        }
        super::session::EvalRequestTurn::ExternalXtraLoad(request) => {
            let next = execute_eval_external_load_request(
                session,
                id.clone(),
                action.clone(),
                request_player,
                owner.clone(),
                request,
            )
            .await;
            Box::pin(finish_pending_eval_request_turn(
                session,
                id,
                action,
                owner,
                request_player,
                sender,
                next,
            ))
            .await
        }
        super::session::EvalRequestTurn::XtraPending(intent) => {
            let result = execute_xtra_pending_request(session, request_player, intent).await;
            let turn = session.borrow_mut().resume_eval(
                id.clone(),
                &action,
                &owner,
                match result {
                    Ok(value) => Ok(value),
                    Err(error) => Err(error),
                },
            );
            Box::pin(finish_pending_eval_request_turn(
                session,
                id,
                action,
                owner,
                request_player,
                sender,
                super::session::EvalRequestTurn::Evaluator(turn),
            ))
            .await
        }
        super::session::EvalRequestTurn::MovieAsync(movie_request) => {
            if !owner_is_current(session, request_player, &owner)
                || movie_request.player_id != request_player
                || !movie_request.owner.same_identity(&owner)
            {
                if let Some(sender) = session
                    .borrow_mut()
                    .detach_pending_eval_route(&id, &action)
                    .map(|(_, _, sender)| sender)
                {
                    let _ = sender.try_send(Err(super::cancelled_scope_error()));
                }
                return true;
            }
            let result = super::handlers::movie::execute_movie_async(
                session.clone(),
                movie_request,
            )
            .await;
            let resumed = session
                .borrow_mut()
                .resume_eval(id.clone(), &action, &owner, result);
            match resumed {
                super::eval::EvalTurn::Complete(result) => {
                    if let Some(sender) = session
                        .borrow_mut()
                        .detach_pending_eval_route(&id, &action)
                        .map(|(_, _, sender)| sender)
                    {
                        let _ = sender.try_send(result);
                    }
                }
                super::eval::EvalTurn::Pending { request: next } => {
                    let next_action = match &next {
                        super::eval::EvalPending::Global { capability, .. }
                        | super::eval::EvalPending::Object { capability, .. }
                        | super::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
                    };
                    session.borrow_mut().requeue_pending_eval_request(
                        super::session::PendingEvalRequest {
                            id,
                            player_id: request_player,
                            owner,
                            action: next_action,
                            request: next,
                            sender,
                        },
                    );
                }
            }
            true
        }
        super::session::EvalRequestTurn::Flash(request) => {
            let result = execute_owned_flash_request(session, request_player, &owner, request).await;
            let resumed = session
                .borrow_mut()
                .resume_eval(id.clone(), &action, &owner, result);
            match resumed {
                super::eval::EvalTurn::Complete(result) => {
                    if let Some(sender) = session
                        .borrow_mut()
                        .detach_pending_eval_route(&id, &action)
                        .map(|(_, _, sender)| sender)
                    {
                        let _ = sender.try_send(result);
                    }
                }
                super::eval::EvalTurn::Pending { request: next } => {
                    let next_action = match &next {
                        super::eval::EvalPending::Global { capability, .. }
                        | super::eval::EvalPending::Object { capability, .. }
                        | super::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
                    };
                    session.borrow_mut().requeue_pending_eval_request(
                        super::session::PendingEvalRequest {
                            id,
                            player_id: request_player,
                            owner,
                            action: next_action,
                            request: next,
                            sender,
                        },
                    );
                }
            }
            true
        }
        super::session::EvalRequestTurn::Evaluator(super::eval::EvalTurn::Complete(result)) => {
            let sender = session
                .borrow_mut()
                .detach_pending_eval_route(&id, &action)
                .map(|(_, _, sender)| sender);
            if let Some(sender) = sender {
                let _ = sender.try_send(result);
            }
            true
        }
        super::session::EvalRequestTurn::Evaluator(super::eval::EvalTurn::Pending { request: next }) => {
            let next_action = match &next {
                super::eval::EvalPending::Global { capability, .. }
                | super::eval::EvalPending::Object { capability, .. }
                | super::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
            };
            session.borrow_mut().requeue_pending_eval_request(
                super::session::PendingEvalRequest {
                    id,
                    player_id: request_player,
                    owner,
                    action: next_action,
                    request: next,
                    sender,
                },
            );
            true
        }
        super::session::EvalRequestTurn::Child(super::driver::DriverTurn::Waiting) => {
            let detached = session
                .borrow_mut()
                .detach_pending_eval_route(&id, &action);
            if let Some((_, owner, sender)) = detached {
                session.borrow_mut().retain_pending_command(super::driver::PendingCommand {
                    player_id: request_player,
                    owner,
                    action: None,
                    started: false,
                    ticket: None,
                    completer: None,
                    event_sender: None,
                    score_continuation: None,
                    child_completion: None,
                    eval_child: Some(id),
                    eval_sender: Some(sender),
                });
            }
            true
        }
        super::session::EvalRequestTurn::Child(super::driver::DriverTurn::Pending(child_action)) => {
            let detached = session
                .borrow_mut()
                .detach_pending_eval_route(&id, &action);
            if let Some((_, owner, sender)) = detached {
                let ticket = child_action.ticket().clone();
                session.borrow_mut().retain_pending_command(super::driver::PendingCommand {
                    player_id: request_player,
                    owner,
                    action: Some(child_action),
                    started: false,
                    ticket: Some(ticket),
                    completer: None,
                    event_sender: None,
                    score_continuation: None,
                    child_completion: None,
                    eval_child: Some(id),
                    eval_sender: Some(sender),
                });
            }
            true
        }
        super::session::EvalRequestTurn::Child(super::driver::DriverTurn::Complete(result)) => {
            let sender = session
                .borrow_mut()
                .detach_pending_eval_route(&id, &action)
                .map(|(_, _, sender)| sender);
            if let Some(sender) = sender {
                let _ = sender.try_send(Ok(result.return_value));
            }
            true
        }
        super::session::EvalRequestTurn::Child(super::driver::DriverTurn::Error(error)) => {
            let sender = session
                .borrow_mut()
                .detach_pending_eval_route(&id, &action)
                .map(|(_, _, sender)| sender);
            if let Some(sender) = sender {
                let _ = sender.try_send(Err(error));
            }
            true
        }
    }
}

/// Consume completion payloads submitted by the host and resume retained
/// evaluator continuations. Unmatched completions stay queued until their
/// pending action arrives; ticket validation remains owner-bound.
pub(crate) async fn pump_pending_eval_requests(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
) -> bool {
    let pending = session
        .borrow_mut()
        .take_pending_eval_request_for(player_id)
        .into_iter()
        .collect::<Vec<_>>();
    let mut advanced = false;
    for request in pending {
        let id = request.id.clone();
        let action = request.action.clone();
        let owner = request.owner.clone();
        let request_player = request.player_id;
        let turn = if let Some((_, internal_request)) = eval_pending_internal_request(&request.request) {
            execute_eval_internal_request(
                session,
                id.clone(),
                action.clone(),
                owner.clone(),
                internal_request,
            )
            .await
            .unwrap_or_else(|| {
                session
                    .borrow_mut()
                    .execute_eval_request(id.clone(), request.request)
            })
        } else {
            session
                .borrow_mut()
                .execute_eval_request(id.clone(), request.request)
        };
        advanced |= finish_pending_eval_request_turn(
            session,
            id,
            action,
            owner,
            request_player,
            request.sender,
            turn,
        )
        .await;
        let _ = JsApi::dispatch_player_notifications(session.clone(), request_player);
    }
    drain_host_teardowns(session);
    advanced
}

pub(crate) fn eval_pending_internal_request(
    pending: &super::eval::EvalPending,
) -> Option<(super::eval::EvalAction, super::driver::InternalVmRequest)> {
    let (capability, request) = match pending {
        super::eval::EvalPending::Global {
            capability,
            request,
            prepared_child: None,
            ..
        }
        | super::eval::EvalPending::Object {
            capability,
            request,
            ..
        }
        | super::eval::EvalPending::SetProperty { capability, request } => {
            (capability, request)
        }
        _ => return None,
    };
    matches!(
        request,
        super::driver::InternalVmRequest::Object { .. }
            | super::driver::InternalVmRequest::ObjectV4 { .. }
            | super::driver::InternalVmRequest::ObjectProperty { .. }
            | super::driver::InternalVmRequest::SetProperty { .. }
            | super::driver::InternalVmRequest::EvaluateValue { .. }
    )
    .then(|| (capability.clone(), request.clone()))
}

/// Execute the evaluator's owner-bound internal requests after the session
/// borrow has ended. Returning `None` leaves legacy handler/child requests on
/// the established evaluator path; the supported JS/value requests resume the
/// exact action that produced them.
pub(crate) async fn execute_eval_internal_request(
    session: &Rc<RefCell<RuntimeSession>>,
    id: super::eval::EvalId,
    action: super::eval::EvalAction,
    owner: super::ownership::OwnerToken,
    request: super::driver::InternalVmRequest,
) -> Option<super::session::EvalRequestTurn> {
    let Some((player_id, anchored_owner)) = session.borrow_mut().eval_action_anchor(&id, &action) else {
        session.borrow_mut().cancel_eval_action(&id, &action, &owner);
        return Some(super::session::EvalRequestTurn::Evaluator(
            super::eval::EvalTurn::Complete(Err(super::cancelled_scope_error())),
        ));
    };
    if !anchored_owner.same_identity(&owner) {
        return Some(super::session::EvalRequestTurn::Evaluator(
            super::eval::EvalTurn::Complete(Err(super::cancelled_scope_error())),
        ));
    }
    if !eval_action_is_current(session, &id, &action, player_id, &owner) {
        session.borrow_mut().cancel_eval_action(&id, &action, &owner);
        return Some(super::session::EvalRequestTurn::Evaluator(
            super::eval::EvalTurn::Complete(Err(super::cancelled_scope_error())),
        ));
    }

    let result = match request {
        super::driver::InternalVmRequest::EvaluateValue { source, mode } => {
            Box::pin(super::eval::invoke_value_request_owned(
                session.clone(),
                player_id,
                owner.clone(),
                source,
                mode,
            ))
            .await
        }
        super::driver::InternalVmRequest::ObjectProperty { receiver, name } => {
            let target = session.borrow_mut().with_player(player_id, |context| {
                let datum = context.player.allocator.try_get_datum(&receiver)?;
                match datum {
                    super::Datum::JsObjectRef(handle) => Some(
                        context
                            .symbols
                            .display(&name)
                            .map(|display| (handle.clone(), display.to_owned()))
                            .map_err(|_| super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object property symbol".to_owned(),
                            )),
                    ),
                    _ => None,
                }
            });
            match target {
                Some(Some(Ok((handle, property)))) => super::js_lingo_loader::get_js_object_prop_explicit(
                    session,
                    player_id,
                    &owner,
                    &handle,
                    &property,
                ),
                Some(Some(Err(error))) => Err(error),
                Some(None) => return None,
                None => Err(super::cancelled_scope_error()),
            }
        }
        super::driver::InternalVmRequest::SetProperty { receiver, name, value } => {
            let target = session.borrow_mut().with_player(player_id, |context| {
                let datum = context.player.allocator.try_get_datum(&receiver)?;
                match datum {
                    super::Datum::JsObjectRef(handle) => Some(
                        context
                            .symbols
                            .display(&name)
                            .map(|display| (handle.clone(), display.to_owned()))
                            .map_err(|_| super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object property symbol".to_owned(),
                            )),
                    ),
                    _ => None,
                }
            });
            match target {
                Some(Some(Ok((handle, property)))) => super::js_lingo_loader::set_js_object_prop_explicit(
                    session,
                    player_id,
                    &owner,
                    &handle,
                    &property,
                    &value,
                )
                .map(|()| super::DatumRef::Void),
                Some(Some(Err(error))) => Err(error),
                Some(None) => return None,
                None => Err(super::cancelled_scope_error()),
            }
        }
        super::driver::InternalVmRequest::Object { receiver, name, args }
        | super::driver::InternalVmRequest::ObjectV4 { receiver, name, args } => {
            let target = session.borrow_mut().with_player(player_id, |context| {
                let datum = context.player.allocator.try_get_datum(&receiver)?;
                match datum {
                    super::Datum::JsObjectRef(handle) => Some(
                        context
                            .symbols
                            .display(&name)
                            .map(|display| (handle.clone(), display.to_owned()))
                            .map_err(|_| super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object handler symbol".to_owned(),
                            )),
                    ),
                    _ => None,
                }
            });
            let Some(target) = target else { return None };
            let target = match target {
                Some(Ok(target)) => target,
                Some(Err(error)) => {
                    let resumed = resume_eval_internal(
                        session,
                        id,
                        action,
                        owner,
                        Err(error),
                    );
                    return Some(super::session::EvalRequestTurn::Evaluator(resumed));
                }
                None => return None,
            };
            super::js_lingo_loader::invoke_js_object_method_explicit(
                session,
                player_id,
                &owner,
                &target.0,
                &target.1,
                &args,
            )
        }
        _ => return None,
    };
    let resumed = resume_eval_internal(session, id, action, owner, result);
    Some(super::session::EvalRequestTurn::Evaluator(resumed))
}

fn resume_eval_internal(
    session: &Rc<RefCell<RuntimeSession>>,
    id: super::eval::EvalId,
    action: super::eval::EvalAction,
    owner: super::ownership::OwnerToken,
    result: Result<super::DatumRef, super::ScriptError>,
) -> super::eval::EvalTurn {
    let resumed = session
        .borrow_mut()
        .resume_eval(id.clone(), &action, &owner, result);
    if matches!(resumed, super::eval::EvalTurn::Complete(Err(_))) {
        session
            .borrow_mut()
            .cancel_eval_action(&id, &action, &owner);
    }
    resumed
}

pub(crate) fn pump_pending_commands(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
) -> Vec<CommandTurn> {
    // A cast transport completion first passes through the cast queue so the
    // file is installed and Flash instances are invalidated.  Promote the
    // resulting ticket to the driver completion queue only after that apply
    // step, preserving the action's exact capability and return value.
    let completed_casts = session
        .borrow_mut()
        .take_completed_property_casts_for(player_id);
    for ticket in completed_casts {
        session.borrow_mut().submit_pending_command_completion(
            player_id,
            ticket,
            crate::player::driver::ActionCompletion::InternalResult(DatumRef::Void),
        );
    }
    let turns = session.borrow_mut().service_pending_commands(player_id);
    drain_host_teardowns(session);
    turns
}

/// Execute the retained actions for one captured owner and resume their
/// continuations. This is the concrete command-loop executor: synchronous VM
/// work, tracing, cooperative yields, and debugger pauses run here, while
/// network/JavaScript actions remain queued with their exact capability for
/// the corresponding host completion. No action is converted to `Void` or a
/// fabricated error merely because it crosses an external boundary.
pub(crate) async fn execute_pending_commands(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
) -> (Vec<CommandTurn>, bool) {
    if !owner_is_current(session, player_id, owner) {
        return (Vec::new(), false);
    }
    let Some(mut command) = session
        .borrow_mut()
        .take_ready_pending_command_for(player_id) else {
        return (Vec::new(), false);
    };
    let mut turns = Vec::with_capacity(1);
    let mut drive_again = false;
        let Some(action) = command.action.take() else {
            // Action-less Waiting is a cooperative driver turn. Resume it
            // synchronously until it reaches a real action or completion.
            turns.push(session.borrow_mut().resume_pending_command(command, None));
            drain_host_teardowns(session);
            return (turns, false);
        };
        command.started = true;
        let ticket = action.ticket().clone();
        command.ticket = Some(ticket.clone());
        command.action = Some(action.clone());
        // Keep the continuation and its cancellation metadata session-owned
        // while host work is awaited. This permits nested ready requests to
        // run and lets reset abort the exact caller even if this future is
        // dropped.
        session.borrow_mut().requeue_pending_command(command);
        let action_result = execute_pending_action(session, player_id, owner, action).await;
        let (turn, needs_followup) = match action_result {
            PendingActionExecution::Complete(completion) => {
                session.borrow_mut().submit_pending_command_completion(
                    player_id,
                    ticket,
                    completion,
                );
                (CommandTurn::Continue, true)
            }
            PendingActionExecution::Retain(action) => {
                session.borrow_mut().update_started_pending_command(player_id, &ticket, action);
                // This is an external wait. Do not immediately run the same
                // action again; the host must submit its completion first.
                (CommandTurn::Continue, false)
            }
        };
        drive_again |= needs_followup;
        turns.push(turn);
    drain_host_teardowns(session);
    (turns, drive_again)
}

/// Execute one action after its owning session borrow has been acquired only
/// for the short synchronous portion. A retained action is still live and is
/// deliberately returned to its command when the host must provide data.
async fn execute_pending_action(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    action: crate::player::driver::PendingAction,
) -> PendingActionExecution {
    use crate::player::driver::{
        ActionCompletion, HostRequest, InternalInvocationRequest, InternalVmRequest,
        PendingAction, SetupCallbackKind,
    };

    if !owner_is_current(session, player_id, owner) {
        return PendingActionExecution::Retain(action);
    }

    match action {
        PendingAction::Internal(request) => {
            execute_internal_action(session, player_id, owner, request).await
        }
        PendingAction::CastLoad { ticket, request } => {
            // Register the request with the session before exposing it to a
            // network transport.  Cache hits complete synchronously through
            // the same ticket path; a cache miss remains an owned request
            // until the transport calls `apply_cast_load`.
            if session.borrow().has_pending_cast_load(&request) {
                if !session.borrow_mut().mark_cast_load_started(&request) {
                    return PendingActionExecution::Retain(PendingAction::CastLoad { ticket, request });
                }
                return execute_cast_load_transport(session, player_id, owner, ticket, request).await;
            }
            let registered = session.borrow_mut().begin_property_cast_load(
                player_id,
                request.clone(),
                ticket.clone(),
            );
            match registered {
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                Ok(None) => {
                    let completed = session
                        .borrow_mut()
                        .take_completed_property_casts_for(player_id);
                    let mut matched = false;
                    for done in completed {
                        if done.same_identity(&ticket) {
                            matched = true;
                        } else {
                            // A cache drain can complete another property
                            // load for this owner at the same time. Preserve
                            // that ticket for its own pending command.
                            session.borrow_mut().submit_pending_command_completion(
                                player_id,
                                done,
                                ActionCompletion::InternalResult(DatumRef::Void),
                            );
                        }
                    }
                    if matched {
                        PendingActionExecution::Complete(ActionCompletion::InternalResult(
                            DatumRef::Void,
                        ))
                    } else {
                        PendingActionExecution::Retain(PendingAction::CastLoad { ticket, request })
                    }
                }
                Ok(Some(request)) => {
                    let _ = session.borrow_mut().mark_cast_load_started(&request);
                    execute_cast_load_transport(session, player_id, owner, ticket, request).await
                }
            }
        }
        PendingAction::CooperativeYield(request) => {
            let now = crate::player::bench_now_ms() as i64;
            if request.deadline_ms > now {
                sleep(std::time::Duration::from_millis((request.deadline_ms - now) as u64)).await;
            }
            PendingActionExecution::Complete(ActionCompletion::Resume)
        }
        PendingAction::Trace(request) => {
            let message = request.message.clone();
            let valid = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) {
                    return false;
                }
                super::trace_output(context.player, &message);
                true
            }).unwrap_or(false);
            if valid {
                PendingActionExecution::Complete(ActionCompletion::Resume)
            } else {
                PendingActionExecution::Retain(PendingAction::Trace(request))
            }
        }
        PendingAction::Breakpoint(request) => {
            execute_breakpoint_action(session, player_id, owner, request).await
        }
        PendingAction::ErrorPause(request) => {
            execute_error_pause_action(session, player_id, owner, request).await
        }
        PendingAction::Host(request) => {
            // Host requests carry serialized URL/script/debugger payloads and
            // have no safe in-process implementation. Keep the owned action
            // visible to the host completion transport instead of guessing a
            // result or dropping the continuation.
            let action = PendingAction::Host(request);
            PendingActionExecution::Retain(action)
        }
        PendingAction::Setup(request) => {
            // The setup executor owns the JS/virtual callback work and
            // returns the exact ticket completion. It runs after this action
            // has left the VM borrow; retaining it here would deadlock the
            // registration path waiting for a host that is already inside
            // the owner-bound command pump.
            let action = PendingAction::Setup(request);
            match execute_setup_pending_action(session.clone(), action.clone()).await {
                Some(completion) => PendingActionExecution::Complete(completion),
                None => PendingActionExecution::Retain(action),
            }
        }
    }
}

/// Execute a cast load created by the evaluator after the short VM
/// preparation borrow has ended.  Driver-originated CastLoad requests are
/// already registered by `DriverContinuation::prepare_internal_action` and
/// therefore stay visible to the host transport; this path handles only an
/// unregistered request (for example an evaluator property assignment).
async fn execute_cast_load_transport(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    ticket: crate::player::driver::CompletionTicket,
    request: crate::player::cast_lib::CastLoadRequest,
) -> PendingActionExecution {
    let task_id = match session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(super::cancelled_scope_error());
        }
        Ok(context
            .player
            .net_manager
            .preload_net_thing(request.requested_url().to_owned()))
    }) {
        Some(Ok(task_id)) => task_id,
        Some(Err(error)) => return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(error),
        ),
        None => return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(super::cancelled_scope_error()),
        ),
    };
    let wait = match session.borrow_mut().with_player(player_id, |context| {
        context.player.net_manager.create_task_future(task_id)
    }) {
        Some(wait) => wait,
        None => return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(super::cancelled_scope_error()),
        ),
    };
    wait.await;
    if !owner_is_current(session, player_id, owner) {
        return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(super::cancelled_scope_error()),
        );
    }
    let fetched = session.borrow_mut().with_player(player_id, |context| {
        let resolved_url = context
            .player
            .net_manager
            .get_task(task_id)
            .map(|task| task.resolved_url.to_string())
            .unwrap_or_else(|| request.requested_url().to_owned());
        let bytes = context
            .player
            .net_manager
            .get_task_result(Some(task_id))
            .unwrap_or_else(|| Err(-1))
            .map_err(|error| format!("cast load task failed ({error})"));
        (resolved_url, bytes)
    });
    let Some((resolved_url, bytes)) = fetched else {
        return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(super::cancelled_scope_error()),
        );
    };
    let applied = session
        .borrow_mut()
        .apply_cast_load(request.complete(resolved_url, bytes));
    if applied {
        // `apply_cast_load` queues the property ticket after the cast is
        // installed.  Let the next owner pump promote it and resume the
        // driver, keeping all completion paths identical.
        PendingActionExecution::Retain(crate::player::driver::PendingAction::CastLoad { ticket, request })
    } else {
        PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::InternalError(
                super::cancelled_scope_error(),
            ),
        )
    }
}

/// Execute a typed Flash request after the evaluator/driver has released its
/// RuntimeSession borrow. The request clone is intentionally mutable: an
/// initial BindGet captures the already-reserved host generation and retries
/// only through the generation-qualified bridge route.
pub(crate) async fn execute_owned_flash_request(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    mut request: super::handlers::datum_handlers::flash_object::FlashRequest,
) -> Result<super::datum_ref::DatumRef, ScriptError> {
    loop {
        if !request.owner.same_identity(owner)
            || !request.owner.is_arena_live()
            || request.player_id != player_id
            || !session.borrow().player_owner_matches(player_id, &request.owner)
        {
            return Err(super::cancelled_scope_error());
        }
        if let Some(generation) = request.expected_generation {
            let current = session.borrow_mut().with_player(player_id, |context| {
                super::handlers::datum_handlers::flash_object::checked_sprite_number(request.sprite_num)
                    .map(|sprite_num| context.player.is_flash_instance_generation_current(sprite_num, generation))
            });
            if !matches!(current, Some(Ok(true))) {
                return Err(ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    "Flash request targets a stale instance generation".to_owned(),
                ));
            }
        }
        match super::handlers::datum_handlers::flash_object::execute_flash_request(&request) {
            Ok(response) => {
                if request.expected_generation.is_some()
                    && request.expected_generation != Some(response.generation)
                {
                    return Err(ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "Flash response generation no longer matches request".to_owned(),
                    ));
                }
                let sprite_num = super::handlers::datum_handlers::flash_object::checked_sprite_number(
                    request.sprite_num,
                )?;
                let initial_bind = matches!(
                    &request.operation,
                    super::handlers::datum_handlers::flash_object::FlashOperation::BindGet { .. }
                ) && request.expected_generation.is_none();
                if initial_bind {
                    let authority_current = session.borrow_mut().with_player(player_id, |context| {
                        context.player.is_flash_instance_generation_current(
                            sprite_num,
                            response.generation,
                        )
                    });
                    match authority_current {
                        Some(true) => request.expected_generation = Some(response.generation),
                        Some(false) => {
                            return Err(ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "Flash response generation is not owned by the current instance".to_owned(),
                            ));
                        }
                        None => return Err(super::cancelled_scope_error()),
                    }
                }
                let result = session.borrow_mut().with_player(player_id, |context| {
                    if !request.owner.same_identity(&context.player.owner)
                        || !request.owner.is_arena_live()
                        || !context.player.is_flash_instance_generation_current(
                            sprite_num,
                            response.generation,
                        )
                    {
                        return Err(super::cancelled_scope_error());
                    }
                    super::handlers::datum_handlers::flash_object::decoded_to_datum(
                        context.player,
                        response,
                        &request,
                    )
                });
                return match result {
                    Some(result) => result,
                    None => Err(super::cancelled_scope_error()),
                };
            }
            Err(super::handlers::datum_handlers::flash_object::FlashRequestError::NotReady {
                generation,
            }) => {
                if let Some(expected) = request.expected_generation {
                    if expected != generation {
                        return Err(ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "Flash instance generation changed while waiting for readiness".to_owned(),
                        ));
                    }
                } else {
                    request.expected_generation = Some(generation);
                }
                super::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
                    &request.owner,
                    request.sprite_num,
                    generation,
                )
                .await?;
            }
            Err(super::handlers::datum_handlers::flash_object::FlashRequestError::Script(error)) => {
                return Err(error);
            }
        }
    }
}

async fn execute_internal_action(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    request: crate::player::driver::InternalInvocationRequest,
) -> PendingActionExecution {
    use crate::player::driver::{
        ActionCompletion, InternalInvocationRequest, InternalVmRequest, PendingAction,
    };

    let InternalInvocationRequest {
        ticket,
        scope,
        request,
        pending_reason,
        effect,
    } = request;
    let retained = |request: InternalVmRequest, pending_reason: Option<String>| {
        PendingActionExecution::Retain(PendingAction::Internal(InternalInvocationRequest {
            ticket: ticket.clone(),
            scope: scope.clone(),
            request,
            pending_reason,
            effect: effect.clone(),
        }))
    };

    match request {
        InternalVmRequest::Object { receiver, name, args }
        | InternalVmRequest::ObjectV4 { receiver, name, args } => {
            let js_target = session.borrow_mut().with_player(player_id, |context| {
                let datum = context.player.allocator.try_get_datum(&receiver)?;
                match datum {
                    crate::director::lingo::datum::Datum::JsObjectRef(handle) => Some(
                        context.symbols.display(&name)
                            .map(|display| (handle.clone(), display.to_owned()))
                            .map_err(|_| super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object handler symbol".to_owned(),
                            )),
                    ),
                    _ => None,
                }
            }).flatten();
            if let Some(js_target) = js_target {
                let (handle, handler_name) = match js_target {
                    Ok(target) => target,
                    Err(error) => return PendingActionExecution::Complete(
                        ActionCompletion::InternalError(error),
                    ),
                };
                let result = super::js_lingo_loader::invoke_js_object_method_explicit(
                    session,
                    player_id,
                    owner,
                    &handle,
                    &handler_name,
                    &args,
                );
                return PendingActionExecution::Complete(match result {
                    Ok(value) => ActionCompletion::InternalResult(value),
                    Err(error) => ActionCompletion::InternalError(error),
                });
            }
            let dispatch = session.borrow_mut().with_player(player_id, |mut context| {
                super::handlers::datum_handlers::player_call_datum_handler(
                    &mut context, &receiver, name.clone(), &args,
                )
            });
            let Some(dispatch) = dispatch else {
                return PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                ));
            };
            match dispatch {
                super::handlers::datum_handlers::DatumDispatch::Child {
                    receiver,
                    handler_ref,
                    args,
                    ..
                } => match super::player_call_script_handler_turn_in_session_sync(
                    &mut session.borrow_mut(),
                    player_id,
                    receiver,
                    handler_ref,
                    &args,
                ) {
                    super::ScriptHandlerTurn::Complete(Ok(scope)) => {
                        PendingActionExecution::Complete(ActionCompletion::InternalResult(scope.return_value))
                    }
                    super::ScriptHandlerTurn::Complete(Err(error)) => {
                        PendingActionExecution::Complete(ActionCompletion::InternalError(error))
                    }
                    super::ScriptHandlerTurn::Waiting => retained(
                        InternalVmRequest::Object { receiver: DatumRef::Void, name, args },
                        pending_reason,
                    ),
                    super::ScriptHandlerTurn::Pending(action) => {
                        PendingActionExecution::Retain(action)
                    }
                },
                super::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
                    receiver,
                    handler_ref,
                    args,
                    completion,
                } => match super::player_call_script_handler_turn_in_session_sync(
                    &mut session.borrow_mut(),
                    player_id,
                    receiver,
                    handler_ref,
                    &args,
                ) {
                    super::ScriptHandlerTurn::Complete(Ok(scope)) => {
                        let result = session.borrow_mut().apply_child_completion(
                            player_id,
                            completion,
                            scope.return_value,
                        );
                        PendingActionExecution::Complete(match result {
                            Ok(value) => ActionCompletion::InternalResult(value),
                            Err(error) => ActionCompletion::InternalError(error),
                        })
                    }
                    super::ScriptHandlerTurn::Complete(Err(error)) => {
                        PendingActionExecution::Complete(ActionCompletion::InternalError(error))
                    }
                    super::ScriptHandlerTurn::Waiting => retained(
                        InternalVmRequest::Object { receiver: DatumRef::Void, name, args },
                        pending_reason,
                    ),
                    super::ScriptHandlerTurn::Pending(action) => {
                        PendingActionExecution::Retain(action)
                    }
                },
                super::handlers::datum_handlers::DatumDispatch::Sync(result) => {
                    PendingActionExecution::Complete(match result {
                        Ok(result) => ActionCompletion::InternalResult(result),
                        Err(error) => ActionCompletion::InternalError(error),
                    })
                }
                super::handlers::datum_handlers::DatumDispatch::Pending { request, reason } => {
                    retained(request, pending_reason.or(Some(reason)))
                }
            }
        }
        InternalVmRequest::ObjectProperty { receiver, name } => {
            let js_target = session.borrow_mut().with_player(player_id, |context| {
                let datum = context.player.allocator.try_get_datum(&receiver)?;
                match datum {
                    crate::director::lingo::datum::Datum::JsObjectRef(handle) => Some(
                        context.symbols.display(&name)
                            .map(|display| (handle.clone(), display.to_owned()))
                            .map_err(|_| super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object property symbol".to_owned(),
                            )),
                    ),
                    _ => None,
                }
            }).flatten();
            if let Some(js_target) = js_target {
                let (handle, property_name) = match js_target {
                    Ok(target) => target,
                    Err(error) => return PendingActionExecution::Complete(
                        ActionCompletion::InternalError(error),
                    ),
                };
                let result = super::js_lingo_loader::get_js_object_prop_explicit(
                    session,
                    player_id,
                    owner,
                    &handle,
                    &property_name,
                );
                return PendingActionExecution::Complete(match result {
                    Ok(value) => ActionCompletion::InternalResult(value),
                    Err(error) => ActionCompletion::InternalError(error),
                });
            }
            let result = session.borrow_mut().with_player(player_id, |mut context| {
                super::script::get_obj_prop(context.player, context.symbols, &receiver, name)
            });
            match result {
                Some(Ok(value)) => PendingActionExecution::Complete(ActionCompletion::InternalResult(value)),
                Some(Err(error)) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                None => PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                )),
            }
        }
        InternalVmRequest::SetProperty { receiver, name, value } => {
            let mut outbox = super::cast_lib::CastNotificationOutbox::default();
            let outcome = session.borrow_mut().with_player(player_id, |context| {
                super::script::set_obj_prop_sync(
                    context.player,
                    context.symbols,
                    &receiver,
                    name,
                    &value,
                    &mut outbox,
                )
            });
            session.borrow_mut().append_notifications(player_id, &mut outbox);
            match outcome {
                Some(Ok(super::script::SetObjPropOutcome::Applied)) => {
                    PendingActionExecution::Complete(ActionCompletion::InternalResult(
                        crate::player::datum_ref::DatumRef::Void,
                    ))
                }
                Some(Ok(super::script::SetObjPropOutcome::JsObject { receiver, name, value })) => {
                    let js_target = session.borrow_mut().with_player(player_id, |context| {
                        match context.player.allocator.try_get_datum(&receiver) {
                            Some(crate::director::lingo::datum::Datum::JsObjectRef(handle)) => {
                                Some(context.symbols.display(&name)
                                    .map(|display| (handle.clone(), display.to_owned())))
                            }
                            _ => None,
                        }
                    }).flatten();
                    let result = js_target.map_or_else(
                        || Err(super::ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "stale or foreign JS object".to_owned(),
                        )),
                        |target| target.map_or_else(
                            |_| Err(super::ScriptError::new_code(
                                super::ScriptErrorCode::InvalidReference,
                                "foreign or stale JS object property symbol".to_owned(),
                            )),
                            |(handle, property_name)| super::js_lingo_loader::set_js_object_prop_explicit(
                                session, player_id, owner, &handle, &property_name, &value,
                            ),
                        ),
                    );
                    PendingActionExecution::Complete(match result {
                        Ok(()) => ActionCompletion::InternalResult(
                            crate::player::datum_ref::DatumRef::Void,
                        ),
                        Err(error) => ActionCompletion::InternalError(error),
                    })
                }
                Some(Ok(super::script::SetObjPropOutcome::FlashSet(request))) => {
                    match execute_owned_flash_request(session, player_id, owner, request).await {
                        Ok(_) => PendingActionExecution::Complete(ActionCompletion::InternalResult(
                            crate::player::datum_ref::DatumRef::Void,
                        )),
                        Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                    }
                }
                Some(Ok(super::script::SetObjPropOutcome::AwaitCastLoad(request))) => {
                    PendingActionExecution::Retain(PendingAction::CastLoad { ticket, request })
                }
                Some(Err(error)) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                None => PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                )),
            }
        }
        InternalVmRequest::EvaluateValue { source, mode } => {
            // Value expressions use the evaluator's strict value-mode
            // continuation. Do not requeue this request through the generic
            // request pump: that would redispatch EvaluateValue here.
            let result = super::eval::invoke_value_request_owned(
                session.clone(),
                player_id,
                owner.clone(),
                source,
                mode,
            )
            .await;
            PendingActionExecution::Complete(match result {
                Ok(value) => ActionCompletion::InternalResult(value),
                Err(error) => ActionCompletion::InternalError(error),
            })
        }
        InternalVmRequest::Flash(request) => {
            match execute_owned_flash_request(session, player_id, owner, request).await {
                Ok(value) => PendingActionExecution::Complete(ActionCompletion::InternalResult(value)),
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            }
        }
        InternalVmRequest::SpriteAsync(request) => {
            match super::session::RuntimeSession::execute_sprite_async_request(
                session.clone(),
                request,
            )
            .await
            {
                Ok(result) => PendingActionExecution::Complete(ActionCompletion::InternalResult(result)),
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            }
        }
        InternalVmRequest::CastMemberAsync(request) => {
            match super::session::RuntimeSession::execute_cast_member_async_request(
                session.clone(),
                request,
            )
            .await
            {
                Ok(super::handlers::datum_handlers::cast_member_ref::CastAsyncExecution::Value(result)) => {
                    PendingActionExecution::Complete(ActionCompletion::InternalResult(result))
                }
                Ok(super::handlers::datum_handlers::cast_member_ref::CastAsyncExecution::Havok {
                    result,
                    step_callbacks,
                    collision_callbacks,
                }) => {
                    // The cast executor has already performed the Havok
                    // operation. Keep its result on the waiting driver and
                    // retain callback payloads for the owner-bound callback
                    // router rather than rerunning the operation.
                    log::debug!(
                        "Havok request produced {} step and {} collision callbacks",
                        step_callbacks.len(),
                        collision_callbacks.len()
                    );
                    PendingActionExecution::Complete(ActionCompletion::InternalResult(result))
                }
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            }
        }
        InternalVmRequest::MovieAsync(request) => {
            match super::handlers::movie::execute_movie_async(session.clone(), request).await {
                Ok(result) => PendingActionExecution::Complete(ActionCompletion::InternalResult(result)),
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            }
        }
        InternalVmRequest::XtraPending(intent) => {
            match execute_xtra_pending_request(session, player_id, intent).await {
                Ok(result) => PendingActionExecution::Complete(ActionCompletion::InternalResult(result)),
                Err(error) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            }
        }
        InternalVmRequest::ExternalXtra(request) => {
            let response = match super::xtra::external::execute_request(&request) {
                Ok(Some(response)) => response,
                Ok(None) => {
                    if let super::xtra::external::ExternalXtraOperation::ProbeStatic {
                        fallback_name,
                        fallback_args,
                        ..
                    } = &request.operation
                    {
                        return PendingActionExecution::Complete(ActionCompletion::RetryInternal(
                            InternalVmRequest::GlobalAfterExternalProbe {
                                owner: request.owner.clone(),
                                name: fallback_name.clone(),
                                args: fallback_args.clone(),
                            },
                        ));
                    }
                    return PendingActionExecution::Complete(ActionCompletion::InternalError(
                        ScriptError::new("external Xtra did not claim the requested handler".to_owned()),
                    ));
                }
                Err(error) => {
                    return PendingActionExecution::Complete(ActionCompletion::InternalError(error));
                }
            };
            let (request, response) = if let super::xtra::external::ExternalXtraOperation::ProbeStatic {
                handler,
                raw_args,
                ..
            } = &request.operation
            {
                let selected = response.xtra_name.clone();
                let prepared = session.borrow_mut().with_player(player_id, |context| {
                    if !request.owner.same_identity(&context.player.owner)
                        || !request.owner.is_arena_live()
                    {
                        return Err(super::cancelled_scope_error());
                    }
                    super::xtra::external::prepare_static_request(
                        context.player,
                        context.symbols,
                        &selected,
                        handler,
                        raw_args,
                    )?
                    .ok_or_else(|| ScriptError::new("probed external Xtra was unloaded".to_owned()))
                });
                let prepared = match prepared {
                    Some(Ok(prepared)) => prepared,
                    Some(Err(error)) => {
                        return PendingActionExecution::Complete(ActionCompletion::InternalError(error));
                    }
                    None => {
                        return PendingActionExecution::Complete(ActionCompletion::InternalError(
                            super::cancelled_scope_error(),
                        ));
                    }
                };
                let response = match super::xtra::external::execute_request(&prepared) {
                    Ok(Some(response)) => response,
                    Ok(None) => {
                        return PendingActionExecution::Complete(ActionCompletion::InternalError(
                            ScriptError::new("selected external Xtra declined during dispatch".to_owned()),
                        ));
                    }
                    Err(error) => return PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                };
                (prepared, response)
            } else {
                (request, response)
            };
            let result = session.borrow_mut().with_player(player_id, |context| {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return Err(super::cancelled_scope_error());
                }
                super::xtra::external::finish_request(
                    context.player,
                    context.symbols,
                    &request,
                    &response,
                )
            });
            match result {
                Some(Ok(result)) => PendingActionExecution::Complete(ActionCompletion::InternalResult(result)),
                Some(Err(error)) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                None => PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                )),
            }
        }
        InternalVmRequest::ExternalXtraLoad(request) => {
            let receiver = session.borrow_mut().with_player(player_id, |context| {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return None;
                }
                context.player.xtra_manager_state.external.take_load_waiter(&request)
            });
            let Some(Some(receiver)) = receiver else {
                return PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                ));
            };
            if request.notify_host {
                super::xtra::external::start_load_request(&request);
            }
            let loaded = receiver.await.unwrap_or_else(|_| {
                Err(ScriptError::new("external Xtra load waiter was cancelled".to_owned()))
            });
            if let Err(error) = loaded {
                return PendingActionExecution::Complete(ActionCompletion::InternalError(error));
            }
            let continuation = session.borrow_mut().with_player(player_id, |context| {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return None;
                }
                context.player.xtra_manager_state.external.take_load_continuation(&request)
            });
            let Some(Some(continuation)) = continuation else {
                return PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                ));
            };
            let response = match super::xtra::external::execute_request(&continuation) {
                Ok(Some(response)) => response,
                Ok(None) => {
                    return PendingActionExecution::Complete(ActionCompletion::InternalError(
                        ScriptError::new("loaded external Xtra did not claim the continuation".to_owned()),
                    ));
                }
                Err(error) => return PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
            };
            let result = session.borrow_mut().with_player(player_id, |context| {
                if !continuation.owner.same_identity(&context.player.owner)
                    || !continuation.owner.is_arena_live()
                {
                    return Err(super::cancelled_scope_error());
                }
                super::xtra::external::finish_request(
                    context.player,
                    context.symbols,
                    &continuation,
                    &response,
                )
            });
            match result {
                Some(Ok(result)) => PendingActionExecution::Complete(ActionCompletion::InternalResult(result)),
                Some(Err(error)) => PendingActionExecution::Complete(ActionCompletion::InternalError(error)),
                None => PendingActionExecution::Complete(ActionCompletion::InternalError(
                    super::cancelled_scope_error(),
                )),
            }
        }
        other => retained(other, pending_reason),
    }
}

async fn execute_breakpoint_action(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    request: crate::player::driver::BreakpointRequest,
) -> PendingActionExecution {
    let (future, completer) = ManualFuture::new();
    let installed = session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return false;
        }
        context.player.current_breakpoint = Some(crate::player::debug::BreakpointContext {
            breakpoint: request.breakpoint.clone(),
            script_ref: request.script_ref.clone(),
            handler_ref: request.handler_ref.clone(),
            bytecode_index: request.bytecode_index,
            completer,
            error: None,
        });
        context.player.pause_script();
        JsApi::dispatch_scope_list(context.player);
        true
    }).unwrap_or(false);
    if !installed {
        return PendingActionExecution::Retain(crate::player::driver::PendingAction::Breakpoint(request));
    }
    future.await;
    let resumed = session.borrow_mut().with_player(player_id, |context| {
        if owner.same_identity(&context.player.owner) {
            context.player.resume_script();
            true
        } else {
            false
        }
    }).unwrap_or(false);
    if resumed {
        PendingActionExecution::Complete(crate::player::driver::ActionCompletion::Resume)
    } else {
        PendingActionExecution::Retain(crate::player::driver::PendingAction::Breakpoint(request))
    }
}

async fn execute_error_pause_action(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: &super::ownership::OwnerToken,
    request: crate::player::driver::ErrorPauseRequest,
) -> PendingActionExecution {
    #[cfg(not(target_arch = "wasm32"))]
    {
        log::error!(
            "Script error in {:?}:{} at bytecode {}: {}",
            request.handler_ref.1,
            request.script_ref.cast_member,
            request.bytecode_index,
            request.error.message
        );
        return PendingActionExecution::Complete(
            crate::player::driver::ActionCompletion::Error(request.error),
        );
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (future, completer) = ManualFuture::new();
        let installed = session.borrow_mut().with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Ok(false);
            }
            let script_name = context.player.movie.cast_manager.get_script_by_ref(&request.script_ref)
                .map(|script| script.name.clone()).unwrap_or_default();
            let handler_name = context
                .symbols
                .display(&request.handler_ref.1)
                .map_err(|_| {
                    ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "foreign or stale breakpoint handler symbol".to_owned(),
                    )
                })?
                .to_owned();
            context.player.current_breakpoint = Some(crate::player::debug::BreakpointContext {
                breakpoint: crate::player::debug::Breakpoint {
                    script_name,
                    handler_name,
                    bytecode_index: request.bytecode_index,
                },
                script_ref: request.script_ref.clone(),
                handler_ref: request.handler_ref.clone(),
                bytecode_index: request.bytecode_index,
                completer,
                error: Some(request.error.clone()),
            });
            context.player.pause_script();
            JsApi::dispatch_scope_list(context.player);
            JsApi::dispatch_script_error(context.player, &request.error);
            Ok(true)
        });
        let installed = match installed {
            Some(Ok(installed)) => installed,
            Some(Err(error)) => {
                return PendingActionExecution::Complete(
                    crate::player::driver::ActionCompletion::Error(error),
                );
            }
            None => false,
        };
        if !installed {
            return PendingActionExecution::Retain(crate::player::driver::PendingAction::ErrorPause(request));
        }
        future.await;
        if session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) { context.player.resume_script(); true } else { false }
        }).unwrap_or(false) {
            PendingActionExecution::Complete(crate::player::driver::ActionCompletion::Error(request.error))
        } else {
            PendingActionExecution::Retain(crate::player::driver::PendingAction::ErrorPause(request))
        }
    }
}

/// Submit an external completion to the explicit session that owns the
/// action. The session sends a private wake command to that player's queue;
/// no ambient player or global generation is consulted.
pub(crate) fn submit_pending_command_completion(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    ticket: crate::player::driver::CompletionTicket,
    completion: crate::player::driver::ActionCompletion,
) {
    session.borrow_mut().submit_pending_command_completion(player_id, ticket, completion);
}

/// Host boundary for suspended command callbacks. The host takes ownership of
/// pending actions, performs serialized external work, then submits the exact
/// completion through `resume_pending_command`; no session borrow spans that
/// external work.
pub(crate) fn take_pending_commands(
    session: &Rc<RefCell<RuntimeSession>>,
) -> Vec<crate::player::driver::PendingCommand> {
    session.borrow_mut().take_pending_commands()
}

/// Host boundary for evaluator-owned external work. The request carries its
/// real `EvalAction` capability and sender; callers must execute the owned
/// request and submit the result through `submit_pending_eval_completion`.
pub(crate) fn take_pending_eval_requests(
    session: &Rc<RefCell<RuntimeSession>>,
    player_id: u32,
) -> Vec<crate::player::session::PendingEvalRequest> {
    session.borrow_mut().take_pending_eval_requests_for(player_id)
}

pub(crate) fn submit_pending_eval_completion(
    session: &Rc<RefCell<RuntimeSession>>,
    id: crate::player::eval::EvalId,
    action: crate::player::eval::EvalAction,
    owner: super::ownership::OwnerToken,
    result: Result<DatumRef, ScriptError>,
) {
    let _ = session
        .borrow_mut()
        .submit_pending_eval_completion(id, action, owner, result);
}

pub(crate) fn resume_pending_command(
    session: &Rc<RefCell<RuntimeSession>>,
    pending: crate::player::driver::PendingCommand,
    completion: Option<crate::player::driver::ActionCompletion>,
) -> CommandTurn {
    session.borrow_mut().resume_pending_command(pending, completion)
}

pub fn player_dispatch(command: PlayerVMCommand) {
    // Route to the ACTIVE player's own command channel (not the global
    // PLAYER_TX) so a nested `#movie` sub-player's commands land in its own
    // queue, processed by its own command loop with its own active-player id.
    // For the main player this is identical (main.queue_tx == the PLAYER_TX
    // channel). Fall back to PLAYER_TX before any player exists (early boot).
    let tx = reserve_player_ref(|p| p.queue_tx.clone());
    if let Err(e) = tx.try_send(PlayerVMExecutionItem {
        command,
        completer: None,
    }) {
        // The channel is closed or full
        eprintln!("Failed to send command to player: {:?}", e);
    }
}

#[allow(dead_code)]
pub async fn player_dispatch_async(command: PlayerVMCommand) -> Result<DatumRef, ScriptError> {
    let tx = unsafe { PLAYER_TX.clone() }.unwrap();
    let (future, completer) = ManualFuture::new();
    let item = PlayerVMExecutionItem {
        command,
        completer: Some(completer),
    };
    tx.send(item).await.unwrap();
    future.await
}

pub(crate) async fn run_player_command(
    command: PlayerVMCommand,
    completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
    session_handle: Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: super::ownership::OwnerToken,
) -> CommandTurn {
    match command {
        command @ (PlayerVMCommand::TriggerAlertHook
        | PlayerVMCommand::TriggerFlashCallback { .. }
        | PlayerVMCommand::TriggerLingoCallbackOnScript { .. }
        | PlayerVMCommand::TriggerLingoCallbackOnScriptRaw { .. }
        | PlayerVMCommand::TriggerLocalConnectionCallback { .. }
        | PlayerVMCommand::TriggerLocalConnectionCallbackRaw { .. }
        | PlayerVMCommand::DispatchFlashEvent { .. }
        | PlayerVMCommand::DispatchFlashEventRaw { .. }) => {
            run_callback_command(command, completer, session_handle, player_id, owner).await
        }
        command => CommandTurn::Complete(
            run_player_command_result(command, session_handle, player_id, owner).await,
            completer,
        ),
    }
}

#[derive(Clone)]
enum RawFlashValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<RawFlashValue>),
    Object(Vec<(String, RawFlashValue)>),
}

impl<'de> serde::Deserialize<'de> for RawFlashValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RawFlashVisitor;
        impl<'de> serde::de::Visitor<'de> for RawFlashVisitor {
            type Value = RawFlashValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value for a Flash callback")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::Null)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::Number(value.to_string()))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::Number(value.to_string()))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::Number(value.to_string()))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(RawFlashValue::String(value))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(RawFlashValue::Array(values))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some((key, value)) = map.next_entry()? {
                    values.push((key, value));
                }
                Ok(RawFlashValue::Object(values))
            }
        }

        deserializer.deserialize_any(RawFlashVisitor)
    }
}

fn raw_flash_array_index(key: &str) -> Option<u32> {
    if key == "0" {
        return Some(0);
    }
    if key.starts_with('0') || key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = key.parse::<u32>().ok()?;
    (value < u32::MAX).then_some(value)
}

fn raw_flash_datum(
    value: &RawFlashValue,
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    flash_cast_lib: i32,
    flash_cast_member: i32,
) -> DatumRef {
    match value {
        RawFlashValue::Null => player.alloc_datum(Datum::Void),
        RawFlashValue::Bool(value) => player.alloc_datum(Datum::Int(i32::from(*value))),
        RawFlashValue::Number(value) => {
            let number = value.parse::<f64>().unwrap_or_default();
            if number.is_finite()
                && number.fract() == 0.0
                && number >= i32::MIN as f64
                && number <= i32::MAX as f64
            {
                player.alloc_datum(Datum::Int(number as i32))
            } else {
                player.alloc_datum(Datum::Float(number))
            }
        }
        RawFlashValue::String(value) => player.alloc_datum(Datum::String(value.clone())),
        RawFlashValue::Array(values) => {
            let values = values
                .iter()
                .map(|value| raw_flash_datum(value, player, symbols, flash_cast_lib, flash_cast_member))
                .collect();
            player.alloc_datum(Datum::List(DatumType::XmlChildNodes, values, false))
        }
        RawFlashValue::Object(values) => {
            let mut unique_values: Vec<(String, RawFlashValue)> = Vec::new();
            for (key, value) in values {
                if let Some((_, existing)) = unique_values.iter_mut().find(|(existing_key, _)| existing_key == key) {
                    *existing = value.clone();
                } else {
                    unique_values.push((key.clone(), value.clone()));
                }
            }
            // JSON.parse exposes the final value for duplicate keys.  Apply
            // that same rule before recognizing the retained Flash-object
            // marker, so a later non-string marker cannot resurrect an older
            // object handle.
            if let Some(path) = unique_values.iter().find_map(|(key, value)| {
                (key == "__dirplayer_stored_path").then(|| match value {
                    RawFlashValue::String(path) => Some(path.as_str()),
                    _ => None,
                }).flatten()
            }) {
                return player.alloc_datum(Datum::FlashObjectRef(
                    FlashObjectRef::from_path_with_member(path, flash_cast_lib, flash_cast_member),
                ));
            }
            let mut properties = std::collections::VecDeque::new();
            let mut flash_type = None;
            let mut ordered: Vec<_> = unique_values.iter().enumerate().collect();
            ordered.sort_by_key(|(position, (key, _))| {
                raw_flash_array_index(key)
                    .map(|index| (0_u8, index as u64, 0_u64))
                    .unwrap_or((1, 0, *position as u64))
            });
            for (_, (key, value)) in ordered {
                if key == "#type" {
                    flash_type = match value {
                        RawFlashValue::String(value) => Some(value.clone()),
                        _ => None,
                    };
                    continue;
                }
                let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern(key)));
                let value_ref = raw_flash_datum(value, player, symbols, flash_cast_lib, flash_cast_member);
                properties.push_back((key_ref, value_ref));
            }
            if let Some(value) = flash_type {
                let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern("#type")));
                properties.push_front((key_ref, player.alloc_datum(Datum::String(value))));
            }
            player.alloc_datum(Datum::PropList(properties, false))
        }
    }
}

fn raw_flash_args(
    args_json: &str,
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    flash_cast_lib: i32,
    flash_cast_member: i32,
) -> Result<Vec<DatumRef>, ScriptError> {
    let mut deserializer = serde_json::Deserializer::from_str(args_json);
    let value = <RawFlashValue as serde::Deserialize>::deserialize(&mut deserializer)
        .map_err(|error| ScriptError::new(format!("invalid Flash callback JSON: {error}")))?;
    use serde::de::Deserializer as _;
    deserializer
        .end()
        .map_err(|error| ScriptError::new(format!("invalid Flash callback JSON: {error}")))?;
    let RawFlashValue::Array(values) = value else {
        return Err(ScriptError::new("Flash callback arguments are not an array".to_owned()));
    };
    let mut args = vec![player.alloc_datum(Datum::Void)];
    args.extend(values.iter().map(|value| {
        raw_flash_datum(value, player, symbols, flash_cast_lib, flash_cast_member)
    }));
    Ok(args)
}

fn command_turn_from_script(
    player_id: u32,
    owner: super::ownership::OwnerToken,
    turn: ScriptHandlerTurn,
    completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
) -> CommandTurn {
    match turn {
        ScriptHandlerTurn::Complete(result) => {
            CommandTurn::Complete(result.map(|scope| scope.return_value), completer)
        }
        ScriptHandlerTurn::Waiting => CommandTurn::Waiting(
            crate::player::driver::PendingCommand {
                player_id,
                owner,
                action: None,
                started: false,
                ticket: None,
                completer,
                event_sender: None,
                score_continuation: None,
                        child_completion: None,
                        eval_child: None,
                        eval_sender: None,
            },
        ),
        ScriptHandlerTurn::Pending(action) => CommandTurn::Pending(
            crate::player::driver::PendingCommand {
                player_id,
                owner,
                ticket: Some(action.ticket().clone()),
                action: Some(action),
                started: false,
                completer,
                event_sender: None,
                score_continuation: None,
                        child_completion: None,
                        eval_child: None,
                        eval_sender: None,
            },
        ),
    }
}

async fn run_callback_command(
    command: PlayerVMCommand,
    completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
    session_handle: Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: super::ownership::OwnerToken,
) -> CommandTurn {
    if !owner_is_current(&session_handle, player_id, &owner) {
        return CommandTurn::Complete(
            Err(crate::player::ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "Flash callback cancelled: player owner changed".to_owned(),
            )),
            completer,
        );
    }
    match command {
        PlayerVMCommand::TriggerAlertHook => {
            let call_params = session_handle.borrow_mut().with_player(player_id, |mut context| {
                let arg_list = vec![
                    context.player.alloc_datum(Datum::String("Script Error".to_owned())),
                    context.player.alloc_datum(Datum::String(
                        "An error occurred in the script".to_owned(),
                    )),
                ];
                match context.player.movie.alert_hook.clone() {
                    Some(ScriptReceiver::ScriptInstance(instance_ref)) => {
                        let instance = context.player.allocator.get_script_instance(&instance_ref);
                        let script = context.player.movie.cast_manager.get_script_by_ref(&instance.script)?;
                        script
                            .get_own_handler_ref(Symbol::builtin(BuiltInSymbol::AlertHook))
                            .map(|handler| (Some(instance_ref), handler, arg_list))
                    }
                    Some(ScriptReceiver::Script(script_ref)) => {
                        let script = context.player.movie.cast_manager.get_script_by_ref(&script_ref)?;
                        script
                            .get_own_handler_ref(Symbol::builtin(BuiltInSymbol::AlertHook))
                            .map(|handler| (None, handler, arg_list))
                    }
                    Some(ScriptReceiver::ScriptText(_)) | None => None,
                }
            });
            let Some(Some((receiver, handler, args))) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::TriggerFlashCallback {
            sprite_num,
            handler_name,
            args,
        } => {
            let call_params = session_handle.borrow_mut().with_player(player_id, |context| {
                let sprite = context.player.movie.score.get_sprite(sprite_num as i16)?;
                for instance_ref in &sprite.script_instance_list {
                    let instance = context.player.allocator.get_script_instance(instance_ref);
                    if let Some(script) = context.player.movie.cast_manager.get_script_by_ref(&instance.script) {
                        if let Some(handler_ref) = script.get_own_handler_ref(handler_name.clone()) {
                            return Some((Some(instance_ref.clone()), handler_ref, args.clone()));
                        }
                    }
                }
                None
            });
            let Some(Some((receiver, handler, args))) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::TriggerLingoCallbackOnScript {
            cast_lib,
            cast_member,
            handler_name,
            args,
        } => {
            let call_params = session_handle.borrow_mut().with_player(player_id, |context| {
                for (instance_id, entry) in context.player.allocator.iter_script_instances() {
                    let instance = &entry.script_instance;
                    if instance.script.cast_lib == cast_lib && instance.script.cast_member == cast_member {
                        if let Some(script) = context.player.movie.cast_manager.get_script_by_ref(&instance.script) {
                            if let Some(handler_ref) = script.get_own_handler_ref(handler_name.clone()) {
                                let receiver = ScriptInstanceRef::from_id(
                                    instance_id as u32,
                                    entry.ref_count.get(),
                                    context.player.owner.clone(),
                                );
                                return Some((Some(receiver), handler_ref, args.clone()));
                            }
                        }
                    }
                }
                None
            });
            let Some(Some((receiver, handler, args))) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::TriggerLingoCallbackOnScriptRaw {
            cast_lib,
            cast_member,
            handler_name,
            args_json,
            flash_cast_lib,
            flash_cast_member,
        } => {
            let call_params = {
                let mut session = session_handle.borrow_mut();
                session.with_player(player_id, |context| {
                    let args = raw_flash_args(
                        &args_json,
                        context.player,
                        context.symbols,
                        flash_cast_lib,
                        flash_cast_member,
                    )?;
                    let handler_name = context.symbols.intern(&handler_name);
                    for (instance_id, entry) in context.player.allocator.iter_script_instances() {
                        let instance = &entry.script_instance;
                        if instance.script.cast_lib == cast_lib && instance.script.cast_member == cast_member {
                            if let Some(script) = context.player.movie.cast_manager.get_script_by_ref(&instance.script) {
                                if let Some(handler_ref) = script.get_own_handler_ref(handler_name.clone()) {
                                    let receiver = ScriptInstanceRef::from_id(
                                        instance_id as u32,
                                        entry.ref_count.get(),
                                        context.player.owner.clone(),
                                    );
                                    return Ok(Some((Some(receiver), handler_ref, args)));
                                }
                            }
                        }
                    }
                    Ok(None)
                })
            };
            let call_params = match call_params {
                Some(Ok(value)) => value,
                Some(Err(error)) => return CommandTurn::Complete(Err(error), completer),
                None => None,
            };
            let Some((receiver, handler, args)) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::TriggerLocalConnectionCallback {
            target,
            handler_name,
            args,
        } => {
            let call_params = session_handle.borrow_mut().with_player(player_id, |context| {
                let instance = context.player.allocator.get_script_instance_opt(&target)?;
                let script = context.player.movie.cast_manager.get_script_by_ref(&instance.script)?;
                let handler_name = context.symbols.intern(&handler_name);
                let handler = script.get_own_handler_ref(handler_name)?;
                Some((Some(target.clone()), handler, args.clone()))
            });
            let Some(Some((receiver, handler, args))) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::TriggerLocalConnectionCallbackRaw {
            connection_name,
            method_name,
            args_json,
        } => {
            let call_params = {
                let mut session = session_handle.borrow_mut();
                session.with_player(player_id, |context| {
                    let Some(lc_path) = context.player.flash_lc_connections.get(&connection_name).cloned() else {
                        return Ok(None);
                    };
                    let Some((handler_name, target)) = context
                        .player
                        .flash_lc_callbacks
                        .get(&(lc_path, method_name))
                        .cloned()
                    else {
                        return Ok(None);
                    };
                    let args = raw_flash_args(&args_json, context.player, context.symbols, 1, 1)?;
                    let handler_name = context.symbols.intern(&handler_name);
                    let handler = context
                        .player
                        .allocator
                        .get_script_instance_opt(&target)
                        .and_then(|instance| context.player.movie.cast_manager.get_script_by_ref(&instance.script))
                        .and_then(|script| script.get_own_handler_ref(handler_name));
                    Ok(handler.map(|handler| (Some(target), handler, args)))
                })
            };
            let call_params = match call_params {
                Some(Ok(value)) => value,
                Some(Err(error)) => return CommandTurn::Complete(Err(error), completer),
                None => None,
            };
            let Some((receiver, handler, args)) = call_params else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let turn = {
                let mut session = session_handle.borrow_mut();
                player_call_script_handler_turn_in_session_sync(
                    &mut session, player_id, receiver, handler, &args,
                )
            };
            command_turn_from_script(player_id, owner.clone(), turn, completer)
        }
        PlayerVMCommand::DispatchFlashEvent {
            cast_lib,
            cast_member,
            handler_name,
            args,
        } => {
            let sprite_nums: Vec<u16> = session_handle
                .borrow_mut()
                .with_player(player_id, |context| {
                    context
                        .player
                        .movie
                        .score
                        .channels
                        .iter()
                        .filter_map(|channel| {
                            let member = channel.sprite.member.as_ref()?;
                            (member.cast_lib == cast_lib && member.cast_member == cast_member)
                                .then_some(channel.number as u16)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if sprite_nums.is_empty() {
                let _ = super::events::player_invoke_global_event_owned(
                    session_handle.clone(), player_id, owner.clone(), handler_name, args,
                ).await;
            } else {
                for sprite_num in sprite_nums {
                    let _ = super::events::player_dispatch_event_to_sprite_targeted_owned(
                        session_handle.clone(), player_id, owner.clone(),
                        handler_name.clone(), args.clone(), sprite_num,
                    ).await;
                }
            }
            CommandTurn::Complete(Ok(DatumRef::Void), completer)
        }
        PlayerVMCommand::DispatchFlashEventRaw { cast_lib, cast_member, body } => {
            let Some((handler_name, raw_args)) = crate::parse_flash_event_body(&body) else {
                return CommandTurn::Complete(Ok(DatumRef::Void), completer);
            };
            let (handler_name, args) = {
                let mut session = session_handle.borrow_mut();
                let Some((handler_name, args)) = session.with_player(player_id, |context| {
                    let args = raw_args
                        .iter()
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
                                Datum::String(token.clone())
                            };
                            context.player.alloc_datum(datum)
                        })
                        .collect();
                    (context.symbols.intern(&handler_name), args)
                }) else {
                    return CommandTurn::Complete(Ok(DatumRef::Void), completer);
                };
                (handler_name, args)
            };
            let sprite_nums: Vec<u16> = session_handle
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.movie.score.channels.iter().filter_map(|channel| {
                        let member = channel.sprite.member.as_ref()?;
                        (member.cast_lib == cast_lib && member.cast_member == cast_member)
                            .then_some(channel.number as u16)
                    }).collect()
                })
                .unwrap_or_default();
            if sprite_nums.is_empty() {
                let _ = super::events::player_invoke_global_event_owned(
                    session_handle.clone(), player_id, owner.clone(), handler_name, args,
                ).await;
            } else {
                for sprite_num in sprite_nums {
                    let _ = super::events::player_dispatch_event_to_sprite_targeted_owned(
                        session_handle.clone(), player_id, owner.clone(),
                        handler_name.clone(), args.clone(), sprite_num,
                    ).await;
                }
            }
            CommandTurn::Complete(Ok(DatumRef::Void), completer)
        }
        _ => unreachable!("non-callback command routed to callback executor"),
    }
}

async fn run_player_command_result(
    command: PlayerVMCommand,
    session_handle: Rc<RefCell<RuntimeSession>>,
    player_id: u32,
    owner: super::ownership::OwnerToken,
) -> Result<DatumRef, ScriptError> {
    player_wait_available().await;
    // The command owns a session handle and capability.  Validate that exact
    // owner after the yield; a numeric player id or process-wide generation
    // can refer to a replacement player after reset.
    if !owner_is_current(&session_handle, player_id, &owner) {
        return Err(crate::player::ScriptError::new_code(
            crate::player::ScriptErrorCode::Abort,
            "Command cancelled: player owner changed (test reset)".to_string(),
        ));
    }
    match command {
        PlayerVMCommand::SetExternalParams(params) => {
            reserve_player_mut(|player| {
                player.external_params = params;
                crate::player::stage::apply_stage_draw_rect(player);
                let (w, h) = crate::player::stage::stage_canvas_dims(player);
                crate::js_api::JsApi::dispatch_stage_size_changed(w, h, player.center_stage);
            });
        }
        PlayerVMCommand::SetBasePath(path) => {
            reserve_player_mut(|player| {
                player.net_manager.set_base_path(Url::parse(&path)
                    .expect(&format!("Invalid base path URL: '{}'", path)));
            });
        }
        PlayerVMCommand::SetStartupDo(code) => {
            reserve_player_mut(|player| {
                player.startup_do = if code.is_empty() { None } else { Some(code) };
            });
        }
        PlayerVMCommand::SetStartupDoBefore(code) => {
            reserve_player_mut(|player| {
                player.startup_do_before = if code.is_empty() { None } else { Some(code) };
            });
        }
        PlayerVMCommand::SetStartupGo(frame) => {
            reserve_player_mut(|player| {
                player.startup_go = if frame == 0 { None } else { Some(frame) };
            });
        }
        PlayerVMCommand::SetMoviePathOverride(path) => {
            reserve_player_mut(|player| {
                player.movie_path_override = if path.is_empty() { None } else { Some(path) };
            });
        }
        PlayerVMCommand::SetMoviePathLabel(path) => {
            reserve_player_mut(|player| {
                player.movie_path_label = if path.is_empty() { None } else { Some(path) };
            });
        }
        PlayerVMCommand::SetSystemFontPath(path) => {
            console_warn!("Loading system font: {}", path);
            player_load_system_font_owned(
                session_handle.clone(),
                player_id,
                owner.clone(),
                path,
            )
            .await?;
        }
        PlayerVMCommand::LoadMovieFromFile(file_path, autoplay) => {
            // `--doBefore` runs "in the scope of the game being curated BEFORE
            // it has been loaded" (SPR README), so it goes here rather than in
            // `run_movie_init_sequence` where `--do` lives.
            crate::player::run_startup_do_before().await;
            match crate::player::load_movie_from_file_owned(
                session_handle.clone(),
                player_id,
                owner.clone(),
                file_path.clone(),
            )
            .await
            {
                Ok(()) => {
                    if autoplay {
                        session_handle
                            .borrow_mut()
                            .with_player(player_id, |context| {
                                if owner.same_identity(&context.player.owner)
                                    && owner.is_arena_live()
                                {
                                    context.player.is_playing = true;
                                    context.player.is_script_paused = false;
                                }
                            });
                    }
                }
                Err(err) => {
                    JsApi::dispatch_movie_load_failed(&file_path, &err.message);
                }
            }
        }
        PlayerVMCommand::SetStageSize(width, height) => {
            reserve_player_mut(|player| {
                player.stage_size = (width, height);
                crate::player::stage::apply_stage_draw_rect(player);
                let (w, h) = crate::player::stage::stage_canvas_dims(player);
                crate::js_api::JsApi::dispatch_stage_size_changed(w, h, player.center_stage);
            });
        }
        PlayerVMCommand::TimeoutTriggered(timeout_ref) => {
            let (is_found, is_playing, is_script_paused, target_ref, handler_name, timeout_name) =
                reserve_player_mut(|player| {
                    if let Some(timeout) = player.timeout_manager.get_timeout(&timeout_ref) {
                        let is_playing = player.is_playing;
                        let is_script_paused = player.is_script_paused;
                        (
                            true,
                            is_playing,
                            is_script_paused,
                            timeout.target_ref.clone(),
                            timeout.handler.to_owned(),
                            timeout.name.to_owned(),
                        )
                    } else {
                        (
                            false,
                            false,
                            false,
                            DatumRef::Void,
                            Symbol::builtin(BuiltInSymbol::EmptyString),
                            "".to_string(),
                        )
                    }
                });
            if !is_found {
                warn!("Timeout triggered but not found: {}", timeout_ref);
                return Ok(DatumRef::Void);
            }
            if !is_playing || is_script_paused {
                // TODO how to handle is_script_paused?
                warn!("Timeout triggered but not playing");
                return Ok(DatumRef::Void);
            }
            let timeout_name_for_args = timeout_name.clone();
            let ref_datum = player_alloc_datum(Datum::TimeoutRef(timeout_name));
            let args = vec![ref_datum];
            // Director 11.5 Scripting Dictionary, `new()` (Timeout): the
            // targetObject "indicates which CHILD OBJECT's handler should be
            // called ... If you omit this parameter, Director looks for the
            // specified handler in the movie script."
            //
            // A target that is NOT an object can't receive a method call, so
            // Director falls back to the movie script and passes the target
            // through as the first argument. AreaZero's Event Manager relies on
            // this: it creates
            //     timeout().new(tName, pDelay, #DelayEventTimeOut, pevent)
            // with pevent a STRING, and its handler is declared
            //     on DelayEventTimeOut tEvent, tTimeOut
            // i.e. (target, timeoutObject). Dispatching the handler ON the
            // string raised "No handler DelayEventTimeOut for string datum".
            let target_is_object = reserve_player_ref(|player| {
                matches!(
                    player.get_datum(&target_ref),
                    Datum::ScriptInstanceRef(_)
                )
            });
            if target_is_object {
                player_dispatch_callback_event(target_ref, handler_name, &args);
            } else if target_ref != DatumRef::Void {
                let mut args = vec![target_ref];
                args.extend(
                    [player_alloc_datum(Datum::TimeoutRef(timeout_name_for_args))]
                );
                player_dispatch_global_event(handler_name, &args);
            } else {
                player_dispatch_global_event(handler_name, &args);
            }
        }
        PlayerVMCommand::PrintMemberBitmapHex(member_ref) => {
            reserve_player_ref(|player| {
                let member = player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .unwrap();
                let bitmap = member.member_type.as_bitmap().unwrap();
                let bitmap = player.bitmap_manager.get_bitmap(bitmap.image_ref).unwrap();
                let bitmap = &bitmap.data;
                warn!("Bitmap hex: {}", bitmap.to_hex_string());
            });
        }
        PlayerVMCommand::PlayMemberSound(member_ref) => {
            use crate::player::handlers::datum_handlers::sound_channel::SoundChannelDatumHandlers;
            let (cl, cm) = (member_ref.cast_lib, member_ref.cast_member);
            reserve_player_mut(|player| {
                let is_sound = player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .map(|m| matches!(m.member_type, CastMemberType::Sound(_)))
                    .unwrap_or(false);
                if !is_sound {
                    console_warn!("[sound-preview] member {}:{} is not a sound member", cl, cm);
                    return;
                }
                let member_datum = player.alloc_datum(Datum::CastMember(member_ref));
                // Preview on channel 1 (Director's default puppetSound channel).
                // Always play ONCE here, ignoring the member's loop flag, so a
                // looping member doesn't run forever from the dev-UI preview.
                let channel = match player.get_sound_channel(1) {
                    Ok(ch) => ch,
                    Err(e) => {
                        console_warn!("[sound-preview] no channel for {}:{}: {}", cl, cm, e.message);
                        return;
                    }
                };
                if let Err(e) =
                    SoundChannelDatumHandlers::handle_play_file(player, &channel, &member_datum, 1)
                {
                    console_warn!("[sound-preview] play failed for {}:{}: {}", cl, cm, e.message);
                }
            });
        }
        PlayerVMCommand::MouseDown((x, y)) => {
            crate::player::wait_for_handler_gap().await;
            crate::player::hold_draw_for_input_handler();
            if !player_is_playing().await {
                return Ok(DatumRef::Void);
            }
            // In Director, mouseDownScript intercepts BEFORE sprites get the event.
            // Only block when it contains executable content (not just a comment).
            // Comments like "--nothing" are stored but don't block propagation.
            let mouse_down_script_active = reserve_player_ref(|player| {
                has_executable_callback(&player.movie.mouse_down_script)
            });
            // Capture click timestamp BEFORE the stepFrame flush below. The flush is
            // async and variable in length (CS rooms have 50+ FurnitureItems in
            // actorList, plus IsoScene.processRoomRollover doing pixel hit-tests),
            // so reading `now` after it would add the flush duration to the
            // inter-click delta and a fast double-click could be misclassified as
            // two single clicks. CS's ACTION_TIMED_ANIMATION (and similar) reads
            // `the doubleClick` to gate `toggleState()`, so a wrong flag silently
            // breaks every animation-toggling action.
            let click_now = chrono::Utc::now().timestamp_millis().abs();
            // Flush actorList stepFrame so cached rollover state (e.g. oMouseSquare)
            // is fresh before the mouseDown handler reads it. On mobile/touch there's
            // no prior mouse_move, so stepFrame hasn't run at the tap position yet.
            // mouse_loc is already set synchronously by the JS-side mouse_down() call.
            {
                let actor_snapshot = reserve_player_ref(|player| {
                    player.actor_list_stepframe_snapshot()
                });
                for (_idx, actor_ref) in actor_snapshot.0.iter().enumerate() {
                    if actor_snapshot.1.contains(&actor_ref.unwrap()) {
                        let _ = call_datum_handler_active(
                            actor_ref, Symbol::builtin(BuiltInSymbol::StepFrame), &vec![],
                        ).await;
                    }
                }
            }

            // `the mouseDownScript` (if set) runs at the END of the
            // mouseDown pipeline, alongside sprite/cast/frame dispatch.
            // We deliberately do NOT short-circuit the sprite dispatch
            // when this script is set — Director's docs say mouseDownScript
            // suppresses sprite handlers unless the script calls pass(),
            // but real movies (e.g. ClubMarian's `checkMouse` global
            // observer that early-returns on most frames) rely on sprite
            // handlers firing regardless. See the post-sprite-dispatch
            // callback at the end of this branch.
            let _ = mouse_down_script_active;

            // A click inside a #scroll field's scrollbar drives the scrollbar and
            // goes no further — Director's field chrome consumes it rather than
            // passing it to the sprite's scripts or moving the caret. Tested
            // BEFORE the normal sprite lookup because that lookup applies
            // `is_click_transparent_sprite`, which makes a non-editable field
            // click-through; the scrollbar must stay live regardless.
            let scrollbar_consumed = reserve_player_mut(|player| {
                player.mouse_loc = (x, y);
                player.movie.mouse_down = true;
                player.movie.click_loc = (x, y);
                handle_field_scrollbar_mouse_down(player, x, y)
            });
            if scrollbar_consumed {
                return Ok(DatumRef::Void);
            }

            // Use scripted=true so only sprites with scripts (behavior or cast member)
            // are detected. Non-scripted sprites (decorations, overlays) are skipped,
            // matching Director behavior.
            reserve_player_mut(|player| {
                let is_double_click = (click_now - player.last_mouse_down_time) < 500;
                player.mouse_loc = (x, y);
                player.movie.mouse_down = true;
                player.movie.click_loc = (x, y);
                player.is_double_click = is_double_click;
                player.last_mouse_down_time = click_now;

                // "the clickOn" should return the topmost sprite at the click point
                // regardless of whether it has a script — use unscripted lookup.
                let any_sprite = get_sprite_at(player, x, y, false);
                if let Some(sprite_number) = any_sprite {
                    player.click_on_sprite = sprite_number as i16;
                    // Capture drag offset for moveable sprites (so sprite doesn't jump to cursor)
                    if let Some(sprite) = player.movie.score.get_sprite(sprite_number as i16) {
                        if sprite.moveable {
                            player.drag_offset = (sprite.loc_h - x, sprite.loc_v - y);
                        }
                    }
                    let sprite = player.movie.score.get_sprite(sprite_number as i16);
                    let sprite_member = sprite
                        .and_then(|x| x.member.as_ref())
                        .and_then(|x| player.movie.cast_manager.find_member_by_ref(&x));
                    if let Some(sprite_member) = sprite_member {
                        // Four cases where a clicked sprite should become the
                        // keyboardFocusSprite (so subsequent keyDown events route
                        // through it):
                        //   - `field.editable` / `text.info.editable` from the file
                        //   - `the editable of sprite` set at runtime
                        //   - The sprite has a behaviour with a `keyDown` handler
                        //     (fake-input pattern: a non-editable text member with
                        //     a sprite behaviour that traps keyDown and renders its
                        //     own buffer into `myMember.text`, e.g. spineworld_dcr's
                        //     `formscript` on the password fields).
                        let member_editable = match &sprite_member.member_type {
                            CastMemberType::Field(f) => f.editable,
                            CastMemberType::Text(t) => t.info.as_ref().map_or(false, |i| i.editable),
                            _ => false,
                        };
                        let sprite_editable = sprite
                            .map(|s| s.editable)
                            .unwrap_or(false);
                        let has_keydown = sprite
                            .map(|s| crate::player::score::sprite_has_handler(player, s, &["keyDown", "keyUp"]))
                            .unwrap_or(false);
                        if sprite_editable || member_editable || has_keydown {
                            player.keyboard_focus_sprite = sprite_number as i16;
                        }
                    }

                    // Toggle hilite for button members on mouseDown
                    if let Some(sprite) = player.movie.score.get_sprite(sprite_number as i16) {
                        if let Some(member_ref) = sprite.member.clone() {
                            if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
                                match &mut member.member_type {
                                    CastMemberType::Button(button) => {
                                        match button.button_type {
                                            crate::player::cast_member::ButtonType::PushButton => {
                                                button.hilite = true;
                                            }
                                            crate::player::cast_member::ButtonType::CheckBox => {
                                                button.hilite = !button.hilite;
                                            }
                                            crate::player::cast_member::ButtonType::RadioButton => {
                                                button.hilite = true;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                } else {
                    player.click_on_sprite = 0;
                }

                // For event dispatch targeting, use scripted lookup —
                // only sprites with behaviors or cast member scripts receive mouseDown.
                let scripted_sprite = get_sprite_at(player, x, y, true);
                if let Some(sprite_number) = scripted_sprite {
                    player.mouse_down_sprite = sprite_number as i16;
                } else {
                    player.mouse_down_sprite = -1;
                }
                debug!(
                    "[mouseDown] mouse_down_sprite={} (scripted lookup at {},{})",
                    player.mouse_down_sprite, x, y
                );
            });

            // If the click landed on a Flash sprite, forward the press into
            // the SWF so its own AS1 `on (press)` / button handlers run.
            // Director normally delivers these by passing the event straight
            // through to the embedded Flash player; in our setup Ruffle's
            // canvas is offscreen so the click never reaches it organically.
            // Use unscripted lookup here — Flash members don't always carry
            // a Director-side behaviour, but their internal buttons still
            // need the event.
            let (flash_forward, debug_info) = reserve_player_ref(|player| {
                let any_sprite = get_sprite_at(player, x, y, false);
                if any_sprite.is_none() {
                    return (None, format!("no sprite at ({},{})", x, y));
                }
                let any_sprite = any_sprite.unwrap();
                let sprite = match player.movie.score.get_sprite(any_sprite as i16) {
                    Some(s) => s,
                    None => return (None, format!("sprite#{} not found", any_sprite)),
                };
                let member_ref = match sprite.member.as_ref() {
                    Some(m) => m,
                    None => return (None, format!("sprite#{} has no member", any_sprite)),
                };
                let member = match player.movie.cast_manager.find_member_by_ref(member_ref) {
                    Some(m) => m,
                    None => return (None, format!("member {:?} not resolvable", member_ref)),
                };
                let type_str = member.member_type.type_string();
                if !matches!(member.member_type, CastMemberType::Flash(_)) {
                    return (None, format!("sprite#{} member type='{}' (not Flash)", any_sprite, type_str));
                }
                let rect = super::score::get_concrete_sprite_rect(player, sprite);
                (
                    Some((any_sprite as i32, x - rect.left, y - rect.top, rect.right - rect.left, rect.bottom - rect.top)),
                    format!("Flash sprite#{} member={}:{} rect=({},{})-({},{})",
                        any_sprite, member_ref.cast_lib, member_ref.cast_member,
                        rect.left, rect.top, rect.right, rect.bottom),
                )
            });
            debug!("[Flash mouseDown] click@({},{}) → {}", x, y, debug_info);

            // Diagnostic: dump the script names attached to the clicked sprite
            // so we can see what behaviours (if any) have a chance to handle
            // mouseUp. If the list is empty / has no on mouseUp, clicking is a
            // no-op regardless of Flash forwarding.
            let sprite_scripts_dump = reserve_player_ref(|player| {
                let any_sprite = match get_sprite_at(player, x, y, false) {
                    Some(s) => s,
                    None => return "none".to_string(),
                };
                let sprite = match player.movie.score.get_sprite(any_sprite as i16) {
                    Some(s) => s,
                    None => return format!("sprite#{} missing", any_sprite),
                };
                let names: Vec<String> = sprite
                    .script_instance_list
                    .iter()
                    .map(|inst_ref| {
                        let inst = player.allocator.get_script_instance(inst_ref);
                        let script = player.movie.cast_manager.get_script_by_ref(&inst.script);
                        let script_name = script.map(|s| s.name.clone()).unwrap_or_else(|| "?".to_string());
                        let handlers = script
                            .map(|s| {
                                s.handlers
                                    .iter()
                                    .map(|(name, _def)| format!("{:?}", name))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default();
                        format!("{}({}/{}: handlers=[{}])",
                            script_name,
                            inst.script.cast_lib, inst.script.cast_member,
                            handlers)
                    })
                    .collect();
                if names.is_empty() {
                    format!("sprite#{} has 0 behaviours", any_sprite)
                } else {
                    format!("sprite#{} behaviours: {}", any_sprite, names.join(" | "))
                }
            });
            debug!("[click sprite scripts] {}", sprite_scripts_dump);
            if let Some((sn, lx, ly, sw, sh)) = flash_forward {
                // AVM1 button hit-testing reads the stage-mouse position
                // that's last updated by MouseMove. A real browser always
                // emits pointermove before pointerdown, so the position is
                // current. Our injected MouseDown alone leaves the stage
                // mouse at its previous location (often 0,0 if no organic
                // input ever reached the SWF), and Ruffle's button test
                // misses the click. Send a MouseMove first to seed it.
                let _ = super::handlers::datum_handlers::flash_object::ruffle_dispatch_mouse_event_global(
                    sn, "move", lx, ly, sw, sh,
                );
                let _ = super::handlers::datum_handlers::flash_object::ruffle_dispatch_mouse_event_global(
                    sn, "down", lx, ly, sw, sh,
                );
            }

            // Temporarily clear ALL is_yield_safe() flags so that updateStage()
            // called from within mouseDown handlers will render (but not yield).
            // MouseDown commands can be processed at any .await point in the frame
            // loop, where multiple flags may be true simultaneously (e.g.
            // is_in_frame_update AND in_enter_frame during enterFrame dispatch).
            // Set in_mouse_command so the frame loop skips frame updates/advancement
            // and updateStage renders without sleeping (preventing re-entrant event
            // dispatch and timing issues with mouseUp processing).
            let saved_yield_flags = reserve_player_mut(|player| {
                let saved = (
                    player.is_in_frame_update,
                    player.in_frame_script,
                    player.in_enter_frame,
                    player.in_prepare_frame,
                    player.in_event_dispatch,
                    player.in_mouse_command,
                );
                player.is_in_frame_update = false;
                player.in_frame_script = false;
                player.in_enter_frame = false;
                player.in_prepare_frame = false;
                player.in_event_dispatch = false;
                player.in_mouse_command = true;
                saved
            });

            // Dispatch to sprite behaviors if the sprite has any, otherwise
            // fall through to frame/movie scripts per Director's propagation chain.
            let sprite_with_behaviors = reserve_player_ref(|player| {
                if player.mouse_down_sprite > 0 {
                    let sprite = player.movie.score.get_sprite(player.mouse_down_sprite);
                    let has_behaviors = sprite.map_or(false, |s| {
                        player.sprite_has_script_instance_ids(
                            player.mouse_down_sprite,
                            s.script_instance_list.as_slice(),
                        )
                    });
                    if has_behaviors {
                        return Some(player.mouse_down_sprite as u16);
                    }
                }
                None
            });

            // A behavior that calls stopEvent() also blocks the cast member
            // script — the message hierarchy stops there (Director 11.5
            // Scripting Dictionary, `stopEvent()`).
            let mut event_stopped = false;
            if let Some(sprite_num) = sprite_with_behaviors {
                event_stopped = player_dispatch_event_to_sprite_targeted(
                    Symbol::builtin(BuiltInSymbol::MouseDown),
                    &vec![],
                    sprite_num,
                ).await;
            } else {
                player_invoke_frame_and_movie_scripts(
                    Symbol::builtin(BuiltInSymbol::MouseDown),
                    &vec![]
                ).await?;
            }

            // Execute cast member script if it exists
            let cast_member_script_call = retained_session_handle()
                .and_then(|handle| {
                    let mut session = handle.borrow_mut();
                    session.with_player(active_player_id() as u32, |context| {
                let player = context.player;
                let symbols = context.symbols;
                if event_stopped || player.mouse_down_sprite <= 0 {
                    return None;
                }

                let sprite = player.movie.score.get_sprite(player.mouse_down_sprite)?;
                let member_ref = sprite.member.as_ref()?;
                let member = player.movie.cast_manager.find_member_by_ref(member_ref)?;

                // First check for member behavior script (stored in member_script_ref)
                if let Some(script_ref) = member.get_member_script_ref() {
                    debug!(
                        "Cast member '{}' has behavior script (cast_lib={}, member={}), executing mouseDown",
                        member.name, script_ref.cast_lib, script_ref.cast_member
                    );

                    if let Some(script) = player.movie.cast_manager.get_script_by_ref(script_ref) {
                        if let Some(handler) = script.get_own_handler_ref(Symbol::builtin(BuiltInSymbol::MouseDown)) {
                            return Some((None, handler, vec![]));
                        }
                    }
                }

                // Fallback: check for script_id and get directly from lctx.scripts
                let script_id = member.get_script_id()?;

                debug!(
                    "Cast member '{}' has script {}, getting from lctx.scripts for mouseDown",
                    member.name, script_id
                );

                let script = {
                    let cast_lib = player.movie.cast_manager.get_cast_mut(member_ref.cast_lib as u32);
                    cast_lib.get_behavior_script_from_lctx(script_id, symbols)
                };

                let script = match script {
                    Some(s) => {
                       debug!("Behavior script {} found for mouseDown", script_id);
                        s
                    }
                    None => {
                        debug!("Behavior script {} NOT FOUND in lctx.scripts for mouseDown", script_id);
                        return None;
                    }
                };

                let handler = script.get_own_handler_ref(Symbol::builtin(BuiltInSymbol::MouseDown))?;

                Some((None, handler, vec![]))
                    })
                })
                .flatten();

            let mut handler_err: Option<ScriptError> = None;
            if let Some((receiver, handler, args)) = cast_member_script_call {
                match player_call_script_handler(receiver, handler, &args).await {
                    ScriptHandlerTurn::Complete(Err(e)) => handler_err = Some(e),
                    ScriptHandlerTurn::Complete(Ok(_))
                    | ScriptHandlerTurn::Waiting
                    | ScriptHandlerTurn::Pending(_) => {}
                }
            }

            // Dispatch mouseDownScript last as well (for cases where it was set
            // during sprite handler execution, e.g. not set at start of event)
            if handler_err.is_none() {
                if let Err(e) = super::events::player_dispatch_movie_callback_owned(
                    session_handle.clone(), player_id, owner.clone(), BuiltInSymbol::MouseDown,
                ).await {
                    handler_err = Some(e);
                }
            }

            // Restore all is_yield_safe() flags and in_mouse_command
            // MUST happen even on error to prevent skip_frame getting stuck
            reserve_player_mut(|player| {
                player.is_in_frame_update = saved_yield_flags.0;
                player.in_frame_script = saved_yield_flags.1;
                player.in_enter_frame = saved_yield_flags.2;
                player.in_prepare_frame = saved_yield_flags.3;
                player.in_event_dispatch = saved_yield_flags.4;
                player.in_mouse_command = saved_yield_flags.5;
            });

            if let Some(e) = handler_err {
                return Err(e);
            }
            return Ok(DatumRef::Void);
        }
        PlayerVMCommand::MouseUp((x, y)) => {
            crate::player::wait_for_handler_gap().await;
            crate::player::hold_draw_for_input_handler();
            if !player_is_playing().await {
                return Ok(DatumRef::Void);
            }
            // `the mouseUpScript` (if set) is dispatched at the END of the
            // mouseUp pipeline (post-sprite/cast/frame), via the existing
            // player_dispatch_movie_callback call further below. We
            // deliberately do NOT short-circuit sprite dispatch here even
            // when mouseUpScript is set — Director's docs say the script
            // suppresses sprite handlers unless it explicitly calls pass(),
            // but real movies (e.g. ClubMarian's `checkMouse` global
            // observer that early-returns on most frames) rely on sprite
            // mouseUp behaviors firing regardless. Without this, every
            // button in such movies is silent.

            // A lift drag ends on release and swallows the mouseUp, matching the
            // mouseDown that started it (the scrollbar is player chrome, not
            // something the movie's scripts see).
            let was_scroll_drag = reserve_player_mut(|player| {
                if player.field_scroll_drag.take().is_some() {
                    player.movie.mouse_down = false;
                    true
                } else {
                    false
                }
            });
            if was_scroll_drag {
                return Ok(DatumRef::Void);
            }

            // Update mouse state and determine which sprite to notify
            let result = reserve_player_mut(|player| {
                player.mouse_loc = (x, y);
                player.movie.mouse_down = false;
                let sprite_num_to_notify = player.mouse_down_sprite;

                // Reset hilite for push buttons on mouseUp
                if player.mouse_down_sprite > 0 {
                    if let Some(sprite) = player.movie.score.get_sprite(player.mouse_down_sprite) {
                        if let Some(member_ref) = sprite.member.clone() {
                            if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
                                if let CastMemberType::Button(button) = &mut member.member_type {
                                    if button.button_type == crate::player::cast_member::ButtonType::PushButton {
                                        button.hilite = false;
                                    }
                                }
                            }
                        }
                    }
                }

                let sprite = if player.mouse_down_sprite > 0 {
                    player.movie.score.get_sprite(player.mouse_down_sprite)
                } else {
                    None
                };
                player.mouse_down_sprite = -1;
                if let Some(sprite) = sprite {
                    let is_inside = concrete_sprite_hit_test(player, sprite, x, y);
                    Some((sprite.script_instance_list.clone(), is_inside, sprite_num_to_notify))
                } else {
                    None
                }
            });
            let is_inside = result.as_ref().map(|x| x.1).unwrap_or(true);
            let event_name = if is_inside {
                "mouseUp"
            } else {
                "mouseUpOutSide"
            };

            // Temporarily clear ALL is_yield_safe() flags so that updateStage()
            // called from within mouseUp handlers will render (but not yield).
            // Same rationale as mouseDown: multiple flags may be true when
            // the mouseUp command is processed at a frame loop .await point.
            // Set in_mouse_command to prevent re-entrant frame updates and
            // make updateStage synchronous (render-only, no sleep).
            let saved_yield_flags = reserve_player_mut(|player| {
                let saved = (
                    player.is_in_frame_update,
                    player.in_frame_script,
                    player.in_enter_frame,
                    player.in_prepare_frame,
                    player.in_event_dispatch,
                    player.in_mouse_command,
                );
                player.is_in_frame_update = false;
                player.in_frame_script = false;
                player.in_enter_frame = false;
                player.in_prepare_frame = false;
                player.in_event_dispatch = false;
                player.in_mouse_command = true;
                saved
            });

            // Mirror the mouseDown forwarding: if the cursor is currently
            // over a Flash sprite, hand the release into Ruffle so the SWF's
            // own AS1 button handlers (`on (release)`) can fire. Uses the
            // current cursor position — AS1 buttons need press+release on the
            // same Flash member to register a click, so resolving by cursor
            // position handles the common click-and-release-in-place case.
            // (releaseOutside semantics for drag-off-the-button can be added
            // later by tracking the press sprite separately.)
            let flash_forward_up = reserve_player_ref(|player| {
                let any_sprite = get_sprite_at(player, x, y, false)?;
                let sprite = player.movie.score.get_sprite(any_sprite as i16)?;
                let member_ref = sprite.member.as_ref()?;
                let member = player.movie.cast_manager.find_member_by_ref(member_ref)?;
                if !matches!(member.member_type, CastMemberType::Flash(_)) {
                    return None;
                }
                let rect = super::score::get_concrete_sprite_rect(player, sprite);
                Some((any_sprite as i32, x - rect.left, y - rect.top, rect.right - rect.left, rect.bottom - rect.top))
            });
            if let Some((sn, lx, ly, sw, sh)) = flash_forward_up {
                // Same MouseMove-before-MouseUp seeding as the down handler.
                let _ = super::handlers::datum_handlers::flash_object::ruffle_dispatch_mouse_event_global(
                    sn, "move", lx, ly, sw, sh,
                );
                let _ = super::handlers::datum_handlers::flash_object::ruffle_dispatch_mouse_event_global(
                    sn, "up", lx, ly, sw, sh,
                );
            }

            // Dispatch to the sprite that originally received mouseDown,
            // or fall through to frame/movie scripts if no sprite was involved.
            let mut event_stopped = false;
            let dispatched_to_sprite = if let Some((_, _, sprite_num)) = result.as_ref() {
                debug!("[mouseUp] dispatching to sprite#{} (event={})", sprite_num, event_name);
                if *sprite_num > 0 {
                    event_stopped = player_dispatch_event_to_sprite_targeted(
                        Symbol::builtin(if is_inside {
                            BuiltInSymbol::MouseUp
                        } else {
                            BuiltInSymbol::MouseUpOutSide
                        }),
                        &vec![],
                        *sprite_num as u16,
                    ).await;
                    true
                } else {
                    false
                }
            } else {
                debug!("[mouseUp] no sprite to dispatch to (result was None)");
                false
            };

            if !dispatched_to_sprite {
                player_invoke_frame_and_movie_scripts(
                    Symbol::builtin(BuiltInSymbol::MouseUp),
                    &vec![]
                ).await;
            }

            // Execute cast member script using the ORIGINAL sprite that had mouseDown,
            // consistent with Director behavior
            let cast_member_script_call = retained_session_handle()
                .and_then(|handle| {
                    let mut session = handle.borrow_mut();
                    session.with_player(active_player_id() as u32, |context| {
                let player = context.player;
                let symbols = context.symbols;
                let sprite_num_to_notify = result.as_ref().map(|r| r.2).unwrap_or(-1);
                // stopEvent() in a behavior halts the hierarchy here too.
                if event_stopped || sprite_num_to_notify <= 0 {
                    return None;
                }

                let sprite = player.movie.score.get_sprite(sprite_num_to_notify)?;
                let member_ref = sprite.member.as_ref()?;
                let member = player.movie.cast_manager.find_member_by_ref(member_ref)?;

                let handler_name = if is_inside { "mouseUp" } else { "mouseUpOutSide" };

                // First check for member behavior script (stored in member_script_ref)
                if let Some(script_ref) = member.get_member_script_ref() {
                    if let Some(script) = player.movie.cast_manager.get_script_by_ref(script_ref) {
                        if let Some(handler) = script.get_own_handler_ref(Symbol::builtin(BuiltInSymbol::MouseUp)) {
                            return Some((None, handler, vec![]));
                        }
                    }
                }

                // Fallback: check for script_id and get directly from lctx.scripts
                let script_id = member.get_script_id()?;

                let script = {
                    let cast_lib = player.movie.cast_manager.get_cast_mut(member_ref.cast_lib as u32);
                    cast_lib.get_behavior_script_from_lctx(script_id, symbols)
                };

                let script = script?;
                let handler = script.get_own_handler_ref(Symbol::builtin(BuiltInSymbol::MouseUp))?;

                // Try to get the handler
                let handler = script.get_own_handler_ref(Symbol::builtin(BuiltInSymbol::MouseUp));
                
                // ADD THIS CHECK:
                if handler.is_none() {
                    debug!("⚠️  Handler '{}' NOT FOUND in script {}", handler_name, script_id);
                    return None;
                }
                
                debug!("✓ Handler '{}' found!", handler_name);
                
                Some((None, handler.unwrap(), vec![]))
                    })
                })
                .flatten();

            let mut handler_err: Option<ScriptError> = None;
            if let Some((receiver, handler, args)) = cast_member_script_call {
                match player_call_script_handler(receiver, handler, &args).await {
                    ScriptHandlerTurn::Complete(Err(e)) => handler_err = Some(e),
                    ScriptHandlerTurn::Complete(Ok(_))
                    | ScriptHandlerTurn::Waiting
                    | ScriptHandlerTurn::Pending(_) => {}
                }
            }

            if handler_err.is_none() {
                if let Err(e) = super::events::player_dispatch_movie_callback_owned(
                    session_handle.clone(), player_id, owner.clone(), BuiltInSymbol::MouseUp,
                ).await {
                    handler_err = Some(e);
                }
            }

            // Restore all is_yield_safe() flags and command_handler_yielding
            // MUST happen even on error to prevent skip_frame getting stuck
            reserve_player_mut(|player| {
                player.is_in_frame_update = saved_yield_flags.0;
                player.in_frame_script = saved_yield_flags.1;
                player.in_enter_frame = saved_yield_flags.2;
                player.in_prepare_frame = saved_yield_flags.3;
                player.in_event_dispatch = saved_yield_flags.4;
                player.in_mouse_command = saved_yield_flags.5;
                player.is_double_click = false;
            });

            if let Some(e) = handler_err {
                return Err(e);
            }
            return Ok(DatumRef::Void);
        }
        PlayerVMCommand::MouseMove((x, y)) => {
            if !player_is_playing().await {
                return Ok(DatumRef::Void);
            }
            reserve_player_mut(|player| {
                player.mouse_loc = (x, y);

                // Continue a field lift drag before anything else — it owns the
                // pointer until release.
                if player.field_scroll_drag.is_some() {
                    update_field_scrollbar_drag(player, y);
                }

                // Drag moveable sprites (use click_on_sprite, not mouse_down_sprite,
                // since moveable sprites don't need scripts to be draggable)
                if player.movie.mouse_down && player.click_on_sprite > 0 {
                    let drag_sprite_num = player.click_on_sprite;
                    let (off_x, off_y) = player.drag_offset;
                    let sprite = player.movie.score.get_sprite_mut(drag_sprite_num);
                    if sprite.moveable {
                        let mut new_h = x + off_x;
                        let mut new_v = y + off_y;

                        // Constraint bounds only apply to Shockwave3D members.
                        // Regular 2D sprites can be dragged freely; bounds-clamping
                        // them caused puzzle pieces to lock to a small area.
                        let is_3d_member = sprite.member.as_ref()
                            .and_then(|mref| player.movie.cast_manager.find_member_by_ref(mref))
                            .map_or(false, |m| matches!(
                                m.member_type,
                                CastMemberType::Shockwave3d(_)
                            ));
                        if is_3d_member {
                            let sprite = player.movie.score.get_sprite(drag_sprite_num).unwrap();
                            let constraint_num = sprite.constraint;
                            if constraint_num > 0 {
                                // Constrain to the bounding rect of the constraint sprite
                                if let Some(constraint_sprite) = player.movie.score.get_sprite(constraint_num as i16) {
                                    let bounds = get_concrete_sprite_rect(player, constraint_sprite);
                                    new_h = new_h.max(bounds.left).min(bounds.right);
                                    new_v = new_v.max(bounds.top).min(bounds.bottom);
                                }
                            } else {
                                // Constrain to stage
                                let stage = &player.movie.rect;
                                new_h = new_h.max(stage.left).min(stage.right);
                                new_v = new_v.max(stage.top).min(stage.bottom);
                            }
                        }

                        let sprite = player.movie.score.get_sprite_mut(drag_sprite_num);
                        sprite.loc_h = new_h;
                        sprite.loc_v = new_v;
                    }
                }

            });
            // The pointer moved, so re-evaluate which sprites it is inside.
            // (The same pass also runs once per frame — mouseWithin is sent
            // every frame the pointer stays inside, not only when it moves.)
            crate::player::events::dispatch_rollover_events();
        }
        PlayerVMCommand::RightMouseDown((x, y)) => {
            if !player_is_playing().await {
                return Ok(DatumRef::Void);
            }
            // Update mouse_loc + flag is already done in lib.rs
            // (right_mouse_down). Here we dispatch the `rightMouseDown`
            // event to the topmost active sprite (so behaviors with
            // `on rightMouseDown me` fire), then to the global event
            // handler in frame/movie scripts. We don't toggle button
            // hilite or set mouse_down_sprite — those are left-button
            // concerns.
            let scripted_sprite = reserve_player_ref(|player| {
                get_sprite_at(player, x, y, true)
            });
            if let Some(sprite_number) = scripted_sprite {
                player_dispatch_event_to_sprite_targeted(
                    Symbol::builtin(BuiltInSymbol::RightMouseDown),
                    &vec![],
                    sprite_number as u16,
                ).await;
            } else {
                player_invoke_frame_and_movie_scripts(
                    Symbol::builtin(BuiltInSymbol::RightMouseDown),
                    &vec![],
                ).await;
            }
        }
        PlayerVMCommand::RightMouseUp((x, y)) => {
            if !player_is_playing().await {
                return Ok(DatumRef::Void);
            }
            let scripted_sprite = reserve_player_ref(|player| {
                get_sprite_at(player, x, y, true)
            });
            if let Some(sprite_number) = scripted_sprite {
                player_dispatch_event_to_sprite_targeted(
                    Symbol::builtin(BuiltInSymbol::RightMouseUp),
                    &vec![],
                    sprite_number as u16,
                ).await;
            } else {
                player_invoke_frame_and_movie_scripts(
                    Symbol::builtin(BuiltInSymbol::RightMouseUp),
                    &vec![],
                ).await;
            }
        }
        PlayerVMCommand::KeyDown(key, code) => {
            crate::player::wait_for_handler_gap().await;
            crate::player::hold_draw_for_input_handler();
            // Set command_handler_yielding so that:
            // 1. updateStage() always yields (bypasses is_yield_safe check),
            //    letting the browser process keyUp events during repeat-while-
            //    keyPressed loops (like the Hook script's movement).
            // 2. The frame loop skips frame updates and advancement to avoid
            //    running frame scripts that would corrupt the shared scope stack.
            reserve_player_mut(|player| {
                player.command_handler_yielding = true;
            });

            let result = player_key_down(key, code).await;

            reserve_player_mut(|player| {
                player.command_handler_yielding = false;
            });

            return result;
        }
        PlayerVMCommand::KeyUp(key, code) => {
            crate::player::wait_for_handler_gap().await;
            crate::player::hold_draw_for_input_handler();
            return player_key_up(key, code).await;
        }
        PlayerVMCommand::TriggerAlertHook
        | PlayerVMCommand::TriggerFlashCallback { .. }
        | PlayerVMCommand::TriggerLingoCallbackOnScript { .. }
        | PlayerVMCommand::TriggerLingoCallbackOnScriptRaw { .. }
        | PlayerVMCommand::TriggerLocalConnectionCallback { .. }
        | PlayerVMCommand::TriggerLocalConnectionCallbackRaw { .. } => {
            unreachable!("callback commands are dispatched through run_callback_command")
        }
        PlayerVMCommand::MultiuserSocketEvent { owner: event_owner, instance_id, generation, event } => {
            if !event_owner.same_identity(&owner) || !event_owner.is_arena_live() {
                // Late callbacks from a retired socket are expected during
                // reset/close.  They must not surface an error against a
                // replacement player that reuses the same numeric id.
                return Ok(DatumRef::Void);
            }
            let result = session_handle
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !event_owner.same_identity(&context.player.owner) {
                        return Ok(());
                    }
                    context.player.xtra_manager_state.multiuser.handle_socket_event(
                        instance_id,
                        generation,
                        event,
                        &context.player.queue_tx,
                        &event_owner,
                    )
                })
                .unwrap_or_else(|| Ok(()))
                .map(|()| DatumRef::Void);
            return result;
        }
        PlayerVMCommand::TriggerXtraCallback { owner: callback_owner, target, handler_name, args } => {
            if !callback_owner.same_identity(&owner) || !callback_owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            player_dispatch_callback_event(target, handler_name, &args);
            return Ok(DatumRef::Void);
        }
        PlayerVMCommand::SetLingoScriptProperty { cast_lib, cast_member, prop_name, value } => {
            use super::script::script_set_prop;

            if let Some(handle) = retained_session_handle() {
                let player_id = active_player_id() as u32;
                let _ = handle.borrow_mut().with_player(player_id, |context| {
                    let mut matching_instances = Vec::new();
                    for (instance_id, instance_entry) in context.player.allocator.iter_script_instances() {
                        let instance = &instance_entry.script_instance;
                        if instance.script.cast_lib == cast_lib && instance.script.cast_member == cast_member {
                            matching_instances.push(ScriptInstanceRef::from_id(
                                instance_id as u32,
                                instance_entry.ref_count.get(),
                                context.player.owner.clone(),
                            ));
                        }
                    }
                    for instance_ref in matching_instances {
                        let _ = script_set_prop(
                            context.player,
                            context.symbols,
                            &instance_ref,
                            prop_name.clone(),
                            &value,
                            false,
                        );
                    }
                });
            }
        }
        PlayerVMCommand::DispatchFlashEvent { .. }
        | PlayerVMCommand::DispatchFlashEventRaw { .. } => {
            unreachable!("callback commands are dispatched through run_callback_command")
        }
        PlayerVMCommand::PumpPending => {}
    }
    Ok(DatumRef::Void)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod raw_flash_tests {
    use super::{raw_flash_args, Datum};
    use async_std::channel;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    fn decode_first_value(json: &str) -> Datum {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 9901,
            generation: 1,
        });
        assert!(session.add_player(1, channel::unbounded().0));
        session
            .with_player(1, |context| {
                let args = raw_flash_args(json, context.player, context.symbols, 7, 8)
                    .expect("Flash JSON fixture should decode");
                context.player.get_datum(&args[1]).clone()
            })
            .expect("raw Flash test player exists")
    }

    #[test]
    fn duplicate_stored_path_uses_final_string_value() {
        let value = decode_first_value(
            r#"[{"__dirplayer_stored_path":"first","__dirplayer_stored_path":"final"}]"#,
        );
        let Datum::FlashObjectRef(object) = value else {
            panic!("duplicate stored path should decode as FlashObjectRef");
        };
        assert_eq!(object.path, "final");
        assert_eq!((object.cast_lib, object.cast_member), (7, 8));
    }

    #[test]
    fn duplicate_stored_path_final_nonstring_stays_property_list() {
        let value = decode_first_value(
            r#"[{"__dirplayer_stored_path":"stale","__dirplayer_stored_path":7}]"#,
        );
        assert!(
            matches!(value, Datum::PropList(_, _)),
            "a non-string final marker must not retain an older FlashObjectRef"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod multiuser_socket_command_tests {
    use super::{run_player_command_result, PlayerVMCommand};
    use async_std::channel;
    use std::{cell::RefCell, rc::Rc};

    use crate::player::{
        ownership::{OwnerKey, OwnerToken},
        session::RuntimeSession,
        symbols::symbol_table::SymbolOwner,
        xtra::manager::MultiuserConnectRequest,
        xtra::multiuser::MultiuserSocketEvent,
    };

    #[test]
    fn stale_and_foreign_socket_events_are_successful_noops() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 42, generation: 1 });
        let (queue, _queued) = channel::unbounded();
        assert!(session.add_player(1, queue));
        let (owner, instance_id, generation) = session
            .with_player(1, |context| {
                let instance_id = context.player.xtra_manager_state.multiuser.create_instance(&Vec::new());
                let generation = context
                    .player
                    .xtra_manager_state
                    .multiuser
                    .instance_generation(instance_id)
                    .expect("instance generation");
                (context.player.owner.clone(), instance_id, generation)
            })
            .expect("test player exists");
        let foreign = OwnerToken::new(OwnerKey { session: 42, player: 1, generation: 99 });
        let session_handle = Rc::new(RefCell::new(session));
        session_handle.borrow_mut().with_player(1, |context| {
            context.player.is_playing = true;
        });

        let current = async_std::task::block_on(Box::pin(run_player_command_result(
            PlayerVMCommand::MultiuserSocketEvent {
                owner: owner.clone(),
                instance_id,
                generation,
                event: MultiuserSocketEvent::Opened(MultiuserConnectRequest {
                    username: "user".to_owned(),
                    password: "password".to_owned(),
                    host: "127.0.0.1".to_owned(),
                    port: 1,
                    movie_id: "movie".to_owned(),
                    mode: 0,
                    encryption_key: String::new(),
                    websocket_path: None,
                    websocket_ssl: Some(false),
                }),
            },
            session_handle.clone(),
            1,
            owner.clone(),
        )));
        assert!(matches!(current, Ok(crate::player::DatumRef::Void)));

        let current_error = async_std::task::block_on(Box::pin(run_player_command_result(
            PlayerVMCommand::MultiuserSocketEvent {
                owner: owner.clone(),
                instance_id,
                generation,
                event: MultiuserSocketEvent::Error("current error".to_owned()),
            },
            session_handle.clone(),
            1,
            owner.clone(),
        )));
        assert!(matches!(current_error, Ok(crate::player::DatumRef::Void)));

        let state_after_current = session_handle
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.player.is_playing,
                    context
                        .player
                        .xtra_manager_state
                        .multiuser
                        .message_state_for_test(instance_id),
                )
            })
            .expect("test player exists");
        assert!(state_after_current.0);
        assert_eq!(
            state_after_current.1,
            Some(vec![(0, String::new()), (-1, "current error".to_owned())])
        );

        let stale = async_std::task::block_on(Box::pin(run_player_command_result(
            PlayerVMCommand::MultiuserSocketEvent {
                owner: owner.clone(),
                instance_id,
                generation: generation.wrapping_add(1),
                event: MultiuserSocketEvent::Error("stale".to_owned()),
            },
            session_handle.clone(),
            1,
            owner.clone(),
        )));
        assert!(matches!(stale, Ok(crate::player::DatumRef::Void)));

        let foreign_result = async_std::task::block_on(Box::pin(run_player_command_result(
            PlayerVMCommand::MultiuserSocketEvent {
                owner: foreign,
                instance_id,
                generation,
                event: MultiuserSocketEvent::Error("foreign".to_owned()),
            },
            session_handle.clone(),
            1,
            owner,
        )));
        assert!(matches!(foreign_result, Ok(crate::player::DatumRef::Void)));

        let state_after_discarded = session_handle
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.player.is_playing,
                    context
                        .player
                        .xtra_manager_state
                        .multiuser
                        .message_state_for_test(instance_id),
                )
            })
            .expect("test player exists");
        assert_eq!(state_after_discarded, state_after_current);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod eval_cleanup_tests {
    use super::resume_eval_internal;
    use async_std::channel;

    use crate::player::{
        datum_ref::DatumRef,
        driver::InternalVmRequest,
        ownership::OwnerToken,
        session::RuntimeSession,
        symbols::symbol_table::SymbolOwner,
        ScriptError,
    };

    fn pending_value_request(session_id: u64) -> (RuntimeSession, crate::player::eval::EvalId, crate::player::eval::EvalAction, OwnerToken) {
        let mut session = RuntimeSession::new(SymbolOwner { session: session_id, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        let (id, action, _receiver) = session
            .start_eval_request(
                1,
                InternalVmRequest::ObjectProperty {
                    receiver: DatumRef::Void,
                    name: crate::player::symbols::symbol::Symbol::builtin(
                        crate::player::symbols::builtin::BuiltInSymbol::Value,
                    ),
                },
            )
            .expect("value request should be retained");
        let owner = session
            .with_player(1, |context| context.player.owner.clone())
            .expect("test player exists");
        (session, id, action, owner)
    }

    #[test]
    fn stale_after_work_completion_cancels_exact_eval_action() {
        let (session, id, action, owner) = pending_value_request(9_901);
        let handle = std::rc::Rc::new(std::cell::RefCell::new(session));
        owner.begin_reset();

        let turn = resume_eval_internal(
            &handle,
            id.clone(),
            action.clone(),
            owner.clone(),
            Err(ScriptError::new("stale completion".to_owned())),
        );
        assert!(matches!(turn, crate::player::eval::EvalTurn::Complete(Err(_))));
        assert!(!handle.borrow_mut().cancel_eval_action(&id, &action, &owner));
    }

    #[test]
    fn forged_eval_action_is_preserved_for_the_real_completion() {
        let (session, id, real_action, owner) = pending_value_request(9_902);
        let handle = std::rc::Rc::new(std::cell::RefCell::new(session));
        let (_foreign_session, _foreign_id, forged, _foreign_owner) = pending_value_request(9_903);

        let turn = resume_eval_internal(
            &handle,
            id.clone(),
            forged,
            owner.clone(),
            Err(ScriptError::new("forged completion".to_owned())),
        );
        assert!(matches!(turn, crate::player::eval::EvalTurn::Complete(Err(_))));
        assert!(handle.borrow_mut().cancel_eval_action(&id, &real_action, &owner));
    }
}
