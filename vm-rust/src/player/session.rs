//! Explicit runtime ownership boundary.
//!
//! The graph owns players while the session owns the sole symbol table shared
//! by those players. A context borrow is intentionally synchronous: callers
//! must finish the borrow before awaiting host I/O or dispatching a callback.

use std::{collections::{HashMap, HashSet, VecDeque}, rc::Rc};

use async_std::channel::{self, Receiver, Sender};
use log::warn;

use crate::director::file::DirectorFile;

use super::allocator::ScriptInstanceAllocatorTrait;
use super::cast_lib::{
    CastLoadRequest, CastLoadResult, CastMemberRef, CastNotification, CastNotificationOutbox,
    PlayerNotification, PlayerNotificationKind,
};
use super::cast_manager::CastPreloadReason;
use super::driver::{
    checked_internal_datum, ActionCompletion, ActionKind, ActionRegistry, CompletionTicket,
    ChildCompletion, DriverContinuation, DriverStart, DriverTurn, GlobalDispatch, PendingAction,
    PendingCommand, ResumePhase,
};
use super::events::W3dClock;
use super::ownership::{OwnerKey, OwnerToken};
use super::{DirPlayer, PlayerVMExecutionItem, ScriptError, ScriptErrorCode};
use super::{datum_ref::DatumRef, script_ref::ScriptInstanceRef};
use super::handlers::datum_handlers::script_instance::ScriptInstanceUtils;
use super::handlers::manager::BuiltInHandlerManager;
use super::js_lingo_loader::JsRuntimeRegistry;
use super::nested::{NestedChildRecord, NestedPlayerRegistry};
use crate::director::lingo::datum::{Datum, VarRef};
use super::symbols::{builtin::BuiltInSymbol, symbol::Symbol};
use super::host_events::{
    NativeHostEventMailbox, NativeNotificationError, NativePlayerNotificationMailbox,
};
use super::host_events::{BrowserHostSinkRef, BrowserHostSinkWeak, HostEventDelivery};

pub type PlayerId = u32;
use super::symbols::symbol_table::{SymbolOwner, SymbolTable};

/// An evaluator child driver is separate from a player's primary handler
/// driver, so a paused debugger handler remains resident and unchanged.
struct EvalChildContinuation {
    action: crate::player::eval::EvalAction,
    owner: OwnerToken,
    parent_scope: Option<super::ScopeToken>,
    parent_scope_count: u32,
    parent_handler_stack_depth: usize,
    completion: Option<ChildCompletion>,
    broadcast: Option<EvalBroadcastContinuation>,
    ancestor_remaining: Vec<super::handlers::types::AncestorCall>,
    ancestor_final_push_return: bool,
    driver: DriverContinuation,
}

/// Evaluator-side continuation for a broadcast global.  The primary driver
/// has an equivalent policy, but evaluator children are owned by this graph
/// and therefore must retain the same ordered behavior and static fallback
/// descriptors across a suspended child callback.
struct EvalBroadcastContinuation {
    remaining: Vec<super::handlers::types::AncestorCall>,
    fallback: Vec<super::driver::StaticEventCall>,
    initial_return: DatumRef,
    last_return_value: DatumRef,
    handled: bool,
    continue_on_error: bool,
    static_event_guard: Option<super::driver::StaticEventGuard>,
}

pub(crate) enum EvalRequestTurn {
    Evaluator(crate::player::eval::EvalTurn),
    Child(DriverTurn),
    MovieAsync(super::handlers::movie::MovieAsyncRequest),
    /// An owner-bound Flash request executed outside the session borrow. The
    /// command pump retains the mutable request clone so an initial BindGet
    /// can capture its first validated generation before evaluator resume.
    Flash(super::handlers::datum_handlers::flash_object::FlashRequest),
    /// An external Xtra request prepared while the evaluator is suspended.
    /// The command pump executes this owned request after releasing its
    /// `RuntimeSession` borrow, then re-enters only for owner-checked
    /// preparation, finish, or redispatch.
    ExternalXtra(super::xtra::external::ExternalXtraRequest),
    /// An owner-local Xtra load whose continuation remains in the session
    /// until the host reports the exact attempt complete.
    ExternalXtraLoad(super::xtra::external::ExternalXtraLoadRequest),
    /// A built-in Multiuser/Curl operation with an owner-local instance fence.
    XtraPending(super::xtra::manager::XtraPendingIntent),
}

/// An evaluator request retained while owner-bound host work runs outside the
/// session borrow.
pub(crate) struct PendingEvalRequest {
    pub(crate) id: crate::player::eval::EvalId,
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) action: crate::player::eval::EvalAction,
    pub(crate) request: crate::player::eval::EvalPending,
    pub(crate) sender: Sender<Result<DatumRef, ScriptError>>,
}

struct InflightEvalRoute {
    id: crate::player::eval::EvalId,
    player_id: PlayerId,
    owner: OwnerToken,
    action: crate::player::eval::EvalAction,
    sender: Sender<Result<DatumRef, ScriptError>>,
}

/// Cancellation for the one frame loop owned by a player generation.
///
/// The sender lives in the session so stop/reset/remove can wake a loop that
/// is parked in pacing or host I/O. The loop keeps the receiver; the epoch
/// check prevents an old finalizer from clearing a replacement loop.
struct PlaybackLoopControl {
    owner: OwnerToken,
    epoch: u64,
    cancel: Sender<()>,
}

pub(crate) struct DeferredRequest {
    pub(crate) player_id: PlayerId,
    pub(crate) request: super::driver::InternalVmRequest,
    pub(crate) reason: String,
    pub(crate) continuation: Option<crate::player::score::BehaviorDefaultsContinuation>,
}

pub struct PlayerGraph {
    players: HashMap<PlayerId, DirPlayer>,
}

impl PlayerGraph {
    pub fn new() -> Self {
        Self {
            players: HashMap::new(),
        }
    }
    pub(crate) fn insert(&mut self, id: PlayerId, player: DirPlayer) -> bool {
        use std::collections::hash_map::Entry;
        match self.players.entry(id) {
            Entry::Vacant(slot) => {
                slot.insert(player);
                true
            }
            Entry::Occupied(_) => false,
        }
    }
    pub fn get_mut(&mut self, id: PlayerId) -> Option<&mut DirPlayer> {
        self.players.get_mut(&id)
    }
    pub fn remove(&mut self, id: PlayerId) -> Option<DirPlayer> {
        self.players.remove(&id)
    }
    pub fn len(&self) -> usize {
        self.players.len()
    }
}

pub struct RuntimeSession {
    symbols: SymbolTable,
    players: PlayerGraph,
    owner: SymbolOwner,
    generation: u64,
    pending_casts: Vec<PendingCastLoad>,
    completed_property_casts: Vec<(PlayerId, CompletionTicket)>,
    drivers: HashMap<PlayerId, DriverContinuation>,
    eval_drivers: HashMap<crate::player::eval::EvalId, EvalChildContinuation>,
    evals: HashMap<crate::player::eval::EvalId, crate::player::eval::EvalContinuation>,
    next_eval_id: u64,
    next_static_event_guard_id: u64,
    static_event_guards: HashMap<PlayerId, Vec<super::driver::StaticEventGuard>>,
    /// Per-player elapsed clocks for W3D animation and particle simulation.
    /// Clock ownership follows the player generation, so reset/replacement
    /// cannot inherit elapsed time from the previous owner.
    w3d_clocks: HashMap<PlayerId, W3dClock>,
    pending_commands: Vec<PendingCommand>,
    pending_completions: Vec<(PlayerId, CompletionTicket, ActionCompletion)>,
    completed_score_defaults: Vec<(PlayerId, crate::player::score::BehaviorDefaultsContinuation, Result<DatumRef, ScriptError>)>,
    deferred_requests: Vec<DeferredRequest>,
    pending_eval_requests: Vec<PendingEvalRequest>,
    inflight_eval_routes: Vec<InflightEvalRoute>,
    actions: ActionRegistry,
    js_lingo: JsRuntimeRegistry,
    renderer_bindings: HashMap<PlayerId, std::rc::Weak<crate::rendering::RendererState>>,
    /// Host resources retired while this session is mutably borrowed. The
    /// owner boundary drains this queue only after releasing the RefMut.
    pending_host_teardowns: Vec<crate::player::xtra::manager::XtraTeardownRequest>,
    /// Native host events are retained in an explicit bounded sink when the
    /// wasm callback surface is unavailable. They are drained by the native
    /// adapter, never silently discarded by notification pumping.
    native_host_events: NativeHostEventMailbox,
    /// Native notifications are partitioned by player so one owner's
    /// backpressure cannot consume another owner's bounded capacity.
    native_player_notifications: HashMap<PlayerId, NativePlayerNotificationMailbox>,
    native_notification_errors: HashMap<PlayerId, NativeNotificationError>,
    /// Session-owned weak bindings for the direct browser host sink. The
    /// strong capability remains on BrowserPlayerHandle.
    host_sinks: HashMap<PlayerId, (OwnerToken, BrowserHostSinkWeak)>,
    /// Owner-generation fence for notification drains. Reentrant drains for
    /// the same generation return immediately; a reset may replace the
    /// entry with its new owner without clearing an older outer drain.
    notification_drains: HashMap<PlayerId, OwnerToken>,
    /// One detached notification task may be queued before its drain starts.
    /// The owner token prevents an old task from clearing a replacement
    /// generation's marker.
    scheduled_notification_drains: HashMap<PlayerId, OwnerToken>,
    /// Detached lifecycle deliveries. These are drained outside the mutable
    /// session/wasm-bindgen handle borrow so callbacks may re-enter safely.
    host_event_deliveries: HashMap<PlayerId, VecDeque<HostEventDelivery>>,
    host_event_drains: HashSet<PlayerId>,
    host_event_inflight: HashMap<PlayerId, usize>,
    /// Owner-qualified linked `#movie` children. Teardown takes records out
    /// before closing channels so cleanup is performed exactly once.
    nested: NestedPlayerRegistry,
    playback_loops: HashMap<PlayerId, PlaybackLoopControl>,
    playback_epochs: HashMap<PlayerId, u64>,
    playback_cancellations: HashMap<PlayerId, (OwnerToken, u64, bool)>,
    playback_replay_requests: HashMap<PlayerId, (OwnerToken, u64)>,
}

/// Shared owner of a runtime session for host operations that may await.
/// Callers borrow the session only for short preparation/application phases.
pub(crate) type RuntimeSessionHandle = Rc<std::cell::RefCell<RuntimeSession>>;

impl RuntimeSession {
    pub(crate) fn into_handle(self) -> RuntimeSessionHandle {
        Rc::new(std::cell::RefCell::new(self))
    }
}

struct PendingCastLoad {
    request: CastLoadRequest,
    owner: OwnerToken,
    state: PendingCastState,
    purpose: PendingCastPurpose,
}

enum PendingCastPurpose {
    Preload,
    Property { ticket: CompletionTicket },
}

enum PendingCastState {
    Waiting,
    ReadyNetwork(CastLoadResult),
    ReadyCached(Rc<DirectorFile>),
}

pub struct ExecutionContext<'a> {
    pub player_id: PlayerId,
    pub symbols: &'a mut SymbolTable,
    pub player: &'a mut DirPlayer,
}

impl ExecutionContext<'_> {
    /// Run a synchronous VM operation against this session-owned player.
    /// The returned value is bounded by this call; callers must queue callbacks
    /// and host work only after this short borrow has ended.
    #[inline(always)]
    pub(crate) fn with_player<R>(&mut self, f: impl FnOnce(&mut DirPlayer) -> R) -> R {
        f(self.player)
    }

    /// Run a synchronous VM operation with the player mutable and the
    /// session-owned symbol table. The two disjoint reborrows end
    /// with this call; callers must queue host work after the short borrow.
    #[inline(always)]
    pub(crate) fn with_player_and_symbols<R>(
        &mut self,
        f: impl FnOnce(&mut DirPlayer, &mut SymbolTable) -> R,
    ) -> R {
        f(self.player, self.symbols)
    }
}

pub(crate) fn active_stage_script_instance_ids_checked(
    context: &mut ExecutionContext<'_>,
) -> Result<Vec<ScriptInstanceRef>, ScriptError> {
    let frame_num = context.player.movie.current_frame;
    let generation = context.player.behavior_channel_cache_generation;
    let cached_channels = if let Some((cached_frame, cached_generation, channels)) =
        &context.player.active_stage_behavior_channels_cache
    {
        (*cached_frame == frame_num && *cached_generation == generation).then(|| channels.clone())
    } else {
        None
    };
    let mut channel_numbers = cached_channels.clone().unwrap_or_default();

    if cached_channels.is_none() {
        // Snapshot only the fields needed for the inclusion predicate. The
        // fallback list is checked lazily below, matching the old path's
        // nonempty-fallback short circuit while avoiding unchecked cached
        // DatumRefs.
        let channels: Vec<(usize, bool)> = context
            .player
            .movie
            .score
            .channels
            .iter()
            .map(|channel| {
                (
                    channel.number,
                    channel.sprite.entered || channel.sprite.puppet,
                )
            })
            .collect();
        channel_numbers = Vec::new();
        for (channel_number, active) in channels {
            if !active {
                continue;
            }
            let fallback_nonempty = context
                .player
                .movie
                .score
                .channels
                .get(channel_number)
                .is_some_and(|channel| !channel.sprite.script_instance_list.is_empty());
            let cached_nonempty = if fallback_nonempty {
                true
            } else if let Some(cached_ref) = context
                .player
                .script_instance_list_cache
                .get(&(channel_number as i16))
                .cloned()
            {
                matches!(
                    checked_internal_datum(context.player, context.symbols, &cached_ref)?,
                    Datum::List(_, items, _) if !items.is_empty()
                )
            } else {
                false
            };
            if cached_nonempty {
                channel_numbers.push(channel_number);
            }
        }
        context.player.active_stage_behavior_channels_cache =
            Some((frame_num, generation, channel_numbers.clone()));
    }

    let frame_channel_is_active = context
        .player
        .movie
        .score
        .active_channel_numbers_for_frame(frame_num)
        .contains(&0);
    if frame_channel_is_active && !channel_numbers.contains(&0) {
        channel_numbers.push(0);
    }

    let mut receivers = Vec::new();
    for channel_number in channel_numbers {
        let Some(fallback) = context
            .player
            .movie
            .score
            .channels
            .get(channel_number)
            .map(|channel| channel.sprite.script_instance_list.clone())
        else {
            continue;
        };
        let ids = checked_sprite_script_instance_ids(context, channel_number as i16, &fallback)?;
        receivers.extend(ids);
    }
    Ok(receivers)
}

pub(crate) fn checked_sprite_script_instance_ids(
    context: &mut ExecutionContext<'_>,
    sprite_id: i16,
    fallback: &[ScriptInstanceRef],
) -> Result<Vec<ScriptInstanceRef>, ScriptError> {
    let Some(cached_ref) = context
        .player
        .script_instance_list_cache
        .get(&sprite_id)
        .cloned()
    else {
        return Ok(fallback.to_vec());
    };
    let generation = *context
        .player
        .script_instance_list_generation
        .get(&sprite_id)
        .unwrap_or(&0);
    if let Some((cached_generation, ids)) = context
        .player
        .script_instance_list_ids_cache
        .get(&sprite_id)
    {
        if *cached_generation == generation {
            return Ok(ids.clone());
        }
    }
    let datum = checked_internal_datum(context.player, context.symbols, &cached_ref)?;
    let ids = match datum {
        Datum::List(_, item_refs, _) => {
            let item_refs = item_refs.clone();
            let mut ids = Vec::new();
            for item_ref in item_refs {
                let item = checked_internal_datum(context.player, context.symbols, &item_ref)?;
                if let Datum::ScriptInstanceRef(id) = item {
                    ids.push(id.clone());
                }
            }
            ids
        }
        _ => fallback.to_vec(),
    };
    // The cache key is safe after checked_internal_datum validated ownership.
    context
        .player
        .script_instance_list_ids_cache
        .insert(sprite_id, (generation, ids.clone()));
    Ok(ids)
}

impl RuntimeSession {
    pub fn new(owner: SymbolOwner) -> Self {
        Self {
            symbols: SymbolTable::with_owner(owner),
            players: PlayerGraph::new(),
            owner,
            generation: owner.generation,
            pending_casts: Vec::new(),
            completed_property_casts: Vec::new(),
            drivers: HashMap::new(),
            eval_drivers: HashMap::new(),
            evals: HashMap::new(),
            next_eval_id: 1,
            next_static_event_guard_id: 1,
            static_event_guards: HashMap::new(),
            w3d_clocks: HashMap::new(),
            pending_commands: Vec::new(),
            pending_completions: Vec::new(),
            completed_score_defaults: Vec::new(),
            deferred_requests: Vec::new(),
            pending_eval_requests: Vec::new(),
            inflight_eval_routes: Vec::new(),
            actions: ActionRegistry::new(),
            js_lingo: JsRuntimeRegistry::default(),
            renderer_bindings: HashMap::new(),
            pending_host_teardowns: Vec::new(),
            native_host_events: NativeHostEventMailbox::default(),
            native_player_notifications: HashMap::new(),
            native_notification_errors: HashMap::new(),
            host_sinks: HashMap::new(),
            notification_drains: HashMap::new(),
            scheduled_notification_drains: HashMap::new(),
            host_event_deliveries: HashMap::new(),
            host_event_drains: HashSet::new(),
            host_event_inflight: HashMap::new(),
            nested: NestedPlayerRegistry::new(),
            playback_loops: HashMap::new(),
            playback_epochs: HashMap::new(),
            playback_cancellations: HashMap::new(),
            playback_replay_requests: HashMap::new(),
        }
    }

    pub(crate) fn collect_player_host_teardowns(&mut self, player_id: PlayerId) {
        let requests = self
            .players
            .get_mut(player_id)
            .map(|player| player.xtra_manager_state.take_teardown_requests())
            .unwrap_or_default();
        self.pending_host_teardowns.extend(requests);
    }

    pub(crate) fn take_host_teardowns(
        &mut self,
    ) -> Vec<crate::player::xtra::manager::XtraTeardownRequest> {
        std::mem::take(&mut self.pending_host_teardowns)
    }

    #[cfg(test)]
    pub(crate) fn pending_host_teardown_count(&self) -> usize {
        self.pending_host_teardowns.len()
    }
    /// Start an owned AST evaluation. The owner capability is captured from
    /// the live player and is checked again before each turn/completion.
    pub(crate) fn start_eval(
        &mut self,
        player_id: PlayerId,
        expression: crate::player::eval::LingoExpr,
    ) -> Result<crate::player::eval::EvalId, ScriptError> {
        let id = crate::player::eval::EvalId::new(self.next_eval_id);
        self.next_eval_id = self.next_eval_id.wrapping_add(1).max(1);
        let continuation = crate::player::eval::EvalContinuation::new(self, player_id, id.clone(), expression)?;
        self.evals.insert(id.clone(), continuation);
        Ok(id)
    }

    pub(crate) fn start_command_eval(
        &mut self,
        player_id: PlayerId,
        lines: Vec<String>,
    ) -> Result<crate::player::eval::EvalId, ScriptError> {
        let id = crate::player::eval::EvalId::new(self.next_eval_id);
        self.next_eval_id = self.next_eval_id.wrapping_add(1).max(1);
        let continuation = crate::player::eval::EvalContinuation::new_command(self, player_id, id.clone(), lines)?;
        self.evals.insert(id.clone(), continuation);
        Ok(id)
    }

    /// Register an already-prepared owner-bound VM request as a standalone
    /// evaluator continuation. The returned receiver is completed only by
    /// `submit_pending_eval_completion`; callers must execute the detached
    /// request outside the session borrow and preserve its real capability.
    pub(crate) fn start_eval_request(
        &mut self,
        player_id: PlayerId,
        request: super::driver::InternalVmRequest,
    ) -> Result<(
        crate::player::eval::EvalId,
        crate::player::eval::EvalAction,
        async_std::channel::Receiver<Result<DatumRef, ScriptError>>,
    ), ScriptError> {
        let id = crate::player::eval::EvalId::new(self.next_eval_id);
        self.next_eval_id = self.next_eval_id.wrapping_add(1).max(1);
        let (continuation, pending) = crate::player::eval::EvalContinuation::new_request(
            self, player_id, id.clone(), request,
        )?;
        let action = match &pending {
            crate::player::eval::EvalPending::Global { capability, .. }
            | crate::player::eval::EvalPending::Object { capability, .. }
            | crate::player::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
        };
        let owner = self
            .with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(super::cancelled_scope_error)?;
        let (sender, receiver) = async_std::channel::bounded(1);
        self.evals.insert(id.clone(), continuation);
        self.retain_pending_eval_request(id.clone(), player_id, owner, pending, sender);
        Ok((id, action, receiver))
    }

    /// Start a nested script callback without replacing the caller's primary
    /// driver. The callback is an evaluator child anchored to the real active
    /// scope; its completion channel preserves the complete ScopeResult,
    /// including `passed`, for event propagation.
    pub(crate) fn start_owned_script_callback(
        &mut self,
        player_id: PlayerId,
        owner: OwnerToken,
        receiver: Option<ScriptInstanceRef>,
        handler_ref: super::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        use_raw_arg_list: bool,
    ) -> Result<(
        crate::player::eval::EvalId,
        EvalRequestTurn,
        async_std::channel::Receiver<Result<super::scope::ScopeResult, ScriptError>>,
    ), ScriptError> {
        let valid = self
            .with_player(player_id, |context| {
                owner.same_identity(&context.player.owner) && owner.is_arena_live()
            })
            .unwrap_or(false);
        if !valid {
            return Err(super::cancelled_scope_error());
        }
        let id = crate::player::eval::EvalId::new(self.next_eval_id);
        self.next_eval_id = self.next_eval_id.wrapping_add(1).max(1);
        let request = super::driver::InternalVmRequest::Object {
            // The child receiver is retained separately below; the request
            // envelope only supplies the evaluator's owner capability.  A
            // ScriptInstanceRef is not a DatumRef and must never be coerced
            // or allocated merely to describe this prepared child.
            receiver: DatumRef::Void,
            name: handler_ref.1.clone(),
            args: args.clone(),
        };
        let (mut continuation, pending) = crate::player::eval::EvalContinuation::new_request(
            self, player_id, id.clone(), request,
        )?;
        let (sender, receiver_channel) = async_std::channel::bounded(1);
        continuation.set_callback_sender(sender);
        let action = match &pending {
            crate::player::eval::EvalPending::Object { capability, .. } => capability.clone(),
            _ => unreachable!("object callback must create an object evaluator request"),
        };
        self.evals.insert(id.clone(), continuation);
        let turn = self.start_eval_child_prepared(
            id.clone(), action, receiver, handler_ref, args, use_raw_arg_list,
            None, None, Vec::new(), false,
        );
        Ok((id, turn, receiver_channel))
    }

    /// Advance one evaluator and consume its expression exactly once.
    pub(crate) fn turn_eval(
        &mut self,
        id: crate::player::eval::EvalId,
    ) -> crate::player::eval::EvalTurn {
        let Some(mut continuation) = self.evals.remove(&id) else {
            return crate::player::eval::EvalTurn::Complete(Err(ScriptError::new(
                "unknown or duplicate evaluator completion".to_owned(),
            )));
        };
        let player_id = continuation.player_id;
        let result = continuation.turn(self);
        self.collect_player_host_teardowns(player_id);
        if matches!(result, crate::player::eval::EvalTurn::Pending { .. })
            || matches!(continuation.state, crate::player::eval::EvalState::Waiting)
        {
            self.evals.insert(id, continuation);
        }
        result
    }

    fn eval_parent_valid(&mut self, child: &EvalChildContinuation) -> bool {
        self.with_player(child.driver.player_id, |context| {
            child.owner.same_identity(&context.player.owner)
                && child.owner.is_arena_live()
                && child.parent_scope_count == context.player.scope_count
                && child.parent_handler_stack_depth == context.player.handler_stack_depth
                && child
                    .parent_scope
                    .as_ref()
                    .map_or(true, |scope| scope.validate_active(context.player))
        })
        .unwrap_or(false)
    }

    fn release_eval_broadcast_guard(&mut self, child: &mut EvalChildContinuation) {
        let player_id = child.driver.player_id;
        if let Some(broadcast) = child.broadcast.as_mut() {
            if let Some(guard) = broadcast.static_event_guard.take() {
                let _ = self.release_static_event_guard(player_id, &guard);
            }
        }
    }

    /// Deliver a nested script callback's complete scope outcome exactly once.
    /// This is deliberately separate from the datum-only evaluator route:
    /// event propagation depends on `ScopeResult::passed` as well as the
    /// returned datum.
    fn send_owned_callback_result(
        &mut self,
        id: &crate::player::eval::EvalId,
        result: Result<super::scope::ScopeResult, ScriptError>,
    ) {
        if let Some(sender) = self
            .evals
            .get_mut(id)
            .and_then(crate::player::eval::EvalContinuation::take_callback_sender)
        {
            let _ = sender.try_send(result);
        }
    }

    fn finish_eval_child(
        &mut self,
        id: crate::player::eval::EvalId,
        mut child: EvalChildContinuation,
        turn: DriverTurn,
    ) -> EvalRequestTurn {
        let player_id = child.driver.player_id;
        self.collect_player_host_teardowns(player_id);
        match turn {
            DriverTurn::Waiting | DriverTurn::Pending(_) => {
                self.eval_drivers.insert(id, child);
                EvalRequestTurn::Child(turn)
            }
            DriverTurn::Complete(result) => {
                if !self.eval_parent_valid(&child) {
                    self.release_eval_broadcast_guard(&mut child);
                    self.send_owned_callback_result(&id, Err(super::cancelled_scope_error()));
                    self.evals.remove(&id);
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(super::cancelled_scope_error()),
                    ));
                }
                let player_id = child.driver.player_id;
                let action = child.action.clone();
                let owner = child.owner.clone();
                if let Some(broadcast) = child.broadcast.take() {
                    return self.continue_eval_broadcast(
                        id,
                        action,
                        owner,
                        broadcast,
                        result.return_value,
                        result.passed,
                    );
                }
                let mut remaining_calls = child.ancestor_remaining.into_iter();
                while let Some(call) = remaining_calls.next() {
                    match self.resolve_ancestor_call(player_id, &call) {
                        Ok(Some((receiver, handler_ref, args))) => {
                            return self.start_eval_child_prepared(
                                id,
                                action,
                                Some(receiver),
                                handler_ref,
                                args,
                                true,
                                None,
                                None,
                                remaining_calls.collect(),
                                child.ancestor_final_push_return,
                            );
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return EvalRequestTurn::Evaluator(self.resume_eval(
                                id,
                                &action,
                                &owner,
                                Err(error),
                            ));
                        }
                    }
                }
                let result_passed = result.passed;
                let result = match child.completion {
                    Some(ChildCompletion::ConstructScript { fallback }) => self
                        .with_player(child.driver.player_id, |context| {
                            super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                                context.player,
                                context.symbols,
                                fallback,
                                result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    Some(ChildCompletion::TimeoutNew { timeout_name, fallback }) => self
                        .with_player(child.driver.player_id, |context| {
                            super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                                context.player,
                                context.symbols,
                                timeout_name,
                                fallback,
                                result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    Some(ChildCompletion::TimeoutForget { timeout_name }) => self
                        .with_player(child.driver.player_id, |context| {
                            super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                                context.player,
                                context.symbols,
                                timeout_name,
                                result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    None => Ok(result.return_value),
                };
                let callback_result = match &result {
                    Ok(return_value) => Ok(super::scope::ScopeResult {
                        return_value: return_value.clone(),
                        passed: result_passed,
                    }),
                    Err(error) => Err(error.clone()),
                };
                self.send_owned_callback_result(&id, callback_result);
                EvalRequestTurn::Evaluator(self.resume_eval(
                    id,
                    &child.action,
                    &child.owner,
                    result,
                ))
            }
            DriverTurn::Error(error) => {
                child.driver.cancel(self);
                if !self.eval_parent_valid(&child) {
                    self.release_eval_broadcast_guard(&mut child);
                    self.send_owned_callback_result(&id, Err(super::cancelled_scope_error()));
                    self.evals.remove(&id);
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(super::cancelled_scope_error()),
                    ));
                }
                if let Some(broadcast) = child.broadcast.take() {
                    if error.code != ScriptErrorCode::Abort && broadcast.continue_on_error {
                        warn!("broadcast evaluator child failed; continuing: {}", error.message);
                        return self.continue_eval_broadcast(
                            id,
                            child.action,
                            child.owner,
                            broadcast,
                            DatumRef::Void,
                            true,
                        );
                    }
                }
                self.release_eval_broadcast_guard(&mut child);
                self.send_owned_callback_result(&id, Err(error.clone()));
                EvalRequestTurn::Evaluator(self.resume_eval(
                    id,
                    &child.action,
                    &child.owner,
                    Err(error),
                ))
            }
        }
    }

    /// Execute the synchronous portion of one evaluator request. Object
    /// handlers use the canonical explicit dispatcher; unsupported handlers
    /// remain an owned pending request for the external host loop. Prepared
    /// global children are started exactly once and never redispatched.
    pub(crate) fn execute_eval_global_dispatch(
        &mut self,
        id: crate::player::eval::EvalId,
        capability: crate::player::eval::EvalAction,
        name: super::symbols::symbol::Symbol,
        args: Vec<super::datum_ref::DatumRef>,
        dispatch: GlobalDispatch,
    ) -> EvalRequestTurn {
        let Some((_, owner)) = self.eval_action_anchor(&id, &capability) else {
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(super::cancelled_scope_error()),
            ));
        };
        match dispatch {
            GlobalDispatch::SyncResult(result) => {
                EvalRequestTurn::Evaluator(self.resume_eval(id, &capability, &owner, result))
            }
            GlobalDispatch::Child { receiver, handler_ref } => self.execute_eval_request(
                id,
                crate::player::eval::EvalPending::Global {
                    capability,
                    request: super::driver::InternalVmRequest::Global {
                        name,
                        args: args.clone(),
                    },
                    reason: None,
                    prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                        receiver,
                        handler_ref,
                        args,
                        use_raw_arg_list: true,
                        completion: None,
                    }),
                },
            ),
            GlobalDispatch::ChildPrepared { receiver, handler_ref, args: prepared_args } => self.execute_eval_request(
                id,
                crate::player::eval::EvalPending::Global {
                    capability,
                    request: super::driver::InternalVmRequest::Global {
                        name,
                        args,
                    },
                    reason: None,
                    prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                        receiver,
                        handler_ref,
                        args: prepared_args,
                        use_raw_arg_list: false,
                        completion: None,
                    }),
                },
            ),
            GlobalDispatch::ChildWithCompletion { receiver, handler_ref, args: prepared_args, completion } => self.execute_eval_request(
                id,
                crate::player::eval::EvalPending::Global {
                    capability,
                    request: super::driver::InternalVmRequest::Global {
                        name,
                        args,
                    },
                    reason: None,
                    prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                        receiver,
                        handler_ref,
                        args: prepared_args,
                        use_raw_arg_list: false,
                        completion: Some(completion),
                    }),
                },
            ),
            GlobalDispatch::AncestorChildren { calls } => self.execute_eval_request(
                id,
                crate::player::eval::EvalPending::Global {
                    capability,
                    request: super::driver::InternalVmRequest::Global { name, args },
                    reason: None,
                    prepared_child: Some(crate::player::eval::PreparedGlobal::Ancestors { calls }),
                },
            ),
            GlobalDispatch::ChildSequence { plan } => self.execute_eval_request(
                id,
                crate::player::eval::EvalPending::Global {
                    capability,
                    request: super::driver::InternalVmRequest::Global { name, args },
                    reason: None,
                    prepared_child: Some(crate::player::eval::PreparedGlobal::Broadcast { plan }),
                },
            ),
            GlobalDispatch::PendingRequest { request, reason } => {
                if matches!(
                    &request,
                    crate::player::driver::InternalVmRequest::MovieAsync(_)
                        | crate::player::driver::InternalVmRequest::Flash(_)
                        | crate::player::driver::InternalVmRequest::ExternalXtra(_)
                        | crate::player::driver::InternalVmRequest::ExternalXtraLoad(_)
                        | crate::player::driver::InternalVmRequest::XtraPending(_)
                ) {
                    self.execute_eval_request(
                        id,
                        crate::player::eval::EvalPending::Global {
                            capability,
                            request,
                            reason: Some(reason),
                            prepared_child: None,
                        },
                    )
                } else {
                    EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Pending {
                        request: crate::player::eval::EvalPending::Global {
                            capability,
                            request,
                            reason: Some(reason),
                            prepared_child: None,
                        },
                    })
                }
            }
            GlobalDispatch::Pending { reason } => EvalRequestTurn::Evaluator(
                crate::player::eval::EvalTurn::Pending {
                    request: crate::player::eval::EvalPending::Global {
                        capability,
                        request: super::driver::InternalVmRequest::Global { name, args },
                        reason: Some(reason),
                        prepared_child: None,
                    },
                },
            ),
        }
    }

    pub(crate) fn execute_eval_request(
        &mut self,
        id: crate::player::eval::EvalId,
        request: crate::player::eval::EvalPending,
    ) -> EvalRequestTurn {
        match request {
            crate::player::eval::EvalPending::Global {
                request: crate::player::driver::InternalVmRequest::ExternalXtra(request),
                prepared_child: None,
                ..
            } => EvalRequestTurn::ExternalXtra(request),
            crate::player::eval::EvalPending::Global {
                request: crate::player::driver::InternalVmRequest::ExternalXtraLoad(request),
                prepared_child: None,
                ..
            } => EvalRequestTurn::ExternalXtraLoad(request),
            crate::player::eval::EvalPending::Global {
                request: crate::player::driver::InternalVmRequest::XtraPending(request),
                prepared_child: None,
                ..
            } => EvalRequestTurn::XtraPending(request),
            crate::player::eval::EvalPending::Global {
                capability,
                request,
                prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                    receiver,
                    handler_ref,
                    args,
                    use_raw_arg_list,
                    completion,
                }),
                ..
            } => self.start_eval_child_prepared(
                id,
                capability,
                receiver,
                handler_ref,
                args,
                use_raw_arg_list,
                completion,
                None,
                Vec::new(),
                false,
            ),
            crate::player::eval::EvalPending::Global {
                capability,
                prepared_child: Some(crate::player::eval::PreparedGlobal::Broadcast { plan }),
                ..
            } => self.start_eval_broadcast(id, capability, plan),
            crate::player::eval::EvalPending::Global {
                capability,
                prepared_child: Some(crate::player::eval::PreparedGlobal::Ancestors { calls }),
                ..
            } => {
                let Some((player_id, owner)) = self.eval_action_anchor(&id, &capability) else {
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(super::cancelled_scope_error()),
                    ));
                };
                let mut remaining_calls = calls.into_iter();
                let mut selected = None;
                while let Some(call) = remaining_calls.next() {
                    match self.resolve_ancestor_call(player_id, &call) {
                        Ok(Some((receiver, handler_ref, args))) => {
                            selected = Some((receiver, handler_ref, args));
                            break;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return EvalRequestTurn::Evaluator(self.resume_eval(
                                id,
                                &capability,
                                &owner,
                                Err(error),
                            ));
                        }
                    }
                }
                let Some((receiver, handler_ref, args)) = selected else {
                    return EvalRequestTurn::Evaluator(self.resume_eval(
                        id,
                        &capability,
                        &owner,
                        Ok(super::datum_ref::DatumRef::Void),
                    ));
                };
                self.start_eval_child_prepared(
                    id,
                    capability,
                    Some(receiver),
                    handler_ref,
                    args,
                    true,
                    None,
                    None,
                    remaining_calls.collect(),
                    false,
                )
            }
            crate::player::eval::EvalPending::Global {
                request: crate::player::driver::InternalVmRequest::MovieAsync(request),
                prepared_child: None,
                ..
            } => EvalRequestTurn::MovieAsync(request),
            crate::player::eval::EvalPending::Global {
                request: crate::player::driver::InternalVmRequest::Flash(request),
                prepared_child: None,
                ..
            } => EvalRequestTurn::Flash(request),
            pending @ crate::player::eval::EvalPending::Global { .. }
            | pending @ crate::player::eval::EvalPending::SetProperty { .. } => {
                EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Pending { request: pending })
            }
            crate::player::eval::EvalPending::Object {
                capability,
                request,
                reason: existing_reason,
            } => {
                let request = match request {
                    crate::player::driver::InternalVmRequest::ExternalXtra(request) => {
                        return EvalRequestTurn::ExternalXtra(request);
                    }
                    crate::player::driver::InternalVmRequest::ExternalXtraLoad(request) => {
                        return EvalRequestTurn::ExternalXtraLoad(request);
                    }
                    crate::player::driver::InternalVmRequest::XtraPending(request) => {
                        return EvalRequestTurn::XtraPending(request);
                    }
                    request => request,
                };
                // A classified request with an attached script can execute
                // that script through the same owner-bound child driver as a
                // global child. This is the actual Flash/sprite callback
                // continuation; only a sprite with no script falls through
                // to the host readiness request. A cast-member request stays
                // pending for its import/physics executor and is never sent
                // back through the synchronous dispatcher.
                if let crate::player::driver::InternalVmRequest::SpriteAsync(sprite_request) = &request {
                    let Some((player_id, owner)) = self.eval_action_anchor(&id, &capability) else {
                        return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                            Err(ScriptError::new(
                                "stale, foreign, or duplicate evaluator action".to_owned(),
                            )),
                        ));
                    };
                    if sprite_request.player_id != player_id
                        || !sprite_request.owner.same_identity(&owner)
                    {
                        return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                            Err(super::cancelled_scope_error()),
                        ));
                    }
                    let child = self.with_player(player_id, |context| {
                        let sprite = match checked_internal_datum(
                            context.player,
                            context.symbols,
                            &sprite_request.receiver,
                        )? {
                            Datum::SpriteRef(number) => context.player.movie.score.get_sprite(*number),
                            _ => None,
                        };
                        let Some(sprite) = sprite else { return Ok(None) };
                        for instance_ref in sprite.script_instance_list.iter().cloned() {
                            if let Some(handler_ref) = ScriptInstanceUtils::get_script_instance_handler(
                                sprite_request.handler.clone(),
                                &instance_ref,
                                context.player,
                            )? {
                                return Ok(Some((
                                    Some(instance_ref),
                                    handler_ref,
                                )));
                            }
                        }
                        Ok(None)
                    });
                    let child = match child {
                        Some(Ok(child)) => child,
                        Some(Err(error)) => {
                            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                                Err(error),
                            ));
                        }
                        None => {
                            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                                Err(super::cancelled_scope_error()),
                            ));
                        }
                    };
                    if let Some((receiver, handler_ref)) = child {
                        return self.start_eval_child(
                            id,
                            capability,
                            receiver,
                            handler_ref,
                            request,
                            None,
                            Vec::new(),
                            false,
                        );
                    }
                }
                let request = match request {
                    crate::player::driver::InternalVmRequest::Flash(request) => {
                        return EvalRequestTurn::Flash(request);
                    }
                    request => request,
                };
                if existing_reason.is_some()
                    || matches!(
                        request,
                        crate::player::driver::InternalVmRequest::CastMemberAsync(_)
                            | crate::player::driver::InternalVmRequest::Flash(_)
                            | crate::player::driver::InternalVmRequest::ObjectProperty { .. }
                    )
                {
                    let pending_reason = existing_reason
                        .or_else(|| super::driver::async_request_reason(&request));
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Pending {
                        request: crate::player::eval::EvalPending::Object {
                            capability,
                            request,
                            reason: pending_reason,
                        },
                    });
                }
                let crate::player::driver::InternalVmRequest::Object {
                    receiver,
                    name,
                    args,
                } = request else {
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(ScriptError::new("invalid evaluator object request".to_owned())),
                    ));
                };
                let Some((player_id, owner)) = self.eval_action_anchor(&id, &capability) else {
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(ScriptError::new(
                            "stale, foreign, or duplicate evaluator action".to_owned(),
                        )),
                    ));
                };
                let dispatch = self.with_player(player_id, |mut context| {
                    super::handlers::datum_handlers::player_call_datum_handler(
                        &mut context,
                        &receiver,
                        name.clone(),
                        &args,
                    )
                });
                let Some(dispatch) = dispatch else {
                    return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                        Err(super::cancelled_scope_error()),
                    ));
                };
                match dispatch {
                    super::handlers::datum_handlers::DatumDispatch::Child {
                        receiver: child_receiver,
                        handler_ref,
                        args,
                        ..
                    } => self.start_eval_child_prepared(
                        id,
                        capability,
                        child_receiver,
                        handler_ref,
                        args,
                        false,
                        None,
                        None,
                        Vec::new(),
                        false,
                    ),
                    super::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
                        receiver: child_receiver,
                        handler_ref,
                        args,
                        completion,
                    } => self.start_eval_child_prepared(
                        id,
                        capability,
                        child_receiver,
                        handler_ref,
                        args,
                        false,
                        Some(completion),
                        None,
                        Vec::new(),
                        false,
                    ),
                    super::handlers::datum_handlers::DatumDispatch::Sync(result) => {
                        let result = match result {
                            Ok(result) => {
                                let valid = self.with_player(player_id, |context| {
                                    checked_internal_datum(context.player, context.symbols, &result)
                                        .map(|_| ())
                                })
                                .and_then(Result::ok)
                                .is_some();
                                if !valid {
                                    Err(ScriptError::new_code(
                                        ScriptErrorCode::InvalidReference,
                                        "object handler returned a foreign datum".to_owned(),
                                    ))
                                } else {
                                    Ok(result)
                                }
                            }
                            Err(error) => Err(error),
                        };
                        EvalRequestTurn::Evaluator(self.resume_eval(id, &capability, &owner, result))
                    }
                    super::handlers::datum_handlers::DatumDispatch::Pending { request, reason } => {
                        EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Pending {
                            request: crate::player::eval::EvalPending::Object {
                                capability,
                                request,
                                reason: existing_reason.or(Some(reason)),
                            },
                        })
                    }
                }
            }
        }
    }

    pub(crate) fn eval_action_anchor(
        &mut self,
        id: &crate::player::eval::EvalId,
        action: &crate::player::eval::EvalAction,
    ) -> Option<(PlayerId, OwnerToken)> {
        let continuation = self.evals.get(id)?;
        if !continuation.waiting_for(action) {
            return None;
        }
        let player_id = continuation.player_id;
        let owner = continuation.owner.clone();
        let scope = continuation.scope.clone();
        let valid = self
            .with_player(player_id, |context| {
                owner.same_identity(&context.player.owner)
                    && owner.is_arena_live()
                    && scope
                        .as_ref()
                        .map_or(true, |scope| scope.validate_active(context.player))
            })
            .unwrap_or(false);
        valid.then_some((player_id, owner))
    }

    fn resolve_ancestor_call(
        &mut self,
        player_id: PlayerId,
        call: &super::handlers::types::AncestorCall,
    ) -> Result<Option<(
        super::script_ref::ScriptInstanceRef,
        super::script::ScriptHandlerRef,
        Vec<super::datum_ref::DatumRef>,
    )>, ScriptError> {
        self.with_player(player_id, |mut context| {
            super::handlers::types::resolve_ancestor_call(&mut context, call)
        })
        .ok_or_else(super::cancelled_scope_error)?
    }

    fn resolve_static_event_call(
        &mut self,
        player_id: PlayerId,
        call: &super::driver::StaticEventCall,
    ) -> Result<Option<(
        Option<super::script_ref::ScriptInstanceRef>,
        super::script::ScriptHandlerRef,
        Vec<super::datum_ref::DatumRef>,
        super::driver::StaticEventGuard,
    )>, ScriptError> {
        let resolved = self.with_player(player_id, |mut context| {
            super::compare::validate_direct_symbol_fields(
                &Datum::Symbol(call.handler_name.clone()),
                context.symbols,
            )?;
            if let Some(receiver) = &call.receiver {
                context
                    .player
                    .allocator
                    .get_script_instance_opt(receiver)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "stale static event receiver".to_owned(),
                        )
                    })?;
            }
            let Some(script) = context
                .player
                .movie
                .cast_manager
                .get_script_by_ref(&call.member_ref)
            else {
                return Err(ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("static event references missing script {:?}", call.member_ref),
                ));
            };
            let Some(handler_ref) = script.get_own_handler_ref(call.handler_name.clone()) else {
                return Ok(None);
            };
            Ok(Some((
                call.receiver.clone(),
                handler_ref,
                call.args.clone(),
            )))
        })
        .ok_or_else(super::cancelled_scope_error)??;
        let Some((receiver, handler_ref, args)) = resolved else {
            return Ok(None);
        };
        let Some(guard) = self.enter_static_event_guard(player_id, call)? else {
            return Ok(None);
        };
        Ok(Some((receiver, handler_ref, args, guard)))
    }

    fn continue_eval_broadcast(
        &mut self,
        id: crate::player::eval::EvalId,
        action: crate::player::eval::EvalAction,
        owner: OwnerToken,
        mut broadcast: EvalBroadcastContinuation,
        return_value: DatumRef,
        passed: bool,
    ) -> EvalRequestTurn {
        if return_value != DatumRef::Void {
            broadcast.last_return_value = return_value;
        }
        let player_id = match self.evals.get(&id) {
            Some(continuation) if continuation.waiting_for(&action) => continuation.player_id,
            _ => {
                return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                    Err(super::cancelled_scope_error()),
                ));
            }
        };
        let static_event_was_active = broadcast.static_event_guard.is_some();
        if let Some(guard) = broadcast.static_event_guard.take() {
            let _ = self.release_static_event_guard(player_id, &guard);
        }
        broadcast.handled |=
            !passed || (static_event_was_active && self.static_event_stopped(player_id));
        for index in 0..broadcast.remaining.len() {
            let call = broadcast.remaining[index].clone();
            match self.resolve_ancestor_call(player_id, &call) {
                Ok(Some((receiver, handler_ref, args))) => {
                    broadcast.remaining = broadcast.remaining[index + 1..].to_vec();
                    return self.start_eval_child_prepared(
                        id,
                        action,
                        Some(receiver),
                        handler_ref,
                        args,
                        false,
                        None,
                        Some(broadcast),
                        Vec::new(),
                        false,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    return EvalRequestTurn::Evaluator(self.resume_eval(
                        id,
                        &action,
                        &owner,
                        Err(error),
                    ));
                }
            }
        }
        if !broadcast.handled {
            for index in 0..broadcast.fallback.len() {
                let static_call = broadcast.fallback[index].clone();
                match self.resolve_static_event_call(player_id, &static_call) {
                    Ok(Some((receiver, handler_ref, args, guard))) => {
                        broadcast.fallback = broadcast.fallback[index + 1..].to_vec();
                        broadcast.static_event_guard = Some(guard);
                        return self.start_eval_child_prepared(
                            id,
                            action,
                            receiver,
                            handler_ref,
                            args,
                            true,
                            None,
                            Some(broadcast),
                            Vec::new(),
                            false,
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return EvalRequestTurn::Evaluator(self.resume_eval(
                            id,
                            &action,
                            &owner,
                            Err(error),
                        ));
                    }
                }
            }
        }
        let final_return = if broadcast.last_return_value == DatumRef::Void {
            broadcast.initial_return
        } else {
            broadcast.last_return_value
        };
        EvalRequestTurn::Evaluator(self.resume_eval(
            id,
            &action,
            &owner,
            Ok(final_return),
        ))
    }

    fn start_eval_broadcast(
        &mut self,
        id: crate::player::eval::EvalId,
        action: crate::player::eval::EvalAction,
        plan: super::driver::BroadcastPlan,
    ) -> EvalRequestTurn {
        let Some((player_id, owner)) = self.eval_action_anchor(&id, &action) else {
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(super::cancelled_scope_error()),
            ));
        };
        let mut broadcast = EvalBroadcastContinuation {
            remaining: plan.calls,
            fallback: plan.fallback,
            initial_return: plan.initial_return.clone(),
            last_return_value: plan.initial_return,
            handled: plan.handled,
            continue_on_error: plan.continue_on_error,
            static_event_guard: None,
        };
        for index in 0..broadcast.remaining.len() {
            let call = broadcast.remaining[index].clone();
            match self.resolve_ancestor_call(player_id, &call) {
                Ok(Some((receiver, handler_ref, args))) => {
                    broadcast.remaining = broadcast.remaining[index + 1..].to_vec();
                    return self.start_eval_child_prepared(
                        id,
                        action,
                        Some(receiver),
                        handler_ref,
                        args,
                        false,
                        None,
                        Some(broadcast),
                        Vec::new(),
                        false,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    return EvalRequestTurn::Evaluator(self.resume_eval(
                        id,
                        &action,
                        &owner,
                        Err(error),
                    ));
                }
            }
        }
        self.continue_eval_broadcast(
            id,
            action,
            owner,
            broadcast,
            DatumRef::Void,
            true,
        )
    }

    fn start_eval_child(
        &mut self,
        id: crate::player::eval::EvalId,
        action: crate::player::eval::EvalAction,
        receiver: Option<ScriptInstanceRef>,
        handler_ref: super::script::ScriptHandlerRef,
        request: super::driver::InternalVmRequest,
        completion: Option<ChildCompletion>,
        ancestor_remaining: Vec<super::handlers::types::AncestorCall>,
        ancestor_final_push_return: bool,
    ) -> EvalRequestTurn {
        let (args, use_raw_arg_list, request_owner) = match request {
            super::driver::InternalVmRequest::Global { args, .. }
            | super::driver::InternalVmRequest::GlobalAfterExternalProbe { args, .. } => (args, true, None),
            super::driver::InternalVmRequest::Object { args, .. } => (args, false, None),
            super::driver::InternalVmRequest::SpriteAsync(request) => {
                (request.args, false, Some((request.player_id, request.owner)))
            }
            _ => {
                return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                    Err(ScriptError::new("invalid prepared evaluator child request".to_owned())),
                ));
            }
        };
        if let Some((request_player, request_owner)) = request_owner {
            let valid = self.eval_action_anchor(&id, &action).is_some_and(|(player_id, owner)| {
                request_player == player_id && request_owner.same_identity(&owner)
                    && request_owner.is_arena_live()
            });
            if !valid {
                return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                    Err(super::cancelled_scope_error()),
                ));
            }
        }
        self.start_eval_child_prepared(
            id,
            action,
            receiver,
            handler_ref,
            args,
            use_raw_arg_list,
            completion,
            None,
            ancestor_remaining,
            ancestor_final_push_return,
        )
    }

    fn start_eval_child_prepared(
        &mut self,
        id: crate::player::eval::EvalId,
        action: crate::player::eval::EvalAction,
        receiver: Option<ScriptInstanceRef>,
        handler_ref: super::script::ScriptHandlerRef,
        args: Vec<super::datum_ref::DatumRef>,
        use_raw_arg_list: bool,
        completion: Option<ChildCompletion>,
        broadcast: Option<EvalBroadcastContinuation>,
        ancestor_remaining: Vec<super::handlers::types::AncestorCall>,
        ancestor_final_push_return: bool,
    ) -> EvalRequestTurn {
        if self.eval_drivers.contains_key(&id) {
            self.send_owned_callback_result(
                &id,
                Err(ScriptError::new("evaluator child action is already running".to_owned())),
            );
            self.evals.remove(&id);
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(ScriptError::new("evaluator child action is already running".to_owned())),
            ));
        }
        let Some(continuation) = self.evals.get(&id) else {
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(super::cancelled_scope_error()),
            ));
        };
        if !continuation.waiting_for(&action) {
            self.send_owned_callback_result(
                &id,
                Err(ScriptError::new("evaluator child action is no longer pending".to_owned())),
            );
            self.evals.remove(&id);
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(ScriptError::new("evaluator child action is no longer pending".to_owned())),
            ));
        }
        let (owner, parent_scope) = continuation.child_anchor();
        let player_id = continuation.player_id;
        let Some((parent_scope_count, parent_handler_stack_depth)) = self.with_player(
            player_id,
            |context| {
                if !owner.same_identity(&context.player.owner)
                    || !owner.is_arena_live()
                    || parent_scope
                        .as_ref()
                        .is_some_and(|scope| !scope.validate_active(context.player))
                {
                    return None;
                }
                Some((context.player.scope_count, context.player.handler_stack_depth))
            },
        ).flatten() else {
            self.send_owned_callback_result(&id, Err(super::cancelled_scope_error()));
            self.evals.remove(&id);
            return EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(
                Err(super::cancelled_scope_error()),
            ));
        };
        let driver = match DriverContinuation::start_subordinate(
            self,
            player_id,
            receiver,
            handler_ref,
            &args,
            use_raw_arg_list,
            parent_scope.as_ref(),
        ) {
            Ok(DriverStart::Early(result)) => {
                if let Some(broadcast) = broadcast {
                    return self.continue_eval_broadcast(
                        id,
                        action,
                        owner,
                        broadcast,
                        result.return_value,
                        result.passed,
                    );
                }
                let mut remaining_calls = ancestor_remaining.into_iter();
                while let Some(call) = remaining_calls.next() {
                    match self.resolve_ancestor_call(player_id, &call) {
                        Ok(Some((receiver, handler_ref, args))) => {
                            return self.start_eval_child_prepared(
                                id,
                                action,
                                Some(receiver),
                                handler_ref,
                                args,
                                true,
                                None,
                                None,
                                remaining_calls.collect(),
                                ancestor_final_push_return,
                            );
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return EvalRequestTurn::Evaluator(self.resume_eval(
                                id,
                                &action,
                                &owner,
                                Err(error),
                            ));
                        }
                    }
                }
                let result_passed = result.passed;
                let result = match completion {
                    Some(ChildCompletion::ConstructScript { fallback }) => self
                        .with_player(player_id, |context| {
                            super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                                context.player, context.symbols, fallback, result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    Some(ChildCompletion::TimeoutNew { timeout_name, fallback }) => self
                        .with_player(player_id, |context| {
                            super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                                context.player, context.symbols, timeout_name, fallback, result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    Some(ChildCompletion::TimeoutForget { timeout_name }) => self
                        .with_player(player_id, |context| {
                            super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                                context.player, context.symbols, timeout_name, result.return_value,
                            )
                        })
                        .ok_or_else(super::cancelled_scope_error)
                        .and_then(|value| value),
                    None => Ok(result.return_value),
                };
                let callback_result = match &result {
                    Ok(return_value) => Ok(super::scope::ScopeResult {
                        return_value: return_value.clone(),
                        passed: result_passed,
                    }),
                    Err(error) => Err(error.clone()),
                };
                self.send_owned_callback_result(&id, callback_result);
                return EvalRequestTurn::Evaluator(self.resume_eval(
                    id,
                    &action,
                    &owner,
                    result,
                ));
            }
            Ok(DriverStart::Running(driver)) => driver,
            Err(error) => {
                if let Some(broadcast) = broadcast {
                    if error.code != ScriptErrorCode::Abort && broadcast.continue_on_error {
                        warn!("broadcast evaluator child could not start; continuing: {}", error.message);
                        return self.continue_eval_broadcast(
                            id,
                            action,
                            owner,
                            broadcast,
                            DatumRef::Void,
                            true,
                        );
                    }
                }
                self.send_owned_callback_result(&id, Err(error.clone()));
                return EvalRequestTurn::Evaluator(self.resume_eval(
                    id,
                    &action,
                    &owner,
                    Err(error),
                ));
            }
        };
        let child = EvalChildContinuation {
            action,
            owner,
            parent_scope,
            parent_scope_count,
            parent_handler_stack_depth,
            completion,
            broadcast,
            ancestor_remaining,
            ancestor_final_push_return,
            driver,
        };
        self.eval_drivers.insert(id.clone(), child);
        let mut child = self.eval_drivers.remove(&id).expect("inserted evaluator child");
        let child_turn = child.driver.turn(self);
        self.finish_eval_child(id, child, child_turn)
    }

    pub(crate) fn turn_eval_child(
        &mut self,
        id: crate::player::eval::EvalId,
    ) -> Option<EvalRequestTurn> {
        let mut child = self.eval_drivers.remove(&id)?;
        let turn = child.driver.turn(self);
        Some(self.finish_eval_child(id, child, turn))
    }

    pub(crate) fn complete_eval_child_action(
        &mut self,
        id: crate::player::eval::EvalId,
        ticket: CompletionTicket,
        completion: ActionCompletion,
    ) -> Option<EvalRequestTurn> {
        let Some(mut child) = self.eval_drivers.remove(&id) else {
            return None;
        };
        if !child.driver.complete(self, ticket, completion) {
            self.eval_drivers.insert(id, child);
            return None;
        }
        let child_turn = child.driver.turn(self);
        Some(self.finish_eval_child(id, child, child_turn))
    }

    /// Apply an externally completed evaluator request only for the original
    /// owner. A foreign, stale, or duplicate completion is rejected before any
    /// player mutation.
    pub(crate) fn complete_eval(
        &mut self,
        id: crate::player::eval::EvalId,
        action: &crate::player::eval::EvalAction,
        owner: &OwnerToken,
        result: Result<DatumRef, ScriptError>,
    ) -> bool {
        let Some(mut continuation) = self.evals.remove(&id) else { return false };
        let accepted = continuation.complete(self, action, owner, result);
        if !accepted || matches!(continuation.state, crate::player::eval::EvalState::Ready) {
            self.evals.insert(id, continuation);
        }
        accepted
    }

    /// Complete a typed sprite/cast async request after its owner-side host
    /// executor has finished. The request's embedded owner and player identity
    /// are checked against the current arena before the evaluator sees the
    /// returned datum; this keeps a late Flash, W3D, or Havok completion from
    /// reaching a replacement player with the same numeric id.
    pub(crate) fn complete_typed_async_eval(
        &mut self,
        id: crate::player::eval::EvalId,
        action: &crate::player::eval::EvalAction,
        request: &super::driver::InternalVmRequest,
        owner: &OwnerToken,
        result: Result<DatumRef, ScriptError>,
    ) -> bool {
        let (player_id, request_owner) = match request {
            super::driver::InternalVmRequest::SpriteAsync(request) => {
                (request.player_id, &request.owner)
            }
            super::driver::InternalVmRequest::CastMemberAsync(request) => {
                (request.player_id, &request.owner)
            }
            super::driver::InternalVmRequest::Flash(request) => {
                (request.player_id, &request.owner)
            }
            _ => return false,
        };
        let current_owner = self.with_player(player_id, |context| context.player.owner.clone());
        let Some(current_owner) = current_owner else { return false };
        if !owner.is_arena_live()
            || !owner.same_identity(request_owner)
            || !owner.same_identity(&current_owner)
            || !current_owner.is_arena_live()
        {
            return false;
        }
        if let super::driver::InternalVmRequest::Flash(request) = request {
            let Some(current) = request.expected_generation else {
                return false;
            };
            if !self.with_player(player_id, |context| {
                context.player.is_flash_instance_generation_current(
                    request.sprite_num as i16,
                    current,
                )
            }).unwrap_or(false) {
                return false;
            }
        }
        self.complete_eval(id, action, &current_owner, result)
    }

    /// Run the owner-bound Flash portion of a typed sprite request. Attached
    /// Lingo scripts are started by `execute_eval_request`; this entrypoint is
    /// for the remaining direct Flash operation after the host readiness wait.
    pub(crate) async fn execute_sprite_async_request(
        session: RuntimeSessionHandle,
        request: super::driver::SpriteAsyncRequest,
    ) -> Result<DatumRef, ScriptError> {
        super::handlers::datum_handlers::sprite::SpriteDatumHandlers::execute_async_request(
            session, request,
        )
        .await
    }

    /// Execute a prepared cast-member request through the owner-bound cast
    /// executor. Load/import waits own only network task state; Havok returns
    /// callback continuations for `start_cast_async_callback` below.
    pub(crate) async fn execute_cast_member_async_request(
        session: RuntimeSessionHandle,
        request: super::driver::CastMemberAsyncRequest,
    ) -> Result<super::handlers::datum_handlers::cast_member_ref::CastAsyncExecution, ScriptError> {
        super::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers::execute_async_request(
            session, request,
        )
        .await
    }

    /// Start one callback returned by a Havok async request. The callback is
    /// an ordinary owner-bound child driver, so its scope setup, result
    /// validation, and any subsequent pending host action use the same
    /// completion machinery as script and global children.
    pub(crate) fn start_cast_async_callback(
        &mut self,
        request: &super::driver::CastMemberAsyncRequest,
        handler: Symbol,
        receiver: &DatumRef,
        args: &[DatumRef],
        is_step_callback: bool,
    ) -> Result<DriverStart, ScriptError> {
        let (instance_ref, handler_ref) = self
            .with_player(request.player_id, |context| {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return Err(super::cancelled_scope_error());
                }
                let instance_ref = match checked_internal_datum(context.player, context.symbols, receiver)? {
                    Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
                    _ => return Err(ScriptError::new("Havok callback receiver is not a script instance".to_owned())),
                };
                let handler_ref = ScriptInstanceUtils::get_script_instance_handler(
                    handler,
                    &instance_ref,
                    context.player,
                )?
                .ok_or_else(|| ScriptError::new("Havok callback handler not found".to_owned()))?;
                Ok((instance_ref, handler_ref))
            })
            .ok_or_else(super::cancelled_scope_error)??;
        let start = DriverContinuation::start_subordinate(
            self,
            request.player_id,
            Some(instance_ref),
            handler_ref,
            args,
            false,
            None,
        )?;
        match start {
            DriverStart::Early(result) => Ok(DriverStart::Early(result)),
            DriverStart::Running(mut driver) => {
                driver.step_callback = is_step_callback;
                Ok(DriverStart::Running(driver))
            }
        }
    }

    /// Accept one owned request completion and immediately resume the saved
    /// frame stack. A completion is consumed once; a later duplicate or a
    /// completion carrying a different owner token returns an error turn
    /// without touching the player.
    pub(crate) fn resume_eval(
        &mut self,
        id: crate::player::eval::EvalId,
        action: &crate::player::eval::EvalAction,
        owner: &OwnerToken,
        result: Result<DatumRef, ScriptError>,
    ) -> crate::player::eval::EvalTurn {
        let Some(mut continuation) = self.evals.remove(&id) else {
            return crate::player::eval::EvalTurn::Complete(Err(ScriptError::new(
                "unknown or duplicate evaluator completion".to_owned(),
            )));
        };
        let error = result.as_ref().err().cloned();
        if !continuation.complete(self, action, owner, result) {
            self.evals.insert(id, continuation);
            return crate::player::eval::EvalTurn::Complete(Err(ScriptError::new(
                "stale, foreign, or duplicate evaluator completion".to_owned(),
            )));
        }
        let player_id = continuation.player_id;
        let turn = if let Some(error) = error {
            crate::player::eval::EvalTurn::Complete(Err(error))
        } else {
            continuation.turn(self)
        };
        // Synchronous evaluator work can close an Xtra instance while
        // `continuation.turn` still owns the session. Move the released host
        // resource into the session queue before returning every turn; the
        // command/evaluator pump drains that queue only after its RefMut is
        // dropped, so socket/listener Drop never re-enters the session.
        self.collect_player_host_teardowns(player_id);
        if matches!(turn, crate::player::eval::EvalTurn::Pending { .. }) {
            self.evals.insert(id, continuation);
        }
        turn
    }

    pub fn symbols(&self) -> &SymbolTable {
        &self.symbols
    }
    pub fn symbols_mut(&mut self) -> &mut SymbolTable {
        &mut self.symbols
    }

    /// Bind presentation state weakly to a session player. The renderer can
    /// then be resolved by an owner-checked frame boundary without creating a
    /// strong session/renderer cycle.
    pub(crate) fn bind_renderer_state(
        &mut self,
        player_id: PlayerId,
        state: &std::rc::Rc<crate::rendering::RendererState>,
    ) {
        self.renderer_bindings.insert(player_id, std::rc::Rc::downgrade(state));
    }

    pub(crate) fn renderer_state(
        &self,
        player_id: PlayerId,
    ) -> Option<std::rc::Weak<crate::rendering::RendererState>> {
        self.renderer_bindings.get(&player_id).cloned()
    }

    pub(crate) fn unbind_renderer_state(&mut self, player_id: PlayerId) {
        self.renderer_bindings.remove(&player_id);
    }
    pub fn players(&self) -> &PlayerGraph {
        &self.players
    }
    pub fn add_player(&mut self, id: PlayerId, tx: Sender<PlayerVMExecutionItem>) -> bool {
        if self.players.players.contains_key(&id) {
            return false;
        }
        let player_owner = OwnerToken::new(OwnerKey {
            session: self.owner.session,
            player: id as u64,
            generation: self.generation,
        });
        let inserted = self
            .players
            .insert(id, DirPlayer::new_with_owner(tx, player_owner.clone()));
        if inserted {
            self.w3d_clocks.insert(id, W3dClock::new(player_owner));
        }
        inserted
    }

    /// Claim the single playback loop for a captured owner. A second `play`
    /// on the same handle is intentionally idempotent; a replacement owner
    /// cannot inherit the old loop or its cancellation channel.
    pub(crate) fn begin_playback_loop(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) -> Result<Option<(u64, Receiver<()>)>, ScriptError> {
        let owner_matches = self
            .players
            .players
            .get(&player_id)
            .is_some_and(|player| player.owner.same_identity(owner) && owner.is_arena_live());
        if !owner_matches {
            return Err(super::cancelled_scope_error());
        }
        if let Some(active) = self.playback_loops.get(&player_id) {
            if active.owner.same_identity(owner) {
                return Ok(None);
            }
            // A stale owner may have been invalidated before its task got a
            // chance to run its finalizer. Wake it before replacing the slot.
            let _ = active.cancel.try_send(());
        }
        // A stopped loop still owes StopMovie/endSprite cleanup. Do not let
        // an immediate replay install a replacement until that old owner has
        // finished its cancellable cleanup boundary.
        if self
            .playback_cancellations
            .get(&player_id)
            .is_some_and(|(active_owner, _, stop_sequence)| {
                *stop_sequence && active_owner.same_identity(owner)
            })
        {
            let epoch = self
                .playback_cancellations
                .get(&player_id)
                .map(|(_, epoch, _)| *epoch)
                .expect("matching playback cancellation disappeared");
            self.playback_replay_requests
                .insert(player_id, (owner.clone(), epoch));
            return Ok(None);
        }
        let epoch = self
            .playback_epochs
            .entry(player_id)
            .and_modify(|epoch| *epoch = epoch.wrapping_add(1).max(1))
            .or_insert(1);
        let epoch = *epoch;
        let (cancel, receiver) = channel::bounded(1);
        self.playback_loops.insert(
            player_id,
            PlaybackLoopControl {
                owner: owner.clone(),
                epoch,
                cancel,
            },
        );
        Ok(Some((epoch, receiver)))
    }

    /// Wake and remove the exact owner-bound loop. Identity is checked before
    /// removal so an old stop cannot cancel a replacement player with the same
    /// numeric id.
    pub(crate) fn cancel_playback_loop(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        stop_sequence: bool,
    ) -> Option<u64> {
        if self
            .playback_replay_requests
            .get(&player_id)
            .is_some_and(|(request_owner, _)| request_owner.same_identity(owner))
        {
            // Reset/remove and a second stop cancel an already queued
            // same-owner replay before old cleanup can hand it back.
            self.playback_replay_requests.remove(&player_id);
        }
        let Some(active) = self.playback_loops.get(&player_id) else {
            return None;
        };
        if !active.owner.same_identity(owner) {
            return None;
        }
        let active = self
            .playback_loops
            .remove(&player_id)
            .expect("playback loop disappeared after owner validation");
        let epoch = active.epoch;
        let _ = active.cancel.try_send(());
        self.playback_cancellations
            .insert(player_id, (owner.clone(), epoch, stop_sequence));
        self.playback_epochs
            .entry(player_id)
            .and_modify(|epoch| *epoch = epoch.wrapping_add(1).max(1))
            .or_insert(1);
        Some(epoch)
    }

    /// Clear a loop only if its owner and epoch still match. This is the
    /// finalizer fence for a loop that was stopped while its future was
    /// suspended and a replacement loop was started immediately afterward.
    pub(crate) fn finish_playback_loop(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        epoch: u64,
    ) -> bool {
        let matches = self.playback_loops.get(&player_id).is_some_and(|active| {
            active.epoch == epoch && active.owner.same_identity(owner)
        });
        if matches {
            self.playback_loops.remove(&player_id);
        }
        self.playback_cancellations.retain(|id, (_, active_epoch, _)| {
            *id != player_id || *active_epoch != epoch
        });
        matches
    }

    pub(crate) fn take_playback_replay_request(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        epoch: u64,
    ) -> bool {
        let Some((request_owner, request_epoch)) =
            self.playback_replay_requests.get(&player_id).cloned()
        else {
            return false;
        };
        if request_epoch != epoch
            || !request_owner.same_identity(owner)
            || !owner.is_arena_live()
        {
            return false;
        }
        self.playback_replay_requests.remove(&player_id);
        true
    }

    pub(crate) fn discard_playback_replay_request(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        epoch: u64,
    ) -> bool {
        let Some((request_owner, request_epoch)) =
            self.playback_replay_requests.get(&player_id)
        else {
            return false;
        };
        if *request_epoch != epoch || !request_owner.same_identity(owner) {
            return false;
        }
        self.playback_replay_requests.remove(&player_id);
        true
    }

    pub(crate) fn take_playback_stop_cleanup(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        epoch: u64,
    ) -> bool {
        let Some((cancel_owner, cancel_epoch, stop_sequence)) =
            self.playback_cancellations.get(&player_id).cloned()
        else {
            return false;
        };
        if cancel_epoch != epoch || !cancel_owner.same_identity(owner) {
            return false;
        }
        stop_sequence
    }

    pub(crate) fn playback_loop_active(
        &self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) -> bool {
        self.playback_loops
            .get(&player_id)
            .is_some_and(|active| active.owner.same_identity(owner))
    }

    pub(crate) fn register_nested_player(
        &mut self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
        member_ref: CastMemberRef,
        command_tx: Sender<PlayerVMExecutionItem>,
        event_tx: Sender<super::events::PlayerVMEvent>,
    ) -> Result<(PlayerId, OwnerToken), ScriptError> {
        let parent = self
            .players
            .players
            .get(&parent_id)
            .ok_or_else(super::cancelled_scope_error)?;
        if !parent.owner.same_identity(parent_owner) || !parent_owner.is_arena_live() {
            return Err(super::cancelled_scope_error());
        }
        if self
            .nested
            .child_for(parent_id, parent_owner, member_ref)
            .is_some()
        {
            return Err(ScriptError::new("nested movie is already active".into()));
        }
        let occupied: std::collections::HashSet<PlayerId> =
            self.players.players.keys().copied().collect();
        let child_id = self.nested.allocate_id(|id| occupied.contains(&id))?;
        if !self.add_player(child_id, command_tx.clone()) {
            return Err(ScriptError::new("nested player id collision".into()));
        }
        let child_owner = self
            .players
            .players
            .get(&child_id)
            .map(|player| player.owner.clone())
            .ok_or_else(super::cancelled_scope_error)?;
        let record = NestedChildRecord {
            parent_id,
            parent_owner: parent_owner.clone(),
            member_ref,
            child_id,
            child_owner: child_owner.clone(),
            command_tx,
            event_tx,
        };
        if let Err(error) = self.nested.insert(record) {
            let _ = self.remove_player(child_id);
            return Err(error);
        }
        Ok((child_id, child_owner))
    }

    pub(crate) fn nested_child_for(
        &self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
        member_ref: CastMemberRef,
    ) -> Option<NestedChildRecord> {
        let parent = self.players.players.get(&parent_id)?;
        if !parent.owner.same_identity(parent_owner) || !parent.owner.is_arena_live() {
            return None;
        }
        self.nested
            .child_for(parent_id, parent_owner, member_ref)
            .filter(|record| {
                self.players
                    .players
                    .get(&record.child_id)
                    .is_some_and(|child| {
                        child.owner.same_identity(&record.child_owner)
                            && child.owner.is_arena_live()
                    })
            })
            .cloned()
    }

    pub(crate) fn nested_children_for(
        &self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
    ) -> Vec<NestedChildRecord> {
        let Some(parent) = self.players.players.get(&parent_id) else {
            return Vec::new();
        };
        if !parent.owner.same_identity(parent_owner) || !parent.owner.is_arena_live() {
            return Vec::new();
        }
        let mut children: Vec<_> = self
            .nested
            .children_for(parent_id, parent_owner)
            .filter(|record| {
                self.players
                    .players
                    .get(&record.child_id)
                    .is_some_and(|child| {
                        child.owner.same_identity(&record.child_owner)
                            && child.owner.is_arena_live()
                    })
            })
            .cloned()
            .collect();
        children.sort_by_key(|record| {
            (
                record.member_ref.cast_lib,
                record.member_ref.cast_member,
                record.child_id,
            )
        });
        children
    }

    pub(crate) fn take_nested_children_for(
        &mut self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
    ) -> Vec<NestedChildRecord> {
        self.nested
            .take_children_for_parent(parent_id, parent_owner)
    }

    pub(crate) fn retire_nested_child_if_owner(
        &mut self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> bool {
        let matches = self.players.players.get(&child_id).is_some_and(|player| {
            player.owner.same_identity(child_owner) && child_owner.is_arena_live()
        });
        if !matches {
            return false;
        }
        let _ = self.remove_player(child_id);
        true
    }

    /// Retire a nested child after its captured owner has already been marked
    /// dead. Identity, rather than liveness, is intentional here: it lets a
    /// child loop finish cancellation while still refusing a numeric-id
    /// replacement owned by a different Rc capability.
    pub(crate) fn retire_nested_child_if_identity(
        &mut self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> bool {
        // Restrict identity-only cleanup to a registry child.  A stale loop
        // must be allowed to retire after its owner is marked dead, but it
        // must never remove an ordinary parent or a replacement player that
        // reused the numeric id.
        let matches = self.nested.contains_child_identity(child_id, child_owner)
            && self
                .players
                .players
                .get(&child_id)
                .is_some_and(|player| player.owner.same_identity(child_owner));
        if !matches {
            return false;
        }
        let _ = self.remove_player(child_id);
        true
    }

    fn cancel_owner_work(&mut self, player_id: PlayerId, owner: &OwnerToken) {
        let mut retained_commands = Vec::new();
        for mut pending in self.pending_commands.drain(..) {
            if pending.player_id != player_id || !pending.owner.same_identity(owner) {
                retained_commands.push(pending);
                continue;
            }
            // Nested callback commands are created without a caller
            // completer; any outer completer is owned by the command loop.
            // Keep this invariant explicit: cancellation must not launch an
            // ambient task while the session is being synchronously unwound.
            debug_assert!(pending.completer.is_none());
            let _ = pending.completer.take();
            if let Some(sender) = pending.event_sender.take() {
                let _ = sender.try_send(Err(super::cancelled_scope_error()));
            }
            if let Some(sender) = pending.eval_sender.take() {
                let _ = sender.try_send(Err(super::cancelled_scope_error()));
            }
        }
        self.pending_commands = retained_commands;
        self.pending_completions.retain(|(id, _, _)| *id != player_id);
        self.pending_eval_requests.retain(|pending| {
            if pending.player_id == player_id && pending.owner.same_identity(owner) {
                let _ = pending.sender.try_send(Err(super::cancelled_scope_error()));
                false
            } else {
                true
            }
        });
        self.inflight_eval_routes.retain(|route| {
            if route.player_id == player_id && route.owner.same_identity(owner) {
                let _ = route.sender.try_send(Err(super::cancelled_scope_error()));
                false
            } else {
                true
            }
        });
        self.deferred_requests.retain(|request| request.player_id != player_id);
        self.completed_score_defaults.retain(|(id, _, _)| *id != player_id);
    }

    fn cancel_eval_continuations(&mut self, owner: &OwnerToken) {
        self.evals.retain(|_, continuation| {
            if continuation.owner.same_identity(owner) {
                if let Some(sender) = continuation.take_callback_sender() {
                    let _ = sender.try_send(Err(super::cancelled_scope_error()));
                }
                false
            } else {
                true
            }
        });
    }

    /// Retire evaluator child drivers together with their parent continuations.
    /// Merely removing the map entry leaves a suspended driver holding scopes,
    /// action tickets, and (for broadcasts) static-event guards alive after a
    /// player reset or numeric-id replacement.
    fn cancel_eval_children_for_owner(&mut self, owner: &OwnerToken) {
        let ids: Vec<_> = self
            .eval_drivers
            .iter()
            .filter_map(|(id, child)| {
                child
                    .owner
                    .same_identity(owner)
                    .then_some(id.clone())
            })
            .collect();
        for id in ids {
            if let Some(mut child) = self.eval_drivers.remove(&id) {
                self.release_eval_broadcast_guard(&mut child);
                let _ = child.driver.cancel(self);
                self.send_owned_callback_result(&id, Err(super::cancelled_scope_error()));
                self.evals.remove(&id);
            }
        }
    }

    /// Cancel one dropped nested callback without resetting its owner's whole
    /// player. This removes the exact pending command/evaluator child, unwinds
    /// its scopes, and completes the callback receiver with Abort. All other
    /// owner work remains retained for the next frame.
    pub(crate) fn cancel_eval_callback(
        &mut self,
        id: &crate::player::eval::EvalId,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) {
        let mut retained_commands = Vec::new();
        let mut canceled_tickets = Vec::new();
        for mut pending in self.pending_commands.drain(..) {
            let matches = pending.player_id == player_id
                && pending.owner.same_identity(owner)
                && pending.eval_child.as_ref() == Some(id);
            if !matches {
                retained_commands.push(pending);
                continue;
            }
            if let Some(action) = pending.action.take() {
                canceled_tickets.push(action.ticket().clone());
            } else if let Some(ticket) = pending.ticket.take() {
                canceled_tickets.push(ticket);
            }
            // Nested callback commands intentionally have no outer command
            // completer; that completer remains owned by the command loop.
            // Keep cancellation synchronous and owner-scoped rather than
            // spawning an ambient task while unwinding the callback.
            debug_assert!(pending.completer.is_none());
            let _ = pending.completer.take();
            if let Some(sender) = pending.event_sender.take() {
                let _ = sender.try_send(Err(super::cancelled_scope_error()));
            }
            if let Some(sender) = pending.eval_sender.take() {
                let _ = sender.try_send(Err(super::cancelled_scope_error()));
            }
        }
        self.pending_commands = retained_commands;
        self.pending_completions.retain(|(queued_player, queued_ticket, _)| {
            *queued_player != player_id
                || !canceled_tickets
                    .iter()
                    .any(|ticket| ticket.same_identity(queued_ticket))
        });
        for ticket in canceled_tickets {
            self.actions.cancel_ticket(&ticket);
        }
        self.pending_eval_requests.retain(|pending| {
            if &pending.id == id
                && pending.player_id == player_id
                && pending.owner.same_identity(owner)
            {
                let _ = pending.sender.try_send(Err(super::cancelled_scope_error()));
                false
            } else {
                true
            }
        });
        self.inflight_eval_routes.retain(|route| {
            if &route.id == id
                && route.player_id == player_id
                && route.owner.same_identity(owner)
            {
                let _ = route.sender.try_send(Err(super::cancelled_scope_error()));
                false
            } else {
                true
            }
        });

        if let Some(mut child) = self.eval_drivers.remove(id) {
            if child.driver.player_id == player_id && child.owner.same_identity(owner) {
                self.release_eval_broadcast_guard(&mut child);
                let _ = child.driver.cancel(self);
            } else {
                self.eval_drivers.insert(id.clone(), child);
            }
        }
        if let Some(mut continuation) = self.evals.remove(id) {
            if continuation.player_id == player_id && continuation.owner.same_identity(owner) {
                if let Some(sender) = continuation.take_callback_sender() {
                    let _ = sender.try_send(Err(super::cancelled_scope_error()));
                }
            } else {
                self.evals.insert(id.clone(), continuation);
            }
        }
    }

    /// Reset one captured owner and cancel every retained callback before the
    /// allocator epoch rotates. The loaded movie remains on the player; the
    /// new owner cannot observe old driver/evaluator capabilities.
    pub(crate) fn reset_player_owned(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) -> Result<OwnerToken, ScriptError> {
        let current_owner = self
            .players
            .players
            .get(&player_id)
            .map(|player| player.owner.clone())
            .ok_or_else(super::cancelled_scope_error)?;
        if !current_owner.same_identity(owner) || !owner.is_arena_live() {
            return Err(super::cancelled_scope_error());
        }
        let nested_children = self.take_nested_children_for(player_id, &current_owner);
        for child in nested_children {
            child.command_tx.close();
            child.event_tx.close();
            let _ = self.remove_player(child.child_id);
        }
        self.cancel_playback_loop(player_id, &current_owner, false);
        self.cancel_owner_work(player_id, &current_owner);
        if let Some(mut driver) = self.drivers.remove(&player_id) {
            let _ = driver.cancel(self);
        }
        self.cancel_eval_children_for_owner(&current_owner);
        self.cancel_eval_continuations(&current_owner);
        self.static_event_guards.remove(&player_id);
        self.actions.cancel_owner(&current_owner);
        self.js_lingo.clear_player(player_id, &current_owner);
        self.cancel_player_cast_loads(player_id);
        // Native snapshots belong to the retired generation.  Drop them
        // before reset installs the replacement owner; lifecycle teardown is
        // carried by the separate host-event queue.
        self.native_player_notifications.remove(&player_id);
        self.native_notification_errors.remove(&player_id);
        let player = self
            .players
            .players
            .get_mut(&player_id)
            .ok_or_else(super::cancelled_scope_error)?;
        player.reset_owned_core();
        // Retire the old NetManager shared state when the player generation
        // rotates. Prepared FileIO requests retain the old Arc and therefore
        // cannot wake or satisfy a task in the replacement generation.
        player.net_manager.reset_owner(player.owner.key());
        let new_owner = player.owner.clone();
        self.collect_player_host_teardowns(player_id);
        self.w3d_clocks
            .insert(player_id, W3dClock::new(new_owner.clone()));
        self.with_player(player_id, |context| {
            crate::js_api::JsApi::dispatch_global_list(context.symbols, context.player);
        });
        Ok(new_owner)
    }

    pub fn remove_player(&mut self, id: PlayerId) -> Option<DirPlayer> {
        self.notification_drains.remove(&id);
        self.native_player_notifications.remove(&id);
        self.native_notification_errors.remove(&id);
        self.host_sinks.remove(&id);
        let owner = self.players.players.get(&id).map(|player| player.owner.clone());
        if let Some(owner) = owner {
            let nested_children = self.take_nested_children_for(id, &owner);
            for child in nested_children {
                child.command_tx.close();
                child.event_tx.close();
                let _ = self.remove_player(child.child_id);
            }
            if let Some(child) = self.nested.remove_child(id) {
                child.command_tx.close();
                child.event_tx.close();
            }
            self.cancel_playback_loop(id, &owner, false);
            self.cancel_owner_work(id, &owner);
            self.js_lingo.clear_player(id, &owner);
            // Drop the continuation before removing the arena. Its frames may
            // retain owner-qualified VM values, and its pending actions must
            // not survive for a replacement with the same numeric player id.
            if let Some(mut driver) = self.drivers.remove(&id) {
                let driver_owner = driver.owner.clone();
                let _ = driver.cancel(self);
                self.actions.cancel_owner(&driver_owner);
            }
            self.actions.cancel_owner(&owner);
            self.cancel_eval_children_for_owner(&owner);
            self.cancel_eval_continuations(&owner);
            self.static_event_guards.remove(&id);
            let mut canceled = Vec::new();
            self.pending_casts.retain(|pending| {
                if pending.owner.same_identity(&owner) {
                    canceled.push((
                        pending.request.clone(),
                        matches!(pending.purpose, PendingCastPurpose::Property { .. }),
                    ));
                    false
                } else {
                    true
                }
            });
            if let Some(player) = self.players.players.get_mut(&id) {
                for (request, property) in canceled {
                    if property {
                        player.movie.cast_manager.get_cast_mut(request.cast_number()).cancel_load(&request);
                    } else {
                        player.movie.cast_manager.cancel_preload(&request);
                    }
                }
                // Move active Xtra resources into the session teardown queue
                // before players.remove returns the DirPlayer. The returned
                // player may be dropped while the outer RefMut is live.
                player.xtra_manager_state.reset();
            }
            self.collect_player_host_teardowns(id);
        }
        self.w3d_clocks.remove(&id);
        self.players.remove(id)
    }

    /// Return owner-checked elapsed animation and particle deltas for a
    /// captured frame timestamp. Each player's clock is independent and is
    /// reset whenever that player's owner generation changes.
    pub(crate) fn w3d_deltas(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        now_ms: f64,
    ) -> Result<(f32, f32), ScriptError> {
        let clock = self
            .w3d_clocks
            .get_mut(&player_id)
            .ok_or_else(super::cancelled_scope_error)?;
        Ok((clock.animation_dt(owner, now_ms)?, clock.particle_dt(owner, now_ms)?))
    }

    /// Cancel owned cast continuations before a caller performs a full player
    /// reset through an already-migrated lifecycle boundary.
    pub fn cancel_player_cast_loads(&mut self, id: PlayerId) -> bool {
        let Some(player) = self.players.players.get_mut(&id) else {
            return false;
        };
        let owner = player.owner.clone();
        let mut canceled = Vec::new();
        self.pending_casts.retain(|pending| {
            if pending.owner.same_identity(&owner) {
                canceled.push((
                    pending.request.clone(),
                    matches!(pending.purpose, PendingCastPurpose::Property { .. }),
                ));
                false
            } else {
                true
            }
        });
        for (request, property) in canceled {
            if property {
                player.movie.cast_manager.get_cast_mut(request.cast_number()).cancel_load(&request);
            } else {
                player.movie.cast_manager.cancel_preload(&request);
            }
        }
        true
    }

    fn publish_pending_cast_notifications(
        &mut self,
        player_id: PlayerId,
        generated: &mut CastNotificationOutbox,
    ) {
        self.append_notifications(player_id, generated);
    }

    pub(crate) fn append_notifications(
        &mut self,
        player_id: PlayerId,
        outbox: &mut CastNotificationOutbox,
    ) {
        if let Some(player) = self.players.get_mut(player_id) {
            for notification in player.movie.cast_manager.take_notifications() {
                outbox.push(notification);
            }
            for notification in outbox.drain() {
                match notification {
                    CastNotification::CastMemberNameChanged(slot) => {
                        player.queue_player_notification(
                            PlayerNotificationKind::CastMemberNameChanged(slot),
                        );
                    }
                    CastNotification::CastNameChanged(cast) => {
                        let name = player
                            .movie
                            .cast_manager
                            .get_cast_or_null(cast)
                            .map(|cast| cast.name.clone())
                            .unwrap_or_default();
                        if let Err(error) = player.queue_host_event(
                            super::host_events::HostEvent::CastNameChanged { cast, name },
                        ) {
                            player.queue_player_notification(
                                PlayerNotificationKind::HostBackpressure(error.capacity),
                            );
                        }
                    }
                    CastNotification::CastListChanged => {
                        let names = player
                            .movie
                            .cast_manager
                            .casts
                            .iter()
                            .map(|cast| cast.name.clone())
                            .collect();
                        if let Err(error) = player.queue_host_event(
                            super::host_events::HostEvent::CastListChanged { names },
                        ) {
                            player.queue_player_notification(
                                PlayerNotificationKind::HostBackpressure(error.capacity),
                            );
                        }
                    }
                    CastNotification::CastMemberListChanged(cast) => {
                        let members = player
                            .movie
                            .cast_manager
                            .get_cast_or_null(cast)
                            .map(|cast| {
                                cast.members
                                    .values()
                                    .map(|member| (member.number, member.name.clone()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Err(error) = player.queue_host_event(
                            super::host_events::HostEvent::CastMemberListChanged { cast, members },
                        ) {
                            player.queue_player_notification(
                                PlayerNotificationKind::HostBackpressure(error.capacity),
                            );
                        }
                    }
                }
            }
        } else {
            // There is no owner to which a cast transition can be delivered.
            // Drain it explicitly rather than retaining a process-wide event
            // that could later be mistaken for another player.
            outbox.drain();
        }
    }

    pub(crate) fn take_player_notifications(
        &mut self,
        player_id: PlayerId,
    ) -> Vec<PlayerNotification> {
        self.players
            .get_mut(player_id)
            .map(|player| std::mem::take(&mut player.pending_player_notifications))
            .unwrap_or_default()
    }

    pub(crate) fn player_owner_matches(&self, player_id: PlayerId, owner: &OwnerToken) -> bool {
        self.players
            .players
            .get(&player_id)
            .map(|player| owner.same_identity(&player.owner) && owner.is_arena_live())
            .unwrap_or(false)
    }

    /// Stop all retained work for an owner whose host mailbox overflowed.
    /// The overflow marker remains until reset/rebind, so subsequent owner
    /// checks continue to reject execution without repeatedly retaining work.
    pub(crate) fn cancel_host_backpressured_owner(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) {
        let active = self.players.players.get(&player_id).is_some_and(|player| {
            owner.same_identity(&player.owner) && player.host_event_backpressure.is_some()
        });
        if !active {
            return;
        }
        self.cancel_owner_work(player_id, owner);
        if let Some(mut driver) = self.drivers.remove(&player_id) {
            let _ = driver.cancel(self);
        }
        self.cancel_eval_children_for_owner(owner);
        self.cancel_eval_continuations(owner);
        self.actions.cancel_owner(owner);
    }

    pub(crate) fn begin_player_notification_drain(
        &mut self,
        player_id: PlayerId,
    ) -> Option<(OwnerToken, Vec<PlayerNotification>)> {
        let owner = self.players.players.get(&player_id)?.owner.clone();
        if self.notification_drains.contains_key(&player_id) {
            return None;
        }
        let batch = self
            .players
            .get_mut(player_id)
            .map(|player| {
                let owner = player.owner.clone();
                let mut batch = std::mem::take(&mut player.pending_player_notifications);
                batch.extend(player.take_host_events().into_iter().map(|kind| PlayerNotification {
                    owner: owner.clone(),
                    kind: PlayerNotificationKind::Host(kind),
                }));
                if let Some(error) = player.host_event_backpressure {
                    batch.push(PlayerNotification {
                        owner: owner.clone(),
                        kind: PlayerNotificationKind::HostBackpressure(error.capacity),
                    });
                }
                batch
            })
            .unwrap_or_default();
        self.notification_drains.insert(player_id, owner.clone());
        Some((owner, batch))
    }

    pub(crate) fn begin_scheduled_player_notification(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) -> bool {
        if !self
            .players
            .players
            .get(&player_id)
            .is_some_and(|player| player.owner.same_identity(owner))
        {
            return false;
        }
        match self.scheduled_notification_drains.entry(player_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(owner.clone());
                true
            }
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().same_identity(owner) => false,
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // A reset/replacement may occur before the old detached task
                // starts. Replace the stale marker; the old task's exact-owner
                // finish check cannot clear this generation's marker.
                entry.insert(owner.clone());
                true
            }
        }
    }

    pub(crate) fn finish_scheduled_player_notification(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) {
        if self
            .scheduled_notification_drains
            .get(&player_id)
            .is_some_and(|scheduled| scheduled.same_identity(owner))
        {
            self.scheduled_notification_drains.remove(&player_id);
        }
    }

    pub(crate) fn finish_player_notification_drain(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) {
        if self
            .notification_drains
            .get(&player_id)
            .is_some_and(|active| active.same_identity(owner))
        {
            self.notification_drains.remove(&player_id);
        }
    }

    pub(crate) fn push_native_host_event(
        &mut self,
        player_id: PlayerId,
        owner: OwnerToken,
        event: super::host_events::HostEvent,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        self.native_host_events.push(super::host_events::NativeHostEvent {
            player_id,
            owner,
            event,
        })
    }

    pub(crate) fn take_native_host_events(
        &mut self,
    ) -> Vec<super::host_events::NativeHostEvent> {
        self.native_host_events.drain()
    }

    pub(crate) fn push_native_player_notification(
        &mut self,
        notification: super::host_events::NativePlayerNotification,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        // A reset may rotate the owner between detached snapshot extraction
        // and mailbox insertion.  Such a snapshot is stale and must not be
        // attributed to the replacement generation.
        if !self.player_owner_matches(notification.player_id, &notification.owner) {
            return Ok(());
        }
        self.native_player_notifications
            .entry(notification.player_id)
            .or_default()
            .push(notification)
    }

    pub(crate) fn take_native_player_notifications(
        &mut self,
        player_id: PlayerId,
    ) -> Vec<super::host_events::NativePlayerNotification> {
        self.native_player_notifications
            .get_mut(&player_id)
            .map(NativePlayerNotificationMailbox::drain)
            .unwrap_or_default()
    }

    pub(crate) fn record_native_notification_error(
        &mut self,
        error: NativeNotificationError,
    ) {
        if self.player_owner_matches(error.player_id, &error.owner) {
            self.native_notification_errors
                .insert(error.player_id, error);
        }
    }

    pub(crate) fn take_native_notification_error(
        &mut self,
        player_id: PlayerId,
    ) -> Option<NativeNotificationError> {
        self.native_notification_errors.remove(&player_id)
    }

    pub(crate) fn bind_host_sink(
        &mut self,
        player_id: PlayerId,
        owner: &OwnerToken,
        sink: &BrowserHostSinkRef,
    ) -> Result<(), ScriptError> {
        if !self.player_owner_matches(player_id, owner) {
            return Err(super::cancelled_scope_error());
        }
        self.host_sinks
            .insert(player_id, (owner.clone(), Rc::downgrade(sink)));
        Ok(())
    }

    pub(crate) fn unbind_host_sink(&mut self, player_id: PlayerId, owner: &OwnerToken) {
        if self
            .host_sinks
            .get(&player_id)
            .is_some_and(|(bound, _)| bound.same_identity(owner))
        {
            self.host_sinks.remove(&player_id);
        }
    }

    pub(crate) fn host_sink(
        &self,
        player_id: PlayerId,
        owner: &OwnerToken,
    ) -> Option<BrowserHostSinkRef> {
        let (bound_owner, sink) = self.host_sinks.get(&player_id)?;
        if !bound_owner.same_identity(owner) || !owner.is_arena_live() {
            return None;
        }
        sink.upgrade()
    }

    pub(crate) fn queue_host_event_delivery(
        &mut self,
        delivery: HostEventDelivery,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        self.queue_host_event_deliveries(vec![delivery])
    }

    pub(crate) fn queue_host_event_deliveries(
        &mut self,
        deliveries: Vec<HostEventDelivery>,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        if deliveries.is_empty() {
            return Ok(());
        }
        let player_id = deliveries[0].player_id;
        debug_assert!(deliveries.iter().all(|delivery| delivery.player_id == player_id));
        let inflight = self.host_event_inflight.get(&player_id).copied().unwrap_or(0);
        let queue = self
            .host_event_deliveries
            .entry(player_id)
            .or_default();
        let terminal_reserve = deliveries.len() == 1
            && matches!(
                &deliveries[0].event,
                super::host_events::HostEvent::OwnerRetired { .. }
            );
        let capacity = super::host_events::MAX_HOST_EVENTS
            + if terminal_reserve {
                super::host_events::HOST_EVENT_TERMINAL_RESERVE
            } else {
                0
            };
        if queue
            .len()
            .saturating_add(inflight)
            .saturating_add(deliveries.len())
            > capacity
        {
            return Err(super::host_events::HostEventOverflow {
                capacity,
            });
        }
        queue.extend(deliveries);
        Ok(())
    }

    pub(crate) fn ensure_host_event_capacity(
        &self,
        player_id: PlayerId,
        additional: usize,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        let queued = self
            .host_event_deliveries
            .get(&player_id)
            .map_or(0, VecDeque::len);
        let inflight = self.host_event_inflight.get(&player_id).copied().unwrap_or(0);
        if queued
            .saturating_add(inflight)
            .saturating_add(additional)
            > super::host_events::MAX_HOST_EVENTS
        {
            return Err(super::host_events::HostEventOverflow {
                capacity: super::host_events::MAX_HOST_EVENTS,
            });
        }
        Ok(())
    }

    pub(crate) fn ensure_host_event_terminal_capacity(
        &self,
        player_id: PlayerId,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        let queued = self
            .host_event_deliveries
            .get(&player_id)
            .map_or(0, VecDeque::len);
        let inflight = self.host_event_inflight.get(&player_id).copied().unwrap_or(0);
        if queued.saturating_add(inflight)
            >= super::host_events::MAX_HOST_EVENTS
                + super::host_events::HOST_EVENT_TERMINAL_RESERVE
        {
            return Err(super::host_events::HostEventOverflow {
                capacity: super::host_events::MAX_HOST_EVENTS
                    + super::host_events::HOST_EVENT_TERMINAL_RESERVE,
            });
        }
        Ok(())
    }

    pub(crate) fn begin_host_event_drain(
        &mut self,
        player_id: PlayerId,
    ) -> Option<Vec<HostEventDelivery>> {
        if self.host_event_drains.contains(&player_id) {
            return None;
        }
        let deliveries = self.host_event_deliveries.remove(&player_id)?;
        self.host_event_drains.insert(player_id);
        self.host_event_inflight
            .insert(player_id, deliveries.len());
        Some(deliveries.into_iter().collect())
    }

    pub(crate) fn host_event_lifecycle_pending(&self, player_id: PlayerId) -> bool {
        self.host_event_drains.contains(&player_id)
            || self
                .host_event_deliveries
                .get(&player_id)
                .is_some_and(|queue| !queue.is_empty())
    }

    pub(crate) fn consume_host_event_delivery(&mut self, player_id: PlayerId) {
        if let Some(inflight) = self.host_event_inflight.get_mut(&player_id) {
            *inflight = inflight.saturating_sub(1);
        }
    }

    pub(crate) fn release_host_event_reservation(
        &mut self,
        player_id: PlayerId,
        count: usize,
    ) {
        if let Some(inflight) = self.host_event_inflight.get_mut(&player_id) {
            *inflight = inflight.saturating_sub(count);
        }
    }

    pub(crate) fn prepend_host_event_deliveries(
        &mut self,
        player_id: PlayerId,
        deliveries: Vec<HostEventDelivery>,
    ) -> Result<(), super::host_events::HostEventOverflow> {
        if deliveries.is_empty() {
            return Ok(());
        }
        let inflight = self.host_event_inflight.get(&player_id).copied().unwrap_or(0);
        let queue = self.host_event_deliveries.entry(player_id).or_default();
        if queue
            .len()
            .saturating_add(inflight)
            .saturating_add(deliveries.len())
            > super::host_events::MAX_HOST_EVENTS
                + super::host_events::HOST_EVENT_TERMINAL_RESERVE
        {
            return Err(super::host_events::HostEventOverflow {
                capacity: super::host_events::MAX_HOST_EVENTS
                    + super::host_events::HOST_EVENT_TERMINAL_RESERVE,
            });
        }
        for delivery in deliveries.into_iter().rev() {
            queue.push_front(delivery);
        }
        Ok(())
    }

    pub(crate) fn finish_host_event_drain(&mut self, player_id: PlayerId) -> bool {
        self.host_event_drains.remove(&player_id);
        self.host_event_inflight.remove(&player_id);
        self.host_event_deliveries
            .get(&player_id)
            .is_some_and(|queue| !queue.is_empty())
    }

    pub(crate) fn clear_pending_host_event_deliveries(&mut self, player_id: PlayerId) {
        if let Some(queue) = self.host_event_deliveries.get_mut(&player_id) {
            queue.retain(|delivery| {
                matches!(
                    &delivery.event,
                    super::host_events::HostEvent::OwnerRetired { .. }
                        | super::host_events::HostEvent::FlashReset { .. }
                )
            });
        }
    }

    pub(crate) fn discard_completed_property_cast(&mut self, ticket: &CompletionTicket) {
        self.completed_property_casts.retain(|(_, completed)| {
            !completed.same_identity(ticket)
        });
    }
    pub(crate) fn retain_pending_eval_request(
        &mut self,
        id: crate::player::eval::EvalId,
        player_id: PlayerId,
        owner: OwnerToken,
        request: crate::player::eval::EvalPending,
        sender: Sender<Result<DatumRef, ScriptError>>,
    ) {
        let action = match &request {
            crate::player::eval::EvalPending::Global { capability, .. }
            | crate::player::eval::EvalPending::Object { capability, .. }
            | crate::player::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
        };
        self.pending_eval_requests.push(PendingEvalRequest {
            id,
            player_id,
            owner,
            action,
            request,
            sender,
        });
        if let Some(player) = self.players.players.get(&player_id) {
            let _ = player.queue_tx.try_send(PlayerVMExecutionItem {
                command: super::commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
    }

    pub(crate) fn take_pending_eval_requests_for(
        &mut self,
        player_id: PlayerId,
    ) -> Vec<PendingEvalRequest> {
        let mut selected = Vec::new();
        let mut retained = Vec::new();
        for pending in std::mem::take(&mut self.pending_eval_requests) {
            if pending.player_id == player_id {
                self.inflight_eval_routes.push(InflightEvalRoute {
                    id: pending.id.clone(),
                    player_id: pending.player_id,
                    owner: pending.owner.clone(),
                    action: pending.action.clone(),
                    sender: pending.sender.clone(),
                });
                selected.push(pending);
            } else {
                retained.push(pending);
            }
        }
        self.pending_eval_requests = retained;
        selected
    }

    /// Apply a host result to an evaluator request. Invalid owner/capability
    /// results leave the request queued; accepted pending turns are requeued
    /// with their newly-issued evaluator capability.
    pub(crate) fn requeue_pending_eval_request(
        &mut self,
        pending: PendingEvalRequest,
    ) {
        // One evaluator continuation owns one sender and one owner route at a
        // time.  A resumed continuation may issue a new capability, so the
        // action changes while the EvalId and owner remain stable.  Retire
        // that prior route by continuation identity rather than by the old
        // action; sibling EvalIds remain untouched.
        self.inflight_eval_routes.retain(|route| {
            !(route.id == pending.id
                && route.player_id == pending.player_id
                && route.owner.same_identity(&pending.owner))
        });
        self.pending_eval_requests.push(pending);
    }

    pub(crate) fn detach_pending_eval_route(
        &mut self,
        id: &crate::player::eval::EvalId,
        action: &crate::player::eval::EvalAction,
    ) -> Option<(PlayerId, OwnerToken, Sender<Result<DatumRef, ScriptError>>)> {
        let index = self.inflight_eval_routes.iter().position(|route| {
            &route.id == id && &route.action == action
        })?;
        let route = self.inflight_eval_routes.swap_remove(index);
        Some((route.player_id, route.owner, route.sender))
    }

    pub(crate) fn submit_pending_eval_completion(
        &mut self,
        id: crate::player::eval::EvalId,
        action: crate::player::eval::EvalAction,
        owner: OwnerToken,
        result: Result<DatumRef, ScriptError>,
    ) -> Option<EvalRequestTurn> {
        let route = if let Some(index) = self.pending_eval_requests.iter().position(|pending| {
            pending.id == id && pending.action == action && pending.owner.same_identity(&owner)
        }) {
            let pending = self.pending_eval_requests.swap_remove(index);
            InflightEvalRoute {
                id: pending.id,
                player_id: pending.player_id,
                owner: pending.owner,
                action: pending.action,
                sender: pending.sender,
            }
        } else if let Some(index) = self.inflight_eval_routes.iter().position(|route| {
            route.id == id && route.action == action && route.owner.same_identity(&owner)
        }) {
            self.inflight_eval_routes.swap_remove(index)
        } else {
            return None;
        };
        let turn = self.resume_eval(id.clone(), &action, &owner, result);
        match turn {
            crate::player::eval::EvalTurn::Complete(result) => {
                let _ = route.sender.try_send(result);
                None
            }
            crate::player::eval::EvalTurn::Pending { request } => {
                let next_action = match &request {
                    crate::player::eval::EvalPending::Global { capability, .. }
                    | crate::player::eval::EvalPending::Object { capability, .. }
                    | crate::player::eval::EvalPending::SetProperty { capability, .. } => capability.clone(),
                };
                self.pending_eval_requests.push(PendingEvalRequest {
                    id,
                    player_id: route.player_id,
                    owner: route.owner,
                    action: next_action,
                    request,
                    sender: route.sender,
                });
                None
            }
        }
    }

    pub(crate) fn retain_deferred_request(
        &mut self,
        player_id: PlayerId,
        request: super::driver::InternalVmRequest,
        reason: String,
    ) {
        self.deferred_requests.push(DeferredRequest {
            player_id,
            request,
            reason,
            continuation: None,
        });
    }

    pub(crate) fn retain_deferred_request_with_continuation(
        &mut self,
        player_id: PlayerId,
        request: super::driver::InternalVmRequest,
        reason: String,
        continuation: crate::player::score::BehaviorDefaultsContinuation,
    ) {
        self.deferred_requests.push(DeferredRequest {
            player_id,
            request,
            reason,
            continuation: Some(continuation),
        });
    }

    pub(crate) fn take_deferred_requests(&mut self) -> Vec<DeferredRequest> {
        std::mem::take(&mut self.deferred_requests)
    }

    /// Apply a host completion to one detached deferred request. The session
    /// handle is borrowed only for the owner check and continuation lookup;
    /// the score continuation then reacquires short player borrows for its
    /// defaults, `on new`, and authored-parameter phases.
    pub(crate) async fn resume_deferred_request(
        session: RuntimeSessionHandle,
        deferred: DeferredRequest,
        result: Result<DatumRef, ScriptError>,
    ) -> Result<Option<DeferredRequest>, ScriptError> {
        let Some(continuation) = deferred.continuation.clone() else {
            // Preserve requests owned by other host routes. This helper is
            // intentionally scoped to score initialization and must never
            // consume an unrelated deferred invocation.
            return Ok(Some(deferred));
        };
        crate::player::score::Score::resume_behavior_defaults_continuation(
            session,
            deferred.player_id,
            continuation,
            result,
        )
        .await?;
        Ok(None)
    }

    /// Retain a command callback across an external wait. The action and
    /// completer stay owned by the session until the exact ticket completes.
    pub(crate) fn retain_pending_command(&mut self, pending: PendingCommand) {
        self.retain_pending_command_inner(pending, true);
    }

    /// Reinsert an action removed by the owner pump without waking the same
    /// loop again. This preserves the host ticket while avoiding a
    /// self-generated PumpPending receive on every retry.
    pub(crate) fn requeue_pending_command(&mut self, pending: PendingCommand) {
        self.retain_pending_command_inner(pending, false);
    }

    fn retain_pending_command_inner(&mut self, pending: PendingCommand, wake: bool) {
        let player_id = pending.player_id;
        let queue_tx = match self.players.players.get(&player_id) {
            Some(player)
                if pending.owner.same_identity(&player.owner)
                    && pending.owner.is_arena_live() => player.queue_tx.clone(),
            _ => {
                // Do not leave a caller future parked when its captured
                // owner has been reset or removed.  The action itself is
                // stale, but its completer still needs an explicit
                // cancellation result.
                if let Some(completer) = pending.completer {
                    async_std::task::spawn_local(async move {
                        completer.complete(Err(super::cancelled_scope_error())).await;
                    });
                }
                if let Some(sender) = pending.event_sender {
                    let _ = sender.try_send(Err(super::cancelled_scope_error()));
                }
                return;
            }
        };
        // Wake an idle owner loop once when this player first becomes
        // pending.  Re-retaining the same external action during a pump does
        // not enqueue an unbounded stream of PumpPending messages.
        let should_wake = !self
            .pending_commands
            .iter()
            .any(|queued| queued.player_id == player_id);
        self.pending_commands.push(pending);
        if wake && should_wake {
            let _ = queue_tx.try_send(PlayerVMExecutionItem {
                command: super::commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
    }

    pub(crate) fn take_completed_score_defaults(
        &mut self,
    ) -> Vec<(
        PlayerId,
        crate::player::score::BehaviorDefaultsContinuation,
        Result<DatumRef, ScriptError>,
    )> {
        std::mem::take(&mut self.completed_score_defaults)
    }

    pub(crate) fn take_pending_commands(&mut self) -> Vec<PendingCommand> {
        std::mem::take(&mut self.pending_commands)
    }

    /// Remove only one owner's pending commands for an external executor.
    /// Other players remain queued in the session, so a host pump cannot
    /// accidentally resume a different player's continuation.
    pub(crate) fn take_pending_commands_for(&mut self, player_id: PlayerId) -> Vec<PendingCommand> {
        let mut selected = Vec::new();
        let mut retained = Vec::new();
        for command in self.take_pending_commands() {
            if command.player_id == player_id {
                selected.push(command);
            } else {
                retained.push(command);
            }
        }
        self.pending_commands = retained;
        selected
    }

    /// Remove only newly runnable work for one owner. Actions marked started
    /// are external waits and remain parked until their exact ticket is
    /// submitted through `service_pending_commands`.
    pub(crate) fn take_ready_pending_commands_for(&mut self, player_id: PlayerId) -> Vec<PendingCommand> {
        let mut selected = Vec::new();
        let mut retained = Vec::new();
        for command in self.take_pending_commands() {
            if command.player_id == player_id && !command.started {
                selected.push(command);
            } else {
                retained.push(command);
            }
        }
        self.pending_commands = retained;
        selected
    }

    pub(crate) fn take_ready_pending_command_for(
        &mut self,
        player_id: PlayerId,
    ) -> Option<PendingCommand> {
        let index = self
            .pending_commands
            .iter()
            .position(|pending| pending.player_id == player_id && !pending.started)?;
        Some(self.pending_commands.remove(index))
    }

    pub(crate) fn update_started_pending_command(
        &mut self,
        player_id: PlayerId,
        ticket: &CompletionTicket,
        action: PendingAction,
    ) {
        if let Some(pending) = self.pending_commands.iter_mut().find(|pending| {
            pending.player_id == player_id
                && pending.started
                && pending.ticket.as_ref().is_some_and(|old| old.same_identity(ticket))
        }) {
            pending.ticket = Some(action.ticket().clone());
            pending.action = Some(action);
        }
    }


    pub(crate) fn update_started_pending_command_with_completion(
        &mut self,
        player_id: PlayerId,
        ticket: &CompletionTicket,
        action: PendingAction,
        completion: ChildCompletion,
    ) {
        if let Some(pending) = self.pending_commands.iter_mut().find(|pending| {
            pending.player_id == player_id
                && pending.started
                && pending.ticket.as_ref().is_some_and(|old| old.same_identity(ticket))
        }) {
            pending.ticket = Some(action.ticket().clone());
            pending.action = Some(action);
            pending.child_completion = Some(completion);
        }
    }

    pub(crate) fn has_pending_commands(&self, player_id: PlayerId) -> bool {
        self.pending_commands.iter().any(|pending| pending.player_id == player_id)
    }

    pub(crate) fn has_ready_pending_commands(&self, player_id: PlayerId) -> bool {
        self.pending_commands.iter().any(|pending| {
            pending.player_id == player_id
                && !pending.started
        })
    }

    pub(crate) fn has_pending_eval_requests(&self, player_id: PlayerId) -> bool {
        self.pending_eval_requests
            .iter()
            .any(|pending| pending.player_id == player_id)
    }

    /// Detach one evaluator request for this owner. Detaching a single
    /// capability lets the scheduler run nested requests concurrently while
    /// an outer host action is awaiting completion.
    pub(crate) fn take_pending_eval_request_for(
        &mut self,
        player_id: PlayerId,
    ) -> Option<PendingEvalRequest> {
        let index = self
            .pending_eval_requests
            .iter()
            .position(|pending| pending.player_id == player_id)?;
        let pending = self.pending_eval_requests.swap_remove(index);
        self.inflight_eval_routes.push(InflightEvalRoute {
            id: pending.id.clone(),
            player_id: pending.player_id,
            owner: pending.owner.clone(),
            action: pending.action.clone(),
            sender: pending.sender.clone(),
        });
        Some(pending)
    }

    /// Accept a host result for the explicit owner-bound action. The command
    /// loop is woken through that player's queue, while the payload remains
    /// session-owned until `service_pending_commands` validates its ticket.
    pub(crate) fn submit_pending_command_completion(
        &mut self,
        player_id: PlayerId,
        ticket: CompletionTicket,
        completion: ActionCompletion,
    ) {
        // A host may complete an action before the command loop has retained
        // its PendingCommand.  The registry entry is the authority for that
        // early-completion case.  Once exact cancellation retires the ticket,
        // reject late results instead of retaining an unmatchable completion.
        if self.actions.details(&ticket).is_none()
            || self.actions.player_id(&ticket) != Some(player_id)
        {
            return;
        }
        self.pending_completions.push((player_id, ticket, completion));
        if let Some(player) = self.players.players.get(&player_id) {
            let _ = player.queue_tx.try_send(PlayerVMExecutionItem {
                command: super::commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
    }

    /// Match host completions to retained actions and resume each matched
    /// continuation. Unmatched pending actions remain retained, so a host can
    /// deliver a result before the command loop has stored the action.
    pub(crate) fn service_pending_commands(&mut self, player_id: PlayerId) -> Vec<super::commands::CommandTurn> {
        let mut turns = Vec::new();
        let mut retained = Vec::new();
        let mut pending = self.take_pending_commands();
        for command in pending.drain(..) {
            if command.player_id != player_id {
                retained.push(command);
                continue;
            }
            let Some(action) = command.action.as_ref() else {
                retained.push(command);
                continue;
            };
            let match_index = self.pending_completions.iter().position(|(player_id, ticket, _)| {
                *player_id == command.player_id && action.ticket().same_identity(ticket)
            });
            let Some(index) = match_index else {
                retained.push(command);
                continue;
            };
            let (_, _, completion) = self.pending_completions.swap_remove(index);
            turns.push(self.resume_pending_command(command, Some(completion)));
        }
        self.pending_commands = retained;
        turns
    }

    /// Resume a retained command after the host completes its exact action.
    /// Owner validation happens before the ticket is consumed, so replacement
    /// sessions and stale completions leave the pending action intact.
    pub(crate) fn resume_pending_command(
        &mut self,
        mut pending: PendingCommand,
        completion: Option<ActionCompletion>,
    ) -> super::commands::CommandTurn {
        let Some(current_owner) = self.players.players.get(&pending.player_id).map(|p| p.owner.clone()) else {
            return super::commands::CommandTurn::Pending(pending);
        };
        if !pending.owner.same_identity(&current_owner) || !pending.owner.is_arena_live() {
            return super::commands::CommandTurn::Pending(pending);
        }
        if let Some(eval_id) = pending.eval_child.clone() {
            let child_turn = match (completion, pending.ticket.clone()) {
                (Some(completion), Some(ticket)) => {
                    self.complete_eval_child_action(eval_id.clone(), ticket, completion)
                }
                (None, _) => self.turn_eval_child(eval_id.clone()),
                (Some(_), None) => None,
            };
            let Some(child_turn) = child_turn else {
                return super::commands::CommandTurn::Pending(pending);
            };
            return match child_turn {
                EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Complete(result)) => {
                    if let Some(sender) = pending.eval_sender.take() {
                        let _ = sender.try_send(result);
                    }
                    super::commands::CommandTurn::Continue
                }
                EvalRequestTurn::Evaluator(crate::player::eval::EvalTurn::Pending { request }) => {
                    if let Some(sender) = pending.eval_sender.take() {
                        self.retain_pending_eval_request(
                            eval_id.clone(),
                            pending.player_id,
                            pending.owner,
                            request,
                            sender,
                        );
                        super::commands::CommandTurn::Continue
                    } else {
                        super::commands::CommandTurn::Pending(pending)
                    }
                }
                EvalRequestTurn::Child(DriverTurn::Waiting) => {
                    pending.action = None;
                    pending.ticket = None;
                    // The completed child has no external action left to
                    // await.  Mark the retained command runnable so the
                    // owner pump can perform its next cooperative turn.
                    pending.started = false;
                    super::commands::CommandTurn::Waiting(pending)
                }
                EvalRequestTurn::Child(DriverTurn::Pending(action)) => {
                    pending.ticket = Some(action.ticket().clone());
                    pending.action = Some(action);
                    pending.started = false;
                    super::commands::CommandTurn::Pending(pending)
                }
                EvalRequestTurn::Child(DriverTurn::Complete(_))
                | EvalRequestTurn::Child(DriverTurn::Error(_)) => {
                    super::commands::CommandTurn::Pending(pending)
                }
                // Movie work is resumed by the owner command pump, not by a
                // child-driver completion. These variants are not emitted by
                // the child driver (typed host work is represented as an
                // InternalVmRequest/PendingAction), but fail explicitly if a
                // future dispatcher routes one here. Retaining the command
                // forever would leak its action ticket and leave the callback
                // receiver waiting indefinitely.
                EvalRequestTurn::MovieAsync(_)
                | EvalRequestTurn::Flash(_)
                | EvalRequestTurn::ExternalXtra(_)
                | EvalRequestTurn::ExternalXtraLoad(_)
                | EvalRequestTurn::XtraPending(_) => {
                    let error = ScriptError::new(
                        "typed host request escaped the child action executor".to_owned(),
                    );
                    if let Some(child_id) = pending.eval_child.take() {
                        if let Some(mut child) = self.eval_drivers.remove(&child_id) {
                            self.release_eval_broadcast_guard(&mut child);
                            let _ = child.driver.cancel(self);
                        }
                        self.send_owned_callback_result(&child_id, Err(error.clone()));
                        self.evals.remove(&child_id);
                    }
                    super::commands::CommandTurn::Complete(
                        Err(error),
                        pending.completer,
                    )
                }
            };
        } 
        if let (Some(completion), Some(ticket)) = (completion, pending.ticket.clone()) {
            if !self.complete_handler_action(ticket, completion) {
                return super::commands::CommandTurn::Pending(pending);
            }
            pending.action = None;
            pending.ticket = None;
        }
        // `Waiting` is the driver's cooperative one-op turn, not an external
        // wait. Keep advancing the same owner-bound driver until it reaches a
        // real pending action or terminal result; retaining `action: None`
        // would block the command queue forever because no host ticket can
        // wake it.
        loop {
            match self.turn_handler(pending.player_id) {
                Some(DriverTurn::Complete(scope)) => {
                    if let Some(sender) = pending.event_sender.take() {
                        let _ = sender.try_send(Ok(scope));
                        break super::commands::CommandTurn::Continue;
                    }
                    let returned = if let Some(child_completion) = pending.child_completion.take() {
                        self.apply_child_completion(pending.player_id, child_completion, scope.return_value)
                    } else {
                        Ok(scope.return_value)
                    };
                    if let Some(continuation) = pending.score_continuation.take() {
                        self.completed_score_defaults.push((
                            pending.player_id,
                            continuation,
                            returned.clone(),
                        ));
                    }
                    break super::commands::CommandTurn::Complete(
                        returned,
                        pending.completer,
                    )
                }
                Some(DriverTurn::Error(error)) => {
                    if let Some(sender) = pending.event_sender.take() {
                        let _ = sender.try_send(Err(error));
                        break super::commands::CommandTurn::Continue;
                    }
                    if let Some(continuation) = pending.score_continuation.take() {
                        self.completed_score_defaults.push((
                            pending.player_id,
                            continuation,
                            Err(error.clone()),
                        ));
                    }
                    break super::commands::CommandTurn::Complete(
                        Err(error),
                        pending.completer,
                    )
                }
                // A waiting command without an action is a cooperative turn
                // (for example a jump or a driver that is waiting for its
                // owning loop to become available).  Return to the owner
                // pump after one turn so a command future cannot monopolize
                // the executor; the next pump will try the driver again.
                Some(DriverTurn::Waiting) => {
                    pending.started = false;
                    break super::commands::CommandTurn::Waiting(pending)
                }
                Some(DriverTurn::Pending(action)) => {
                    pending.ticket = Some(action.ticket().clone());
                    pending.action = Some(action);
                    pending.started = false;
                    break super::commands::CommandTurn::Pending(pending);
                }
                None => {
                    break super::commands::CommandTurn::Complete(
                        Err(super::ScriptError::new(
                            "command handler driver disappeared".to_owned(),
                        )),
                        pending.completer,
                    )
                }
            }
        }
    }

    pub(crate) fn apply_child_completion(
        &mut self,
        player_id: PlayerId,
        completion: super::driver::ChildCompletion,
        value: DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        match completion {
            super::driver::ChildCompletion::ConstructScript { fallback } => self
                .with_player(player_id, |context| {
                    super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                        context.player,
                        context.symbols,
                        fallback,
                        value,
                    )
                })
                .ok_or_else(super::cancelled_scope_error)
                .and_then(|result| result),
            super::driver::ChildCompletion::TimeoutNew { timeout_name, fallback } => self
                .with_player(player_id, |context| {
                    super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                        context.player,
                        context.symbols,
                        timeout_name,
                        fallback,
                        value,
                    )
                })
                .ok_or_else(super::cancelled_scope_error)
                .and_then(|result| result),
            super::driver::ChildCompletion::TimeoutForget { timeout_name } => self
                .with_player(player_id, |context| {
                    super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                        context.player,
                        context.symbols,
                        timeout_name,
                        value,
                    )
                })
                .ok_or_else(super::cancelled_scope_error)
                .and_then(|result| result),
        }
    }



    pub fn with_player<R>(
        &mut self,
        id: PlayerId,
        f: impl FnOnce(ExecutionContext<'_>) -> R,
    ) -> Option<R> {
        let player = self.players.get_mut(id)?;
        Some(f(ExecutionContext {
            player_id: id,
            symbols: &mut self.symbols,
            player,
        }))
    }

    pub(crate) fn enter_static_event_guard(
        &mut self,
        player_id: PlayerId,
        call: &super::driver::StaticEventCall,
    ) -> Result<Option<super::driver::StaticEventGuard>, ScriptError> {
        let owner = self
            .with_player(player_id, |context| context.player.owner.clone())
            .ok_or_else(super::cancelled_scope_error)?;
        let registration_id = self.next_static_event_guard_id;
        self.next_static_event_guard_id = self.next_static_event_guard_id.wrapping_add(1).max(1);
        let already_active = self.with_player(player_id, |mut context| {
            let handler_name = context
                .symbols
                .display(&call.handler_name)
                .map_err(|_| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "foreign or stale static event symbol".to_owned(),
                    )
                })?
                .to_owned();
            let mut already_active = false;
            for (member_ref, active_name, active_args) in
                &context.player.active_static_event_handlers
            {
                if *member_ref != call.member_ref
                    || active_name != &handler_name
                    || active_args.len() != call.args.len()
                {
                    continue;
                }
                let mut same_args = true;
                for (active, current) in active_args.iter().zip(call.args.iter()) {
                    let active_text = crate::player::datum_formatting::format_datum(
                        active,
                        context.symbols,
                        context.player,
                    )?;
                    let current_text = crate::player::datum_formatting::format_datum(
                        current,
                        context.symbols,
                        context.player,
                    )?;
                    if active_text != current_text {
                        same_args = false;
                        break;
                    }
                }
                if same_args {
                    already_active = true;
                    break;
                }
            }
            Ok::<(bool, String), ScriptError>((already_active, handler_name))
        })
        .ok_or_else(super::cancelled_scope_error)??;
        if already_active.0 {
            return Ok(None);
        }
        let guard = super::driver::StaticEventGuard {
            owner,
            registration_id,
            member_ref: call.member_ref.clone(),
            handler_name: already_active.1,
            args: call.args.clone(),
        };
        self.with_player(player_id, |context| {
            context.player.active_static_event_handlers.push((
                guard.member_ref.clone(),
                guard.handler_name.clone(),
                guard.args.clone(),
            ));
        })
        .ok_or_else(super::cancelled_scope_error)?;
        self.static_event_guards
            .entry(player_id)
            .or_default()
            .push(guard.clone());
        Ok(Some(guard))
    }

    pub(crate) fn release_static_event_guard(
        &mut self,
        player_id: PlayerId,
        guard: &super::driver::StaticEventGuard,
    ) -> bool {
        let Some(player_owner) = self.with_player(player_id, |context| context.player.owner.clone()) else {
            return false;
        };
        if !player_owner.same_identity(&guard.owner) || !guard.owner.is_arena_live() {
            return false;
        }
        let Some(records) = self.static_event_guards.get_mut(&player_id) else {
            return false;
        };
        let Some(index) = records.iter().rposition(|record| {
            record.registration_id == guard.registration_id
                && record.owner.same_identity(&guard.owner)
        }) else {
            return false;
        };
        let record = records.remove(index);
        if records.is_empty() {
            self.static_event_guards.remove(&player_id);
        }
        self.with_player(player_id, |context| {
            let Some(index) = context.player.active_static_event_handlers.iter().rposition(
                |(member_ref, handler_name, args)| {
                    *member_ref == record.member_ref
                        && handler_name == &record.handler_name
                        && args == &record.args
                },
            ) else {
                return false;
            };
            context.player.active_static_event_handlers.remove(index);
            true
        })
        .unwrap_or(false)
    }

    pub(crate) fn static_event_stopped(&mut self, player_id: PlayerId) -> bool {
        self.with_player(player_id, |context| context.player.event_stopped)
            .unwrap_or(false)
    }

    /// Run a JS bridge operation with the session-owned runtime registry.
    /// General VM contexts intentionally do not carry this unrelated state.
    pub(crate) fn with_player_js<R>(
        &mut self,
        id: PlayerId,
        f: impl FnOnce(ExecutionContext<'_>, &mut JsRuntimeRegistry) -> R,
    ) -> Option<R> {
        let player = self.players.get_mut(id)?;
        Some(f(
            ExecutionContext {
                player_id: id,
                symbols: &mut self.symbols,
                player,
            },
            &mut self.js_lingo,
        ))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn js_lingo_registry_mut(&mut self) -> &mut JsRuntimeRegistry {
        &mut self.js_lingo
    }

    pub(crate) fn js_lingo_registry(&self) -> &JsRuntimeRegistry {
        &self.js_lingo
    }

    pub(crate) fn clear_js_lingo_for_player(&mut self, id: PlayerId) {
        if let Some(player) = self.players.players.get(&id) {
            self.js_lingo.clear_player(id, &player.owner);
        }
    }
    pub fn reset_movie_symbols(&mut self) {
        self.symbols.reset_movie_display_claims();
    }

    pub(crate) fn take_completed_property_casts_for(
        &mut self,
        player_id: PlayerId,
    ) -> Vec<CompletionTicket> {
        let mut selected = Vec::new();
        let mut retained = Vec::new();
        for (id, ticket) in std::mem::take(&mut self.completed_property_casts) {
            if id == player_id {
                selected.push(ticket);
            } else {
                retained.push((id, ticket));
            }
        }
        self.completed_property_casts = retained;
        selected
    }

    pub(crate) fn has_pending_cast_load(&self, request: &CastLoadRequest) -> bool {
        self.pending_casts.iter().any(|pending| {
            pending.request.owner_key() == request.owner_key()
                && pending.request.source_path() == request.source_path()
                && pending.request.cast_number() == request.cast_number()
        })
    }

    pub(crate) fn mark_cast_load_started(&mut self, request: &CastLoadRequest) -> bool {
        self.pending_casts.iter().any(|pending| {
            pending.request.owner_key() == request.owner_key()
                && pending.request.source_path() == request.source_path()
                && pending.request.cast_number() == request.cast_number()
                && matches!(pending.state, PendingCastState::Waiting)
        })
    }

    /// Start the canonical session-owned driver. The old ambient trampoline is
    /// deliberately not used as an adapter; root command/event callers are
    /// migrated to this API in the following owner-threading increment.
    pub(crate) fn begin_property_cast_load(
        &mut self,
        id: PlayerId,
        request: CastLoadRequest,
        ticket: CompletionTicket,
    ) -> Result<Option<CastLoadRequest>, ScriptError> {
        let Some(player) = self.players.players.get(&id) else {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "property load player was removed".to_owned(),
            ));
        };
        if !player.owner.is_arena_live()
            || player.owner.key() != request.owner_key()
            || player
                .movie
                .cast_manager
                .get_cast_or_null(request.cast_number())
                .is_none()
        {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "property load owner or cast was replaced".to_owned(),
            ));
        }
        let cached = player.dir_cache.get(request.source_path()).cloned();
        let cache_hit = cached.is_some();
        self.pending_casts.push(PendingCastLoad {
            request: request.clone(),
            owner: player.owner.clone(),
            state: cached
                .map(PendingCastState::ReadyCached)
                .unwrap_or(PendingCastState::Waiting),
            purpose: PendingCastPurpose::Property { ticket },
        });
        if cache_hit {
            let mut generated = CastNotificationOutbox::default();
            self.drain_ready_casts(id, &mut generated);
            self.publish_pending_cast_notifications(id, &mut generated);
            Ok(None)
        } else {
            Ok(Some(request))
        }
    }

    pub(crate) fn take_completed_property_casts(&mut self) -> Vec<(PlayerId, CompletionTicket)> {
        std::mem::take(&mut self.completed_property_casts)
    }

    pub(crate) fn start_handler(
        &mut self,
        id: PlayerId,
        receiver: Option<super::script_ref::ScriptInstanceRef>,
        handler_ref: super::script::ScriptHandlerRef,
        args: &[super::datum_ref::DatumRef],
        use_raw_arg_list: bool,
    ) -> Result<Option<super::scope::ScopeResult>, super::ScriptError> {
        if self.drivers.contains_key(&id) {
            return Err(super::ScriptError::new("handler already running for player".to_owned()));
        }
        match DriverContinuation::start(self, id, receiver, handler_ref, args, use_raw_arg_list)? {
            DriverStart::Early(result) => Ok(Some(result)),
            DriverStart::Running(driver) => {
                self.drivers.insert(id, driver);
                Ok(None)
            }
        }
    }

    /// Classify one global invocation while the target player and this
    /// session's symbol table are borrowed together. Script handlers become
    /// child frames; virtual and synchronous builtins return their result;
    /// asynchronous handlers remain owned by the caller's pending request.
    ///
    /// The ordering mirrors `player_call_global_handler`: constructor birth,
    /// script lookup (with `new` excluded), first-argument receiver, cached
    /// movie handler, movie/frame/global scripts, active stage instances,
    /// virtual globals, async builtins, async Xtras, then the sync manager.
    pub(crate) fn dispatch_global(
        &mut self,
        id: PlayerId,
        name: &Symbol,
        args: &[DatumRef],
    ) -> Result<GlobalDispatch, ScriptError> {
        self.dispatch_global_inner(id, name, args, true)
    }

    /// Redispatch a global after an external Xtra probe declined it. The
    /// bypass is one-shot, preserving ordinary builtin/script precedence.
    pub(crate) fn dispatch_global_after_external_probe(
        &mut self,
        id: PlayerId,
        name: &Symbol,
        args: &[DatumRef],
    ) -> Result<GlobalDispatch, ScriptError> {
        self.dispatch_global_inner(id, name, args, false)
    }

    /// Prepare the asynchronous movie verbs which can reach the global
    /// pending boundary. The request owns the validated arguments and owner
    /// capability, so the command/driver executor can run it after this
    /// session borrow ends.
    fn prepare_async_global_request_in_context(
        context: &mut ExecutionContext<'_>,
        id: PlayerId,
        name: &Symbol,
        args: &[DatumRef],
    ) -> Result<Option<super::driver::InternalVmRequest>, ScriptError> {
        let kind = match name.clone().into_builtin() {
            Some(BuiltInSymbol::Do) => super::handlers::movie::MovieAsyncKind::Do,
            Some(BuiltInSymbol::UpdateStage) => {
                super::handlers::movie::MovieAsyncKind::UpdateStage {
                    now_ms: crate::player::bench_now_ms().max(0.0),
                }
            }
            Some(BuiltInSymbol::Go) => super::handlers::movie::MovieAsyncKind::Go,
            Some(BuiltInSymbol::Play) => super::handlers::movie::MovieAsyncKind::Play,
            Some(BuiltInSymbol::Nothing) => super::handlers::movie::MovieAsyncKind::Nothing,
            _ => return Ok(None),
        };
        let request = super::handlers::movie::MovieHandlers::prepare_movie_async(
            context,
            id,
            kind,
            args,
        )?;
        Ok(Some(super::driver::InternalVmRequest::MovieAsync(request)))
    }

    pub(crate) fn prepare_async_global_request(
        &mut self,
        id: PlayerId,
        name: &Symbol,
        args: &[DatumRef],
    ) -> Result<Option<super::driver::InternalVmRequest>, ScriptError> {
        let request = self
            .with_player(id, |mut context| {
                Self::prepare_async_global_request_in_context(&mut context, id, name, args)
            })
            .ok_or_else(super::cancelled_scope_error)??;
        Ok(request)
    }

    fn dispatch_global_inner(
        &mut self,
        id: PlayerId,
        name: &Symbol,
        args: &[DatumRef],
        allow_external_probe: bool,
    ) -> Result<GlobalDispatch, ScriptError> {
        self.with_player(id, |mut context| {
            if !context.player.owner.is_arena_live() {
                return Err(super::cancelled_scope_error());
            }
            let display = context
                .symbols
                .display(name)
                .map_err(|_| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale global handler symbol".to_owned(),
                ))?
                .to_owned();

            // Director 6's `birth(script, ...)` constructor has priority over
            // every static handler with the same name. Prepare it through the
            // same owned child route as `new`; a bytecode constructor remains
            // suspended until its child result is applied.
            if display.eq_ignore_ascii_case("birth") && !args.is_empty() {
                let first = checked_internal_datum(context.player, context.symbols, &args[0])?;
                if matches!(first, Datum::ScriptRef(_)) {
                    match super::handlers::datum_handlers::script::ScriptDatumHandlers::prepare_constructor_named(
                        context.player,
                        context.symbols,
                        &args[0],
                        &args[1..],
                        name.clone(),
                    )? {
                        super::handlers::datum_handlers::script::ScriptConstructorPlan::Complete(result) => {
                            return Ok(GlobalDispatch::SyncResult(Ok(result)));
                        }
                        super::handlers::datum_handlers::script::ScriptConstructorPlan::Child {
                            receiver,
                            handler_ref,
                            args,
                            fallback,
                        } => {
                            return Ok(GlobalDispatch::ChildWithCompletion {
                                receiver: Some(receiver),
                                handler_ref,
                                args,
                                completion: super::driver::ChildCompletion::ConstructScript { fallback },
                            });
                        }
                    }
                }
            }

            // `new` must skip script lookup, but virtual movie globals still
            // precede the asynchronous builtin implementation below.
            if !name.eq_builtin(BuiltInSymbol::New)
                && context
                    .player
                    .movie
                    .cast_manager
                    .any_script_defines_handler(name.clone())
            {
                if let Some(first_arg) = args.first() {
                    // Keep this check inside the script-name gate so cheap
                    // builtin calls retain their legacy argument laziness.
                    checked_internal_datum(context.player, context.symbols, first_arg)?;
                }
                if let Some((receiver, handler_ref)) =
                    ScriptInstanceUtils::get_handler_from_first_arg(
                        context.player,
                        context.symbols,
                        args,
                        name,
                    )?
                {
                    return Ok(GlobalDispatch::Child {
                        receiver,
                        handler_ref,
                    });
                }

                if let Some(handler_ref) = context
                    .player
                    .movie
                    .cast_manager
                    .movie_handler_ref(name.clone())
                {
                    return Ok(GlobalDispatch::Child {
                        receiver: None,
                        handler_ref,
                    });
                }

                if let Some(script) = context
                    .player
                    .movie
                    .cast_manager
                    .find_movie_script_with_handler(name.clone())
                {
                    if let Some(handler_ref) = script.get_own_handler_ref(name.clone()) {
                        return Ok(GlobalDispatch::Child {
                            receiver: None,
                            handler_ref,
                        });
                    }
                }

                if let Some(frame_script) = context
                    .player
                    .movie
                    .score
                    .get_script_in_frame(context.player.movie.current_frame)
                {
                    let script_ref = CastMemberRef {
                        cast_lib: frame_script.cast_lib.into(),
                        cast_member: frame_script.cast_member.into(),
                    };
                    if let Some(script) = context
                        .player
                        .movie
                        .cast_manager
                        .get_script_by_ref(&script_ref)
                    {
                        if let Some(handler_ref) = script.get_own_handler_ref(name.clone()) {
                            return Ok(GlobalDispatch::Child {
                                receiver: None,
                                handler_ref,
                            });
                        }
                    }
                }

                // Clone the refs before resolving them so an invalid global
                // datum is reported as InvalidReference rather than being
                // silently skipped or dereferenced through an ambient player.
                let global_refs: Vec<DatumRef> =
                    context.player.globals.values().cloned().collect();
                for datum_ref in global_refs {
                    let datum = checked_internal_datum(context.player, context.symbols, &datum_ref)?;
                    if let Datum::VarRef(VarRef::Script(script_ref)) = datum {
                        if let Some(script) = context
                            .player
                            .movie
                            .cast_manager
                            .get_script_by_ref(script_ref)
                        {
                            if let Some(handler_ref) = script.get_own_handler_ref(name.clone()) {
                                return Ok(GlobalDispatch::Child {
                                    receiver: None,
                                    handler_ref,
                                });
                            }
                        }
                    }
                }

                let active_instances = active_stage_script_instance_ids_checked(&mut context)?;
                for instance_ref in active_instances {
                    let instance = context
                        .player
                        .allocator
                        .get_script_instance_opt(&instance_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                format!("stale active stage instance {}", instance_ref),
                            )
                        })?;
                    let script = context
                        .player
                        .movie
                        .cast_manager
                        .get_script_by_ref(&instance.script)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                format!(
                                    "active stage instance {} references missing script {}:{}",
                                    instance_ref, instance.script.cast_lib, instance.script.cast_member
                                ),
                            )
                        })?;
                    if let Some(handler_ref) = script.get_own_handler_ref(name.clone()) {
                        return Ok(GlobalDispatch::Child {
                            receiver: Some(instance_ref),
                            handler_ref,
                        });
                    }
                }
            }

            let virtual_handlers: Vec<_> = context.player.virtual_scripts.values().cloned().collect();
            for virtual_handler in virtual_handlers {
                if virtual_handler.script_type() != crate::director::enums::ScriptType::Movie {
                    continue;
                }
                let expectation = super::SetupExpectation::capture(context.player, None);
                let virtual_result = virtual_handler.call_handler(
                    context.player,
                    context.symbols,
                    None,
                    name.clone(),
                    &args.to_vec(),
                );
                if !expectation.validate(context.player) {
                    return Err(super::cancelled_scope_error());
                }
                match virtual_result? {
                    Some(result) => {
                        checked_internal_datum(context.player, context.symbols, &result)?;
                        return Ok(GlobalDispatch::SyncResult(Ok(result)));
                    }
                    None => {}
                }
            }

            match name.into_builtin() {
                Some(BuiltInSymbol::GetVariable) => {
                    if let Some(request) = super::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_global_bind_get(
                        context.player,
                        context.symbols,
                        args,
                    )? {
                        return Ok(GlobalDispatch::PendingRequest {
                            request: super::driver::InternalVmRequest::Flash(request),
                            reason: "Flash getVariable requires owner/generation-bound host execution".to_owned(),
                        });
                    }
                }
                Some(BuiltInSymbol::Call) => {
                    match BuiltInHandlerManager::prepare_call(&mut context, args)? {
                        super::handlers::manager::CallPreparation::Complete(result) => {
                            return Ok(GlobalDispatch::SyncResult(Ok(result)));
                        }
                        super::handlers::manager::CallPreparation::Broadcast(plan) => {
                            return Ok(GlobalDispatch::ChildSequence { plan });
                        }
                        super::handlers::manager::CallPreparation::Single {
                            receiver,
                            handler_name,
                            args: call_args,
                        } => {
                            match super::handlers::datum_handlers::player_call_datum_handler(
                                &mut context,
                                &receiver,
                                handler_name,
                                &call_args,
                            ) {
                                super::handlers::datum_handlers::DatumDispatch::Sync(result) => {
                                    return Ok(GlobalDispatch::SyncResult(result));
                                }
                                super::handlers::datum_handlers::DatumDispatch::Child {
                                    receiver,
                                    handler_ref,
                                    args,
                                    ..
                                } => return Ok(GlobalDispatch::ChildPrepared {
                                    receiver,
                                    handler_ref,
                                    args,
                                }),
                                super::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
                                    receiver,
                                    handler_ref,
                                    args,
                                    completion,
                                } => return Ok(GlobalDispatch::ChildWithCompletion {
                                    receiver,
                                    handler_ref,
                                    args,
                                    completion,
                                }),
                                super::handlers::datum_handlers::DatumDispatch::Pending { request, reason } => {
                                    return Ok(GlobalDispatch::PendingRequest { request, reason });
                                }
                            }
                        }
                    }
                }
                Some(BuiltInSymbol::New) => {
                    match super::handlers::types::TypeHandlers::prepare_new(&mut context, args)? {
                        super::handlers::types::TypeNewPlan::Complete(result) => {
                            return Ok(GlobalDispatch::SyncResult(Ok(result)));
                        }
                        super::handlers::types::TypeNewPlan::ScriptChild { receiver, handler_ref, args, fallback } => {
                            return Ok(GlobalDispatch::ChildWithCompletion {
                                receiver: Some(receiver),
                                handler_ref,
                                args,
                                completion: ChildCompletion::ConstructScript { fallback },
                            });
                        }
                        super::handlers::types::TypeNewPlan::Xtra { name, args } => {
                            let request = if super::xtra::manager::is_xtra_registered(
                                context.player,
                                &name,
                            ) {
                                if let Some(request) =
                                    super::xtra::external::prepare_create_request(
                                        context.player,
                                        context.symbols,
                                        &name,
                                        &args,
                                    )?
                                {
                                    super::driver::InternalVmRequest::ExternalXtra(request)
                                } else {
                                    return Ok(GlobalDispatch::SyncResult(
                                        super::handlers::types::TypeHandlers::new_explicit(
                                            &mut context,
                                            &args,
                                        ),
                                    ));
                                }
                            } else {
                                let encoded = super::xtra::external::encode_args_from_player(
                                    context.player,
                                    &args,
                                    context.symbols,
                                )?;
                                let load = super::xtra::external::prepare_load_request(
                                    context.player,
                                    super::xtra::external::ExternalXtraContinuation::Create {
                                        xtra_name: name.clone(),
                                        args: encoded,
                                    },
                                )?;
                                super::driver::InternalVmRequest::ExternalXtraLoad(load)
                            };
                            return Ok(GlobalDispatch::PendingRequest {
                                request,
                                reason: format!("new external Xtra '{}'", name),
                            });
                        }
                    }
                }
                Some(BuiltInSymbol::NewObject) => {
                    return Ok(GlobalDispatch::SyncResult(
                        super::handlers::types::TypeHandlers::new_object(&mut context, &args.to_vec()),
                    ));
                }
                Some(BuiltInSymbol::Value) => {
                    return Ok(GlobalDispatch::SyncResult(
                        super::handlers::types::TypeHandlers::value(&mut context, &args.to_vec()),
                    ));
                }
                Some(BuiltInSymbol::CallAncestor) => {
                    match super::handlers::types::TypeHandlers::prepare_call_ancestor(&mut context, args)? {
                        super::handlers::types::AncestorCallPlan::Complete(result) => {
                            return Ok(GlobalDispatch::SyncResult(Ok(result)));
                        }
                        super::handlers::types::AncestorCallPlan::Children(calls) => {
                            return Ok(GlobalDispatch::AncestorChildren { calls });
                        }
                    }
                }
                Some(BuiltInSymbol::SendSprite) => {
                    let plan = BuiltInHandlerManager::prepare_send_sprite(&mut context, args)?;
                    return Ok(GlobalDispatch::ChildSequence { plan });
                }
                Some(BuiltInSymbol::SendAllSprites) => {
                    let plan = BuiltInHandlerManager::prepare_send_all_sprites(&mut context, args)?;
                    return Ok(GlobalDispatch::ChildSequence { plan });
                }
                Some(BuiltInSymbol::ImportFileInto) => {
                    let Some(receiver) = args.first() else {
                        return Err(ScriptError::new(
                            "importFileInto: missing member argument".to_owned(),
                        ));
                    };
                    let first = checked_internal_datum(context.player, context.symbols, receiver)?;
                    if matches!(first, Datum::CastMember(_)) {
                        let request = super::driver::classify_async_object(
                            &mut context,
                            receiver,
                            name,
                            &args[1..],
                        )?;
                        let reason = super::driver::async_request_reason(&request)
                            .unwrap_or_else(|| "importFileInto requires owner-bound executor".to_owned());
                        return Ok(GlobalDispatch::PendingRequest { request, reason });
                    }
                }
                _ => {}
            }

            if let Some(request) = Self::prepare_async_global_request_in_context(
                &mut context,
                id,
                &name,
                args,
            )? {
                return Ok(GlobalDispatch::PendingRequest {
                    request,
                    reason: format!("async builtin global handler {display}"),
                });
            }
            // A global call whose first argument is an owner-bound Multiuser
            // or Curl instance is an object request in Director syntax. Keep
            // the receiver check here, before the sync manager can consume
            // arguments or mutate state, so deferred work remains an owned
            // pending request for the session executor.
            if let Some(receiver_ref) = args.first() {
                let receiver = checked_internal_datum(
                    context.player,
                    context.symbols,
                    receiver_ref,
                )?;
                if let Datum::XtraInstance(xtra_name, _) = receiver {
                    let xtra_name = xtra_name.clone();
                    if matches!(xtra_name.to_ascii_lowercase().as_str(), "multiuser" | "curl" | "fileio" | "xmlparser")
                        && crate::player::xtra::external::is_registered_for_player(
                            context.player,
                            &xtra_name,
                        )
                    {
                        let instance_id = match receiver {
                            Datum::XtraInstance(_, instance_id) => *instance_id,
                            _ => unreachable!("receiver was checked as XtraInstance"),
                        };
                        if let Some(request) = crate::player::xtra::external::prepare_instance_request(
                            context.player,
                            context.symbols,
                            &xtra_name,
                            instance_id,
                            &display,
                            args.get(1..).unwrap_or(&[]),
                        )? {
                            return Ok(GlobalDispatch::PendingRequest {
                                request: super::driver::InternalVmRequest::ExternalXtra(request),
                                reason: format!("external Xtra '{}' instance handler", xtra_name),
                            });
                        }
                    }
                    let deferred =
                        (!crate::player::xtra::external::is_registered_for_player(context.player, &xtra_name)
                            && xtra_name.eq_ignore_ascii_case("multiuser")
                            && (display.eq_ignore_ascii_case("connectToNetServer")
                                || display.eq_ignore_ascii_case("sendNetMessage")))
                            || (!crate::player::xtra::external::is_registered_for_player(context.player, &xtra_name)
                                && xtra_name.eq_ignore_ascii_case("curl")
                                && display.eq_ignore_ascii_case("execAsync"));
                    if deferred {
                        let pending = super::xtra::manager::call_instance_handler_pending_explicit(
                            context.player,
                            context.symbols,
                            &xtra_name,
                            receiver_ref,
                            &display,
                            args.get(1..).unwrap_or(&[]),
                        )?;
                        match pending {
                            super::xtra::manager::XtraPendingOrValue::Pending(request) => {
                                return Ok(GlobalDispatch::PendingRequest {
                                    request: super::driver::InternalVmRequest::XtraPending(request),
                                    reason: format!("deferred {} handler on owner-bound {} Xtra instance", display, xtra_name),
                                });
                            }
                            super::xtra::manager::XtraPendingOrValue::Value(value) => {
                                return Ok(GlobalDispatch::SyncResult(Ok(value)));
                            }
                        }
                    }
                    if matches!(xtra_name.to_ascii_lowercase().as_str(), "fileio" | "xmlparser") {
                        let pending = super::xtra::manager::call_instance_handler_pending_explicit(
                            context.player,
                            context.symbols,
                            &xtra_name,
                            receiver_ref,
                            &display,
                            args.get(1..).unwrap_or(&[]),
                        )?;
                        match pending {
                            super::xtra::manager::XtraPendingOrValue::Pending(request) => {
                                return Ok(GlobalDispatch::PendingRequest {
                                    request: super::driver::InternalVmRequest::XtraPending(request),
                                    reason: format!("FileIO/XMLParser {} handler", display),
                                });
                            }
                            super::xtra::manager::XtraPendingOrValue::Value(value) => {
                                return Ok(GlobalDispatch::SyncResult(Ok(value)));
                            }
                        }
                    }
                }
            }

            // Probe host plugins only after all ordinary high-priority routes
            // have declined. The executor preserves raw args until a plugin
            // claims the handler, so unrelated builtins keep lazy conversion.
            if allow_external_probe {
                if let Some(request) = super::xtra::external::prepare_static_probe(
                    context.player,
                    context.symbols,
                    name.clone(),
                    &display,
                    args,
                )? {
                    return Ok(GlobalDispatch::PendingRequest {
                        request: super::driver::InternalVmRequest::ExternalXtra(request),
                        reason: format!("external Xtra static handler {}", display),
                    });
                }
            }

            // OpenURL and BudAPI browser effects carry owned arguments into
            // the host executor. This keeps window/clipboard calls outside
            // the RuntimeSession borrow while retaining external-plugin
            // precedence above these built-ins.
            if let Some(outcome) = super::xtra::manager::prepare_openurl_handler_explicit(
                context.player,
                context.symbols,
                &display,
                args,
            ) {
                return match outcome? {
                    super::xtra::manager::XtraPendingOrValue::Value(value) =>
                        Ok(GlobalDispatch::SyncResult(Ok(value))),
                    super::xtra::manager::XtraPendingOrValue::Pending(request) =>
                        Ok(GlobalDispatch::PendingRequest {
                            request: super::driver::InternalVmRequest::XtraPending(request),
                            reason: format!("OpenURL {} host effect", display),
                        }),
                };
            }
            if let Some(outcome) = super::xtra::manager::prepare_budapi_handler_explicit(
                context.player,
                context.symbols,
                &display,
                args,
            ) {
                return match outcome? {
                    super::xtra::manager::XtraPendingOrValue::Value(value) =>
                        Ok(GlobalDispatch::SyncResult(Ok(value))),
                    super::xtra::manager::XtraPendingOrValue::Pending(request) =>
                        Ok(GlobalDispatch::PendingRequest {
                            request: super::driver::InternalVmRequest::XtraPending(request),
                            reason: format!("BudAPI {} host effect", display),
                        }),
                };
            }

            // SysMenu is owner-local. Menu mutations complete synchronously;
            // print/message-box effects become typed host intents only after
            // external-plugin precedence has declined, so the browser call
            // never runs while this RuntimeSession borrow is held.
            if let Some(outcome) = super::xtra::manager::prepare_sysmenu_handler_explicit(
                context.player,
                context.symbols,
                &display,
                args,
            ) {
                return match outcome? {
                    super::xtra::manager::XtraPendingOrValue::Value(value) =>
                        Ok(GlobalDispatch::SyncResult(Ok(value))),
                    super::xtra::manager::XtraPendingOrValue::Pending(request) =>
                        Ok(GlobalDispatch::PendingRequest {
                            request: super::driver::InternalVmRequest::XtraPending(request),
                            reason: format!("SysMenu {} host effect", display),
                        }),
                };
            }

            let expectation = super::SetupExpectation::capture(context.player, None);
            let result =
                BuiltInHandlerManager::call_handler(&mut context, name.clone(), &args.to_vec());
            if !expectation.validate(context.player) {
                return Err(super::cancelled_scope_error());
            }
            if let Ok(result_ref) = &result {
                checked_internal_datum(context.player, context.symbols, result_ref)?;
            }
            Ok(GlobalDispatch::SyncResult(result))
        })
        .ok_or_else(super::cancelled_scope_error)?
    }

    /// Advance one synchronous turn. Pending actions are returned to the owner
    /// loop, which can await host work without borrowing this session.
    pub(crate) fn turn_handler(&mut self, id: PlayerId) -> Option<DriverTurn> {
        let mut driver = self.drivers.remove(&id)?;
        let turn = driver.turn(self);
        if matches!(turn, DriverTurn::Waiting | DriverTurn::Pending(_)) {
            self.drivers.insert(id, driver);
        } else {
            // A terminal turn drops its frame stack. Retire any action still
            // keyed to that exact owner so a late completion cannot accumulate
            // after the driver has finished or failed.
            let owner = driver.owner.clone();
            let _ = driver.cancel(self);
            self.actions.cancel_owner(&owner);
        }
        self.collect_player_host_teardowns(id);
        Some(turn)
    }

    #[cfg(test)]
    pub(crate) fn insert_driver_for_test(&mut self, driver: DriverContinuation) {
        self.drivers.insert(driver.player_id, driver);
    }

    pub(crate) fn allocate_action(
        &mut self,
        owner: &OwnerToken,
        scope: &super::ScopeToken,
        kind: ActionKind,
        resume: ResumePhase,
    ) -> Option<CompletionTicket> {
        self.actions.allocate(owner, Some(scope), kind, resume)
    }

    pub(crate) fn allocate_setup_action(
        &mut self,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
    ) -> Option<CompletionTicket> {
        self.actions.allocate(
            owner,
            scope,
            ActionKind::SetupCallback,
            ResumePhase::SetupCallback,
        )
    }

    pub(crate) fn authorize_action(
        &mut self,
        ticket: &CompletionTicket,
        owner: &OwnerToken,
        scope: &super::ScopeToken,
        expected_kind: ActionKind,
        expected_resume: ResumePhase,
    ) -> Option<(ActionKind, ResumePhase)> {
        self.actions
            .authorize(ticket, owner, Some(scope), expected_kind, expected_resume)
    }

    pub(crate) fn validate_setup_action(
        &self,
        ticket: &CompletionTicket,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
    ) -> bool {
        self.actions.validate(
            ticket,
            owner,
            scope,
            ActionKind::SetupCallback,
            ResumePhase::SetupCallback,
        )
    }

    pub(crate) fn authorize_setup_action(
        &mut self,
        ticket: &CompletionTicket,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
    ) -> Option<(ActionKind, ResumePhase)> {
        self.actions.authorize(
            ticket,
            owner,
            scope,
            ActionKind::SetupCallback,
            ResumePhase::SetupCallback,
        )
    }

    pub(crate) fn action_details(
        &self,
        ticket: &CompletionTicket,
    ) -> Option<(ActionKind, ResumePhase)> {
        self.actions.details(ticket)
    }

    /// Complete exactly one queued action. The ticket carries a private Arc
    /// capability while owner/scope identity are checked against this session.
    pub(crate) fn complete_handler_action(
        &mut self,
        ticket: CompletionTicket,
        completion: ActionCompletion,
    ) -> bool {
        let id = match self.actions.player_id(&ticket) {
            Some(id) => id,
            None => return false,
        };
        let Some(mut driver) = self.drivers.remove(&id) else { return false };
        if !driver.complete(self, ticket, completion) {
            self.drivers.insert(id, driver);
            return false;
        }
        self.drivers.insert(id, driver);
        true
    }

    /// Prepare cast requests synchronously. Cache hits are reserved in cast
    /// order; only the contiguous ready prefix is applied.
    pub fn prepare_cast_loads(
        &mut self,
        id: PlayerId,
        reason: CastPreloadReason,
    ) -> Vec<CastLoadRequest> {
        let mut outbound = Vec::new();
        // A cast reset or replacement can leave an old reservation in the
        // queue. Retire it before the manager checks its per-slot barrier, or
        // the stale requirement would suppress the fresh reservation.
        self.purge_invalid_pending();
        {
            let player = match self.players.get_mut(id) {
                Some(player) => player,
                None => return outbound,
            };
            let owner = player.owner.key();
            let requests = player.movie.cast_manager.prepare_preload_requests(
                reason,
                owner,
                &player.dir_cache,
                player.net_manager.base_path.as_ref(),
                player.net_manager.override_base_path.as_deref(),
            );
            for request in requests {
                let state = match player.dir_cache.get(request.cache_key()).cloned() {
                    Some(file) => PendingCastState::ReadyCached(file),
                    None => {
                        outbound.push(request.clone());
                        PendingCastState::Waiting
                    }
                };
                self.pending_casts.push(PendingCastLoad {
                    request,
                    owner: player.owner.clone(),
                    state,
                    purpose: PendingCastPurpose::Preload,
                });
            }
        }
        let mut generated = CastNotificationOutbox::default();
        self.drain_ready_casts(id, &mut generated);
        self.publish_pending_cast_notifications(id, &mut generated);
        outbound
    }

    fn drain_ready_casts(
        &mut self,
        id: PlayerId,
        generated: &mut CastNotificationOutbox,
    ) -> bool {
        let mut applied_any = false;
        self.purge_invalid_pending();
        loop {
            let Some(position) = self.pending_casts.iter().position(|pending| {
                self.players
                    .players
                    .get(&id)
                    .is_some_and(|player| pending.owner.same_identity(&player.owner))
            }) else {
                break;
            };
            // Revalidate the queue head on every pass. A cast slot can have
            // been replaced or retired since the initial purge, and a stale
            // waiting entry must not block a later ready entry for this owner.
            let request = self.pending_casts[position].request.clone();
            let pending_owner = self.pending_casts[position].owner.clone();
            let property = matches!(
                self.pending_casts[position].purpose,
                PendingCastPurpose::Property { .. }
            );
            let player_id = request.owner_key().player as PlayerId;
            let valid = self.players.players.get(&player_id).is_some_and(|player| {
                player.owner.key() == request.owner_key()
                    && player.owner.same_identity(&pending_owner)
                    && if property {
                        player
                            .movie
                            .cast_manager
                            .get_cast_or_null(request.cast_number())
                            .is_some_and(|cast| cast.is_load_current(&request))
                    } else {
                        player.movie.cast_manager.is_preload_current(&request)
                    }
            });
            if !valid {
                let stale = self.pending_casts.remove(position);
                if let Some(player) = self.players.players.get_mut(&player_id) {
                    if player.owner.same_identity(&stale.owner) {
                        match stale.purpose {
                            PendingCastPurpose::Property { .. } => {
                                player.movie.cast_manager.get_cast_mut(stale.request.cast_number()).cancel_load(&stale.request);
                            }
                            PendingCastPurpose::Preload => {
                                player.movie.cast_manager.retire_preload_requirement(&stale.request);
                            }
                        }
                    }
                }
                continue;
            }
            if matches!(self.pending_casts[position].state, PendingCastState::Waiting) {
                break;
            }
            let cast_number = request.cast_number();
            let cast_exists = self
                .players
                .players
                .get(&id)
                .is_some_and(|player| {
                    player
                        .movie
                        .cast_manager
                        .get_cast_or_null(cast_number)
                        .is_some()
                });
            if !cast_exists {
                let stale = self.pending_casts.remove(position);
                if let Some(player) = self.players.players.get_mut(&id) {
                    if player.owner.same_identity(&stale.owner) {
                        match stale.purpose {
                            PendingCastPurpose::Property { .. } => {
                                player.movie.cast_manager.get_cast_mut(stale.request.cast_number()).cancel_load(&stale.request);
                            }
                            PendingCastPurpose::Preload => {
                                player.movie.cast_manager.retire_preload_requirement(&stale.request);
                            }
                        }
                    }
                }
                continue;
            }
            let pending = self.pending_casts.remove(position);
            let Some(player) = self.players.get_mut(id) else {
                break;
            };
            let request = pending.request;
            let owner = player.owner.key();
            let property = matches!(
                &pending.purpose,
                PendingCastPurpose::Property { .. }
            );
            let applied = match pending.state {
                PendingCastState::Waiting => unreachable!("waiting cast at ready drain"),
                PendingCastState::ReadyNetwork(result) => player
                    .movie
                    .cast_manager
                    .get_cast_mut(cast_number)
                    .apply_load_result(
                        &request,
                        result,
                        owner,
                        &mut player.bitmap_manager,
                        &mut player.dir_cache,
                        &mut self.symbols,
                        generated,
                    ),
                PendingCastState::ReadyCached(file) => {
                    if property {
                        player
                            .movie
                            .cast_manager
                            .get_cast_mut(cast_number)
                            .apply_cached_property_file(
                                &request,
                                owner,
                                file,
                                &mut player.bitmap_manager,
                                &mut self.symbols,
                                generated,
                            )
                    } else {
                        player
                            .movie
                            .cast_manager
                            .get_cast_mut(cast_number)
                            .apply_cached_file(
                                &request,
                                owner,
                                file,
                                &mut player.bitmap_manager,
                                &mut self.symbols,
                                generated,
                            )
                    }
                }
            };
            if applied {
                match pending.purpose {
                    PendingCastPurpose::Preload => {
                        player.movie.cast_manager.complete_preload(
                            &request,
                            &mut player.bitmap_manager,
                            generated,
                        );
                        applied_any = true;
                    }
                    PendingCastPurpose::Property { ticket } => {
                        player.invalidate_flash_for_cast_lib(cast_number as i32);
                        self.completed_property_casts.push((id, ticket));
                    }
                }
            } else {
                match pending.purpose {
                    PendingCastPurpose::Property { .. } => {
                        player.movie.cast_manager.get_cast_mut(request.cast_number()).cancel_load(&request);
                    }
                    PendingCastPurpose::Preload => {
                        if !player.movie.cast_manager.cancel_preload(&request) {
                            player
                                .movie
                                .cast_manager
                                .retire_preload_requirement(&request);
                        }
                    }
                }
            }
        }
        if applied_any {
            if let Some(player) = self.players.get_mut(id) {
                player
                    .movie
                    .cast_manager
                    .finalize_preloads_if_ready(&mut player.bitmap_manager, generated);
            }
        }
        applied_any
    }

    fn purge_invalid_pending(&mut self) {
        let mut position = 0;
        let mut finalize_ids = Vec::new();
        while position < self.pending_casts.len() {
            let request = self.pending_casts[position].request.clone();
            let pending_owner = self.pending_casts[position].owner.clone();
            let property = matches!(
                self.pending_casts[position].purpose,
                PendingCastPurpose::Property { .. }
            );
            let player_id = request.owner_key().player as PlayerId;
            let valid = self.players.players.get(&player_id).is_some_and(|player| {
                player.owner.key() == request.owner_key()
                    && player.owner.same_identity(&pending_owner)
                    && if property {
                        player
                            .movie
                            .cast_manager
                            .get_cast_or_null(request.cast_number())
                            .is_some_and(|cast| cast.is_load_current(&request))
                    } else {
                        player.movie.cast_manager.is_preload_current(&request)
                    }
            });
            if valid {
                position += 1;
                continue;
            }
            let stale = self.pending_casts.remove(position);
            let is_preload = matches!(stale.purpose, PendingCastPurpose::Preload);
            if let Some(player) = self.players.players.get_mut(&player_id) {
                if player.owner.same_identity(&stale.owner) {
                    if is_preload {
                        finalize_ids.push(player_id);
                    }
                    match stale.purpose {
                        PendingCastPurpose::Property { .. } => {
                            player.movie.cast_manager.get_cast_mut(stale.request.cast_number()).cancel_load(&stale.request);
                        }
                        PendingCastPurpose::Preload => {
                            player.movie.cast_manager.retire_preload_requirement(&stale.request);
                        }
                    }
                }
            }
        }
        finalize_ids.sort_unstable();
        finalize_ids.dedup();
        for id in finalize_ids {
            if let Some(player) = self.players.get_mut(id) {
                let mut generated = CastNotificationOutbox::default();
                player.movie.cast_manager.finalize_preloads_if_ready(
                    &mut player.bitmap_manager,
                    &mut generated,
                );
                for notification in generated.drain() {
                    player.movie.cast_manager.pending_notifications.push(notification);
                }
            }
            let mut generated = CastNotificationOutbox::default();
            self.publish_pending_cast_notifications(id, &mut generated);
        }
    }

    /// Apply an owned network completion after an external wait. Both the
    /// player owner and the retained per-load capability must still match.
    pub fn apply_cast_load(&mut self, result: CastLoadResult) -> bool {
        // A completion can arrive after a cast reset or owner replacement.
        // Retire every invalid queue entry before looking up this completion,
        // so stale entries cannot leak or remain ahead of a ready request.
        self.purge_invalid_pending();
        let position = self.pending_casts.iter().position(|pending| {
            std::sync::Arc::ptr_eq(pending.request.capability(), result.capability())
        });
        let Some(position) = position else {
            return false;
        };
        if !matches!(self.pending_casts[position].state, PendingCastState::Waiting) {
            return false;
        }
        let request = self.pending_casts[position].request.clone();
        let pending_owner = self.pending_casts[position].owner.clone();
        let property = matches!(
            self.pending_casts[position].purpose,
            PendingCastPurpose::Property { .. }
        );
        let player_id = request.owner_key().player as PlayerId;
        let owner_valid = self.players.players.get(&player_id).is_some_and(|player| {
            player.owner.key() == request.owner_key() && player.owner.same_identity(&pending_owner)
        });
        if !owner_valid {
            let stale = self.pending_casts.remove(position);
            if let Some(player) = self.players.players.get_mut(&player_id) {
                if player.owner.same_identity(&stale.owner) {
                    if property {
                        player.movie.cast_manager.get_cast_mut(stale.request.cast_number()).cancel_load(&stale.request);
                    } else {
                        player.movie.cast_manager.retire_preload_requirement(&stale.request);
                    }
                }
            }
            return false;
        }
        let cast_valid = self
            .players
            .players
            .get(&player_id)
            .is_some_and(|player| {
                if property {
                    player
                        .movie
                        .cast_manager
                        .get_cast_or_null(request.cast_number())
                        .is_some_and(|cast| cast.is_load_current(&request))
                } else {
                    player.movie.cast_manager.is_preload_current(&request)
                }
            });
        if !cast_valid {
            let stale = self.pending_casts.remove(position);
            if let Some(player) = self.players.players.get_mut(&player_id) {
                if player.owner.same_identity(&stale.owner) {
                    if property {
                        player.movie.cast_manager.get_cast_mut(stale.request.cast_number()).cancel_load(&stale.request);
                    } else {
                        player.movie.cast_manager.retire_preload_requirement(&stale.request);
                    }
                }
            }
            return false;
        }
        self.pending_casts[position].state = PendingCastState::ReadyNetwork(result);
        let mut generated = CastNotificationOutbox::default();
        self.drain_ready_casts(player_id, &mut generated);
        self.publish_pending_cast_notifications(player_id, &mut generated);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::channel;
    use binary_reader::Endian;
    use crate::director::chunks::{config::ConfigChunk, ChunkContainer};
    use crate::director::file::DirectorFile;
    use crate::player::cast_lib::CastLibState;
    use crate::player::cast_manager::CastPreloadState;
    use url::Url;

    fn session_with_casts(modes: &[u16]) -> RuntimeSession {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 77,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        session
            .with_player(1, |ctx| {
                ctx.player.net_manager.base_path = Some(Url::parse("file:///tmp/").unwrap());
                for (index, mode) in modes.iter().copied().enumerate() {
                    ctx.player
                        .movie
                        .cast_manager
                        .casts
                        .push(super::super::cast_lib::CastLib::test_external(
                            (index + 1) as u32,
                            mode,
                        ));
                }
            })
            .unwrap();
        session
    }

    fn add_external_casts(session: &mut RuntimeSession, id: PlayerId, modes: &[u16]) {
        session
            .with_player(id, |ctx| {
                ctx.player.net_manager.base_path = Some(Url::parse("file:///tmp/").unwrap());
                for (index, mode) in modes.iter().copied().enumerate() {
                    ctx.player
                        .movie
                        .cast_manager
                        .casts
                        .push(super::super::cast_lib::CastLib::test_external(
                            (index + 1) as u32,
                            mode,
                        ));
                }
            })
            .unwrap();
    }

    fn empty_cached_file(name: &str) -> Rc<DirectorFile> {
        Rc::new(DirectorFile {
            base_path: Url::parse("file:///tmp/").unwrap(),
            file_name: name.to_owned(),
            endian: Endian::Big,
            after_burned: false,
            version: 500,
            cast_entries: Vec::new(),
            casts: Vec::new(),
            config: ConfigChunk {
                len: 0,
                file_version: 0,
                movie_top: 0,
                movie_left: 0,
                movie_bottom: 0,
                movie_right: 0,
                min_member: 0,
                max_member: 0,
                field9: 0,
                field10: 0,
                pre_d77field11: 0,
                d7_stage_color_g: 0,
                d7_stage_color_b: 0,
                comment_font: 0,
                comment_size: 0,
                comment_style: 0,
                pre_d7_stage_color: 0,
                d7_stage_color_is_rgb: 0,
                d7_stage_color_r: 0,
                bit_depth: 0,
                field17: 0,
                field18: 0,
                field19: 0,
                director_version: 0,
                field21: 0,
                field22: 0,
                field23: 0,
                field24: 0,
                field25: 0,
                field26: 0,
                frame_rate: 0,
                platform: 0,
                protection: 0,
                field29: 0,
                checksum: 0,
                remnants: Vec::new(),
            },
            score: None,
            tile_list: None,
            frame_labels: None,
            score_order: None,
            media: None,
            xmedia: None,
            cast_info: None,
            effect: None,
            thum: None,
            xtra_list: None,
            key_table: None,
            chunk_container: ChunkContainer {
                deserialized_chunks: HashMap::new(),
                chunk_info: HashMap::new(),
                cached_chunk_views: HashMap::new(),
            },
            font_table: HashMap::new(),
        })
    }

    #[test]
    fn dispatch_global_rejects_retired_player_before_builtin_work() {
        let mut session = session_with_casts(&[]);
        session
            .with_player(1, |ctx| {
                ctx.player.owner.begin_reset();
            })
            .unwrap();
        let before = session
            .with_player(1, |ctx| (ctx.player.scope_count, ctx.player.handler_stack_depth))
            .unwrap();
        let result = session.dispatch_global(
            1,
            &Symbol::builtin(BuiltInSymbol::Voidp),
            &[],
        );
        assert!(matches!(
            result,
            Err(ScriptError {
                code: ScriptErrorCode::Abort,
                ..
            })
        ));
        assert_eq!(
            session
                .with_player(1, |ctx| (ctx.player.scope_count, ctx.player.handler_stack_depth))
                .unwrap(),
            before
        );
    }

    #[test]
    fn duplicate_player_id_preserves_existing_player() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 42,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(7, tx.clone()));
        let owner_before = session
            .with_player(7, |ctx| ctx.player.owner.key())
            .unwrap();
        assert_eq!(
            owner_before,
            OwnerKey {
                session: 42,
                player: 7,
                generation: 1
            }
        );
        assert!(!session.add_player(7, tx));
        assert_eq!(session.players().len(), 1);
        let mut second = RuntimeSession::new(SymbolOwner {
            session: 42,
            generation: 1,
        });
        let first_symbol = session.symbols_mut().intern("sessionLocal");
        let second_symbol = second.symbols_mut().intern("sessionLocal");
        assert_ne!(first_symbol, second_symbol);
        assert_eq!(session.symbols().display(&first_symbol), Ok("sessionLocal"));
        assert!(
            session
                .with_player(7, |ctx| {
                    assert_eq!(ctx.player_id, 7);
                    assert_eq!(ctx.player.owner.key(), owner_before);
                    assert!(!ctx.player.globals.is_empty());
                })
                .is_some()
        );
    }

    #[test]
    fn out_of_order_completion_waits_for_cast_order() {
        let mut session = session_with_casts(&[0, 0]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 2);

        assert!(session.apply_cast_load(requests[1].complete(
            requests[1].requested_url().to_owned(),
            Err("second failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(1, |ctx| {
                    (
                        ctx.player.movie.cast_manager.get_cast_or_null(1).unwrap().state,
                        ctx.player.movie.cast_manager.get_cast_or_null(2).unwrap().state,
                    )
                })
                .unwrap(),
            (CastLibState::Loading, CastLibState::Loading)
        );

        assert!(session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("first failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(1, |ctx| {
                    (
                        ctx.player.movie.cast_manager.get_cast_or_null(1).unwrap().state,
                        ctx.player.movie.cast_manager.get_cast_or_null(2).unwrap().state,
                        ctx.player.movie.cast_manager.preload_state,
                    )
                })
                .unwrap(),
            (CastLibState::None, CastLibState::None, CastPreloadState::Ready)
        );
    }

    #[test]
    fn duplicate_buffered_completion_is_rejected() {
        let mut session = session_with_casts(&[0, 0]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert!(session.apply_cast_load(requests[1].complete(
            requests[1].requested_url().to_owned(),
            Err("second failed".to_owned()),
        )));
        assert!(!session.apply_cast_load(requests[1].complete(
            requests[1].requested_url().to_owned(),
            Err("duplicate".to_owned()),
        )));
        assert!(session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("first failed".to_owned()),
        )));
    }

    #[test]
    fn repeated_preload_reason_preserves_mode_two_request() {
        let mut session = session_with_casts(&[2]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 1);
        assert!(session
            .prepare_cast_loads(1, CastPreloadReason::AfterFrameOne)
            .is_empty());
        assert!(session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(1, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Ready
        );
    }

    #[test]
    fn cancellation_rejects_old_completion_and_allows_replacement() {
        let mut session = session_with_casts(&[0]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 1);
        assert!(session.cancel_player_cast_loads(1));
        assert!(!session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("late".to_owned()),
        )));

        let replacement = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(replacement.len(), 1);
        assert!(!std::sync::Arc::ptr_eq(
            requests[0].capability(),
            replacement[0].capability()
        ));
        assert!(!session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("stale".to_owned()),
        )));
        assert!(session
            .with_player(1, |ctx| !ctx
                .player
                .movie
                .cast_manager
                .retire_preload_requirement(&replacement[0]))
            .unwrap());
    }

    #[test]
    fn dropped_callback_rejects_late_driver_completion() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 42, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let ticket = session
            .actions
            .allocate(&owner, None, ActionKind::InternalInvocation, ResumePhase::ApplyOpcode)
            .expect("test action must allocate");
        let eval_id = crate::player::eval::EvalId::new(1);
        // The host may answer before the owner pump has retained its command.
        session.submit_pending_command_completion(
            1,
            ticket.clone(),
            ActionCompletion::Resume,
        );
        assert_eq!(session.pending_completions.len(), 1);
        session.pending_commands.push(PendingCommand {
            player_id: 1,
            owner: owner.clone(),
            action: None,
            started: true,
            ticket: Some(ticket.clone()),
            completer: None,
            event_sender: None,
            score_continuation: None,
            child_completion: None,
            eval_child: Some(eval_id.clone()),
            eval_sender: None,
        });
        session.cancel_eval_callback(&eval_id, 1, &owner);
        assert!(session.pending_completions.is_empty());
        assert!(session.actions.details(&ticket).is_none());
        // A late duplicate is rejected by the action registry and cannot
        // create an unmatchable completion entry.
        session.submit_pending_command_completion(1, ticket, ActionCompletion::Resume);
        assert!(session.pending_completions.is_empty());
    }

    #[test]
    fn cached_snapshot_waits_behind_network_cast() {
        let mut session = session_with_casts(&[0, 0]);
        session
            .with_player(1, |ctx| {
                ctx.player.dir_cache.insert(
                    "file:///tmp/external-2.cct".into(),
                    empty_cached_file("cached-old.cct"),
                );
            })
            .unwrap();

        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].cast_number(), 1);

        // Replacing the cache after reservation must not change the immutable
        // snapshot retained by the second cast while the first waits.
        session
            .with_player(1, |ctx| {
                ctx.player.dir_cache.insert(
                    "file:///tmp/external-2.cct".into(),
                    empty_cached_file("cached-new.cct"),
                );
            })
            .unwrap();
        assert_eq!(
            session
                .with_player(1, |ctx| {
                    (
                        ctx.player.movie.cast_manager.get_cast_or_null(1).unwrap().state,
                        ctx.player.movie.cast_manager.get_cast_or_null(2).unwrap().state,
                    )
                })
                .unwrap(),
            (CastLibState::Loading, CastLibState::Loading)
        );
        let (before_owner, before_batch) = session
            .begin_player_notification_drain(1)
            .expect("player owner must be live before cast completion");
        assert!(before_batch
            .iter()
            .all(|event| event.owner.same_identity(&before_owner)));
        assert!(!before_batch.iter().any(|event| matches!(
            &event.kind,
            PlayerNotificationKind::Host(
                super::super::host_events::HostEvent::CastListChanged { .. }
            )
        )));
        session.finish_player_notification_drain(1, &before_owner);
        assert!(session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("first failed".to_owned()),
        )));

        let (first_state, second_file_name, second_state) = session
            .with_player(1, |ctx| {
                (
                    ctx.player.movie.cast_manager.get_cast_or_null(1).unwrap().state,
                    ctx.player.movie.cast_manager.get_cast_or_null(2).unwrap().file_name.clone(),
                    ctx.player.movie.cast_manager.get_cast_or_null(2).unwrap().state,
                )
            })
            .unwrap();
        assert_eq!(first_state, CastLibState::None);
        assert_eq!(second_file_name, "cached-old.cct");
        assert_eq!(second_state, CastLibState::Loaded);

        let (owner, notifications) = session
            .begin_player_notification_drain(1)
            .expect("player owner must be live after cast completion");
        session.finish_player_notification_drain(1, &owner);
        assert!(notifications
            .iter()
            .all(|event| event.owner.same_identity(&owner)));
        assert_eq!(
            notifications
                .iter()
                .filter(|event| matches!(&event.kind, PlayerNotificationKind::Host(
                    super::super::host_events::HostEvent::CastListChanged { .. }
                )))
                .count(),
            1
        );
        assert!(!session
            .with_player(1, |ctx| {
                ctx.player.movie.cast_manager.finalize_preloads_if_ready(
                    &mut ctx.player.bitmap_manager,
                    &mut CastNotificationOutbox::default(),
                )
            })
            .unwrap());
    }

    #[test]
    fn interleaved_players_drain_only_their_own_cast_order() {
        let mut session = session_with_casts(&[0, 0]);
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(2, tx));
        add_external_casts(&mut session, 2, &[0, 0]);

        let first = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        let second = session.prepare_cast_loads(2, CastPreloadReason::MovieLoaded);
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);

        assert!(session.apply_cast_load(second[1].complete(
            second[1].requested_url().to_owned(),
            Err("player two second failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(2, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Loading
        );

        assert!(session.apply_cast_load(first[0].complete(
            first[0].requested_url().to_owned(),
            Err("player one first failed".to_owned()),
        )));
        assert!(session.apply_cast_load(first[1].complete(
            first[1].requested_url().to_owned(),
            Err("player one second failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(1, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Ready
        );
        assert_eq!(
            session
                .with_player(2, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Loading
        );

        assert!(session.apply_cast_load(second[0].complete(
            second[0].requested_url().to_owned(),
            Err("player two first failed".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(2, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Ready
        );
    }

    #[test]
    fn missing_cast_retires_all_stale_queue_entries() {
        let mut session = session_with_casts(&[0, 0]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 2);
        session
            .with_player(1, |ctx| ctx.player.movie.cast_manager.casts.clear())
            .unwrap();

        assert!(!session.apply_cast_load(requests[1].complete(
            requests[1].requested_url().to_owned(),
            Err("cast was reset".to_owned()),
        )));
        assert!(session.pending_casts.is_empty());
        assert_eq!(
            session
                .with_player(1, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Idle
        );
    }

    #[test]
    fn stale_owner_queue_entry_is_removed_without_touching_current_barrier() {
        let mut session = session_with_casts(&[0]);
        let requests = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(requests.len(), 1);
        session.pending_casts[0].owner = OwnerToken::new(requests[0].owner_key());

        assert!(!session.apply_cast_load(requests[0].complete(
            requests[0].requested_url().to_owned(),
            Err("stale owner".to_owned()),
        )));
        assert!(session.pending_casts.is_empty());
        assert_eq!(
            session
                .with_player(1, |ctx| ctx.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Loading
        );
        assert!(session
            .with_player(1, |ctx| {
                ctx.player
                    .movie
                    .cast_manager
                    .is_preload_current(&requests[0])
            })
            .unwrap());
    }

    #[test]
    fn replaced_cast_reserves_fresh_capability_before_preload_check() {
        let mut session = session_with_casts(&[0]);
        let old = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(old.len(), 1);

        session
            .with_player(1, |ctx| {
                ctx.player.movie.cast_manager.casts[0] =
                    super::super::cast_lib::CastLib::test_external(1, 0);
            })
            .unwrap();
        let fresh = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        assert_eq!(fresh.len(), 1);
        assert!(!std::sync::Arc::ptr_eq(
            old[0].capability(),
            fresh[0].capability()
        ));
        assert!(!session.apply_cast_load(old[0].complete(
            old[0].requested_url().to_owned(),
            Err("stale cast replacement".to_owned()),
        )));
        assert!(session
            .with_player(1, |ctx| {
                ctx.player
                    .movie
                    .cast_manager
                    .is_preload_current(&fresh[0])
            })
            .unwrap());
        assert!(session.apply_cast_load(fresh[0].complete(
            fresh[0].requested_url().to_owned(),
            Err("fresh cast failed".to_owned()),
        )));
    }

    #[test]
    fn w3d_clocks_are_interleaved_per_player_and_reset_with_owner() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 78,
            generation: 1,
        });
        let (tx1, _rx1) = channel::unbounded();
        let (tx2, _rx2) = channel::unbounded();
        assert!(session.add_player(1, tx1));
        assert!(session.add_player(2, tx2));
        let owner1 = session.with_player(1, |ctx| ctx.player.owner.clone()).unwrap();
        let owner2 = session.with_player(2, |ctx| ctx.player.owner.clone()).unwrap();

        assert_eq!(session.w3d_deltas(1, &owner1, 1_000.0).unwrap().0, 1.0 / 30.0);
        assert_eq!(session.w3d_deltas(2, &owner2, 1_000.0).unwrap().0, 1.0 / 30.0);
        // Frame timestamps remain fractional all the way through the
        // owner-scoped clock; callers must not truncate browser RAF samples
        // before the phase executor receives them.
        let fractional = session.w3d_deltas(1, &owner1, 1_000.25).unwrap().0;
        assert!((fractional - 0.00025).abs() < 0.000001);
        let player1_dt = session.w3d_deltas(1, &owner1, 1_010.0).unwrap().0;
        let player2_dt = session.w3d_deltas(2, &owner2, 1_100.0).unwrap().0;
        assert!((player1_dt - 0.00975).abs() < 0.0001);
        assert!((player2_dt - 0.1).abs() < 0.0001);

        let new_owner = session.reset_player_owned(1, &owner1).unwrap();
        assert!(session.w3d_deltas(1, &owner1, 2_000.0).is_err());
        assert_eq!(session.w3d_deltas(1, &new_owner, 2_000.0).unwrap().0, 1.0 / 30.0);
    }

    #[test]
    fn resetting_parent_recursively_retires_nested_children() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 79,
            generation: 1,
        });
        let (parent_tx, _parent_rx) = channel::unbounded();
        assert!(session.add_player(1, parent_tx));
        let parent_owner = session
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let (command_tx, _command_rx) = channel::unbounded();
        let (event_tx, _event_rx) = channel::unbounded();
        let (child_id, child_owner) = session
            .register_nested_player(
                1,
                &parent_owner,
                CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                },
                command_tx,
                event_tx,
            )
            .unwrap();
        assert!(session
            .with_player(child_id, |context| {
                context.player.owner.same_identity(&child_owner)
            })
            .unwrap());
        let replacement = session.reset_player_owned(1, &parent_owner).unwrap();
        assert!(!child_owner.is_arena_live());
        assert!(session.with_player(child_id, |_| ()).is_none());
        assert!(session.nested_children_for(1, &replacement).is_empty());
        let (replacement_tx, _replacement_rx) = channel::unbounded();
        assert!(session.add_player(child_id, replacement_tx));
        let replacement_owner = session
            .with_player(child_id, |context| context.player.owner.clone())
            .unwrap();
        assert!(!session.retire_nested_child_if_owner(child_id, &child_owner));
        assert!(session
            .with_player(child_id, |context| {
                context.player.owner.same_identity(&replacement_owner)
            })
            .unwrap());
    }

    #[test]
    fn native_notification_drain_consumes_repeated_queue_without_js() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 80,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let handle = session.into_handle();
        for _ in 0..32 {
            handle.borrow_mut().with_player(1, |context| {
                context
                    .player
                    .queue_player_notification(PlayerNotificationKind::ScoreChanged);
            });
            crate::js_api::JsApi::dispatch_player_notifications(handle.clone(), 1).unwrap();
            assert!(handle
                .borrow_mut()
                .with_player(1, |context| context.player.pending_player_notifications.is_empty())
                .unwrap());
        }
    }

    #[test]
    fn native_notification_drain_preserves_all_owned_kinds_for_two_players() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 804, generation: 1 });
        let (first_tx, _first_rx) = channel::unbounded();
        let (second_tx, _second_rx) = channel::unbounded();
        assert!(session.add_player(1, first_tx));
        assert!(session.add_player(2, second_tx));
        let handle = session.into_handle();

        for player_id in [1, 2] {
            handle.borrow_mut().with_player(player_id, |context| {
                // Keep the CastMemberChanged notification backed by a real
                // member in this player's own cast manager.  The native
                // snapshot path deliberately rejects missing members, so a
                // synthetic reference here would turn the test's diagnostic
                // error into a wasm-bindgen panic on native targets.
                let mut cast = super::super::cast_lib::CastLib::test_external(1, 0);
                cast.members.insert(
                    2,
                    super::super::cast_member::CastMember::new(
                        2,
                        super::super::cast_member::CastMemberType::Text(
                            super::super::cast_member::TextMember::new(),
                        ),
                    ),
                );
                context.player.movie.cast_manager.casts.push(cast);
                context.player.queue_player_notification(PlayerNotificationKind::ScoreChanged);
                context.player.queue_player_notification(PlayerNotificationKind::ChannelChanged(3));
                context.player.queue_player_notification(PlayerNotificationKind::ChannelNameChanged(4));
                context.player.queue_player_notification(PlayerNotificationKind::ChannelNamesChanged);
                context.player.queue_player_notification(PlayerNotificationKind::CastMemberNameChanged(6));
                context.player.queue_player_notification(PlayerNotificationKind::CastMemberChanged(CastMemberRef { cast_lib: 1, cast_member: 2 }));
                context.player.queue_player_notification(PlayerNotificationKind::DatumSnapshot(DatumRef::Void));
                context.player.queue_player_notification(PlayerNotificationKind::ScriptInstanceSnapshot(None));
                context.player.queue_player_notification(PlayerNotificationKind::Host(
                    super::super::host_events::HostEvent::FrameChanged { frame: player_id },
                ));
            });
            let dispatch = crate::js_api::JsApi::dispatch_player_notifications(handle.clone(), player_id);
            if dispatch.is_err() {
                let diagnostic = handle
                    .borrow_mut()
                    .take_native_notification_error(player_id)
                    .map(|error| format!("{}: {}", error.notification, error.message))
                    .unwrap_or_else(|| "no native notification diagnostic".to_owned());
                panic!("native notification dispatch failed for player {player_id}: {diagnostic}");
            }
        }

        let events = [1, 2]
            .into_iter()
            .flat_map(|player_id| {
                handle
                    .borrow_mut()
                    .take_native_player_notifications(player_id)
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 18);
        for player_id in [1, 2] {
            let owner = handle.borrow_mut().with_player(player_id, |context| context.player.owner.clone()).unwrap();
            let owned = events.iter().filter(|event| event.player_id == player_id).collect::<Vec<_>>();
            assert_eq!(owned.len(), 9);
            assert!(owned.iter().all(|event| event.owner.same_identity(&owner)));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::ScoreChanged(snapshot) if snapshot.channel_count == 0)));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::ChannelChanged(snapshot) if snapshot.channel == 3)));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::ChannelNameChanged { channel: 4, .. })));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::ChannelNamesChanged(_))));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::CastMemberNameChanged { slot: 6, .. })));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::CastMemberChanged(snapshot) if snapshot.member_ref == (1, 2))));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::DatumSnapshot(snapshot) if snapshot.type_name == "void")));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::ScriptInstanceSnapshot(snapshot) if snapshot.instance_id.is_none())));
            assert!(owned.iter().any(|event| matches!(&event.kind, super::super::host_events::NativePlayerNotificationKind::Host(super::super::host_events::HostEvent::FrameChanged { frame }) if *frame == player_id)));
        }
    }

    #[test]
    fn native_notification_capacity_is_partitioned_by_player() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 805, generation: 1 });
        let (first_tx, _first_rx) = channel::unbounded();
        let (second_tx, _second_rx) = channel::unbounded();
        assert!(session.add_player(1, first_tx));
        assert!(session.add_player(2, second_tx));
        let first_owner = session
            .players
            .players
            .get(&1)
            .expect("first player")
            .owner
            .clone();
        let second_owner = session
            .players
            .players
            .get(&2)
            .expect("second player")
            .owner
            .clone();
        for _ in 0..super::super::host_events::MAX_HOST_EVENTS {
            session
                .push_native_player_notification(super::super::host_events::NativePlayerNotification {
                    player_id: 1,
                    owner: first_owner.clone(),
                    kind: super::super::host_events::NativePlayerNotificationKind::Host(
                        super::super::host_events::HostEvent::FrameChanged { frame: 1 },
                    ),
                })
                .unwrap();
        }
        assert!(session
            .push_native_player_notification(super::super::host_events::NativePlayerNotification {
                player_id: 2,
                owner: second_owner,
                kind: super::super::host_events::NativePlayerNotificationKind::Host(
                    super::super::host_events::HostEvent::FrameChanged { frame: 2 },
                ),
            })
            .is_ok());
        assert_eq!(session.take_native_player_notifications(2).len(), 1);
        assert_eq!(session.take_native_player_notifications(1).len(), super::super::host_events::MAX_HOST_EVENTS);
    }

    #[test]
    fn native_foreign_datum_error_consumes_only_failure_and_retries_tail() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 806, generation: 1 });
        let (first_tx, _first_rx) = channel::unbounded();
        let (second_tx, _second_rx) = channel::unbounded();
        assert!(session.add_player(1, first_tx));
        assert!(session.add_player(2, second_tx));
        let foreign = session
            .with_player(2, |context| {
                context
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::Int(7))
            })
            .unwrap();
        let handle = session.into_handle();
        handle.borrow_mut().with_player(1, |context| {
            context
                .player
                .queue_player_notification(PlayerNotificationKind::DatumSnapshot(foreign));
            context.player.queue_player_notification(PlayerNotificationKind::Host(
                super::super::host_events::HostEvent::FrameChanged { frame: 7 },
            ));
        });

        assert!(crate::js_api::JsApi::dispatch_player_notifications(handle.clone(), 1).is_err());
        let error = handle
            .borrow_mut()
            .take_native_notification_error(1)
            .expect("foreign datum conversion must be observable");
        assert_eq!(error.player_id, 1);
        assert_eq!(error.notification, "DatumSnapshot");
        assert!(error.message.contains("foreign or stale datum"));

        assert!(crate::js_api::JsApi::dispatch_player_notifications(handle.clone(), 1).is_ok());
        let events = handle.borrow_mut().take_native_player_notifications(1);
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            super::super::host_events::NativePlayerNotificationKind::Host(
                super::super::host_events::HostEvent::FrameChanged { frame: 7 }
            )
        )));
    }

    #[test]
    fn purge_for_player_a_publishes_invalid_player_b_cast_notifications_to_b() {
        let mut session = session_with_casts(&[0, 0]);
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(2, tx));
        add_external_casts(&mut session, 2, &[0, 0]);

        let first = session.prepare_cast_loads(1, CastPreloadReason::MovieLoaded);
        let _second = session.prepare_cast_loads(2, CastPreloadReason::MovieLoaded);
        assert_eq!(first.len(), 2);
        // Replace B through the normal cast-manager load path. This queues a
        // real empty cast-list transition, clears B's preload requirements,
        // and leaves the old pending requests for A's completion to purge.
        let replacement = empty_cached_file("replacement.cct");
        session
            .with_player(2, |mut context| {
                context.with_player_and_symbols(|player, symbols| {
                    let mut dir_cache = std::mem::take(&mut player.dir_cache);
                    player.movie.cast_manager.load_from_dir(
                        &replacement,
                        &mut player.net_manager,
                        &mut player.bitmap_manager,
                        &mut dir_cache,
                        symbols,
                    );
                    player.dir_cache = dir_cache;
                });
            })
            .unwrap();

        assert!(session.apply_cast_load(first[0].complete(
            first[0].requested_url().to_owned(),
            Err("player one failed while player two was stale".to_owned()),
        )));
        assert_eq!(
            session
                .with_player(2, |context| context.player.movie.cast_manager.preload_state)
                .unwrap(),
            CastPreloadState::Idle
        );
        assert!(!session
            .pending_casts
            .iter()
            .any(|pending| pending.request.owner_key().player == 2));

        let (player_a_owner, player_a) = session
            .begin_player_notification_drain(1)
            .expect("player A owner must be live");
        session.finish_player_notification_drain(1, &player_a_owner);
        assert!(player_a
            .iter()
            .all(|notification| notification.owner.same_identity(&player_a_owner)));
        assert_eq!(
            player_a
                .iter()
                .filter(|notification| matches!(
                    &notification.kind,
                    PlayerNotificationKind::Host(
                        super::super::host_events::HostEvent::CastListChanged { .. }
                    )
                ))
                .count(),
            0
        );
        let (player_b_owner, player_b) = session
            .begin_player_notification_drain(2)
            .expect("player B owner must be live");
        session.finish_player_notification_drain(2, &player_b_owner);
        assert!(player_b
            .iter()
            .all(|notification| notification.owner.same_identity(&player_b_owner)));
        assert_eq!(
            player_b
                .iter()
                .filter(|notification| matches!(
                    &notification.kind,
                    PlayerNotificationKind::Host(
                        super::super::host_events::HostEvent::CastListChanged { names }
                    ) if names.is_empty()
                ))
                .count(),
            1
        );

        assert!(session.apply_cast_load(first[1].complete(
            first[1].requested_url().to_owned(),
            Err("player one second cast failed".to_owned()),
        )));
        let (player_a_owner, player_a) = session
            .begin_player_notification_drain(1)
            .expect("player A owner must remain live");
        session.finish_player_notification_drain(1, &player_a_owner);
        assert!(player_a
            .iter()
            .all(|notification| notification.owner.same_identity(&player_a_owner)));
        assert_eq!(
            player_a
                .iter()
                .filter(|notification| matches!(
            &notification.kind,
            PlayerNotificationKind::Host(
                super::super::host_events::HostEvent::CastListChanged { names }
            ) if names.is_empty()
        ))
                .count(),
            0
        );
        assert_eq!(
            player_a
                .iter()
                .filter(|notification| matches!(
                    &notification.kind,
                    PlayerNotificationKind::Host(
                        super::super::host_events::HostEvent::CastListChanged { names }
                    ) if names.iter().any(|name| name == "external-1")
                ))
                .count(),
            1
        );
    }

    #[test]
    fn native_host_backpressure_stops_detached_drain_without_spill_queue() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 802,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let (other_tx, _other_rx) = channel::unbounded();
        assert!(session.add_player(2, other_tx));
        let other_owner = session
            .with_player(2, |context| context.player.owner.clone())
            .unwrap();
        session.with_player(1, |context| {
            context.player.host_event_mailbox = crate::player::host_events::HostEventMailbox::new(1);
            assert!(context.player.queue_host_event(crate::player::host_events::HostEvent::FrameChanged {
                frame: 1,
            }).is_ok());
            assert!(context.player.queue_host_event(crate::player::host_events::HostEvent::MovieLoaded {
                version: 5,
                cast_names: vec!["main".into()],
            }).is_err());
            assert!(context.player.queue_host_event(crate::player::host_events::HostEvent::FrameChanged {
                frame: 2,
            }).is_err());
            assert!(context.player.pending_player_notifications.is_empty());
        });
        session.with_player(2, |context| {
            assert!(context
                .player
                .queue_host_event(crate::player::host_events::HostEvent::FrameChanged { frame: 9 })
                .is_ok());
        });
        assert!(session.player_owner_matches(2, &other_owner));
        let (owner, batch) = session.begin_player_notification_drain(1).unwrap();
        assert!(matches!(
            batch.as_slice(),
            [
                PlayerNotification { kind: PlayerNotificationKind::Host(crate::player::host_events::HostEvent::FrameChanged { frame: 1 }), .. },
                PlayerNotification { kind: PlayerNotificationKind::HostBackpressure(1), .. },
            ]
        ));
        session.finish_player_notification_drain(1, &owner);
        let (_owner, retry_batch) = session.begin_player_notification_drain(1).unwrap();
        assert!(matches!(
            retry_batch.last(),
            Some(PlayerNotification {
                kind: PlayerNotificationKind::HostBackpressure(1),
                ..
            })
        ));
    }

    #[test]
    fn native_host_backpressure_cancels_only_retained_owner_work() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 803,
            generation: 1,
        });
        let (first_tx, _first_rx) = channel::unbounded();
        let (second_tx, _second_rx) = channel::unbounded();
        assert!(session.add_player(1, first_tx));
        assert!(session.add_player(2, second_tx));
        let first_owner = session
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let second_owner = session
            .with_player(2, |context| context.player.owner.clone())
            .unwrap();
        let first_ticket = session
            .actions
            .allocate(
                &first_owner,
                None,
                ActionKind::InternalInvocation,
                ResumePhase::ApplyOpcode,
            )
            .unwrap();
        let second_ticket = session
            .actions
            .allocate(
                &second_owner,
                None,
                ActionKind::InternalInvocation,
                ResumePhase::ApplyOpcode,
            )
            .unwrap();
        session.pending_commands.push(PendingCommand {
            player_id: 1,
            owner: first_owner.clone(),
            action: None,
            started: true,
            ticket: Some(first_ticket.clone()),
            completer: None,
            event_sender: None,
            score_continuation: None,
            child_completion: None,
            eval_child: None,
            eval_sender: None,
        });
        session.pending_commands.push(PendingCommand {
            player_id: 2,
            owner: second_owner.clone(),
            action: None,
            started: true,
            ticket: Some(second_ticket.clone()),
            completer: None,
            event_sender: None,
            score_continuation: None,
            child_completion: None,
            eval_child: None,
            eval_sender: None,
        });
        session.with_player(1, |context| {
            context.player.host_event_backpressure =
                Some(crate::player::host_events::HostEventOverflow { capacity: 1 });
        });

        // This is the production cancellation boundary reached by the owner
        // pump after a terminal host mailbox overflow. It must retire A's
        // action and pending command without touching sibling B.
        session.cancel_host_backpressured_owner(1, &first_owner);
        assert!(!session
            .pending_commands
            .iter()
            .any(|pending| pending.player_id == 1));
        assert!(session
            .pending_commands
            .iter()
            .any(|pending| pending.player_id == 2));
        assert!(session.actions.details(&first_ticket).is_none());
        assert!(session.actions.details(&second_ticket).is_some());

        // A completion arriving for B remains accepted after A is cancelled;
        // this is the cross-owner execution fence the overflow marker relies
        // on to avoid poisoning an unrelated browser player.
        session.submit_pending_command_completion(
            2,
            second_ticket.clone(),
            ActionCompletion::Resume,
        );
        assert!(session
            .pending_completions
            .iter()
            .any(|(player_id, ticket, completion)| {
                *player_id == 2
                    && ticket.same_identity(&second_ticket)
                    && matches!(completion, ActionCompletion::Resume)
            }));
    }

    #[test]
    fn detached_notification_batch_preserves_replacement_generation() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 81,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let old_owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        session.with_player(1, |context| {
            context
                .player
                .queue_player_notification(PlayerNotificationKind::ScoreChanged);
        });
        let (drain_owner, batch) = session.begin_player_notification_drain(1).unwrap();
        assert_eq!(batch.len(), 1);
        let new_owner = session.reset_player_owned(1, &old_owner).unwrap();
        session.finish_player_notification_drain(1, &drain_owner);
        assert!(new_owner.is_arena_live());
        let replacement_batch = session.take_player_notifications(1);
        assert_eq!(replacement_batch.len(), 1);
        assert!(replacement_batch[0].owner.same_identity(&new_owner));
    }

    #[test]
    fn scheduled_notification_marker_replaces_stale_owner_without_old_clear() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 83,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let old_owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        assert!(session.begin_scheduled_player_notification(1, &old_owner));

        let new_owner = session.reset_player_owned(1, &old_owner).unwrap();
        assert!(session.begin_scheduled_player_notification(1, &new_owner));
        session.finish_scheduled_player_notification(1, &old_owner);
        assert!(!session.begin_scheduled_player_notification(1, &new_owner));

        session.finish_scheduled_player_notification(1, &new_owner);
        assert!(session.begin_scheduled_player_notification(1, &new_owner));
    }

    #[test]
    fn failed_playback_cleanup_consumes_same_owner_replay_without_restarting() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 85,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let (epoch, _cancel_rx) = session.begin_playback_loop(1, &owner).unwrap().unwrap();
        assert_eq!(session.cancel_playback_loop(1, &owner, true), Some(epoch));
        assert!(session.begin_playback_loop(1, &owner).unwrap().is_none());
        assert!(session.discard_playback_replay_request(1, &owner, epoch));
        assert!(!session.take_playback_replay_request(1, &owner, epoch));
    }

    #[test]
    fn deferred_datum_snapshot_retains_reference_identity_until_drain() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 84,
            generation: 1,
        });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let retained = session
            .with_player(1, |context| context.player.alloc_datum(Datum::String("old".into())))
            .unwrap();
        let retained_id = retained.unwrap();
        let queued = retained.clone();
        drop(retained);
        session.with_player(1, |context| {
            context
                .player
                .queue_player_notification(PlayerNotificationKind::DatumSnapshot(queued));
        });
        let replacement_id = session
            .with_player(1, |context| {
                context
                    .player
                    .alloc_datum(Datum::String("replacement".into()))
                    .unwrap()
        })
        .unwrap();
        assert_ne!(replacement_id, retained_id);
        let (_owner, batch) = session.begin_player_notification_drain(1).unwrap();
        assert!(matches!(
            batch.first().map(|notification| &notification.kind),
            Some(PlayerNotificationKind::DatumSnapshot(reference))
                if reference.unwrap() == retained_id
        ));
        drop(batch);
        let recycled = session.with_player(1, |context| {
            let first = context
                .player
                .alloc_datum(Datum::String("recycled-one".into()))
                .unwrap();
            let second = context
                .player
                .alloc_datum(Datum::String("recycled-two".into()))
                .unwrap();
            (first, second)
        }).unwrap();
        assert!(recycled.0 == retained_id || recycled.1 == retained_id);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reset_player_owned_retires_old_fileio_task_state() {
        use url::Url;

        let mut session = RuntimeSession::new(SymbolOwner { session: 82, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let old_owner = session.with_player(1, |context| {
            context
                .player
                .net_manager
                .set_base_path(Url::parse("https://old.example/").unwrap());
            let prepared = context.player.net_manager.prepare_net_thing("same.txt".to_owned());
            (context.player.owner.clone(), prepared)
        }).unwrap();
        let old_shared = old_owner.1.shared_state.clone();
        let old_task_id = old_owner.1.task.id;
        let replacement_owner = session.reset_player_owned(1, &old_owner.0).unwrap();
        let fresh = session.with_player(1, |context| {
            context
                .player
                .net_manager
                .set_base_path(Url::parse("https://new.example/").unwrap());
            context.player.net_manager.prepare_net_thing("same.txt".to_owned())
        }).unwrap();
        assert!(replacement_owner.is_arena_live());
        assert_eq!(old_task_id, fresh.task.id, "replacement may reuse task ids");
        async_std::task::block_on(async {
            old_shared.lock().await.fulfill_task(old_task_id, Ok(b"old".to_vec())).await;
        });
        assert!(session.with_player(1, |context| {
            context.player.net_manager.get_task_result(Some(fresh.task.id)).is_none()
        }).unwrap());
        async_std::task::block_on(async {
            fresh.shared_state.lock().await.fulfill_task(fresh.task.id, Ok(b"fresh".to_vec())).await;
        });
        assert_eq!(session.with_player(1, |context| {
            context.player.net_manager.get_task_result(Some(fresh.task.id))
        }).unwrap(), Some(Ok(b"fresh".to_vec())));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "session_callback_tests.rs"]
mod callback_tests;
