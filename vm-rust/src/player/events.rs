use async_std::channel::Receiver;
use std::collections::HashSet;
use itertools::Itertools;
use log::{warn, debug};

use crate::{
    director::lingo::datum::{Datum, VarRef},
    player::{
        handlers::datum_handlers::player_call_datum_handler, reserve_player_mut, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
    },
};

use super::{
    allocator::ScriptInstanceAllocatorTrait,
    cast_lib::CastMemberRef, handlers::datum_handlers::script_instance::ScriptInstanceUtils,
    player_semaphone, reserve_player_ref,
    script_ref::ScriptInstanceRef, DatumRef, ScriptError, ScriptErrorCode, ScriptReceiver,
    score::ScoreRef, ownership::OwnerToken, session::{ExecutionContext, RuntimeSessionHandle},
};

use crate::player::cancelled_scope_error;

/// Run one owner-bound script turn to completion.  A pending action is moved
/// into the session command queue and the event waits on its exact completion
/// sender; no player or symbol-table borrow is held across the wait. Event
/// propagation preserves the completed `ScopeResult` so errors and the
/// handler's `passed` flag survive an external action.
async fn await_owned_event_handler(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    receiver: Option<ScriptInstanceRef>,
    handler_ref: crate::player::script::ScriptHandlerRef,
    args: &Vec<DatumRef>,
) -> Result<bool, ScriptError> {
    let scope = crate::player::eval::invoke_script_callback_owned(
        session.clone(),
        player_id,
        owner.clone(),
        receiver,
        handler_ref,
        args.clone(),
        false,
    )
    .await?;
    Ok(!scope.passed)
}

/// Preserve the legacy event-error side effects without consulting an ambient
/// player.  A callback may finish after reset, so reporting is allowed only
/// when the captured owner is still the live player.
fn report_owned_event_error(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    handler_name: Symbol,
    error: &ScriptError,
) -> Result<(), ScriptError> {
    if error.code == ScriptErrorCode::Abort {
        return Ok(());
    }
    crate::player::bytecode::handler_manager::dump_execution_history_on_error(&error.message);
    log::warn!(
        "Error in owned event handler '{}': {}",
        handler_name.into_builtin().map(|builtin| builtin.as_str()).unwrap_or("<dynamic>"),
        error.message,
    );
    let reported = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return false;
            }
            context
                .player
                .on_script_error_with_symbols(error, Some(context.symbols));
            true
        })
        .unwrap_or(false);
    if reported {
        Ok(())
    } else {
        Err(cancelled_scope_error())
    }
}

/// Dispatch one datum handler through the session-owned evaluator.  The
/// dispatcher is synchronous only while it holds the player borrow; an
/// unsupported/child result is converted back into the original owned object
/// request and awaited through the real EvalId/EvalAction pump.  This keeps
/// event and timeout callers from retaining a request and advancing the next
/// receiver before its result arrives.
async fn invoke_datum_owned(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    receiver: DatumRef,
    handler_name: Symbol,
    args: Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    let dispatch = session
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            Ok(player_call_datum_handler(
                &mut context,
                &receiver,
                handler_name.clone(),
                &args,
            ))
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    let request = match dispatch {
        crate::player::handlers::datum_handlers::DatumDispatch::Sync(result) => return result,
        crate::player::handlers::datum_handlers::DatumDispatch::Pending { request, .. } => request,
        crate::player::handlers::datum_handlers::DatumDispatch::Child { .. }
        | crate::player::handlers::datum_handlers::DatumDispatch::ChildWithCompletion { .. } => {
            crate::player::driver::InternalVmRequest::Object {
                receiver,
                name: handler_name,
                args,
            }
        }
    };
    {
            crate::player::eval::invoke_request_owned(
                session.clone(),
                player_id,
                owner.clone(),
                request,
            )
            .await
    }
}

/// Owner-scoped `stopEvent()` state for dispatches that can suspend.  The
/// guard restores the enclosing value on every return path, but only if the
/// captured player is still the same live owner after the await.  A reset or
/// replacement therefore cannot be mutated by stale cleanup.
struct OwnedEventStopScope {
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    previous: bool,
}

impl OwnedEventStopScope {
    fn enter(
        session: &RuntimeSessionHandle,
        player_id: u32,
        owner: &OwnerToken,
    ) -> Result<Self, ScriptError> {
        let previous = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(crate::player::cancelled_scope_error());
                }
                Ok(std::mem::replace(&mut context.player.event_stopped, false))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        Ok(Self {
            session: session.clone(),
            player_id,
            owner: owner.clone(),
            previous,
        })
    }
}

impl Drop for OwnedEventStopScope {
    fn drop(&mut self) {
        let _ = self.session.borrow_mut().with_player(self.player_id, |context| {
            if self.owner.same_identity(&context.player.owner) && self.owner.is_arena_live() {
                context.player.event_stopped = self.previous;
            }
        });
    }
}

/// Owner-scoped score context for an event that can suspend.  The guard is
/// deliberately independent of callback completion: if the future is
/// dropped, its `Drop` restores the captured context, while a reset or
/// replacement owner makes the cleanup a no-op.
pub(crate) struct OwnedScoreContextScope {
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    previous: ScoreRef,
}

impl OwnedScoreContextScope {
    pub(crate) fn enter(
        session: &RuntimeSessionHandle,
        player_id: u32,
        owner: &OwnerToken,
        score_ref: ScoreRef,
    ) -> Result<Self, ScriptError> {
        let previous = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, owner)?;
                Ok(std::mem::replace(
                    &mut context.player.current_score_context,
                    score_ref,
                ))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        Ok(Self {
            session: session.clone(),
            player_id,
            owner: owner.clone(),
            previous,
        })
    }
}

impl Drop for OwnedScoreContextScope {
    fn drop(&mut self) {
        let _ = self.session.borrow_mut().with_player(self.player_id, |context| {
            if self.owner.same_identity(&context.player.owner) && self.owner.is_arena_live() {
                context.player.current_score_context = self.previous.clone();
            }
        });
    }
}

fn validate_event_owner(
    context: &ExecutionContext<'_>,
    owner: &OwnerToken,
) -> Result<(), ScriptError> {
    if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
        Ok(())
    } else {
        Err(crate::player::cancelled_scope_error())
    }
}

/// Own one static-event recursion guard for the complete async callback.
/// Dropping an event future must release its registration; otherwise a later
/// frame is incorrectly classified as recursive and skipped.  The owner
/// check in `release_static_event_guard` keeps a cancelled callback from
/// mutating a replacement player.
struct OwnedStaticEventGuard {
    session: RuntimeSessionHandle,
    player_id: u32,
    guard: Option<crate::player::driver::StaticEventGuard>,
}

impl OwnedStaticEventGuard {
    fn new(
        session: RuntimeSessionHandle,
        player_id: u32,
        guard: crate::player::driver::StaticEventGuard,
    ) -> Self {
        Self { session, player_id, guard: Some(guard) }
    }

    fn release(&mut self) {
        if let Some(guard) = self.guard.take() {
            let _ = self.session.borrow_mut().release_static_event_guard(self.player_id, &guard);
        }
    }
}

impl Drop for OwnedStaticEventGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Explicit owner-bound global event entrypoint.  Frontends capture the
/// session/player/owner once and use this function for the entire dispatch;
/// it never reselects a thread-local active player after an external wait.
pub(crate) async fn player_invoke_global_event_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: Symbol,
    args: Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    let owner_valid = session
        .borrow_mut()
        .with_player(player_id, |context| {
            owner.same_identity(&context.player.owner) && owner.is_arena_live()
        })
        .unwrap_or(false);
    if !owner_valid {
        return Err(crate::player::cancelled_scope_error());
    }
    let _event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
    let (instances, static_scripts) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            let player = &mut *context.player;
            let mut instances = player.active_stage_script_instance_ids();
            for value in player.get_hydrated_globals().values() {
                match value {
                    Datum::VarRef(VarRef::ScriptInstance(instance))
                    | Datum::ScriptInstanceRef(instance) => instances.push(instance.clone()),
                    _ => {}
                }
            }
            let mut static_scripts = Vec::new();
            if let Some(frame) = player.movie.score.get_script_in_frame(player.movie.current_frame) {
                static_scripts.push(CastMemberRef {
                    cast_lib: frame.cast_lib.into(),
                    cast_member: frame.cast_member.into(),
                });
            }
            let movie_scripts = player.movie.cast_manager.get_movie_scripts();
            if let Some(movie_scripts) = movie_scripts.as_ref() {
                static_scripts.extend(movie_scripts.iter().map(|script| script.member_ref.clone()));
            }
            Ok::<_, ScriptError>((instances, static_scripts))
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;

    let mut handled = false;
    for instance in instances {
        let handler = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                ScriptInstanceUtils::get_script_instance_handler(handler_name.clone(), &instance, context.player)
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        if let Some(handler_ref) = handler {
            let instance_handled = await_owned_event_handler(
                &session,
                player_id,
                &owner,
                Some(instance.clone()),
                handler_ref,
                &args,
            )
            .await?;
            handled |= instance_handled;
            let stopped = session
                .borrow_mut()
                .with_player(player_id, |context| -> Result<_, ScriptError> {
                    validate_event_owner(&context, &owner)?;
                    Ok(context.player.event_stopped)
                })
                .ok_or_else(crate::player::cancelled_scope_error)??;
            if stopped {
                return Ok(DatumRef::Void);
            }
        }
    }

    if handled {
        return Ok(DatumRef::Void);
    }

    for member_ref in static_scripts {
        let handler = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                Ok(context.player
                    .movie
                    .cast_manager
                    .get_script_by_ref(&member_ref)
                    .map(|script| script.get_own_handler_ref(handler_name.clone()))
                    .unwrap_or(None))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let Some(handler_ref) = handler else { continue };
        let receiver = session.borrow_mut().with_player(player_id, |context| -> Result<_, ScriptError> {
            validate_event_owner(&context, &owner)?;
            let player = &*context.player;
            Ok((player.movie.frame_script_member.as_ref() == Some(&member_ref))
                .then(|| player.movie.frame_script_instance.clone())
                .flatten())
        }).ok_or_else(crate::player::cancelled_scope_error)??;
        let call = crate::player::driver::StaticEventCall {
            member_ref: member_ref.clone(),
            receiver: receiver.clone(),
            handler_name: handler_name.clone(),
            args: args.clone(),
        };
        session
            .borrow_mut()
            .with_player(player_id, |context| validate_event_owner(&context, &owner))
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let Some(guard) = session.borrow_mut().enter_static_event_guard(player_id, &call)? else {
            continue;
        };
        let mut guard = OwnedStaticEventGuard::new(session.clone(), player_id, guard);
        let result = await_owned_event_handler(&session, player_id, &owner, receiver, handler_ref, &args).await;
        guard.release();
        let handled_here = result?;
        let stopped = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                Ok(context.player.event_stopped)
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        if handled_here || stopped {
            return Ok(DatumRef::Void);
        }
    }
    Ok(DatumRef::Void)
}

/// Dispatch a queued targeted event through the captured runtime owner.
///
/// Targeted events deliver to every supplied behavior in order.  A handled
/// behavior does not stop sibling delivery; `stopEvent()` does.  When no
/// behavior handles the event, Director falls through to the frame/movie
/// static scripts.  The owner-bound entrypoints below keep all of those
/// decisions valid across a suspended callback and make reset cancellation
/// terminal instead of redirecting work to the replacement player.
pub(crate) async fn player_invoke_targeted_event_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: Symbol,
    args: Vec<DatumRef>,
    instance_refs: Option<Vec<ScriptInstanceRef>>,
) -> Result<DatumRef, ScriptError> {
    let result: Result<DatumRef, ScriptError> = async {
        let _event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
        let mut handled = false;

        if let Some(instance_refs) = instance_refs {
            for instance_ref in instance_refs {
                let handler = session
                    .borrow_mut()
                    .with_player(player_id, |context| -> Result<_, ScriptError> {
                        validate_event_owner(&context, &owner)?;
                        ScriptInstanceUtils::get_script_instance_handler(
                            handler_name.clone(),
                            &instance_ref,
                            context.player,
                        )
                    })
                    .ok_or_else(crate::player::cancelled_scope_error)??;
                let Some(handler_ref) = handler else { continue };

                let handled_here = await_owned_event_handler(
                    &session,
                    player_id,
                    &owner,
                    Some(instance_ref),
                    handler_ref,
                    &args,
                )
                .await?;
                handled |= handled_here;

                let stopped = session
                    .borrow_mut()
                    .with_player(player_id, |context| -> Result<_, ScriptError> {
                        validate_event_owner(&context, &owner)?;
                        Ok(context.player.event_stopped)
                    })
                    .ok_or_else(crate::player::cancelled_scope_error)??;
                if stopped {
                    return Ok(DatumRef::Void);
                }
            }
        }

        if !handled {
            let _ = player_invoke_static_event_owned(
                &session,
                player_id,
                &owner,
                handler_name.clone(),
                &args,
            )
            .await?;
        }
        Ok(DatumRef::Void)
    }
    .await;

    if let Err(error) = &result {
        if error.code != ScriptErrorCode::Abort {
            report_owned_event_error(&session, player_id, &owner, handler_name, error)?;
        }
    }
    result
}

/// Production and unit-test entrypoint for the queued `PlayerVMEvent::Targeted`
/// variant.  Keeping the enum unpacking here prevents the event loop from
/// falling back to the ambient targeted dispatcher after it has captured its
/// session/player/owner.
async fn dispatch_targeted_vm_event_owned(
    session: Option<&RuntimeSessionHandle>,
    player_id: u32,
    owner: Option<&OwnerToken>,
    item: PlayerVMEvent,
) -> Result<DatumRef, ScriptError> {
    let PlayerVMEvent::Targeted(handler_name, args, instance_refs) = item else {
        return Err(crate::player::cancelled_scope_error());
    };
    let (Some(session), Some(owner)) = (session, owner) else {
        return Err(crate::player::cancelled_scope_error());
    };
    player_invoke_targeted_event_owned(
        session.clone(),
        player_id,
        owner.clone(),
        handler_name,
        args,
        instance_refs,
    )
    .await
}

/// Owner-bound dispatcher for queued global events.  The global implementation
/// already preserves behavior ordering and static fallback; this adapter adds
/// the queue boundary's captured-owner error reporting exactly once.
async fn dispatch_global_vm_event_owned(
    session: Option<&RuntimeSessionHandle>,
    player_id: u32,
    owner: Option<&OwnerToken>,
    item: PlayerVMEvent,
) -> Result<DatumRef, ScriptError> {
    let PlayerVMEvent::Global(handler_name, args) = item else {
        return Err(crate::player::cancelled_scope_error());
    };
    let (Some(session), Some(owner)) = (session, owner) else {
        return Err(crate::player::cancelled_scope_error());
    };
    let result = player_invoke_global_event_owned(
        session.clone(),
        player_id,
        owner.clone(),
        handler_name.clone(),
        args,
    )
    .await;
    if let Err(error) = &result {
        if error.code != ScriptErrorCode::Abort {
            report_owned_event_error(session, player_id, owner, handler_name, error)?;
        }
    }
    result
}

/// Owner-bound dispatcher for queued datum callbacks.  Callback failures are
/// reported against the captured symbol table before the event loop releases
/// the queue item; a reset turns the operation into Abort without touching the
/// replacement player.
async fn dispatch_callback_vm_event_owned(
    session: Option<&RuntimeSessionHandle>,
    player_id: u32,
    owner: Option<&OwnerToken>,
    item: PlayerVMEvent,
) -> Result<DatumRef, ScriptError> {
    let PlayerVMEvent::Callback(receiver, handler_name, args) = item else {
        return Err(crate::player::cancelled_scope_error());
    };
    let (Some(session), Some(owner)) = (session, owner) else {
        return Err(crate::player::cancelled_scope_error());
    };
    let result = invoke_datum_owned(session, player_id, owner, receiver, handler_name.clone(), args).await;
    if let Err(error) = &result {
        if error.code != ScriptErrorCode::Abort {
            report_owned_event_error(session, player_id, owner, handler_name, error)?;
        }
    }
    result
}

/// Dispatch a Flash event to one sprite through the captured session owner.
/// Behaviors and frame/movie fallback are resolved immediately before each
/// callback, so a suspended callback cannot redirect work to a replacement.
pub(crate) async fn player_dispatch_event_to_sprite_targeted_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: Symbol,
    args: Vec<DatumRef>,
    sprite_num: u16,
) -> Result<bool, ScriptError> {
    let _event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
    let instance_ids = session
        .borrow_mut()
        .with_player(player_id, |context| -> Result<Option<Vec<ScriptInstanceRef>>, ScriptError> {
            validate_event_owner(&context, &owner)?;
            let Some(sprite) = context.player.movie.score.get_sprite(sprite_num as i16) else {
                return Ok(None);
            };
            let fallback = sprite.script_instance_list.clone();
            Ok(Some(context
                .player
                .get_sprite_script_instance_ids(sprite_num as i16, &fallback)))
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    let Some(instance_ids) = instance_ids else {
        return Ok(false);
    };

    let mut handled = false;
    for instance in instance_ids {
        let handler = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                ScriptInstanceUtils::get_script_instance_handler(
                    handler_name.clone(),
                    &instance,
                    context.player,
                )
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let Some(handler) = handler else { continue };
        let handled_here = match await_owned_event_handler(
            &session,
            player_id,
            &owner,
            Some(instance),
            handler,
            &args,
        )
        .await {
            Ok(handled_here) => handled_here,
            Err(error) => {
                report_owned_event_error(
                    &session,
                    player_id,
                    &owner,
                    handler_name.clone(),
                    &error,
                )?;
                return Err(error);
            }
        };
        if handled_here {
            // Every behavior receives a targeted event. `pass` controls only
            // propagation to frame/movie scripts, not delivery to siblings.
            handled = true;
        }
        let stopped = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                Ok(context.player.event_stopped)
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        if stopped {
            return Ok(true);
        }
    }

    if handled {
        return Ok(true);
    }
    player_invoke_static_event_owned(&session, player_id, &owner, handler_name, &args).await
}

pub enum PlayerVMEvent {
    Global(Symbol, Vec<DatumRef>),
    Targeted(Symbol, Vec<DatumRef>, Option<Vec<ScriptInstanceRef>>),
    Callback(DatumRef, Symbol, Vec<DatumRef>),
}

/// Session-owned clock state for W3D frame work. The owner supplies the current
/// timestamp; this state never reads a host clock and is never shared globally.
#[derive(Clone, Debug)]
pub(crate) struct W3dClock {
    owner: OwnerToken,
    last_animation_ms: f64,
    last_particle_ms: f64,
}

impl W3dClock {
    pub(crate) fn new(owner: OwnerToken) -> Self {
        Self { owner, last_animation_ms: 0.0, last_particle_ms: 0.0 }
    }

    fn check_owner(&self, owner: &OwnerToken) -> Result<(), ScriptError> {
        if !self.owner.same_identity(owner) || !owner.is_arena_live() {
            return Err(w3d_invalid("stale or replaced W3D clock owner"));
        }
        Ok(())
    }

    pub(crate) fn animation_dt(
        &mut self,
        owner: &OwnerToken,
        now_ms: f64,
    ) -> Result<f32, ScriptError> {
        self.check_owner(owner)?;
        let dt = if self.last_animation_ms == 0.0 {
            1.0_f32 / 30.0
        } else {
            ((now_ms - self.last_animation_ms).max(0.0) as f32 / 1000.0).min(0.1)
        };
        self.last_animation_ms = now_ms;
        Ok(dt)
    }

    pub(crate) fn particle_dt(
        &mut self,
        owner: &OwnerToken,
        now_ms: f64,
    ) -> Result<f32, ScriptError> {
        self.check_owner(owner)?;
        let dt = if self.last_particle_ms == 0.0 {
            1.0_f32 / 30.0
        } else {
            ((now_ms - self.last_particle_ms).max(0.0) as f32 / 1000.0).min(0.1)
        };
        self.last_particle_ms = now_ms;
        Ok(dt)
    }
}

#[cfg(test)]
mod w3d_clock_tests {
    use super::{prepare_w3d_collision_callbacks, prepare_w3d_timer_events, sync_w3d_dirty_transforms, w3d_symbol_display, w3d_symbol_eq_ascii_case_symbol, w3d_timer_fire_values, W3dCallbackReceiver, W3dClock, W3dDirtyTransformInput};
    use crate::director::lingo::datum::Datum;
    use crate::director::enums::Shockwave3dInfo;
    use crate::player::cast_lib::CastLib;
    use crate::player::cast_member::{CastMember, CastMemberType, RegisteredW3dEvent, Shockwave3dMember, Shockwave3dRuntimeState};
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::script::ScriptInstance;
    use crate::player::ScriptError;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::{SymbolOwner, SymbolTable}};
    use crate::player::ownership::{OwnerKey, OwnerToken};
    use std::collections::HashSet;
    use std::rc::Rc;

    fn timer_event(begin_ms: u32, period_ms: u32, repetitions: u32) -> RegisteredW3dEvent {
        RegisteredW3dEvent {
            event_name: Symbol::builtin(BuiltInSymbol::TimeMS),
            handler_name: Symbol::builtin(BuiltInSymbol::Timeout),
            script_instance: None,
            begin_ms,
            period_ms,
            repetitions,
            registered_at_ms: 0.0,
            fires_so_far: 0,
            last_fire_ms: 0.0,
        }
    }

    #[test]
    fn clocks_are_independent_and_resettable() {
        let first_owner = OwnerToken::new(OwnerKey { session: 1, player: 1, generation: 1 });
        let second_owner = OwnerToken::new(OwnerKey { session: 1, player: 2, generation: 1 });
        let mut first = W3dClock::new(first_owner.clone());
        let mut second = W3dClock::new(second_owner.clone());
        let mut precise = W3dClock::new(first_owner.clone());
        assert!((precise.animation_dt(&first_owner, 1_000.25).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!((precise.animation_dt(&first_owner, 1_000.75).unwrap() - 0.0005).abs() < 0.000001);
        assert!((first.animation_dt(&first_owner, 1_000.9).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!((second.animation_dt(&second_owner, 4_000.9).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!((first.animation_dt(&first_owner, 1_050.9).unwrap() - 0.05).abs() < f32::EPSILON);
        assert!((second.animation_dt(&second_owner, 4_250.9).unwrap() - 0.1).abs() < f32::EPSILON);

        // Particle time has its own stream, and a fresh owner starts at the
        // Director-compatible 1/30-second step after session reset.
        assert!((first.particle_dt(&first_owner, 9_000.0).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        let mut reset = W3dClock::new(first_owner.clone());
        assert!((reset.animation_dt(&first_owner, 9_000.0).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!(reset.animation_dt(&second_owner, 9_010.0).is_err());

        // The legacy zero timestamp is a sentinel, so repeated zero or a
        // first nonzero timestamp still receives the initial Director step.
        let mut zero = W3dClock::new(first_owner.clone());
        assert!((zero.animation_dt(&first_owner, 0.0).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!((zero.animation_dt(&first_owner, 0.0).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        assert!((zero.animation_dt(&first_owner, 1_000.0).unwrap() - 1.0 / 30.0).abs() < f32::EPSILON);
        // Saturating subtraction preserves the old backward-time behavior.
        assert_eq!(zero.animation_dt(&first_owner, 900.0).unwrap(), 0.0);
        first_owner.mark_arena_dead();
        assert!(zero.animation_dt(&first_owner, 950.0).is_err());
    }

    #[test]
    fn timer_preserves_registration_order_and_repetition_deadlines() {
        let mut first = timer_event(0, 10, 2);
        let mut second = timer_event(50, 10, 0);
        let first_values = w3d_timer_fire_values(&first, 50.0).expect("first timer due");
        let second_values = w3d_timer_fire_values(&second, 50.0).expect("second timer due");
        assert_eq!(first_values.0[4], 50.0);
        assert_eq!(second_values.0[4], 50.0);
        assert!(!first_values.1);
        assert!(!second_values.1);

        first.fires_so_far = 1;
        first.last_fire_ms = 50.0;
        let final_values = w3d_timer_fire_values(&first, 60.0).expect("final repetition due");
        assert_eq!(final_values.0, [0.0, 10.0, 60.0, 10.0, 60.0]);
        assert!(final_values.1);
    }

    #[test]
    fn foreign_dynamic_symbols_and_table_folding_are_validated() {
        let mut local = SymbolTable::with_owner(SymbolOwner { session: 1, generation: 1 });
        let mut foreign_table = SymbolTable::with_owner(SymbolOwner { session: 2, generation: 1 });
        let foreign = foreign_table.intern("callbackFromOtherSession");
        assert!(w3d_symbol_display(&local, &foreign, "callback").is_err());
        let local_symbol = local.intern("localCallback");
        assert_eq!(w3d_symbol_display(&local, &local_symbol, "callback").unwrap(), "localCallback");

        let non_ascii_lhs = local.intern("äCallback");
        let non_ascii_rhs = local.intern("ÄCallback");
        // SymbolTable intentionally folds Unicode spellings on intern. Both
        // spellings therefore denote the same local symbol.
        assert_eq!(non_ascii_lhs, non_ascii_rhs);
        assert!(w3d_symbol_eq_ascii_case_symbol(&local, &non_ascii_lhs, &non_ascii_rhs, "callback").unwrap());
        let distinct_rhs = local.intern("callbackOther");
        assert!(!w3d_symbol_eq_ascii_case_symbol(&local, &local_symbol, &distinct_rhs, "callback").unwrap());
    }

    #[test]
    fn dirty_transform_input_is_owner_qualified() {
        let local_owner = OwnerToken::new(OwnerKey { session: 3, player: 1, generation: 1 });
        let foreign_owner = OwnerToken::new(OwnerKey { session: 3, player: 2, generation: 1 });
        let dirty = W3dDirtyTransformInput::new(foreign_owner, HashSet::new());
        assert!(!dirty.owner.same_identity(&local_owner));
    }

    #[test]
    fn foreign_dirty_input_is_rejected_by_collision_prep() {
        let session_owner = SymbolOwner { session: 11, generation: 1 };
        let mut session = RuntimeSession::new(session_owner);
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let outcome = session.with_player(1, |mut runtime| {
            let foreign_owner = OwnerToken::new(OwnerKey { session: 12, player: 1, generation: 1 });
            let dirty = W3dDirtyTransformInput::new(foreign_owner, HashSet::new());
            prepare_w3d_collision_callbacks(&mut runtime, &dirty).is_err()
        });
        assert_eq!(outcome, Some(true));
    }

    #[test]
    fn collision_prep_rejects_inner_foreign_target_but_ignores_it_for_instance_receiver() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 13, generation: 1 });
        let (tx1, _rx1) = async_std::channel::unbounded();
        let (tx2, _rx2) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx1));
        assert!(session.add_player(2, tx2));

        let foreign_instance = session.with_player(2, |mut runtime| {
            runtime.player.allocator.alloc_script_instance(ScriptInstance {
                instance_id: 4,
                script: CastMemberRef { cast_lib: 0, cast_member: 1 },
                ancestor: None,
                properties: fxhash::FxHashMap::default(),
                begin_sprite_called: false,
            })
        }).expect("foreign player exists");
        let foreign_target = Datum::ScriptInstanceRef(foreign_instance);

        let outcome = session.with_player(1, |mut runtime| {
            runtime.player.movie.cast_manager.casts.push(CastLib::test_external(1, 0));
            let node_a = runtime.symbols.intern("modelA");
            let node_b = runtime.symbols.intern("modelB");
            let handler = runtime.symbols.intern("collisionHandler");
            let mut scene = crate::director::chunks::w3d::types::W3dScene::default();
            let mut a = crate::director::chunks::w3d::types::W3dNode::default();
            a.name = node_a.clone();
            a.node_type = crate::director::chunks::w3d::types::W3dNodeType::Model;
            let mut b = crate::director::chunks::w3d::types::W3dNode::default();
            b.name = node_b.clone();
            b.node_type = crate::director::chunks::w3d::types::W3dNodeType::Model;
            scene.nodes.extend([a, b]);
            let mut w3d = Shockwave3dMember {
                info: Shockwave3dInfo {
                    loops: false, duration: 0, direct_to_stage: false, animation_enabled: false,
                    preload: false, reg_point: (0, 0), default_rect: (0, 0, 0, 0),
                    camera_position: None, camera_rotation: None, bg_color: None, ambient_color: None,
                },
                w3d_data: Vec::new(), source_scene: None, parsed_scene: Some(Rc::new(scene)),
                runtime_state: Shockwave3dRuntimeState::default(), converted_from_text: false,
                text3d_state: None, text3d_source: None,
            };
            let local_instance = runtime.player.allocator.alloc_script_instance(ScriptInstance {
                instance_id: 5,
                script: CastMemberRef { cast_lib: 0, cast_member: 1 },
                ancestor: None,
                properties: fxhash::FxHashMap::default(),
                begin_sprite_called: false,
            });
            for node in [node_a, node_b] {
                w3d.runtime_state.collision_modifiers.insert(node, crate::player::cast_member::W3dCollisionModifier {
                    callback_handler: Some(runtime.symbols.display(&handler).unwrap().to_owned()),
                    callback_instance: Some(local_instance.clone()),
                    callback_target: Some(foreign_target.clone()),
                    ..Default::default()
                });
            }
            runtime.player.movie.cast_manager.casts[0].members.insert(
                1, CastMember::new(1, CastMemberType::Shockwave3d(w3d)),
            );
            let dirty = W3dDirtyTransformInput::new(runtime.player.owner.clone(), HashSet::new());
            let ignored = prepare_w3d_collision_callbacks(&mut runtime, &dirty).expect("instance target is ignored");
            assert_eq!(ignored.len(), 2);
            assert!(ignored.iter().all(|request| matches!(request.receiver, W3dCallbackReceiver::Instance(_))));

            for modifier in runtime.player.movie.cast_manager.casts[0].members.get_mut(&1).unwrap()
                .member_type.as_shockwave3d_mut().unwrap().runtime_state.collision_modifiers.values_mut()
            {
                modifier.callback_instance = None;
            }
            prepare_w3d_collision_callbacks(&mut runtime, &dirty).is_err()
        });
        assert_eq!(outcome, Some(true));
    }

    #[test]
    fn foreign_due_timer_handler_is_rejected_before_consumption() {
        let session_owner = SymbolOwner { session: 9, generation: 1 };
        let mut session = RuntimeSession::new(session_owner);
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let mut foreign_symbols = SymbolTable::with_owner(SymbolOwner { session: 10, generation: 1 });
        let foreign_handler = foreign_symbols.intern("foreignTimerHandler");
        let outcome = session.with_player(1, |mut runtime| {
            runtime.player.movie.cast_manager.casts.push(CastLib::test_external(1, 0));
            let info = Shockwave3dInfo {
                loops: false, duration: 0, direct_to_stage: false, animation_enabled: false,
                preload: false, reg_point: (0, 0), default_rect: (0, 0, 0, 0),
                camera_position: None, camera_rotation: None, bg_color: None, ambient_color: None,
            };
            let mut w3d = Shockwave3dMember {
                info, w3d_data: Vec::new(), source_scene: None, parsed_scene: None,
                runtime_state: Shockwave3dRuntimeState::default(), converted_from_text: false,
                text3d_state: None, text3d_source: None,
            };
            w3d.runtime_state.registered_events.push(RegisteredW3dEvent {
                event_name: Symbol::builtin(BuiltInSymbol::TimeMS), handler_name: foreign_handler,
                script_instance: None, begin_ms: 0, period_ms: 10, repetitions: 0,
                registered_at_ms: 0.0, fires_so_far: 0, last_fire_ms: 0.0,
            });
            let cast = runtime.player.movie.cast_manager.casts.get_mut(0).unwrap();
            cast.members.insert(1, CastMember::new(1, CastMemberType::Shockwave3d(w3d)));
            let result = prepare_w3d_timer_events(&mut runtime, 0.0);
            let consumed = runtime.player.movie.cast_manager.casts[0].members[&1]
                .member_type.as_shockwave3d().unwrap().runtime_state.registered_events[0].fires_so_far;
            (result, consumed)
        });
        let (result, consumed) = outcome.expect("test player exists");
        assert!(result.is_err());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn dirty_transform_flush_updates_matched_finite_only() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 14, generation: 1 });
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let outcome = session.with_player(1, |mut runtime| {
            runtime.player.movie.cast_manager.casts.push(CastLib::test_external(1, 0));
            let node_a = runtime.symbols.intern("flushNodeA");
            let node_b = runtime.symbols.intern("flushNodeB");
            let datum_a = runtime.player.alloc_datum(Datum::transform3d([
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                3.0, 0.0, 0.0, 1.0,
            ]));
            let datum_b = runtime.player.alloc_datum(Datum::transform3d([
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                7.0, 0.0, 0.0, 1.0,
            ]));
            let id_a = datum_a.unwrap();
            let id_b = datum_b.unwrap();
            let mut w3d = Shockwave3dMember {
                info: Shockwave3dInfo {
                    loops: false, duration: 0, direct_to_stage: false, animation_enabled: false,
                    preload: false, reg_point: (0, 0), default_rect: (0, 0, 0, 0),
                    camera_position: None, camera_rotation: None, bg_color: None, ambient_color: None,
                },
                w3d_data: Vec::new(), source_scene: None, parsed_scene: None,
                runtime_state: Shockwave3dRuntimeState::default(), converted_from_text: false,
                text3d_state: None, text3d_source: None,
            };
            w3d.runtime_state.node_transform_datums.insert(node_a.clone(), datum_a.clone());
            w3d.runtime_state.node_transform_datums.insert(node_b.clone(), datum_b.clone());
            w3d.runtime_state.node_transforms.insert(node_a.clone(), [0.0; 16]);
            w3d.runtime_state.node_transforms.insert(node_b.clone(), [7.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 7.0, 0.0, 0.0, 1.0]);
            runtime.player.movie.cast_manager.casts[0].members.insert(
                1, CastMember::new(1, CastMemberType::Shockwave3d(w3d)),
            );

            let dirty = W3dDirtyTransformInput::new(
                runtime.player.owner.clone(),
                HashSet::from([id_a, 999_999]),
            );
            sync_w3d_dirty_transforms(&mut runtime.player, runtime.symbols, &dirty)?;
            let member = runtime.player.movie.cast_manager.casts[0].members.get(&1).unwrap();
            let w3d = member.member_type.as_shockwave3d().unwrap();
            assert_eq!(w3d.runtime_state.node_transforms[&node_a][12], 3.0);
            assert_eq!(w3d.runtime_state.node_transforms[&node_b][12], 7.0);

            if let Datum::Transform3d(matrix) = runtime.player.get_datum_mut(&datum_a) {
                matrix[12] = f64::NAN;
            }
            let dirty_nonfinite = W3dDirtyTransformInput::new(
                runtime.player.owner.clone(),
                HashSet::from([id_a]),
            );
            sync_w3d_dirty_transforms(
                &mut runtime.player,
                runtime.symbols,
                &dirty_nonfinite,
            )?;
            let member = runtime.player.movie.cast_manager.casts[0].members.get(&1).unwrap();
            let w3d = member.member_type.as_shockwave3d().unwrap();
            assert_eq!(w3d.runtime_state.node_transforms[&node_a][12], 3.0);
            Ok::<(), ScriptError>(())
        });
        assert!(outcome.expect("test player exists").is_ok());
    }
}

#[derive(Clone)]
pub(crate) enum W3dCallbackReceiver {
    Instance(ScriptInstanceRef),
    Static { target: Option<DatumRef> },
    Global,
}

/// An owned callback request. The owner identity guards handles while the
/// request waits outside the synchronous player borrow.
pub(crate) struct W3dCallbackRequest {
    pub(crate) owner: OwnerToken,
    pub(crate) receiver: W3dCallbackReceiver,
    pub(crate) handler_name: Symbol,
    pub(crate) args: Vec<DatumRef>,
}

/// Dispatch one W3D callback against the captured session owner.
///
/// Static callbacks use the frame/movie script hierarchy.  Their optional
/// target is already represented as the first argument by the preparation
/// step and must not be interpreted as a member selector.  Global callbacks
/// use the normal instance-then-static hierarchy, while instance callbacks
/// resolve the captured receiver directly.
pub(crate) async fn dispatch_w3d_callback_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    callback: W3dCallbackRequest,
) -> Result<(), ScriptError> {
    if !callback.owner.same_identity(&owner) || !owner.is_arena_live() {
        return Err(crate::player::cancelled_scope_error());
    }
    let _event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
    match callback.receiver {
        W3dCallbackReceiver::Instance(instance) => {
            let handler = session
                .borrow_mut()
                .with_player(player_id, |context| -> Result<_, ScriptError> {
                    validate_event_owner(&context, &owner)?;
                    ScriptInstanceUtils::get_script_instance_handler(
                        callback.handler_name.clone(),
                        &instance,
                        context.player,
                    )
                })
                .ok_or_else(crate::player::cancelled_scope_error)??;
            if let Some(handler) = handler {
                let _ = await_owned_event_handler(
                    &session,
                    player_id,
                    &owner,
                    Some(instance),
                    handler,
                    &callback.args,
                )
                .await?;
            }
        }
        W3dCallbackReceiver::Static { .. } => {
            let _ = player_invoke_static_event_owned(
                &session,
                player_id,
                &owner,
                callback.handler_name,
                &callback.args,
            )
            .await?;
        }
        W3dCallbackReceiver::Global => {
            let _ = player_invoke_global_event_owned(
                session,
                player_id,
                owner,
                callback.handler_name,
                callback.args,
            )
            .await?;
        }
    }
    Ok(())
}

/// Dirty Transform3d ids captured by the owner of a session. Keeping the
/// owner beside the ids prevents a global/thread-local dirty set from leaking
/// one player's transform writes into another player's collision pass.
pub(crate) struct W3dDirtyTransformInput {
    pub(crate) owner: OwnerToken,
    pub(crate) ids: HashSet<usize>,
}

impl W3dDirtyTransformInput {
    pub(crate) fn new(owner: OwnerToken, ids: HashSet<usize>) -> Self {
        Self { owner, ids }
    }
}

fn w3d_invalid(message: impl Into<String>) -> ScriptError {
    ScriptError::new_code(ScriptErrorCode::InvalidReference, message.into())
}

fn w3d_check_instance(runtime: &ExecutionContext<'_>, instance: &ScriptInstanceRef) -> Result<(), ScriptError> {
    if !instance.owner().same_identity(&runtime.player.owner)
        || runtime.player.allocator.get_script_instance_opt(instance).is_none()
    {
        return Err(w3d_invalid("foreign or stale W3D callback ScriptInstanceRef"));
    }
    Ok(())
}

fn w3d_check_ref(runtime: &ExecutionContext<'_>, reference: &DatumRef, label: &str) -> Result<(), ScriptError> {
    if let Some(owner) = reference.owner() {
        if !owner.same_identity(&runtime.player.owner)
            || runtime.player.allocator.try_get_datum(reference).is_none()
        {
            return Err(w3d_invalid(format!("foreign or stale W3D callback {label}")));
        }
    }
    if let Some(datum) = runtime.player.allocator.try_get_datum(reference) {
        match datum {
            Datum::Symbol(symbol) if !runtime.symbols.owns(symbol) => {
                return Err(w3d_invalid(format!("foreign or stale W3D callback {label} symbol")));
            }
            Datum::ScriptInstanceRef(instance) => w3d_check_instance(runtime, instance)?,
            _ => {}
        }
    }
    Ok(())
}

fn w3d_require_live_player(runtime: &ExecutionContext<'_>) -> Result<(), ScriptError> {
    if !runtime.player.owner.is_arena_live() {
        return Err(w3d_invalid("W3D callback preparation requires a live player"));
    }
    Ok(())
}

fn w3d_symbol_display(
    symbols: &SymbolTable,
    symbol: &Symbol,
    label: &str,
) -> Result<String, ScriptError> {
    symbols
        .display(symbol)
        .map(str::to_owned)
        .map_err(|_| w3d_invalid(format!("foreign or stale W3D {label} symbol")))
}

fn w3d_symbol_lower(
    symbols: &SymbolTable,
    symbol: &Symbol,
    label: &str,
) -> Result<String, ScriptError> {
    symbols
        .lower(symbol)
        .map(str::to_owned)
        .map_err(|_| w3d_invalid(format!("foreign or stale W3D {label} symbol")))
}

fn w3d_symbol_eq_ascii_case(
    symbols: &SymbolTable,
    symbol: &Symbol,
    text: &str,
    label: &str,
) -> Result<bool, ScriptError> {
    Ok(w3d_symbol_lower(symbols, symbol, label)?.eq_ignore_ascii_case(text))
}

fn w3d_symbol_eq_ascii_case_symbol(
    symbols: &SymbolTable,
    lhs: &Symbol,
    rhs: &Symbol,
    label: &str,
) -> Result<bool, ScriptError> {
    let lhs_lower = w3d_symbol_lower(symbols, lhs, label)?;
    let rhs_display = w3d_symbol_display(symbols, rhs, label)?;
    Ok(lhs_lower.eq_ignore_ascii_case(&rhs_display))
}

fn w3d_symbol_display_ascii_lower(
    symbols: &SymbolTable,
    symbol: &Symbol,
    label: &str,
) -> Result<String, ScriptError> {
    Ok(w3d_symbol_display(symbols, symbol, label)?.to_ascii_lowercase())
}

fn w3d_timer_fire_values(
    event: &crate::player::cast_member::RegisteredW3dEvent,
    now_ms: f64,
) -> Option<([f64; 5], bool)> {
    // Director: first fire happens at registered_at + begin. Subsequent fires
    // happen every period_ms after the previous fire.
    let next_due = if event.fires_so_far == 0 {
        event.registered_at_ms + event.begin_ms as f64
    } else {
        event.last_fire_ms + event.period_ms as f64
    };
    if now_ms < next_due {
        return None;
    }
    let delta = now_ms - event.last_fire_ms;
    let time = if event.fires_so_far == 0 {
        0.0
    } else {
        now_ms - (event.registered_at_ms + event.begin_ms as f64)
    };
    let duration = if event.repetitions == 0 || event.repetitions == 1 {
        0.0
    } else {
        (event.repetitions as f64 - 1.0) * event.period_ms as f64
    };
    let remove = event.repetitions > 0 && event.fires_so_far + 1 >= event.repetitions;
    Some(([0.0, delta, time, duration, now_ms], remove))
}

pub(crate) fn sync_w3d_dirty_transforms(
    player: &mut crate::player::DirPlayer,
    symbols: &SymbolTable,
    dirty: &W3dDirtyTransformInput,
) -> Result<(), ScriptError> {
    if !player.owner.is_arena_live() {
        return Err(w3d_invalid("W3D dirty-transform flush requires a live player"));
    }
    if !dirty.owner.same_identity(&player.owner) || !dirty.owner.is_arena_live() {
        return Err(w3d_invalid("foreign or stale W3D dirty-transform input"));
    }
    if dirty.ids.is_empty() {
        return Ok(());
    }
    let mut entries = Vec::new();
    for cast in &player.movie.cast_manager.casts {
        for (member_num, member) in &cast.members {
            if let Some(w3d) = member.member_type.as_shockwave3d() {
                for (node_name, datum_ref) in &w3d.runtime_state.node_transform_datums {
                    entries.push((cast.number as i32, *member_num, node_name.clone(), datum_ref.clone()));
                }
            }
        }
    }
    for (cast_lib, cast_member, node_name, datum_ref) in entries {
        // DatumRef::unwrap returns the legacy zero id for Void. Select only
        // dirty ids first so unrelated retained mappings remain untouched.
        let id = datum_ref.unwrap();
        if !dirty.ids.contains(&id) {
            continue;
        }
        let Some(owner) = datum_ref.owner() else {
            return Err(w3d_invalid("malformed W3D dirty-transform datum mapping"));
        };
        if !owner.same_identity(&player.owner) || !owner.is_arena_live() {
            return Err(w3d_invalid("foreign or stale W3D dirty-transform datum reference"));
        }
        if !symbols.owns(&node_name) {
            return Err(w3d_invalid("foreign W3D dirty-transform node symbol"));
        }
        let matrix = match player.allocator.try_get_datum(&datum_ref) {
            Some(Datum::Transform3d(matrix)) => (**matrix).map(|value| value),
            Some(_) => continue,
            None => return Err(w3d_invalid("stale W3D dirty-transform datum reference")),
        };
        let m32: [f32; 16] = matrix.map(|value| value as f32);
        if m32.iter().any(|value| !value.is_finite()) {
            continue;
        }
        let member_ref = CastMemberRef { cast_lib, cast_member: cast_member as i32 };
        if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
            if let Some(w3d) = member.member_type.as_shockwave3d_mut() {
                w3d.runtime_state.node_transforms.insert(node_name, m32);
            }
        }
    }
    Ok(())
}

pub fn player_dispatch_global_event(handler_name: Symbol, args: &Vec<DatumRef>) {
    if let Some(tx) = crate::player::active_event_tx() {
        let _ = tx.try_send(PlayerVMEvent::Global(
            handler_name.to_owned(),
            args.to_owned(),
        ));
    }
}

pub fn player_dispatch_callback_event(
    receiver: DatumRef,
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) {
    if let Some(tx) = crate::player::active_event_tx() {
        let _ = tx.try_send(PlayerVMEvent::Callback(
            receiver,
            handler_name.to_owned(),
            args.to_owned(),
        ));
    }
}

pub fn player_dispatch_targeted_event(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    instance_ids: Option<&Vec<ScriptInstanceRef>>,
) {
    if let Some(tx) = crate::player::active_event_tx() {
        let _ = tx.try_send(PlayerVMEvent::Targeted(
            handler_name.to_owned(),
            args.to_owned(),
            instance_ids.map(|x| x.to_owned()),
        ));
    }
}

pub fn player_dispatch_event_to_sprite(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    sprite_num: u16,
) {
    let instance_ids = reserve_player_mut(|player| {
        // Check the cache first — it may contain extra instances added
        // via scriptInstanceList.add() (e.g. goal parent scripts).
        let fallback = player
            .movie
            .score
            .get_sprite(sprite_num as i16)
            .map(|sprite| sprite.script_instance_list.clone());
        fallback.map(|fallback| {
            player.get_sprite_script_instance_ids(sprite_num as i16, fallback.as_slice())
        })
    });
    if instance_ids.is_none() {
        return;
    }
    let instance_ids = instance_ids.unwrap();
    let tx = crate::player::active_event_tx().unwrap();
    tx.try_send(PlayerVMEvent::Targeted(
        handler_name.to_owned(),
        args.to_owned(),
        Some(instance_ids),
    ))
    .unwrap();
}

/// Returns true when a behavior called `stopEvent()`, so the caller can also
/// skip the rest of the hierarchy it owns (the cast member script and the
/// primary event handler live in `commands.rs`, outside this dispatch).
/// Re-evaluate which sprites the pointer is inside and fire the rollover
/// events. Called on every pointer move AND once per frame: `on mouseWithin`
/// "contains statements that run when the mouse is within the active area of
/// the sprite" (Director 11.5 Scripting Dictionary), which Director keeps
/// sending each frame the pointer stays there — it is not a movement event.
///
/// Merlin's Revenge needs both halves. Its button behaviour arms itself from
/// mouseEnter/mouseWithin and then polls the movie's own mouse object from
/// inside mouseWithin:
///   on mouseWithin me
///     if p.clickposs then
///       if g.mouse.mouse.click then p.master.buttClicked(p.action)
/// so with mouseWithin only on movement, a click on a stationary cursor was
/// never seen.
///
/// Only the front-most sprite under the pointer is hovered, as in Director,
/// where these events follow `the rollover`: a sprite covered by another does
/// not see the pointer at all. Sending them to every overlapping sprite that
/// had a handler let a lower sprite's mouseEnter run after the front-most
/// sprite's and undo it (Matematik i Maaneby's map: a ground patch carrying
/// one house's behaviour sits under another house, and a pointer landing on
/// the house lit it and then unlit it in the same event).
pub fn dispatch_rollover_events() {
    let (now_hovered, prev_hovered) = reserve_player_mut(|player| {
        let (x, y) = player.mouse_loc;
        let prev_hovered = std::mem::take(&mut player.hovered_sprites);
        let now_hovered: Vec<i16> = crate::player::score::get_sprites_at(player, x, y)
            .first()
            .map(|num| *num as i16)
            .into_iter()
            .collect();
        player.hovered_sprites = now_hovered.clone();
        (now_hovered, prev_hovered)
    });

    // Leaving is reported before entering, so a behaviour that tears down on
    // mouseLeave can't clobber the state a freshly entered sprite just set.
    for sprite_num in &prev_hovered {
        if !now_hovered.contains(sprite_num) {
            player_dispatch_event_to_sprite(Symbol::builtin(BuiltInSymbol::MouseLeave), &vec![], *sprite_num as u16);
        }
    }
    for sprite_num in &now_hovered {
        let handler = if prev_hovered.contains(sprite_num) {
            "mouseWithin"
        } else {
            "mouseEnter"
        };
        let handler = match handler {
            "mouseWithin" => BuiltInSymbol::MouseWithin,
            _ => BuiltInSymbol::MouseEnter,
        };
        player_dispatch_event_to_sprite(Symbol::builtin(handler), &vec![], *sprite_num as u16);
    }
}

pub async fn player_dispatch_event_to_sprite_targeted(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    sprite_num: u16,
) -> bool {
    let instance_ids = reserve_player_mut(|player| {
        // Check the cache first — it may contain extra instances added
        // via scriptInstanceList.add() (e.g. goal parent scripts).
        
        let fallback = player
            .movie
            .score
            .get_sprite(sprite_num as i16)
            .map(|sprite| sprite.script_instance_list.clone());
        fallback.map(|fallback| {
            player.get_sprite_script_instance_ids(sprite_num as i16, fallback.as_slice())
        })
    });
    let Some(instance_ids) = instance_ids else {
        return false;
    };

    player_wait_available().await;

    // Dispatch to ALL of the sprite's behaviors in a single pass, then — if
    // none of them handled it (or the sprite has no behaviors at all) — fall
    // through to the frame + movie scripts. Director's message hierarchy for a
    // sprite-directed event is behaviors → frame → movie, stopping at the first
    // non-passing handler.
    //
    // The previous per-instance loop broke this two ways: with an EMPTY
    // instance list (a Flash sprite carries no behaviors) the loop body never
    // ran, so the static-script fall-through never fired — that's why a
    // `getURL("event: FlashLoaderLoaded")` whose handler lives in a movie
    // script (Neopets DGS `on FlashLoaderLoaded`) never reached it and the DGS
    // loader stalled. And with multiple behaviors it fired the static scripts
    // once per non-handling behavior. `player_invoke_targeted_event` with the
    // full instance list does the right thing in both cases.
    //
    // The `stopEvent()` scope is opened here rather than delegated to
    // `player_invoke_targeted_event` so the flag can be read back out before
    // it's restored, and reported to the caller.
    let prev_stopped =
        reserve_player_mut(|player| std::mem::replace(&mut player.event_stopped, false));
    let _ = player_invoke_targeted_event_inner(
        handler_name,
        args,
        Some(&instance_ids),
    ).await;
    reserve_player_mut(|player| std::mem::replace(&mut player.event_stopped, prev_stopped))
}

pub async fn player_invoke_event_to_instances(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    instance_refs: &Vec<ScriptInstanceRef>,
) -> Result<bool, ScriptError> {
    let session = crate::player::retained_session_handle()
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let player_id = crate::player::active_player_id() as u32;
    let owner = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let recv_instance_handlers = session
        .borrow_mut()
        .with_player(player_id, |context| {
        let mut result = vec![];
        for instance_ref in instance_refs {
            let handler_pair = ScriptInstanceUtils::get_script_instance_handler(
                handler_name.clone(),
                instance_ref,
                context.player,
            )?;
            if let Some(handler_pair) = handler_pair {
                result.push((instance_ref.clone(), handler_pair));
            }
        }
        Ok::<Vec<(ScriptInstanceRef, crate::player::script::ScriptHandlerRef)>, ScriptError>(result)
    })
    .ok_or_else(crate::player::cancelled_scope_error)??;

    let mut handled = false;
    for (script_instance_ref, handler_ref) in recv_instance_handlers {
        match await_owned_event_handler(
            &session,
            player_id,
            &owner,
            Some(script_instance_ref),
            handler_ref,
            args,
        ).await {
            Ok(scope_handled) => {
                if scope_handled {
                    handled = true;
                    // Don't break — in Director, all behaviors on a sprite
                    // receive the event. `pass` only controls propagation
                    // beyond behaviors to cast member/frame/movie scripts.
                }
                // `stopEvent()` DOES break: the Scripting Dictionary is
                // explicit that "neither subsequent scripts nor other
                // behaviors on the sprite receive the event". Heatwave
                // Racing's "main menu scroll bhv" relies on this — clicking
                // an off-centre menu entry scrolls the carousel and calls
                // stopEvent() so the "Jump to Marker Button" behavior on the
                // same sprite does NOT navigate.
                if reserve_player_ref(|player| player.event_stopped) {
                    return Ok(true);
                }
            }
            Err(err) => {
                if err.code != ScriptErrorCode::Abort {
                    // Dump bytecode execution history before the error
                    crate::player::bytecode::handler_manager::dump_execution_history_on_error(&err.message);
                    // Log the error to console
                    web_sys::console::error_1(
                        &format!("⚠ Error in handler '{}': {}", handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>"), err.message).into()
                    );
                    // Report to player's error handler
                    reserve_player_mut(|player| {
                        player.on_script_error(&err);
                    });
                }
                // Return the error to caller (abort propagates to stop handler chain)
                return Err(err);
            }
        }
    }
    
    Ok(handled)
}

/// Run one event dispatch with a fresh `stopEvent()` scope.
///
/// `stopEvent()` "applies only to the current event being handled" (Director
/// 11.5 Scripting Dictionary), so each dispatch starts with the flag clear and
/// restores whatever the enclosing dispatch had — otherwise a `sendSprite()`
/// made from inside a stopped handler would clear the outer event's flag and
/// let the rest of the hierarchy run after all.
pub(crate) async fn with_event_scope<F, T>(f: F) -> Result<T, ScriptError>
where
    F: std::future::Future<Output = Result<T, ScriptError>>,
{
    let prev_stopped =
        reserve_player_mut(|player| std::mem::replace(&mut player.event_stopped, false));
    let result = f.await;
    reserve_player_mut(|player| player.event_stopped = prev_stopped);
    result
}

pub async fn player_invoke_targeted_event(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    instance_refs: Option<&Vec<ScriptInstanceRef>>,
) -> Result<DatumRef, ScriptError> {
    with_event_scope(player_invoke_targeted_event_inner(
        handler_name,
        args,
        instance_refs,
    ))
    .await
}

async fn player_invoke_targeted_event_inner(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
    instance_refs: Option<&Vec<ScriptInstanceRef>>,
) -> Result<DatumRef, ScriptError> {
    let handled = match instance_refs {
        Some(instance_refs) => {
            player_invoke_event_to_instances(handler_name.clone(), args, instance_refs).await?
        }
        None => false,
    };
    if !handled {
        player_invoke_static_event(handler_name, args).await?;
    }
    Ok(DatumRef::Void)
}

pub async fn player_invoke_frame_and_movie_scripts(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    with_event_scope(player_invoke_frame_and_movie_scripts_inner(handler_name, args)).await
}

async fn player_invoke_frame_and_movie_scripts_inner(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    let session = crate::player::retained_session_handle()
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let player_id = crate::player::active_player_id() as u32;
    let owner = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let active_static_scripts = reserve_player_mut(|player| {
        let frame_script = player
            .movie
            .score
            .get_script_in_frame(player.movie.current_frame);
        let movie_scripts = player.movie.cast_manager.get_movie_scripts();
        let movie_scripts = movie_scripts.as_ref().unwrap();
        let mut active_static_scripts: Vec<CastMemberRef> = vec![];
        
        // Frame script first
        if let Some(frame_script) = &frame_script {
            let script_ref = CastMemberRef {
                cast_lib: frame_script.cast_lib.into(),
                cast_member: frame_script.cast_member.into(),
            };
            active_static_scripts.push(script_ref);
        }
        
        // Then movie scripts
        for movie_script in movie_scripts {
            active_static_scripts.push(movie_script.member_ref.to_owned());
        }
        
        active_static_scripts
    });

    let mount_gen = reserve_player_ref(|player| player.movie_mount_generation);
    for script_member_ref in active_static_scripts {
        // An eager go(frame, movie) swapped the movie under this dispatch:
        // the remaining member refs belong to the OLD movie and would resolve
        // against the NEW movie's casts. Stop here.
        if reserve_player_ref(|player| player.movie_mount_generation) != mount_gen {
            break;
        }
        let found = reserve_player_ref(|player| {
            let script = player
                .movie
                .cast_manager
                .get_script_by_ref(&script_member_ref);
            let handler = script.and_then(|x| x.get_own_handler_ref(handler_name.clone()));
            let receiver = if player.movie.frame_script_member.as_ref() == Some(&script_member_ref) {
                player.movie.frame_script_instance.clone()
            } else {
                None
            };
            handler.map(|handler| (receiver, handler))
        });
        let Some((receiver, handler)) = found else {
            continue;
        };
        let result = await_owned_event_handler(
            &session,
            player_id,
            &owner,
            receiver,
            handler,
            args,
        )
        .await?;
        if result || reserve_player_ref(|player| player.event_stopped) {
            break;
        }
    }
    Ok(DatumRef::Void)
}

pub(crate) async fn player_invoke_static_event_owned(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    handler_name: Symbol,
    args: &[DatumRef],
) -> Result<bool, ScriptError> {
    let active_static_scripts = session
        .borrow_mut()
        .with_player(player_id, |mut context| -> Result<_, ScriptError> {
            validate_event_owner(&context, owner)?;
            let frame_script = context
                .player
                .movie
                .score
                .get_script_in_frame(context.player.movie.current_frame);
            let movie_scripts = context.player.movie.cast_manager.get_movie_scripts();
            let mut scripts = Vec::new();
            if let Some(frame_script) = frame_script {
                scripts.push(CastMemberRef {
                    cast_lib: frame_script.cast_lib.into(),
                    cast_member: frame_script.cast_member.into(),
                });
            }
            if let Some(movie_scripts) = movie_scripts.as_ref() {
                scripts.extend(movie_scripts.iter().map(|script| script.member_ref.clone()));
            }
            Ok(scripts)
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;

    for member_ref in active_static_scripts {
        let found = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, owner)?;
                let handler = context
                    .player
                    .movie
                    .cast_manager
                    .get_script_by_ref(&member_ref)
                    .and_then(|script| script.get_own_handler_ref(handler_name.clone()));
                let receiver = if context.player.movie.frame_script_member.as_ref() == Some(&member_ref) {
                    context.player.movie.frame_script_instance.clone()
                } else {
                    None
                };
                Ok(handler.map(|handler| (receiver, handler)))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let Some((receiver, handler)) = found else { continue };

        let call = crate::player::driver::StaticEventCall {
            member_ref: member_ref.clone(),
            receiver: receiver.clone(),
            handler_name: handler_name.clone(),
            args: args.to_vec(),
        };
        session
            .borrow_mut()
            .with_player(player_id, |context| validate_event_owner(&context, owner))
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let Some(guard) = session.borrow_mut().enter_static_event_guard(player_id, &call)? else {
            continue;
        };
        let mut guard = OwnedStaticEventGuard::new(session.clone(), player_id, guard);
        let owned_args = args.to_vec();
        let result = await_owned_event_handler(
            session,
            player_id,
            owner,
            receiver,
            handler,
            &owned_args,
        )
        .await;
        guard.release();
        let handled = result?;
        let stopped = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, owner)?;
                Ok(context.player.event_stopped)
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        if handled || stopped {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn player_invoke_static_event(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> Result<bool, ScriptError> {
    let session = crate::player::retained_session_handle()
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let player_id = crate::player::active_player_id() as u32;
    let owner = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let active_static_scripts = session
        .borrow_mut()
        .with_player(player_id, |context| {
            let frame_script = context
                .player
                .movie
                .score
                .get_script_in_frame(context.player.movie.current_frame);
            let movie_scripts = context.player.movie.cast_manager.get_movie_scripts();
            let mut scripts = Vec::new();
            if let Some(frame_script) = frame_script {
                scripts.push(CastMemberRef {
                    cast_lib: frame_script.cast_lib.into(),
                    cast_member: frame_script.cast_member.into(),
                });
            }
            if let Some(movie_scripts) = movie_scripts.as_ref() {
                scripts.extend(movie_scripts.iter().map(|script| script.member_ref.clone()));
            }
            scripts
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let mut handled = false;
    for member_ref in active_static_scripts {
        let found = session
            .borrow_mut()
            .with_player(player_id, |context| {
                let handler = context
                    .player
                    .movie
                    .cast_manager
                    .get_script_by_ref(&member_ref)
                    .and_then(|script| script.get_own_handler_ref(handler_name.clone()));
                let receiver = if context.player.movie.frame_script_member.as_ref() == Some(&member_ref) {
                    context.player.movie.frame_script_instance.clone()
                } else {
                    None
                };
                handler.map(|handler| (receiver, handler))
            })
            .ok_or_else(crate::player::cancelled_scope_error)?;
        let Some((receiver, handler)) = found else { continue };
        let call = crate::player::driver::StaticEventCall {
            member_ref: member_ref.clone(),
            receiver: receiver.clone(),
            handler_name: handler_name.clone(),
            args: args.clone(),
        };
        let Some(guard) = session.borrow_mut().enter_static_event_guard(player_id, &call)? else {
            continue;
        };
        let mut guard = OwnedStaticEventGuard::new(session.clone(), player_id, guard);
        let result = await_owned_event_handler(
            &session,
            player_id,
            &owner,
            receiver,
            handler,
            args,
        )
        .await;
        guard.release();
        let handled_here = result?;
        let stopped = session
            .borrow_mut()
            .with_player(player_id, |context| context.player.event_stopped)
            .unwrap_or(true);
        if handled_here || stopped {
            handled = true;
            break;
        }
    }
    Ok(handled)
}

pub async fn player_invoke_global_event(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    with_event_scope(player_invoke_global_event_inner(handler_name, args)).await
}

async fn player_invoke_global_event_inner(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    // First stage behavior script
    // Then frame behavior script
    // Then movie script
    // If frame is changed during exitFrame, event is no longer propagated
    // TODO find stage behaviors first

    let active_instance_scripts = reserve_player_mut(|player| {
        let mut active_instance_scripts: Vec<ScriptInstanceRef> = vec![];
        active_instance_scripts.extend(player.active_stage_script_instance_ids());
        for global in player.get_hydrated_globals().values() {
            match global {
                Datum::VarRef(VarRef::ScriptInstance(script_instance_ref)) => {
                    active_instance_scripts.push(script_instance_ref.clone());
                }
                Datum::ScriptInstanceRef(script_instance_ref) => {
                    active_instance_scripts.push(script_instance_ref.clone());
                }
                _ => {}
            }
        }

        active_instance_scripts.to_owned()
    });

    let handled =
        player_invoke_event_to_instances(handler_name.clone(), args, &active_instance_scripts).await?;
    if handled {
        return Ok(DatumRef::Void);
    }
    player_invoke_static_event(handler_name, args).await?;

    Ok(DatumRef::Void)
}

pub async fn player_dispatch_movie_callback(
    handler_name: BuiltInSymbol,
) -> Result<(), ScriptError> {
    let session = crate::player::retained_session_handle()
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let player_id = crate::player::active_player_id() as u32;
    let owner = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .ok_or_else(crate::player::cancelled_scope_error)?;
    player_dispatch_movie_callback_owned(session, player_id, owner, handler_name).await
}

/// Dispatch a movie callback against a captured runtime owner. Script text is
/// evaluated through the owned evaluator and handler actions retain their
/// exact `ScopeResult` until the caller resumes them.
pub(crate) async fn player_dispatch_movie_callback_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: BuiltInSymbol,
) -> Result<(), ScriptError> {
    enum CallbackAction {
        CallHandler(Option<ScriptInstanceRef>, crate::player::script::ScriptHandlerRef),
        EvalText(String),
    }
    let action = session
        .borrow_mut()
        .with_player(player_id, |context| -> Result<Option<CallbackAction>, ScriptError> {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            let callback = match handler_name {
                BuiltInSymbol::MouseDown => &context.player.movie.mouse_down_script,
                BuiltInSymbol::MouseUp => &context.player.movie.mouse_up_script,
                BuiltInSymbol::KeyDown => &context.player.movie.key_down_script,
                BuiltInSymbol::KeyUp => &context.player.movie.key_up_script,
                BuiltInSymbol::Timeout => &context.player.movie.timeout_script,
                _ => return Ok(None),
            };
            let Some(callback) = callback.as_ref() else { return Ok(None); };
            Ok(match callback {
                ScriptReceiver::ScriptInstance(instance_ref) => {
                    let script_instance = context.player.allocator.get_script_instance(instance_ref);
                    let script = context.player.movie.cast_manager.get_script_by_ref(&script_instance.script);
                    script.and_then(|script| script.get_own_handler_ref(Symbol::builtin(handler_name)))
                        .map(|handler| CallbackAction::CallHandler(Some(instance_ref.clone()), handler))
                }
                ScriptReceiver::Script(script_ref) => {
                    let script = context.player.movie.cast_manager.get_script_by_ref(script_ref);
                    script.and_then(|script| script.get_own_handler_ref(Symbol::builtin(handler_name)))
                        .map(|handler| CallbackAction::CallHandler(None, handler))
                }
                ScriptReceiver::ScriptText(text) if text.trim().is_empty() || text.trim().starts_with("--") => None,
                ScriptReceiver::ScriptText(text) => Some(CallbackAction::EvalText(text.clone())),
            })
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    match action {
        Some(CallbackAction::CallHandler(receiver, handler)) => {
            await_owned_event_handler(&session, player_id, &owner, receiver, handler, &Vec::new()).await?;
        }
        Some(CallbackAction::EvalText(text)) => {
            crate::player::eval_lingo_command_owned(
                session.clone(),
                player_id,
                owner.clone(),
                text,
            ).await?;
        }
        None => {}
    }
    Ok(())
}

/// Tick the per-frame W3D `#timeMS` event registrations. Walks every
/// Shockwave3D member's `runtime_state.registered_events`, fires any whose
/// next deadline has passed, advances the bookkeeping, and removes
/// completed (non-infinite) entries.
///
/// Per the Director docs, `#timeMS` handlers receive five positional args:
/// type (always 0), delta (ms since the last fire), time (ms since the
/// first fire), duration (total ms over all repetitions, or 0 for
/// infinite), and systemTime (absolute ms). Other event types
/// (#collideAny / #collideWith / #animationStarted / #animationEnded)
/// are stored by registerForEvent but not fired here — their producers
/// aren't wired up yet.
pub(crate) fn prepare_w3d_timer_events(
    runtime: &mut ExecutionContext<'_>,
    now_ms: f64,
) -> Result<Vec<W3dCallbackRequest>, ScriptError> {
    use crate::player::cast_member::CastMemberType;
    w3d_require_live_player(runtime)?;
    let symbols = &runtime.symbols;

    // Gather pending fires across all members in one borrow, then allocate
    // argument handles after that borrow ends. Each entry is
    // (instance, handler, type/delta/time/duration/systemTime).
    let prepared = {
        let player = &mut runtime.player;
        let mut prepared = Vec::new();
        let cast_count = player.movie.cast_manager.casts.len();
        for cast_idx in 0..cast_count {
            // We need to walk every member with a 3D runtime. Iterate by
            // (cast_lib, member_number) and fetch via the manager so we can
            // mutate the registered_events list in place.
            let member_numbers: Vec<u32> = player
                .movie
                .cast_manager
                .casts
                .get(cast_idx)
                .map(|c| c.members.keys().copied().collect())
                .unwrap_or_default();
            for member_num in member_numbers {
                let member_ref = crate::player::cast_lib::CastMemberRef {
                    // Cast-library references are one-based, while `casts`
                    // is a zero-based Rust vector. The mutable lookup below
                    // rejects cast_lib=0, so preserve the actual identity.
                    cast_lib: (cast_idx + 1) as i32,
                    cast_member: member_num as i32,
                };
                let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref)
                else { continue };
                let CastMemberType::Shockwave3d(w3d) = &mut member.member_type else { continue };
                let events = &mut w3d.runtime_state.registered_events;
                if events.is_empty() { continue; }
                let mut i = 0;
                while i < events.len() {
                    let Some((instance, handler_name, values, remove)) = ({
                        let ev = &mut events[i];
                        if !ev.event_name.eq_builtin(BuiltInSymbol::TimeMS) {
                            None
                        } else {
                            if let Some((values, remove)) = w3d_timer_fire_values(ev, now_ms) {
                                if !symbols.owns(&ev.handler_name) {
                                    return Err(w3d_invalid("foreign or stale W3D timer handler symbol"));
                                }
                                let instance = ev.script_instance.clone();
                                if let Some(instance) = instance.as_ref() {
                                    if !instance.owner().same_identity(&player.owner)
                                        || player.allocator.get_script_instance_opt(instance).is_none()
                                    {
                                        return Err(w3d_invalid("foreign or stale W3D callback ScriptInstanceRef"));
                                    }
                                }
                                let handler_name = ev.handler_name.clone();
                                ev.fires_so_far += 1;
                                ev.last_fire_ms = now_ms;
                                Some((instance, handler_name, values, remove))
                            } else { None }
                        }
                    }) else {
                        i += 1;
                        continue;
                    };
                    prepared.push((instance, handler_name, values, remove));
                    if remove {
                        events.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
        }
        prepared
    };
    let owner = runtime.player.owner.clone();
    let player = &mut runtime.player;
    let mut fires = Vec::with_capacity(prepared.len());
    for (instance, handler_name, values, _) in prepared {
        let args = values
            .into_iter()
            .map(|v| player.alloc_datum(Datum::Float(v)))
            .collect();
        fires.push(W3dCallbackRequest {
            owner: owner.clone(),
            receiver: match instance {
                Some(instance) => W3dCallbackReceiver::Instance(instance),
                None => W3dCallbackReceiver::Static { target: None },
            },
            handler_name,
            args,
        });
    }
    Ok(fires)
}

/// Per-frame W3D animation clock advance.
///
/// The renderer was advancing its own `self.animation_time` and never wrote
/// back to `runtime_state.animation_time`. Anything else reading the
/// runtime-state field (e.g. `bone[n].worldTransform` getters used by
/// ClubMarian to pin the head to the body's bone[6]) saw a stale zero, so
/// the body animated but the head stayed frozen at the motion's first
/// frame. Move the dt advance into a per-frame tick on the shared runtime
/// state so every reader sees the same time.
pub(crate) fn tick_w3d_animations(
    runtime: &mut ExecutionContext<'_>,
    dt_seconds: f32,
) -> Result<(), ScriptError> {
    use crate::player::cast_member::CastMemberType;
    w3d_require_live_player(runtime)?;
    let symbols = &runtime.symbols;
    let player = &mut runtime.player;
    for cast in player.movie.cast_manager.casts.iter_mut() {
            for (_, member) in cast.members.iter_mut() {
                if let CastMemberType::Shockwave3d(w3d) = &mut member.member_type {
                    // ── Per-MODEL bonesPlayer clocks ──
                    // Each model animates independently (Rasterwerks clones N bots into
                    // one member; sharing a single clock froze/cross-posed them). Snapshot
                    // motion durations first (immutable scene borrow) for end-detection.
                    let motion_durations: Vec<(String, f32)> = match w3d.parsed_scene.as_ref() {
                        Some(scene) => scene.motions.iter()
                            .map(|m| Ok((w3d_symbol_lower(symbols, &m.name, "motion name")?, m.duration())))
                            .collect::<Result<_, ScriptError>>()?,
                        None => Vec::new(),
                    };
                    let dur_of = |name: &Symbol| -> Result<f32, ScriptError> {
                        let nl = w3d_symbol_display_ascii_lower(symbols, name, "motion name")?;
                        Ok(motion_durations.iter().find(|(n, _)| *n == nl).map(|(_, d)| *d).unwrap_or(0.0))
                    };
                    for bp in w3d.runtime_state.bones_players.values_mut() {
                        if !bp.animation_playing || bp.motion_ended { continue; }
                        bp.animation_time += dt_seconds * bp.play_rate * bp.animation_scale;
                        if bp.blend_weight < 1.0 && bp.blend_duration > 0.0 {
                            bp.blend_elapsed += dt_seconds;
                            bp.blend_weight = (bp.blend_elapsed / bp.blend_duration).min(1.0);
                        }
                        // Non-looping end: advance the per-model queue, else hold last frame.
                        if !bp.animation_loop {
                            if let Some(cur) = bp.current_motion.clone() {
                                let duration = dur_of(&cur)?;
                                let eff_end = if bp.animation_end_time >= 0.0 {
                                    bp.animation_end_time.min(duration)
                                } else { duration };
                                if eff_end > 0.0 && bp.animation_time >= eff_end {
                                    if !bp.motion_queue.is_empty() {
                                        let queued_name = bp.motion_queue[0].name.clone();
                                        w3d_symbol_lower(symbols, &queued_name, "queued motion name")?;
                                        let q = bp.motion_queue.remove(0);
                                        bp.current_motion = Some(q.name.clone());
                                        bp.animation_loop = q.looped;
                                        bp.animation_start_time = q.start_time;
                                        bp.animation_end_time = q.end_time;
                                        bp.animation_scale = q.scale;
                                        bp.animation_time = if q.offset >= 0.0 { q.offset } else { q.start_time };
                                        bp.motion_ended = false;
                                        bp.previous_motion = None;
                                        bp.blend_weight = 1.0;
                                    } else {
                                        bp.animation_time = eff_end; // hold final frame
                                        bp.motion_ended = true;
                                    }
                                }
                            }
                        }
                    }

                    // ── Legacy member clock (keyframe / motion_transforms path) ──
                    if w3d.runtime_state.animation_playing {
                        let rate = w3d.runtime_state.play_rate;
                        let scale = w3d.runtime_state.animation_scale;
                        w3d.runtime_state.animation_time += dt_seconds * rate * scale;
                        if w3d.runtime_state.blend_weight < 1.0 && w3d.runtime_state.blend_duration > 0.0 {
                            w3d.runtime_state.blend_elapsed += dt_seconds;
                            w3d.runtime_state.blend_weight =
                                (w3d.runtime_state.blend_elapsed / w3d.runtime_state.blend_duration).min(1.0);
                        }
                    }
                }
            }
    }
    Ok(())
}

/// Advance every W3D member's #particle systems once per frame. Separate from
/// tick_w3d_animations because particles run regardless of animation_playing
/// (that tick early-returns for members with no playing motion). For each
/// particle model resource we sync the emitter parameters (set via
/// `resource.emitter.*` into runtime_state.emitters) and the world emit position
/// (from the model node that references the resource) into the sim, then step it.
pub(crate) fn tick_w3d_particles(
    runtime: &mut ExecutionContext<'_>,
    dt: f32,
) -> Result<(), ScriptError> {
    use crate::player::cast_member::CastMemberType;
    w3d_require_live_player(runtime)?;
    let symbols = &runtime.symbols;
    let player = &mut runtime.player;
    for cast in player.movie.cast_manager.casts.iter_mut() {
            for (_, member) in cast.members.iter_mut() {
                if let CastMemberType::Shockwave3d(w3d) = &mut member.member_type {
                    if w3d.runtime_state.particles.is_empty() { continue; }
                    let scene = w3d.parsed_scene.clone();
                    let names: Vec<Symbol> = w3d.runtime_state.particles.keys().cloned().collect();
                    for name in names {
                        w3d_symbol_display(symbols, &name, "particle resource")?;
                        // Emitter params (cloned) and the emit position from the model
                        // node that references this resource — gathered before the
                        // mutable particle borrow to avoid overlapping borrows.
                        let em = w3d.runtime_state.emitters.get(&name).cloned();
                        let world_pos = if let Some(sc) = scene.as_ref() {
                            let mut world_pos = None;
                            for n in &sc.nodes {
                                w3d_symbol_display(symbols, &n.model_resource_name, "particle model resource")?;
                                let model_match = n.model_resource_name == name;
                                let resource_match = if model_match {
                                    false
                                } else {
                                    w3d_symbol_display(symbols, &n.resource_name, "particle resource")?;
                                    n.resource_name == name
                                };
                                if model_match || resource_match {
                                    w3d_symbol_display(symbols, &n.name, "particle node")?;
                                    let t = w3d.runtime_state.node_transforms.get(&n.name)
                                        .copied()
                                        .unwrap_or(n.transform);
                                    world_pos = Some([t[12], t[13], t[14]]);
                                    break;
                                }
                            }
                            world_pos.unwrap_or([0.0, 0.0, 0.0])
                        } else {
                            [0.0, 0.0, 0.0]
                        };

                        if let Some(ps) = w3d.runtime_state.particles.get_mut(&name) {
                            // Emit from emitter.region when a script set it (e.g. the car demos
                            // track the exhaust pipe via `emitter.region = [exhaust.worldPosition]`),
                            // otherwise from the model node's world position (the faucet translates
                            // its ColdWater model). Without this the car smoke emitted at the origin
                            // and whited out the camera, blanking the whole 3D scene.
                            ps.emitter_position = match &em {
                                Some(e) if e.has_region =>
                                    [e.region[0] as f32, e.region[1] as f32, e.region[2] as f32],
                                _ => world_pos,
                            };
                            if let Some(em) = &em {
                                let d = em.direction;
                                let len = (d[0]*d[0] + d[1]*d[1] + d[2]*d[2]).sqrt();
                                if len > 1e-6 {
                                    ps.direction = [(d[0]/len) as f32, (d[1]/len) as f32, (d[2]/len) as f32];
                                }
                                ps.initial_speed = em.min_speed as f32;
                                ps.max_speed = em.max_speed.max(em.min_speed) as f32;
                                ps.speed_range = (em.max_speed - em.min_speed).max(0.0) as f32;
                                // emitter.angle is the cone half-angle in degrees.
                                ps.angle_range = (em.angle as f32).to_radians();
                                ps.stream = em.mode.eq_ignore_ascii_case("stream");
                                ps.loop_enabled = em.is_loop;
                                // `emitter.numParticles = 0` is how scripts silence an
                                // effect (Agent Free Ride's Particles.ImmediateStop);
                                // skipping n == 0 left the system emitting its previous
                                // count forever, at whatever region it last had — four
                                // idle riders' powder jets sat at the world origin as
                                // white blobs on the horizon.
                                let n = (em.num_particles.max(0) as usize).min(10000);
                                if ps.max_particles != n {
                                    ps.initialize(n);
                                }
                            }
                            if ps.positions.is_empty() && ps.max_particles > 0 {
                                ps.initialize(ps.max_particles);
                            }
                            ps.update(dt);
                        }
                    }
                }
            }
    }
    Ok(())
}

// ─── Native Shockwave3D #collision modifier helpers ───
const W3D_COL_IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Column-major 4x4 multiply (m[col*4+row]).
fn w3d_col_mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            r[col * 4 + row] = a[row] * b[col * 4]
                + a[4 + row] * b[col * 4 + 1]
                + a[8 + row] * b[col * 4 + 2]
                + a[12 + row] * b[col * 4 + 3];
        }
    }
    r
}

/// Transform a point by a column-major 4x4 (w = 1).
fn w3d_col_xform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Per-frame native W3D collision detection (Director #collision modifier).
///
/// `addModifier(#collision)` registers a `W3dCollisionModifier` per model; this
/// sweeps every enabled collision model in each 3D member, builds a world-space
/// AABB for each (from the model resource's primitive dimensions and the node's
/// accumulated world transform), tests all pairs for overlap, and fires each
/// colliding model's `setCollisionCallback` handler with a `collisionData`
/// property list (`#modelA / #modelB / #pointOfContact / #collisionNormal`).
/// Only detection + dispatch are implemented; resolution is intentionally a
/// no-op (every consumer so far sets `collision.resolve = 0`).
pub(crate) fn prepare_w3d_collision_callbacks(
    runtime: &mut ExecutionContext<'_>,
    dirty_transforms: &W3dDirtyTransformInput,
) -> Result<Vec<W3dCallbackRequest>, ScriptError> {
    use crate::director::lingo::datum::Shockwave3dObjectRef;
    use crate::player::cast_member::CastMemberType;
    use std::collections::VecDeque;
    w3d_require_live_player(runtime)?;

    struct ColModel {
        name: String,
        min: [f32; 3],
        max: [f32; 3],
        immovable: bool,
        handler: Option<String>,
        instance: Option<ScriptInstanceRef>,
        target: Option<crate::director::lingo::datum::Datum>,
    }
    // A member-level #collideAny registration (registerForEvent(#collideAny, …)).
    struct CollideAnyReg {
        handler: String,
        instance: Option<ScriptInstanceRef>,
    }
    struct Fire {
        instance: Option<ScriptInstanceRef>,
        handler: String,
        data: DatumRef,
        target: Option<DatumRef>,
    }

    sync_w3d_dirty_transforms(&mut runtime.player, runtime.symbols, dirty_transforms)?;
    let symbols = &mut runtime.symbols;
    let owner = runtime.player.owner.clone();
    let fires: Vec<Fire> = {
        let player = &mut runtime.player;
        // Flush any live transform datums (model.transform.position = ...) into
        // the node_transforms cache so collision reads current positions rather
        // than the previous render's snapshot.
        let mut fires: Vec<Fire> = Vec::new();
        let cast_count = player.movie.cast_manager.casts.len();
        for cast_idx in 0..cast_count {
            let member_numbers: Vec<u32> = player
                .movie
                .cast_manager
                .casts
                .get(cast_idx)
                .map(|c| c.members.keys().copied().collect())
                .unwrap_or_default();
            for member_num in member_numbers {
                let member_ref = CastMemberRef {
                    cast_lib: cast_idx as i32,
                    cast_member: member_num as i32,
                };

                // Gather world AABBs + callbacks for every enabled collision
                // model in this member (owned, so the borrow ends before alloc).
                let (models, collide_any_regs): (Vec<ColModel>, Vec<CollideAnyReg>) = {
                    let Some(member) = player.movie.cast_manager.find_member_by_ref(&member_ref)
                    else { continue };
                    let CastMemberType::Shockwave3d(w3d) = &member.member_type else { continue };
                    if w3d.runtime_state.collision_modifiers.is_empty() { continue; }
                    let Some(scene) = w3d.parsed_scene.as_ref() else { continue };
                    let rs = &w3d.runtime_state;

                    // Local transform of a node: runtime override (case-insensitive)
                    // falling back to the parsed scene node.
                    let local_tf = |nm: Symbol| -> Result<[f32; 16], ScriptError> {
                        w3d_symbol_lower(symbols, &nm, "node name")?;
                        if let Some(transform) = rs.node_transforms.get(&nm).copied() {
                            return Ok(transform);
                        }
                        for n in &scene.nodes {
                            if w3d_symbol_eq_ascii_case_symbol(symbols, &n.name, &nm, "scene node name")? {
                                return Ok(n.transform);
                            }
                        }
                        Ok(W3D_COL_IDENTITY)
                    };

                    let mut out: Vec<ColModel> = Vec::new();
                    for (mname, cmod) in rs.collision_modifiers.iter() {
                        if !cmod.enabled { continue; }
                        w3d_symbol_lower(symbols, mname, "collision model name")?;
                        let mut detached = false;
                        for d in &rs.detached_nodes {
                            if w3d_symbol_eq_ascii_case_symbol(symbols, d, mname, "detached node name")? {
                                detached = true;
                                break;
                            }
                        }
                        if detached { continue; }
                        let mut node = None;
                        for n in &scene.nodes {
                            if w3d_symbol_eq_ascii_case_symbol(symbols, &n.name, mname, "scene node name")? {
                                node = Some(n);
                                break;
                            }
                        }
                        let Some(node) = node else { continue };

                        // Local half-extents from the model resource's primitive dims.
                        let res_key = if !node.model_resource_name.is_empty() {
                            &node.model_resource_name
                        } else {
                            &node.resource_name
                        };
                        let prim_he = scene
                            .model_resources
                            .get(&res_key)
                            .and_then(|r| match r.primitive_type.as_deref().unwrap_or("") {
                                // box: width=X, height=Y, length=Z (see primitive build).
                                "box" => Some([
                                    r.primitive_width * 0.5,
                                    r.primitive_height * 0.5,
                                    r.primitive_length * 0.5,
                                ]),
                                "sphere" => {
                                    let s = r.primitive_radius.max(0.01);
                                    Some([s, s, s])
                                }
                                // cylinder axis is Y; radius spans X/Z.
                                "cylinder" => {
                                    let rad = r.primitive_radius.max(r.primitive_top_radius).max(0.01);
                                    Some([rad, r.primitive_height * 0.5, rad])
                                }
                                "plane" => Some([r.primitive_width * 0.5, 0.05, r.primitive_length * 0.5]),
                                // Mesh model (no primitive dims): fall through to mesh bounds.
                                _ => None,
                            });
                        // For mesh models, size the collision box from the actual
                        // geometry so #collision matches the visible model. (The old
                        // 0.5-unit fallback never reached On the Run's bonuses, which
                        // float ~1.5 above the road.) Half-extents are taken about the
                        // model origin (max |min|,|max| per axis), matching the
                        // origin-centred ±he box used below.
                        let he = if let Some(he) = prim_he {
                            he
                        } else {
                            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
                            let mut any = false;
                            if let Some(meshes) = scene.clod_meshes.get(&res_key) {
                                for m in meshes {
                                    for p in &m.positions {
                                        any = true;
                                        for k in 0..3 { if p[k] < mn[k] { mn[k] = p[k]; } if p[k] > mx[k] { mx[k] = p[k]; } }
                                    }
                                }
                            }
                            w3d_symbol_lower(symbols, res_key, "mesh resource name")?;
                            for rm in &scene.raw_meshes {
                                if !w3d_symbol_eq_ascii_case_symbol(symbols, &rm.name, res_key, "raw mesh name")? {
                                    continue;
                                }
                                for p in &rm.positions {
                                    any = true;
                                    for k in 0..3 { if p[k] < mn[k] { mn[k] = p[k]; } if p[k] > mx[k] { mx[k] = p[k]; } }
                                }
                            }
                            if any {
                                [
                                    mn[0].abs().max(mx[0].abs()).max(0.05),
                                    mn[1].abs().max(mx[1].abs()).max(0.05),
                                    mn[2].abs().max(mx[2].abs()).max(0.05),
                                ]
                            } else {
                                [0.5, 0.5, 0.5]
                            }
                        };

                        // Accumulate the world transform up the parent chain.
                        let mut world = local_tf(node.name.clone())?;
                        let mut cur_parent = node.parent_name.clone();
                        for _ in 0..32 {
                            if cur_parent.is_empty()
                                || w3d_symbol_eq_ascii_case(symbols, &cur_parent, "World", "parent name")?
                            {
                                break;
                            }
                            let pm = local_tf(cur_parent.clone())?;
                            world = w3d_col_mat_mul(&pm, &world);
                            w3d_symbol_lower(symbols, &cur_parent, "parent name")?;
                            cur_parent = {
                                let mut parent = None;
                                for n in &scene.nodes {
                                    if w3d_symbol_eq_ascii_case_symbol(symbols, &n.name, &cur_parent, "scene node name")? {
                                        parent = Some(n.parent_name.clone());
                                        break;
                                    }
                                }
                                parent.unwrap_or_default()
                            };
                        }

                        // World AABB from the 8 transformed local-box corners.
                        let mut min = [f32::MAX; 3];
                        let mut max = [f32::MIN; 3];
                        for &sx in &[-he[0], he[0]] {
                            for &sy in &[-he[1], he[1]] {
                                for &sz in &[-he[2], he[2]] {
                                    let wp = w3d_col_xform_point(&world, [sx, sy, sz]);
                                    for k in 0..3 {
                                        if wp[k] < min[k] { min[k] = wp[k]; }
                                        if wp[k] > max[k] { max[k] = wp[k]; }
                                    }
                                }
                            }
                        }

                        out.push(ColModel {
                            name: w3d_symbol_display(symbols, &node.name, "collision model")?,
                            min,
                            max,
                            immovable: cmod.immovable,
                            handler: cmod.callback_handler.clone(),
                            instance: cmod.callback_instance.clone(),
                            target: cmod.callback_target.clone(),
                        });
                    }
                    // Member-level #collideAny handlers (registerForEvent(#collideAny,
                    // handler, scriptInstance)) — fired once per colliding pair below.
                    let mut collide_any = Vec::new();
                    for e in &rs.registered_events {
                        if w3d_symbol_eq_ascii_case(symbols, &e.event_name, "collideAny", "event name")?
                            && !e.handler_name.is_empty()
                        {
                            collide_any.push(CollideAnyReg {
                                handler: w3d_symbol_display(symbols, &e.handler_name, "collision handler")?,
                                instance: e.script_instance.clone(),
                            });
                        }
                    }
                    (out, collide_any)
                };

                // Test all pairs; for each overlapping pair, fire the callback of
                // every model in the pair that has one, with that model as modelA.
                for i in 0..models.len() {
                    for j in (i + 1)..models.len() {
                        let a = &models[i];
                        let b = &models[j];
                        let overlap = a.min[0] <= b.max[0] && a.max[0] >= b.min[0]
                            && a.min[1] <= b.max[1] && a.max[1] >= b.min[1]
                            && a.min[2] <= b.max[2] && a.max[2] >= b.min[2];
                        if !overlap { continue; }

                        let mid = [
                            ((a.min[0] + a.max[0] + b.min[0] + b.max[0]) * 0.25) as f64,
                            ((a.min[1] + a.max[1] + b.min[1] + b.max[1]) * 0.25) as f64,
                            ((a.min[2] + a.max[2] + b.min[2] + b.max[2]) * 0.25) as f64,
                        ];
                        for (selfm, otherm) in [(a, b), (b, a)] {
                            let Some(handler) = selfm.handler.clone() else { continue };
                            // collisionData property list: modelA is the model whose
                            // callback is firing; the handlers check both modelA and
                            // modelB, so this is consistent with Director.
                            let model_a = player.alloc_datum(Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
                                cast_lib: member_ref.cast_lib,
                                cast_member: member_ref.cast_member,
                                object_type: BuiltInSymbol::Model,
                                name: symbols.intern(&selfm.name),
                            }));
                            let model_b = player.alloc_datum(Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
                                cast_lib: member_ref.cast_lib,
                                cast_member: member_ref.cast_member,
                                object_type: BuiltInSymbol::Model,
                                name: symbols.intern(&otherm.name),
                            }));
                            let poc = player.alloc_datum(Datum::Vector(mid));
                            let normal = player.alloc_datum(Datum::Vector([1.0, 0.0, 0.0]));
                            let k_a = player.alloc_datum(Datum::Symbol(symbols.intern("modelA")));
                            let k_b = player.alloc_datum(Datum::Symbol(symbols.intern("modelB")));
                            let k_poc = player.alloc_datum(Datum::Symbol(symbols.intern("pointOfContact")));
                            let k_n = player.alloc_datum(Datum::Symbol(symbols.intern("collisionNormal")));
                            let pairs: VecDeque<(DatumRef, DatumRef)> = VecDeque::from(vec![
                                (k_a, model_a),
                                (k_b, model_b),
                                (k_poc, poc),
                                (k_n, normal),
                            ]);
                            let data = player.alloc_datum(Datum::PropList(pairs, false));
                            let target = selfm.target.clone().map(|t| player.alloc_datum(t));
                            fires.push(Fire {
                                instance: selfm.instance.clone(),
                                handler,
                                data,
                                target,
                            });
                        }

                        // Member-level #collideAny handlers (registerForEvent) fire
                        // once per colliding pair. Director 11.5 `collisionData`:
                        // modelA / modelB / pointOfContact / collisionNormal — modelA
                        // and modelB are "one"/"the other" of the pair. Order the
                        // immovable model as modelB, the convention On the Run's bonus
                        // pickup relies on (collisionData.modelB = the bonus hit).
                        if !collide_any_regs.is_empty() {
                            let (ma, mb) = if a.immovable && !b.immovable { (b, a) } else { (a, b) };
                            for reg in &collide_any_regs {
                                let model_a = player.alloc_datum(Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
                                    cast_lib: member_ref.cast_lib,
                                    cast_member: member_ref.cast_member,
                                    object_type: BuiltInSymbol::Model,
                                    name: symbols.intern(&ma.name),
                                }));
                                let model_b = player.alloc_datum(Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
                                    cast_lib: member_ref.cast_lib,
                                    cast_member: member_ref.cast_member,
                                    object_type: BuiltInSymbol::Model,
                                    name: symbols.intern(&mb.name),
                                }));
                                let poc = player.alloc_datum(Datum::Vector(mid));
                                let normal = player.alloc_datum(Datum::Vector([1.0, 0.0, 0.0]));
                                let k_a = player.alloc_datum(Datum::Symbol(symbols.intern("modelA")));
                                let k_b = player.alloc_datum(Datum::Symbol(symbols.intern("modelB")));
                                let k_poc = player.alloc_datum(Datum::Symbol(symbols.intern("pointOfContact")));
                                let k_n = player.alloc_datum(Datum::Symbol(symbols.intern("collisionNormal")));
                                let pairs: VecDeque<(DatumRef, DatumRef)> = VecDeque::from(vec![
                                    (k_a, model_a),
                                    (k_b, model_b),
                                    (k_poc, poc),
                                    (k_n, normal),
                                ]);
                                let data = player.alloc_datum(Datum::PropList(pairs, false));
                                fires.push(Fire {
                                    instance: reg.instance.clone(),
                                    handler: reg.handler.clone(),
                                    data,
                                    target: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        fires
    };
    let mut requests = Vec::with_capacity(fires.len());
    for fire in fires {
        if let Some(instance) = fire.instance.as_ref() {
            w3d_check_instance(runtime, instance)?;
        }
        w3d_check_ref(runtime, &fire.data, "collision data")?;
        if fire.instance.is_none() {
            if let Some(target) = fire.target.as_ref() {
            w3d_check_ref(runtime, target, "collision target")?;
            }
        }
        let handler_name = runtime.symbols.intern(&fire.handler);
        let receiver = match fire.instance {
            Some(instance) => W3dCallbackReceiver::Instance(instance),
            None => W3dCallbackReceiver::Static { target: fire.target },
        };
        let args = match &receiver {
            W3dCallbackReceiver::Static { target: Some(target) } => vec![target.clone(), fire.data],
            _ => vec![fire.data],
        };
        requests.push(W3dCallbackRequest {
            owner: owner.clone(),
            receiver,
            handler_name,
            args,
        });
    }
    Ok(requests)
}

/// Drain queued PhysX collision reports across all PhysX members and
/// dispatch them to the script's registered `collisionCallback` handler.
///
/// PhysX simulate() runs synchronously inside a Lingo call and only queues
/// collisions onto `physx.state.pending_collisions`. AGEIA's xtra fires the
/// registered callback automatically after each simulate; we approximate
/// that by sweeping queued reports here, after prepareFrame behaviors (the
/// usual home of simulate()), and invoking the handler globally.
///
/// The arg shape mirrors `notify_collisions`: one positional `collisions`
/// list, each entry a PropList of `#objectA / #objectB / #contactPoints /
/// #contactNormals`. ClubMarian's `on collisionCallback collisions` reads
/// `collisions[i].objectA.name` to test for terrain — relies on this.
pub(crate) fn prepare_physx_collision_callbacks(
    runtime: &mut ExecutionContext<'_>,
) -> Result<Vec<W3dCallbackRequest>, ScriptError> {
    use crate::director::lingo::datum::{DatumType, PhysXObjectRef};
    use crate::player::cast_member::CastMemberType;
    use std::collections::VecDeque;
    w3d_require_live_player(runtime)?;

    struct Fire {
        handler_name: Symbol,
        cast_lib: i32,
        cast_member: i32,
        // (a_id, b_id, a_name, b_name, points, normals)
        collisions: Vec<(u32, u32, Option<Symbol>, Option<Symbol>, Vec<[f64; 3]>, Vec<[f64; 3]>)>,
    }

    let fires: Vec<Fire> = {
        let symbols = &runtime.symbols;
        let player = &mut runtime.player;
        let mut fires: Vec<Fire> = Vec::new();
        let cast_count = player.movie.cast_manager.casts.len();
        for cast_idx in 0..cast_count {
            let member_numbers: Vec<u32> = player
                .movie
                .cast_manager
                .casts
                .get(cast_idx)
                .map(|c| c.members.keys().copied().collect())
                .unwrap_or_default();
            for member_num in member_numbers {
                let member_ref = CastMemberRef {
                    cast_lib: cast_idx as i32,
                    cast_member: member_num as i32,
                };
                let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref)
                else { continue };
                let CastMemberType::PhysXPhysics(physx) = &mut member.member_type else { continue };
                if physx.state.pending_collisions.is_empty() { continue; }
                let Some(handler) = physx.state.collision_callback_handler.clone() else {
                    // No handler registered — drop the queue so it doesn't grow
                    // unbounded. Director's xtra discards reports the script
                    // never asked to hear about.
                    physx.state.pending_collisions.clear();
                    continue;
                };
                if !symbols.owns(&handler) {
                    return Err(w3d_invalid("foreign or stale PhysX callback handler symbol"));
                }
                // Resolve body names while we still hold the borrow.
                let body_names: std::collections::HashMap<u32, Symbol> =
                    physx.state.bodies.iter().map(|b| (b.id, b.name.clone())).collect();
                for (a, b, _, _) in &physx.state.pending_collisions {
                    for id in [a, b] {
                        if let Some(name) = body_names.get(id) {
                            if !symbols.owns(name) {
                                return Err(w3d_invalid("foreign or stale PhysX body name symbol"));
                            }
                        }
                    }
                }
                let mut drained: Vec<(u32, u32, Vec<[f64; 3]>, Vec<[f64; 3]>)> = Vec::new();
                std::mem::swap(&mut drained, &mut physx.state.pending_collisions);
                let collisions = drained.into_iter().map(|(a, b, pts, nms)| {
                    let na = body_names.get(&a).cloned();
                    let nb = body_names.get(&b).cloned();
                    (a, b, na, nb, pts, nms)
                }).collect();
                fires.push(Fire {
                    handler_name: handler,
                    cast_lib: cast_idx as i32,
                    cast_member: member_num as i32,
                    collisions,
                });
            }
        }
        fires
    };
    let owner = runtime.player.owner.clone();
    let player = &mut runtime.player;
    let mut requests = Vec::with_capacity(fires.len());
    for fire in fires {
        let arg_refs = {
            let cast_lib = fire.cast_lib;
            let cast_member = fire.cast_member;
            let mut reports = VecDeque::new();
            for (a_id, b_id, name_a, name_b, points, normals) in &fire.collisions {
                let a_ref = match name_a {
                    Some(n) => player.alloc_datum(Datum::PhysXObjectRef(PhysXObjectRef {
                        cast_lib, cast_member,
                        object_type: BuiltInSymbol::RigidBody,
                        id: *a_id, name: n.clone(),
                    })),
                    None => DatumRef::Void,
                };
                let b_ref = match name_b {
                    Some(n) => player.alloc_datum(Datum::PhysXObjectRef(PhysXObjectRef {
                        cast_lib, cast_member,
                        object_type: BuiltInSymbol::RigidBody,
                        id: *b_id, name: n.clone(),
                    })),
                    None => DatumRef::Void,
                };
                let mut pts = VecDeque::new();
                for p in points { pts.push_back(player.alloc_datum(Datum::Vector(*p))); }
                let pts_list = player.alloc_datum(Datum::List(DatumType::List, pts, false));
                let mut nms = VecDeque::new();
                for n in normals { nms.push_back(player.alloc_datum(Datum::Vector(*n))); }
                let nms_list = player.alloc_datum(Datum::List(DatumType::List, nms, false));
                let key_a = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ObjectA)));
                let key_b = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ObjectB)));
                let key_pts = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ContactPoints)));
                let key_nms = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ContactNormals)));
                let mut props = VecDeque::new();
                props.push_back((key_a, a_ref));
                props.push_back((key_b, b_ref));
                props.push_back((key_pts, pts_list));
                props.push_back((key_nms, nms_list));
                reports.push_back(player.alloc_datum(Datum::PropList(props, false)));
            }
            let collisions_list = player.alloc_datum(Datum::List(DatumType::List, reports, false));
            vec![collisions_list]
        };
        requests.push(W3dCallbackRequest {
            owner: owner.clone(),
            receiver: W3dCallbackReceiver::Global,
            handler_name: fire.handler_name,
            args: arg_refs,
        });
    }
    Ok(requests)
}

fn event_owner_is_live(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) -> bool {
    owner.is_arena_live()
        && session
            .borrow_mut()
            .with_player(player_id, |context| owner.same_identity(&context.player.owner))
            .unwrap_or(false)
}

fn drain_event_owner_reclaims(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) {
    let _ = session.borrow_mut().with_player(player_id, |context| {
        if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
            context.player.drain_allocator_reclaims();
        }
    });
}

/// Retire only a stale nested child when its owner-bound event loop exits.
/// Identity is checked inside RuntimeSession so a numeric id reused by a
/// replacement player cannot be touched.  Host teardown is deliberately
/// drained after the RefMut is released because cleanup may re-enter session.
fn retire_stale_nested_event_owner(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) {
    let retired = session
        .try_borrow_mut()
        .map(|mut runtime| runtime.retire_nested_child_if_identity(player_id, owner))
        .unwrap_or(false);
    if retired {
        crate::player::commands::drain_host_teardowns(session);
    }
}

async fn wait_event_owner_available(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) -> Result<bool, ScriptError> {
    let playing = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.is_playing)
        })
        .ok_or_else(cancelled_scope_error)??;
    if !playing {
        return Ok(false);
    }

    crate::player::wait_for_handler_gap_owned(session.clone(), player_id, owner.clone()).await?;
    Ok(session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.is_playing)
        })
        .ok_or_else(cancelled_scope_error)??)
}

pub async fn run_event_loop(
    rx: Receiver<PlayerVMEvent>,
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) {
    warn!("Starting owner-bound event loop for player {}", player_id);

    // A bounded receive wait lets reset/shutdown retire an idle loop without
    // advancing simulation time or consulting a global player.
    while !rx.is_closed() {
        if !event_owner_is_live(&session, player_id, &owner) {
            warn!("Event loop stopped (owner retired)");
            retire_stale_nested_event_owner(&session, player_id, &owner);
            return;
        }

        let item = match async_std::future::timeout(
            std::time::Duration::from_millis(4),
            rx.recv(),
        )
        .await
        {
            Ok(Ok(item)) => item,
            Ok(Err(_)) => break,
            Err(_) => continue,
        };

        match wait_event_owner_available(&session, player_id, &owner).await {
            Ok(true) => {}
            Ok(false) => {
                drop(item);
                drain_event_owner_reclaims(&session, player_id, &owner);
                continue;
            }
            Err(_) => {
                retire_stale_nested_event_owner(&session, player_id, &owner);
                return;
            }
        }

        // Skip event processing when a command handler (mouseDown, keyDown, etc.)
        // is actively executing. The event loop and command loop share the scope
        // stack, so processing events (which push/pop scopes) during a command
        // handler's async yield (e.g. nothing()) would corrupt the scope data.
        // Ephemeral events like mouseWithin will be re-dispatched on the next tick.
        // A score/puppet transition is modal in Director (input is ignored while it
        // plays), so hold events during it too; it self-clears when the animation
        // completes (renderer-driven, with a time failsafe) and can't get stuck.
        // User input resets the idle-timeout counter (Director: mouse CLICKS and
        // keystrokes restart the timeout period, gated by timeoutMouse /
        // timeoutKeyDown — mouse MOVEMENT does not).
        {
            let name = match &item {
                PlayerVMEvent::Global(n, _) => n.into_builtin().map(|builtin| builtin.as_str()),
                PlayerVMEvent::Targeted(n, _, _) => n.into_builtin().map(|builtin| builtin.as_str()),
                _ => None,
            };
            if let Some(name) = name {
                let is_mouse =
                    matches!(name, "mouseDown" | "mouseUp" | "rightMouseDown" | "rightMouseUp");
                let is_key = matches!(name, "keyDown" | "keyUp");
                if is_mouse || is_key {
                    let _ = session.borrow_mut().with_player(player_id, |context| {
                        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                            return;
                        }
                        if (is_mouse && context.player.movie.timeout_mouse)
                            || (is_key && context.player.movie.timeout_keydown)
                        {
                            context.player.movie.timeout_last_reset_ms =
                                crate::player::testing_shared::now_ms();
                        }
                    });
                }
            }
        }

        let skip = match session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return None;
            }
            Some(
                context.player.in_mouse_command
                    || context.player.command_handler_yielding
                    || context.player.is_in_transition
                    || context.player.transition_hold_active(),
            )
        }) {
            Some(Some(skip)) => skip,
            _ => {
                retire_stale_nested_event_owner(&session, player_id, &owner);
                return;
            }
        };
        if skip {
            drop(item);
            drain_event_owner_reclaims(&session, player_id, &owner);
            continue;
        }

        if crate::player::wait_for_handler_gap_owned(
            session.clone(),
            player_id,
            owner.clone(),
        )
        .await
        .is_err()
        {
            retire_stale_nested_event_owner(&session, player_id, &owner);
            return;
        }
        let final_state = session.borrow_mut().with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return None;
            }
            Some((
                context.player.is_playing,
                context.player.in_mouse_command
                    || context.player.command_handler_yielding
                    || context.player.is_in_transition
                    || context.player.transition_hold_active(),
            ))
        });
        match final_state {
            Some(Some((true, false))) => {}
            Some(Some((false, _))) | Some(Some((true, true))) => {
                drop(item);
                drain_event_owner_reclaims(&session, player_id, &owner);
                continue;
            }
            _ => {
                retire_stale_nested_event_owner(&session, player_id, &owner);
                return;
            }
        }

        let result = match item {
            PlayerVMEvent::Global(name, args) => {
                dispatch_global_vm_event_owned(
                    Some(&session),
                    player_id,
                    Some(&owner),
                    PlayerVMEvent::Global(name, args),
                )
                .await
            }
            PlayerVMEvent::Targeted(name, args, instances) => {
                dispatch_targeted_vm_event_owned(
                    Some(&session),
                    player_id,
                    Some(&owner),
                    PlayerVMEvent::Targeted(name, args, instances),
                )
                .await
            }
            PlayerVMEvent::Callback(receiver, name, args) => {
                dispatch_callback_vm_event_owned(
                    Some(&session),
                    player_id,
                    Some(&owner),
                    PlayerVMEvent::Callback(receiver, name, args),
                )
                .await
            }
        };
        let _ = result;
        drain_event_owner_reclaims(&session, player_id, &owner);
    }
    retire_stale_nested_event_owner(&session, player_id, &owner);
    warn!("Event loop stopped!");
}

pub fn player_unwrap_result(result: Result<DatumRef, ScriptError>) -> DatumRef {
    match result {
        Ok(result) => result,
        Err(err) => {
            if err.code != ScriptErrorCode::Abort {
                reserve_player_mut(|player| player.on_script_error(&err));
            }
            DatumRef::Void
        }
    }
}

pub async fn player_dispatch_event_beginsprite(
    handler_name: Symbol,
    args: &Vec<DatumRef>
) -> Result<Vec<(ScoreRef, u32)>, ScriptError> {
    let (sprite_instances, frame_instances, all_channels) =
        reserve_player_mut(|player| {
            let mut sprite_instances: Vec<(ScoreRef, usize, ScriptInstanceRef)> = Vec::new();
            let mut frame_instances: Vec<(usize, ScriptInstanceRef)> = Vec::new();
            let mut all_channels = Vec::new();

            // Collect stage sprites - include all entered sprites with behaviors,
            // not just those in sprite_spans. Sprites initialized from channel_initialization_data
            // (D6+ path) also need beginSprite dispatched.
            for channel_number in player.active_stage_behavior_channels() {
                let Some((number, entered, fallback)) = player
                    .movie
                    .score
                    .channels
                    .get(channel_number)
                    .map(|channel| {
                        (
                            channel.number,
                            channel.sprite.entered,
                            channel.sprite.script_instance_list.clone(),
                        )
                    })
                else {
                    continue;
                };

                if !entered {
                    continue;
                }

                let instances = player.get_sprite_script_instance_ids(number as i16, fallback.as_slice());
                if instances.is_empty() || instances.iter().any(|script_ref| {
                    player
                        .allocator
                        .get_script_instance_entry(script_ref.id())
                        .map_or(true, |entry| entry.script_instance.begin_sprite_called)
                }) {
                    continue;
                }

                if number == 0 {
                    // Frame behavior (channel 0)
                    frame_instances.extend(
                        instances.into_iter().map(|inst| (number, inst))
                    );
                } else {
                    // Sprite behaviors (channel > 0) - include ScoreRef::Stage
                    sprite_instances.extend(
                        instances
                            .into_iter()
                            .map(|inst| (ScoreRef::Stage, number, inst))
                    );
                }

                all_channels.push((ScoreRef::Stage, number as u32));
            }

            // Collect filmloop sprites
            let active_filmloops = player.get_active_filmloop_scores();
            for (member_ref, filmloop_current_frame) in active_filmloops {
                let Some(filmloop_score) = player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .and_then(|member| match &member.member_type {
                        super::cast_member::CastMemberType::FilmLoop(film_loop) => {
                            Some(&film_loop.score)
                        }
                        _ => None,
                    })
                else {
                    continue;
                };
                for channel_number in filmloop_score.active_channel_numbers_for_frame(filmloop_current_frame) {
                    let Some(channel) = filmloop_score.channels.get(channel_number) else {
                        continue;
                    };
                    if channel.sprite.script_instance_list.is_empty() || !channel.sprite.entered {
                        continue;
                    }
                    if channel.sprite.script_instance_list.iter().any(|script_ref| {
                        player
                            .allocator
                            .get_script_instance_entry(script_ref.id())
                            .map_or(true, |entry| entry.script_instance.begin_sprite_called)
                    }) {
                        continue;
                    }

                    let instances = channel.sprite.script_instance_list.clone();

                    // Filmloop sprites go into sprite_instances (they don't have frame behaviors)
                    // Include ScoreRef::FilmLoop so we can set the correct context when dispatching
                    if channel.number > 0 {
                        let score_ref = ScoreRef::FilmLoop(member_ref.clone());
                        sprite_instances.extend(
                            instances.into_iter().map(|inst| (score_ref.clone(), channel.number, inst))
                        );
                        all_channels.push((ScoreRef::FilmLoop(member_ref.clone()), channel.number as u32));
                    }
                }
            }

            (sprite_instances, frame_instances, all_channels)
        });
    
    if sprite_instances.is_empty() && frame_instances.is_empty() {
        return Ok(Vec::new());
    }

    if frame_instances.len() > 0 {
        let _ = player_invoke_frame_and_movie_scripts(
            handler_name.clone(),
            args,
        )
        .await;
    }
    
    // Dispatch to sprite behaviors (number > 0)
    // Set the score context before invoking each event so sprite property access works correctly
    for (score_ref, sprite_number, behavior) in sprite_instances {
        // Set the score context for this sprite's behavior
        reserve_player_mut(|player| {
            player.current_score_context = score_ref.clone();
        });

        let receivers = vec![behavior.clone()];
        if let Err(err) = player_invoke_targeted_event(handler_name.clone(), args, Some(receivers).as_ref()).await {
            if err.code == ScriptErrorCode::Abort {
                reserve_player_mut(|player| {
                    player.current_score_context = ScoreRef::Stage;
                });
                return Ok(vec![]);
            }
            web_sys::console::error_1(
                &format!("Error in {} for sprite {}: {}", handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>"), sprite_number, err.message).into()
            );
            reserve_player_mut(|player| {
                player.on_script_error(&err);
            });
        }
    }

    // Reset the score context to Stage after each invocation
    reserve_player_mut(|player| {
        player.current_score_context = ScoreRef::Stage;
    });

    Ok(all_channels)
}

/// Owner-bound result of the beginSprite discovery/dispatch phase. The movie
/// initializer owns the final `begin_sprite_called` mutation: it marks every
/// returned channel after the callback phase, then runs its existing targeted
/// remainder pass for `unhandled_behaviors`. The remainder includes active
/// but non-entered and partially initialized stage behavior refs, matching the
/// legacy initializer's unmarked-behavior scan.
pub(crate) struct BeginSpriteDispatch {
    pub(crate) initialized_channels: Vec<(ScoreRef, u32)>,
    pub(crate) unhandled_behaviors: Vec<ScriptInstanceRef>,
}

#[derive(Clone)]
struct OwnedBeginSpriteTarget {
    score_ref: ScoreRef,
    sprite_number: u32,
    instance: ScriptInstanceRef,
}

/// Dispatch beginSprite with an explicit session owner while preserving the
/// legacy frame/movie-first, stage-sprite, then film-loop order. All handles
/// are validated by their owner before lookup; no VM borrow survives a child
/// callback await.
pub(crate) async fn player_dispatch_event_beginsprite_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: Symbol,
    args: Vec<DatumRef>,
) -> Result<BeginSpriteDispatch, ScriptError> {
    let (frame_instances, targets, initialized_channels, remainder_candidates) = session
        .borrow_mut()
        .with_player(player_id, |mut context| -> Result<_, ScriptError> {
            validate_event_owner(&context, &owner)?;
            // Prime the generation/frame-aware channel cache through the
            // fallible session helper before asking the legacy player method
            // for channel numbers. This prevents its unchecked cache scan
            // from touching a foreign cached scriptInstanceList.
            crate::player::session::active_stage_script_instance_ids_checked(&mut context)?;
            let symbols = &*context.symbols;
            let player = &mut *context.player;
            let mut frame_instances = Vec::new();
            let mut targets = Vec::new();
            let mut initialized_channels = Vec::new();
            let mut remainder_candidates = Vec::new();

            for channel_number in player.active_stage_behavior_channels() {
                let Some((number, entered, fallback)) = player
                    .movie
                    .score
                    .channels
                    .get(channel_number)
                    .map(|channel| (
                        channel.number,
                        channel.sprite.entered,
                        channel.sprite.script_instance_list.clone(),
                    ))
                else {
                    continue;
                };
                // Resolve the cached scriptInstanceList through the checked
                // score helper. The legacy numeric allocator lookup is not an
                // ownership check and could select a stale replacement ref.
                let instances = crate::player::score::get_sprite_script_instance_ids_checked(
                    player,
                    symbols,
                    number as i16,
                    fallback.as_slice(),
                )?;
                if instances.is_empty() {
                    continue;
                }
                remainder_candidates.extend(instances.iter().filter_map(|instance| {
                    player
                        .allocator
                        .get_script_instance_opt(instance)
                        .filter(|value| !value.begin_sprite_called)
                        .map(|_| instance.clone())
                }));
                if !entered
                    || instances.iter().any(|instance| {
                        player
                            .allocator
                            .get_script_instance_opt(instance)
                            .is_some_and(|value| value.begin_sprite_called)
                    })
                {
                    continue;
                }
                initialized_channels.push((ScoreRef::Stage, number as u32));
                if number == 0 {
                    frame_instances.extend(instances);
                } else {
                    targets.extend(instances.into_iter().map(|instance| OwnedBeginSpriteTarget {
                        score_ref: ScoreRef::Stage,
                        sprite_number: number as u32,
                        instance,
                    }));
                }
            }

            for (member_ref, frame) in player.get_active_filmloop_scores() {
                let Some(filmloop_score) = player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .and_then(|member| match &member.member_type {
                        super::cast_member::CastMemberType::FilmLoop(film_loop) => {
                            Some(&film_loop.score)
                        }
                        _ => None,
                    })
                else {
                    continue;
                };
                for channel_number in filmloop_score.active_channel_numbers_for_frame(frame) {
                    let Some(channel) = filmloop_score.channels.get(channel_number) else {
                        continue;
                    };
                    if channel.number == 0
                        || channel.sprite.script_instance_list.is_empty()
                        || !channel.sprite.entered
                    {
                        continue;
                    }
                    let instances = channel.sprite.script_instance_list.clone();
                    if instances.iter().any(|instance| {
                        !instance.owner().same_identity(&player.owner)
                            || player.allocator.get_script_instance_opt(instance).is_none()
                            || player
                                .allocator
                                .get_script_instance_opt(instance)
                                .is_some_and(|value| value.begin_sprite_called)
                    }) {
                        continue;
                    }
                    let score_ref = ScoreRef::FilmLoop(member_ref.clone());
                    initialized_channels.push((score_ref.clone(), channel.number as u32));
                    targets.extend(instances.into_iter().map(|instance| OwnedBeginSpriteTarget {
                        score_ref: score_ref.clone(),
                        sprite_number: channel.number as u32,
                        instance,
                    }));
                }
            }
            Ok((frame_instances, targets, initialized_channels, remainder_candidates))
        })
        .ok_or_else(cancelled_scope_error)??;

    // Legacy beginSprite sends frame and movie scripts before sprite
    // behaviors. Its static helper performs fresh owner-checked lookups.
    let mut handled_instances = frame_instances.clone();
    let mut unhandled_behaviors = Vec::new();

    if !frame_instances.is_empty() {
        let score_scope = OwnedScoreContextScope::enter(
            &session,
            player_id,
            &owner,
            ScoreRef::Stage,
        )?;
        let event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
        let static_result = player_invoke_static_event_owned(
            &session,
            player_id,
            &owner,
            handler_name.clone(),
            &args,
        )
        .await;
        drop(event_scope);
        drop(score_scope);
        if let Err(error) = static_result {
            if error.code == ScriptErrorCode::Abort {
                // The initializer marks returned channels only after this
                // phase completes.  An abort therefore returns no channels,
                // including the frame channel that was discovered above.
                return Ok(BeginSpriteDispatch {
                    initialized_channels: Vec::new(),
                    unhandled_behaviors: Vec::new(),
                });
            }
            let reported = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                        context.player.on_script_error(&error);
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !reported {
                return Err(cancelled_scope_error());
            }
        }
    }

    for target in targets {
        let handler = session
            .borrow_mut()
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                validate_event_owner(&context, &owner)?;
                if !target.instance.owner().same_identity(&context.player.owner)
                    || context.player.allocator.get_script_instance_opt(&target.instance).is_none()
                {
                    return Err(cancelled_scope_error());
                }
                ScriptInstanceUtils::get_script_instance_handler(
                    handler_name.clone(),
                    &target.instance,
                    context.player,
                )
            })
            .ok_or_else(cancelled_scope_error)??;
        let Some(handler) = handler else {
            unhandled_behaviors.push(target.instance);
            continue;
        };
        handled_instances.push(target.instance.clone());

        let score_scope = OwnedScoreContextScope::enter(
            &session,
            player_id,
            &owner,
            target.score_ref.clone(),
        )?;
        let event_scope = OwnedEventStopScope::enter(&session, player_id, &owner)?;
        let result = await_owned_event_handler(
            &session,
            player_id,
            &owner,
            Some(target.instance.clone()),
            handler,
            &args,
        )
        .await;
        drop(event_scope);
        drop(score_scope);

        if let Err(error) = result {
            if error.code == ScriptErrorCode::Abort {
                return Ok(BeginSpriteDispatch {
                    initialized_channels: Vec::new(),
                    unhandled_behaviors: Vec::new(),
                });
            }
            let reported = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                        context.player.on_script_error(&error);
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !reported {
                return Err(cancelled_scope_error());
            }
        }
    }

    for instance in remainder_candidates {
        if !handled_instances.iter().any(|handled| {
            handled.id() == instance.id() && handled.owner().same_identity(instance.owner())
        })
            && !unhandled_behaviors
                .iter()
                .any(|unhandled| {
                    unhandled.id() == instance.id()
                        && unhandled.owner().same_identity(instance.owner())
                })
        {
            unhandled_behaviors.push(instance);
        }
    }

    Ok(BeginSpriteDispatch {
        initialized_channels,
        unhandled_behaviors,
    })
}

pub async fn dispatch_event_endsprite(sprite_nums: Vec<u32>) {
    // Legacy function - calls the new implementation with stage score
    dispatch_event_endsprite_for_score(ScoreRef::Stage, sprite_nums).await;
}

pub async fn dispatch_event_endsprite_for_score(score_ref: ScoreRef, sprite_nums: Vec<u32>) {
    let (sprite_tuple, frame_tuple) =
        reserve_player_mut(|player| {
            let mut sprite_tuple = Vec::new();
            let mut frame_tuple = Vec::new();

            // Get the appropriate score based on score_ref
            let score = match &score_ref {
                ScoreRef::Stage => &player.movie.score,
                ScoreRef::FilmLoop(member_ref) => {
                    match player.movie.cast_manager.find_member_by_ref(member_ref) {
                        Some(member) => {
                            if let super::cast_member::CastMemberType::FilmLoop(film_loop) = &member.member_type {
                                &film_loop.score
                            } else {
                                return (sprite_tuple, frame_tuple); // Not a filmloop, return empty
                            }
                        }
                        None => return (sprite_tuple, frame_tuple), // Member not found, return empty
                    }
                }
            };

            for channel_number in sprite_nums.iter().copied().unique() {
                let Some(channel) = score.channels.get(channel_number as usize) else {
                    continue;
                };

                if channel.sprite.script_instance_list.is_empty() {
                    continue;
                }

                let entry = (
                    channel.sprite.number as u16,
                    channel.sprite.script_instance_list.clone(),
                );

                if channel.number > 0 {
                    sprite_tuple.push(entry);
                } else {
                    frame_tuple.push(entry);
                }
            }

            (sprite_tuple, frame_tuple)
        });

    // Dispatch to frame behaviors first (number == 0)
    if frame_tuple.len() > 0 {
        let _ = player_invoke_frame_and_movie_scripts(Symbol::builtin(BuiltInSymbol::EndSprite), &vec![]).await;
    }

    // Set the score context for this dispatch
    reserve_player_mut(|player| {
        player.current_score_context = score_ref.clone();
    });

    // Dispatch to sprite behaviors (number > 0)
    for (sprite_num, behaviors) in sprite_tuple {
        for behavior in behaviors {
            let receivers = vec![behavior.clone()];

            if let Err(err) = player_invoke_event_to_instances(
                    Symbol::builtin(BuiltInSymbol::EndSprite), &vec![], &receivers
                ).await {
                if err.code == ScriptErrorCode::Abort {
                    break;
                }
                web_sys::console::error_1(
                    &format!("Error in endSprite for sprite {}: {}", sprite_num, err.message).into()
                );
                reserve_player_mut(|player| {
                    player.on_script_error(&err);
                });
            }
        }
    }

    // Reset the score context to Stage
    reserve_player_mut(|player| {
        player.current_score_context = ScoreRef::Stage;
    });
}

/// Resolve the handler a sprite's CAST MEMBER script defines for `handler_name`.
///
/// Director's message order sends an event to a sprite's behaviors first and
/// then "to a script attached to the cast member assigned to the sprite"
/// (11.5 Scripting Dictionary, Messages), before frame and movie scripts. Mouse
/// events already walk that step in `commands.rs`; this is the same lookup for
/// the frame events, which a member script receives just as a behavior does.
///
/// Mirrors the mouse path: the member's own `member_script_ref` first, then a
/// behavior script attached via `script_id` (those live in the cast's lctx
/// rather than as Script-type members, so `get_script_by_ref` misses them).
fn get_member_script_handler(
    player: &mut crate::player::DirPlayer,
    symbols: &mut SymbolTable,
    sprite_num: i16,
    handler_name: Symbol,
) -> Option<crate::player::script::ScriptHandlerRef> {
    let member_ref = player
        .movie
        .score
        .get_sprite(sprite_num)
        .and_then(|sprite| sprite.member.clone())?;
    let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;

    if let Some(script_ref) = member.get_member_script_ref() {
        if let Some(script) = player.movie.cast_manager.get_script_by_ref(script_ref) {
            if let Some(handler) = script.get_own_handler_ref(handler_name.clone()) {
                return Some(handler);
            }
        }
    }

    let script_id = member.get_script_id()?;
    let script = {
        let cast_lib = player
            .movie
            .cast_manager
            .get_cast_mut(member_ref.cast_lib as u32);
        cast_lib.get_behavior_script_from_lctx(script_id, symbols)
    }?;
    script.get_own_handler_ref(handler_name)
}

pub async fn dispatch_event_to_all_behaviors(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) {
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use crate::js_api::ascii_safe;
    let Some(session) = crate::player::retained_session_handle() else {
        return;
    };
    let player_id = crate::player::active_player_id() as u32;
    let Some(owner) = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone()) else {
        return;
    };
    // Skip event dispatch if we're initializing behavior properties
    let skip = reserve_player_mut(|player| {
        if player.is_initializing_behavior_props {
            warn!(
                "Blocking event '{}' during property initialization",
                handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>")
            );
            return true;
        }
        // Prevent re-entrant event dispatch (this can cause infinite loops)
        if player.is_dispatching_events {
            debug!(
                "Blocking re-entrant event dispatch for '{}'",
                handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>")
            );
            return true;
        }
        player.is_dispatching_events = true;
        false
    });

    if skip {
        return;
    }
    // Include ScoreRef to track which score context each sprite belongs to
    let (sprite_behaviors, _frame_behaviors) = session
        .borrow_mut()
        .with_player(player_id, |context| {
        let player = context.player;
        let symbols = context.symbols;
        let mut sprites: Vec<(ScoreRef, usize, Vec<ScriptInstanceRef>, Option<crate::player::script::ScriptHandlerRef>)> = Vec::new();
        let mut frames = Vec::new();

        for channel_number in player.active_stage_message_channels() {
            let Some((number, entered, puppet, fallback)) = player
                .movie
                .score
                .channels
                .get(channel_number)
                .map(|channel| {
                    (
                        channel.number,
                        channel.sprite.entered,
                        channel.sprite.puppet,
                        channel.sprite.script_instance_list.clone(),
                    )
                })
            else {
                continue;
            };

            if !entered && !puppet {
                continue;
            }

            let behaviors =
                player.get_sprite_script_instance_ids(number as i16, fallback.as_slice());

            // A sprite with no behaviors may still carry a cast member script,
            // which is the next receiver in Director's message order — so the
            // channel can't be skipped on an empty behavior list alone.
            let member_handler = if number > 0 {
                get_member_script_handler(player, symbols, number as i16, handler_name.clone())
            } else {
                None
            };

            if behaviors.is_empty() && member_handler.is_none() {
                continue;
            }

            if number > 0 {
                sprites.push((ScoreRef::Stage, number, behaviors, member_handler));
            } else if number == 0 {
                frames.push((number, behaviors));  // Store tuple with channel number
            }
        }

        // Collect filmloop sprites
        let active_filmloops = player.get_active_filmloop_scores();
        for (member_ref, filmloop_current_frame) in active_filmloops {
            let Some(filmloop_score) = player
                .movie
                .cast_manager
                .find_member_by_ref(&member_ref)
                .and_then(|member| match &member.member_type {
                    super::cast_member::CastMemberType::FilmLoop(film_loop) => {
                        Some(&film_loop.score)
                    }
                    _ => None,
                })
            else {
                continue;
            };
            for channel_number in filmloop_score.active_channel_numbers_for_frame(filmloop_current_frame) {
                let Some(channel) = filmloop_score.channels.get(channel_number) else {
                    continue;
                };
                if channel.sprite.script_instance_list.is_empty() || !channel.sprite.entered {
                    continue;
                }
                let behaviors = channel.sprite.script_instance_list.clone();
                if channel.number > 0 {
                    sprites.push((ScoreRef::FilmLoop(member_ref.clone()), channel.number, behaviors, None));
                }
            }
        }

        (sprites, frames)
    })
    .unwrap_or_else(|| (Vec::new(), Vec::new()));
    // Dispatch to sprite behaviors first (channel order)
    // Set the score context before invoking each event so sprite property access works correctly
    let mount_gen = reserve_player_ref(|player| player.movie_mount_generation);
    for (score_ref, sprite_number, behaviors, member_handler) in sprite_behaviors {
        // An eager go(frame, movie) swapped the movie under this dispatch
        // (the Miniclip wrapper does exactly this from an exitFrame behavior).
        // The remaining receivers belong to the OLD movie — their script refs
        // would resolve against the NEW movie's casts. Stop dispatching.
        if reserve_player_ref(|player| player.movie_mount_generation) != mount_gen {
            reserve_player_mut(|player| {
                player.is_dispatching_events = false;
                player.current_score_context = ScoreRef::Stage;
            });
            return;
        }
        // Set the score context for this sprite's behaviors
        reserve_player_mut(|player| {
            player.current_score_context = score_ref.clone();
        });

        for behavior in behaviors {
            // Same mid-dispatch movie-swap stop, for sibling behaviors on the
            // sprite whose earlier behavior called go(frame, movie).
            if reserve_player_ref(|player| player.movie_mount_generation) != mount_gen {
                reserve_player_mut(|player| {
                    player.is_dispatching_events = false;
                    player.current_score_context = ScoreRef::Stage;
                });
                return;
            }
            let (script_name, instance_id, scope_count) = reserve_player_ref(|player| {
                let script_instance = player.allocator.get_script_instance(&behavior);
                let name = player.movie.cast_manager
                    .get_script_by_ref(&script_instance.script)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                (name, script_instance.instance_id, player.scope_count)
            });
            debug!(
                "Invoking '{}' on sprite {} behavior '{}' (instance #{}) scope_count {}",
                handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>"),
                sprite_number,
                ascii_safe(&script_name.to_string()),
                instance_id,
                scope_count
            );
            let receivers = vec![behavior.clone()];

            if let Err(err) = player_invoke_event_to_instances(handler_name.clone(), args, &receivers).await {
                if err.code == ScriptErrorCode::Abort {
                    // abort is flow control: stop all remaining handlers
                    reserve_player_mut(|player| {
                        player.is_dispatching_events = false;
                        player.current_score_context = ScoreRef::Stage;
                    });
                    return;
                }
                crate::console_error!(
                    "Error in {} for sprite {} behavior '{}': {}",
                    handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>"), sprite_number, ascii_safe(&script_name.to_string()), err.message
                );
                reserve_player_mut(|player| {
                    player.on_script_error(&err);
                });
            }
        }

        // Then the sprite's cast member script — next in Director's message
        // order, after the behaviors and before the frame/movie scripts below.
        // (Skipped if a behavior above swapped the movie via eager go().)
        let swapped = reserve_player_ref(|player| player.movie_mount_generation) != mount_gen;
        if let Some(handler) = member_handler.filter(|_| !swapped) {
            reserve_player_mut(|player| {
                player.member_script_sprite_num = sprite_number as i16;
            });
            let result = await_owned_event_handler(
                &session,
                player_id,
                &owner,
                None,
                handler,
                args,
            ).await.map(|_| ());
            reserve_player_mut(|player| {
                player.member_script_sprite_num = 0;
            });
            if let Err(err) = result {
                if err.code == ScriptErrorCode::Abort {
                    reserve_player_mut(|player| {
                        player.is_dispatching_events = false;
                        player.current_score_context = ScoreRef::Stage;
                    });
                    return;
                }
                web_sys::console::error_1(
                    &format!(
                        "Error in {} for sprite {} member script: {}",
                        handler_name.into_builtin().map(|b| b.as_str()).unwrap_or("<dynamic>"), sprite_number, err.message
                    )
                    .into(),
                );
                reserve_player_mut(|player| {
                    player.on_script_error(&err);
                });
            }
        }

        // Reset the score context to Stage after processing this sprite's behaviors
        reserve_player_mut(|player| {
            player.current_score_context = ScoreRef::Stage;
        });
    }
    // Dispatch event to frame/movie scripts — unless the movie was swapped by
    // an eager go(frame, movie) above; the NEW movie's scripts must not get
    // this old-movie event (its init sequence has not run yet).
    let swapped = reserve_player_ref(|player| player.movie_mount_generation) != mount_gen;
    if !swapped {
        if let Err(err) = player_invoke_frame_and_movie_scripts(handler_name, args).await {
            if err.code != ScriptErrorCode::Abort {
                reserve_player_mut(|player| player.on_script_error(&err));
            }
        }
    }

    // Reset the flag after dispatching
    reserve_player_mut(|player| {
        player.is_dispatching_events = false;
    });
}

pub async fn player_wait_available() {
    player_semaphone().lock().await;
}

/// The global idle timeout (`the timeoutLength` / `the timeoutScript`, distinct
/// from `timeout()` objects). Once `timeoutLength` ticks pass without mouse-click
/// or key input, Director sends a `timeOut` event and runs the primary
/// `timeoutScript`. Called once per frame tick. Memory-game card resolution
/// (`gotOne` / `reAnimate`) depends on this.
pub async fn check_global_timeout() {
    let fire = reserve_player_mut(|player| {
        if !player.is_playing {
            return false;
        }
        let now = crate::player::testing_shared::now_ms();
        let m = &mut player.movie;
        // Un-initialized (movie just started): anchor the counter to now.
        if m.timeout_last_reset_ms <= 0.0 {
            m.timeout_last_reset_ms = now;
            return false;
        }
        if m.timeout_length <= 0 {
            return false;
        }
        let lapsed_ticks = ((now - m.timeout_last_reset_ms) * 60.0 / 1000.0) as i32;
        if lapsed_ticks >= m.timeout_length {
            m.timeout_last_reset_ms = now; // re-base for the next period
            true
        } else {
            false
        }
    });
    if !fire {
        return;
    }
    // Primary `the timeoutScript` (e.g. the memory game's "gotOne" / "reAnimate").
    if let Err(err) = player_dispatch_movie_callback(BuiltInSymbol::Timeout).await {
        if err.code != ScriptErrorCode::Abort && err.code != ScriptErrorCode::HandlerNotFound {
            reserve_player_mut(|player| player.on_script_error(&err));
        }
    }
    // `on timeOut` system message to frame/movie scripts.
    if let Err(err) = player_invoke_frame_and_movie_scripts(Symbol::builtin(BuiltInSymbol::Timeout), &vec![]).await {
        if err.code != ScriptErrorCode::Abort && err.code != ScriptErrorCode::HandlerNotFound {
            reserve_player_mut(|player| player.on_script_error(&err));
        }
    }
}

/// Dispatch system events to all timeout targets
/// System events include: prepareMovie, startMovie, stopMovie, prepareFrame, exitFrame
pub async fn dispatch_system_event_to_timeouts(
    handler_name: BuiltInSymbol,
    args: &Vec<DatumRef>,
) {
    // Get all timeout targets that are currently scheduled
    let timeout_targets = reserve_player_ref(|player| {
        let mut targets = Vec::new();
        for (_timeout_name, timeout) in player.timeout_manager.timeouts.iter() {
            if timeout.is_scheduled {
                targets.push(timeout.target_ref.clone());
            }
        }
        targets
    });

    let Some(session) = crate::player::retained_session_handle() else {
        return;
    };
    let player_id = crate::player::active_player_id() as u32;
    let Some(owner) = session.borrow_mut().with_player(player_id, |context| context.player.owner.clone()) else {
        return;
    };
    // Dispatch the event to each timeout target. Every target is awaited
    // before the next one is selected, so a pending target cannot be skipped
    // or run concurrently against the shared scope stack.
    for target_ref in timeout_targets {
        let result = invoke_datum_owned(
            &session,
            player_id,
            &owner,
            target_ref,
            Symbol::builtin(handler_name),
            args.clone(),
        )
        .await;
        if let Err(err) = result {
            if err.code == ScriptErrorCode::Abort {
                return; // abort stops the entire handler chain
            }
            // HandlerNotFound is expected when a script doesn't have the event handler
            // (e.g., timeout target script doesn't have prepareFrame or exitFrame).
            // This is normal Director behavior - just silently skip.
            if err.code != ScriptErrorCode::HandlerNotFound {
                // Log actual errors but continue with other timeouts
                log::warn!("Timeout system event {} error: {}", handler_name, err.message);
            }
        }
    }
}


#[cfg(all(test, not(target_arch = "wasm32")))]
mod owned_event_scope_tests {
    use super::{
        dispatch_w3d_callback_owned, player_dispatch_event_beginsprite_owned,
        player_dispatch_event_to_sprite_targeted_owned, player_invoke_global_event_owned,
        dispatch_targeted_vm_event_owned, event_owner_is_live, run_event_loop,
        wait_event_owner_available, OwnedEventStopScope,
        OwnedScoreContextScope, PlayerVMEvent,
        W3dCallbackReceiver, W3dCallbackRequest,
    };
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::ownership::OwnerToken;
    use crate::player::score::ScoreRef;
    use crate::player::session::{RuntimeSession, RuntimeSessionHandle};
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolOwner};
    use crate::player::DatumRef;
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use async_std::channel;
    use futures::future::{select, Either, FutureExt};
    use std::{cell::RefCell, rc::Rc};

    fn session_handle() -> RuntimeSessionHandle {
        let mut session = RuntimeSession::new(SymbolOwner { session: 901, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        Rc::new(RefCell::new(session))
    }

    fn owner(session: &RuntimeSessionHandle) -> OwnerToken {
        session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .expect("test player exists")
    }

    /// Run an owned async operation with the same pending-action/evaluator
    /// pump used by the production command loop. Calling an event future
    /// directly would leave its real child request waiting forever because
    /// this test module has no frontend command task to service it.
    async fn drive_with_production_pump<F, T>(
        session: RuntimeSessionHandle,
        player_id: u32,
        owner: OwnerToken,
        operation: F,
    ) -> Result<T, crate::player::ScriptError>
    where
        F: std::future::Future<Output = Result<T, crate::player::ScriptError>>,
    {
        let operation = operation.fuse();
        futures::pin_mut!(operation);
        let pump = async {
            loop {
                let turns = crate::player::commands::pump_pending_commands(&session, player_id);
                for turn in turns {
                    finish_test_command_turn(&session, player_id, &owner, turn).await;
                }
                let _ = crate::player::commands::pump_pending_eval_requests(&session, player_id).await;
                let (turns, _) = crate::player::commands::execute_pending_commands(
                    &session,
                    player_id,
                    &owner,
                )
                .await;
                for turn in turns {
                    finish_test_command_turn(&session, player_id, &owner, turn).await;
                }
                async_std::task::yield_now().await;
            }
        };
        futures::pin_mut!(pump);
        let selected = async_std::future::timeout(
            std::time::Duration::from_secs(2),
            select(operation, pump),
        )
        .await
        .map_err(|_| crate::player::ScriptError::new("owned event production pump timed out".to_owned()))?;
        match selected {
            Either::Left((result, _)) => result,
            Either::Right((never, _)) => match never {},
        }
    }

    async fn finish_test_command_turn(
        session: &RuntimeSessionHandle,
        player_id: u32,
        owner: &OwnerToken,
        turn: crate::player::commands::CommandTurn,
    ) {
        if !session
            .borrow_mut()
            .with_player(player_id, |context| {
                owner.same_identity(&context.player.owner) && owner.is_arena_live()
            })
            .unwrap_or(false)
        {
            if let crate::player::commands::CommandTurn::Complete(_, Some(completer)) = turn {
                completer
                    .complete(Err(crate::player::cancelled_scope_error()))
                    .await;
            }
            return;
        }
        match turn {
            crate::player::commands::CommandTurn::Continue => {}
            crate::player::commands::CommandTurn::Waiting(pending)
            | crate::player::commands::CommandTurn::Pending(pending) => {
                session.borrow_mut().requeue_pending_command(pending);
            }
            crate::player::commands::CommandTurn::Complete(result, completer) => {
                if let Some(completer) = completer {
                    let result = match result {
                        Err(error) if error.code == crate::player::ScriptErrorCode::Abort => {
                            Ok(DatumRef::Void)
                        }
                        result => result,
                    };
                    completer.complete(result).await;
                }
            }
        }
    }

    #[test]
    fn nested_event_scopes_restore_the_saved_stop_state() {
        let session = session_handle();
        let captured = owner(&session);
        session
            .borrow_mut()
            .with_player(1, |context| context.player.event_stopped = true)
            .unwrap();

        let outer = OwnedEventStopScope::enter(&session, 1, &captured).unwrap();
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.event_stopped),
            Some(false)
        );
        {
            let inner = OwnedEventStopScope::enter(&session, 1, &captured).unwrap();
            assert_eq!(
                session
                    .borrow_mut()
                    .with_player(1, |context| context.player.event_stopped),
                Some(false)
            );
            drop(inner);
        }
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.event_stopped),
            Some(false)
        );
        drop(outer);
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.event_stopped),
            Some(true)
        );
    }

    #[test]
    fn stale_scope_cleanup_cannot_change_a_replacement_player() {
        let session = session_handle();
        let captured = owner(&session);
        let guard = OwnedEventStopScope::enter(&session, 1, &captured).unwrap();

        let replacement = session
            .borrow_mut()
            .reset_player_owned(1, &captured)
            .expect("captured owner can reset its player");
        assert!(!captured.is_arena_live());
        assert!(!captured.same_identity(&replacement));
        session
            .borrow_mut()
            .with_player(1, |context| context.player.event_stopped = true)
            .unwrap();

        drop(guard);
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.event_stopped),
            Some(true),
            "dropping an old event scope must not restore state on the replacement owner"
        );
    }

    #[test]
    fn score_context_scope_restores_and_rejects_stale_cleanup() {
        let session = session_handle();
        let captured = owner(&session);
        let nested = ScoreRef::FilmLoop(CastMemberRef { cast_lib: 1, cast_member: 1 });
        session
            .borrow_mut()
            .with_player(1, |context| context.player.current_score_context = nested.clone())
            .unwrap();

        let guard = OwnedScoreContextScope::enter(
            &session,
            1,
            &captured,
            ScoreRef::Stage,
        )
        .unwrap();
        assert!(session
            .borrow_mut()
            .with_player(1, |context| {
                matches!(&context.player.current_score_context, ScoreRef::Stage)
            })
            .unwrap());
        drop(guard);
        assert!(session
            .borrow_mut()
            .with_player(1, |context| {
                matches!(
                    &context.player.current_score_context,
                    ScoreRef::FilmLoop(member) if member.cast_lib == 1 && member.cast_member == 1
                )
            })
            .unwrap());

        let stale_guard = OwnedScoreContextScope::enter(
            &session,
            1,
            &captured,
            ScoreRef::Stage,
        )
        .unwrap();
        let replacement = session
            .borrow_mut()
            .reset_player_owned(1, &captured)
            .expect("captured owner can reset its player");
        let replacement_context = ScoreRef::FilmLoop(CastMemberRef { cast_lib: 2, cast_member: 2 });
        session
            .borrow_mut()
            .with_player(1, |context| context.player.current_score_context = replacement_context.clone())
            .unwrap();
        drop(stale_guard);
        assert!(session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.owner.same_identity(&replacement)
                    && matches!(
                        &context.player.current_score_context,
                        ScoreRef::FilmLoop(member) if member.cast_lib == 2 && member.cast_member == 2
                    )
            })
            .unwrap());
    }

    #[test]
    fn global_dispatch_uses_a_real_owner_scope_and_restores_outer_state() {
        let session = session_handle();
        let captured = owner(&session);
        session
            .borrow_mut()
            .with_player(1, |context| context.player.event_stopped = true)
            .unwrap();
        let handler = Symbol::builtin(BuiltInSymbol::Voidp);

        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_invoke_global_event_owned(
                session.clone(),
                1,
                captured,
                handler,
                vec![DatumRef::Void],
            ),
        ));
        assert!(result.is_ok());
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.event_stopped),
            Some(true)
        );
    }

    #[test]
    fn owned_begin_sprite_returns_channels_and_unmarked_remainder() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let remainder = session
            .borrow_mut()
            .with_player(1, |context| {
                let instance = context.player.allocator.alloc_script_instance(
                    crate::player::script::ScriptInstance {
                        instance_id: 3,
                        script: CastMemberRef { cast_lib: 1, cast_member: 1 },
                        ancestor: None,
                        properties: fxhash::FxHashMap::default(),
                        begin_sprite_called: false,
                    },
                );
                let mut channel = crate::player::score::SpriteChannel::new(2);
                channel.sprite.puppet = true;
                channel.sprite.script_instance_list = vec![instance.clone()];
                context.player.movie.score.channels.push(channel);
                instance
            })
            .unwrap();
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_dispatch_event_beginsprite_owned(
                session.clone(),
                1,
                captured,
                event,
                Vec::new(),
            ),
        ))
        .expect("beginSprite dispatch completes");

        assert!(result
            .initialized_channels
            .iter()
            .any(|(score, number)| matches!(score, ScoreRef::Stage) && *number == 1));
        assert!(result.unhandled_behaviors.iter().any(|instance| {
            instance.id() == remainder.id() && instance.owner().same_identity(remainder.owner())
        }));
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| {
                    matches!(
                        context.player.get_datum(value),
                        crate::director::lingo::datum::Datum::Int(42)
                    )
                })
            }),
            Some(Some(true))
        );
        assert!(session
            .borrow_mut()
            .with_player(1, |context| context
                .player
                .movie
                .score
                .channels
                .iter()
                .flat_map(|channel| channel.sprite.script_instance_list.iter())
                .filter_map(|instance| context.player.allocator.get_script_instance_opt(instance))
                .all(|instance| !instance.begin_sprite_called))
            .unwrap());
    }


    fn event_handler(name_id: u16, bytecode_array: Vec<crate::director::chunks::handler::Bytecode>) -> std::rc::Rc<crate::director::chunks::handler::HandlerDef> {
        std::rc::Rc::new(crate::director::chunks::handler::HandlerDef {
            name_id,
            bytecode_array,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: std::cell::RefCell::new(None),
        })
    }

    fn event_script(
        member_ref: crate::player::cast_lib::CastMemberRef,
        script_type: crate::director::enums::ScriptType,
        event: crate::player::symbols::symbol::Symbol,
        handler: std::rc::Rc<crate::director::chunks::handler::HandlerDef>,
    ) -> std::rc::Rc<crate::player::script::Script> {
        let mut handlers = fxhash::FxHashMap::default();
        handlers.insert(event.clone(), handler);
        std::rc::Rc::new(crate::player::script::Script {
            member_ref,
            name: "owned-event-fixture".to_owned(),
            chunk: crate::director::chunks::script::ScriptChunk {
                script_number: 1,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: std::collections::HashMap::new(),
            },
            script_type,
            handlers,
            handler_names_raw: vec!["ownedEvent".to_owned()],
            handler_names: vec![event],
            properties: std::cell::RefCell::new(fxhash::FxHashMap::default()),
        })
    }

    fn playback_counter_handler(
        global_index: u16,
        names_len: usize,
        fail_after_increment: bool,
    ) -> std::rc::Rc<crate::director::chunks::handler::HandlerDef> {
        let mut bytecode_array = vec![
            crate::director::chunks::handler::Bytecode::new(
                crate::director::lingo::opcode::OpCode::GetGlobal,
                global_index as i64,
                0,
            ),
            crate::director::chunks::handler::Bytecode::new(
                crate::director::lingo::opcode::OpCode::PushInt8,
                1,
                1,
            ),
            crate::director::chunks::handler::Bytecode::new(
                crate::director::lingo::opcode::OpCode::Add,
                0,
                2,
            ),
            crate::director::chunks::handler::Bytecode::new(
                crate::director::lingo::opcode::OpCode::SetGlobal,
                global_index as i64,
                3,
            ),
        ];
        bytecode_array.push(crate::director::chunks::handler::Bytecode::new(
            if fail_after_increment {
                crate::director::lingo::opcode::OpCode::Invalid
            } else {
                crate::director::lingo::opcode::OpCode::Ret
            },
            0,
            4,
        ));
        std::rc::Rc::new(crate::director::chunks::handler::HandlerDef {
            name_id: 0,
            bytecode_array,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: (1..names_len as u16).collect(),
            compiled_ir: std::cell::RefCell::new(None),
        })
    }

    fn install_playback_cleanup_fixture(
        session: &RuntimeSessionHandle,
    ) -> (Symbol, Symbol, CastMemberRef) {
        let (stop_count, end_count) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.symbols.intern("ownedStopMovieCount"),
                    context.symbols.intern("ownedEndSpriteCount"),
                )
            })
            .expect("cleanup fixture player exists");
        let stop_movie = Symbol::builtin(BuiltInSymbol::StopMovie);
        let end_sprite = Symbol::builtin(BuiltInSymbol::EndSprite);
        let member_ref = CastMemberRef { cast_lib: 1, cast_member: 1 };
        let filmloop_ref = CastMemberRef { cast_lib: 1, cast_member: 2 };
        session
            .borrow_mut()
            .with_player(1, |context| {
                let mut script = event_script(
                    member_ref.clone(),
                    crate::director::enums::ScriptType::Score,
                    stop_movie.clone(),
                    playback_counter_handler(2, 4, true),
                );
                std::rc::Rc::get_mut(&mut script)
                    .expect("cleanup fixture script must be uniquely owned")
                    .handlers
                    .insert(
                        end_sprite,
                        playback_counter_handler(3, 4, false),
                    );
                let instance = context.player.allocator.alloc_script_instance(
                    crate::player::script::ScriptInstance {
                        instance_id: 1,
                        script: member_ref.clone(),
                        ancestor: None,
                        properties: fxhash::FxHashMap::default(),
                        begin_sprite_called: false,
                    },
                );
                let filmloop_instance = context.player.allocator.alloc_script_instance(
                    crate::player::script::ScriptInstance {
                        instance_id: 2,
                        script: member_ref.clone(),
                        ancestor: None,
                        properties: fxhash::FxHashMap::default(),
                        begin_sprite_called: false,
                    },
                );
                let mut cast = crate::player::cast_lib::CastLib::test_external(1, 0);
                cast.name_symbols = std::rc::Rc::from(vec![
                    stop_movie,
                    Symbol::builtin(BuiltInSymbol::EndSprite),
                    stop_count.clone(),
                    end_count.clone(),
                ]);
                cast.scripts.insert(1, script);
                let mut filmloop_score = crate::player::score::Score::empty();
                filmloop_score.sprite_spans.push(crate::player::score::ScoreSpriteSpan {
                    channel_number: 1,
                    start_frame: 1,
                    end_frame: 20,
                    scripts: Vec::new(),
                });
                filmloop_score.channels = vec![
                    crate::player::score::SpriteChannel::new(0),
                    crate::player::score::SpriteChannel::new(1),
                ];
                filmloop_score.channels[1].sprite.entered = true;
                filmloop_score.channels[1].sprite.visible = true;
                filmloop_score.channels[1].sprite.script_instance_list = vec![filmloop_instance];
                cast.members.insert(
                    2,
                    crate::player::cast_member::CastMember::new(
                        1,
                        crate::player::cast_member::CastMemberType::FilmLoop(
                            crate::player::cast_member::FilmLoopMember {
                                info: crate::director::enums::FilmLoopInfo {
                                    reg_point: (0, 0),
                                    width: 1,
                                    height: 1,
                                    center: 0,
                                    crop: 0,
                                    sound: 0,
                                    loops: 0,
                                },
                                score_chunk: crate::director::chunks::score::ScoreChunk {
                                    header: crate::director::chunks::score::ScoreChunkHeader {
                                        total_length: 0,
                                        unk1: 0,
                                        unk2: 0,
                                        entry_count: 0,
                                        unk3: 0,
                                        entry_size_sum: 0,
                                    },
                                    entries: Vec::new(),
                                    frame_intervals: Vec::new(),
                                    frame_data: Default::default(),
                                    sprite_details: std::collections::HashMap::new(),
                                },
                                score: filmloop_score,
                                current_frame: 1,
                                initial_rect: crate::player::geometry::IntRect {
                                    left: 0,
                                    top: 0,
                                    right: 1,
                                    bottom: 1,
                                },
                                cached_total_frames: Some(20),
                            },
                        ),
                    ),
                );
                context.player.movie.cast_manager.casts.push(cast);
                context.player.movie.score.channels = vec![
                    crate::player::score::SpriteChannel::new(0),
                    crate::player::score::SpriteChannel::new(1),
                ];
                let channel = &mut context.player.movie.score.channels[1];
                channel.sprite.entered = true;
                channel.sprite.visible = true;
                channel.sprite.member = Some(filmloop_ref.clone());
                channel.sprite.script_instance_list = vec![instance];
                context.player.movie.score.sprite_spans.push(
                    crate::player::score::ScoreSpriteSpan {
                        channel_number: 1,
                        start_frame: 1,
                        end_frame: 20,
                        scripts: Vec::new(),
                    },
                );
                let zero = context.player.alloc_datum(
                    crate::director::lingo::datum::Datum::Int(0),
                );
                context.player.globals.insert(stop_count.clone(), zero);
                let zero = context.player.alloc_datum(
                    crate::director::lingo::datum::Datum::Int(0),
                );
                context.player.globals.insert(end_count.clone(), zero);
            })
            .expect("cleanup fixture must remain owner-bound");
        (stop_count, end_count, filmloop_ref)
    }

    fn read_playback_counter(session: &RuntimeSessionHandle, marker: &Symbol) -> i32 {
        session
            .borrow_mut()
            .with_player(1, |context| {
                context
                    .player
                    .globals
                    .get(marker)
                    .and_then(|value| match context.player.get_datum(value) {
                        crate::director::lingo::datum::Datum::Int(value) => Some(*value),
                        _ => None,
                    })
                    .unwrap_or(0)
            })
            .expect("cleanup counter player exists")
    }

    fn install_behavior_fixture(
        session: &RuntimeSessionHandle,
        first_handler: std::rc::Rc<crate::director::chunks::handler::HandlerDef>,
        second_handler: std::rc::Rc<crate::director::chunks::handler::HandlerDef>,
        static_handler: std::rc::Rc<crate::director::chunks::handler::HandlerDef>,
    ) -> (Symbol, Symbol) {
        let (event, marker, stop_event, pass) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.symbols.intern("ownedGlobalEvent"),
                    context.symbols.intern("ownedLaterBehaviorMarker"),
                    context.symbols.intern("stopEvent"),
                    context.symbols.intern("pass"),
                )
            })
            .expect("test player exists");
        session
            .borrow_mut()
            .with_player(1, |context| {
                let first_ref = crate::player::cast_lib::CastMemberRef { cast_lib: 1, cast_member: 1 };
                let second_ref = crate::player::cast_lib::CastMemberRef { cast_lib: 1, cast_member: 2 };
                let static_ref = crate::player::cast_lib::CastMemberRef { cast_lib: 1, cast_member: 3 };
                let first_instance = context.player.allocator.alloc_script_instance(crate::player::script::ScriptInstance {
                    instance_id: 1,
                    script: first_ref.clone(),
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                });
                let second_instance = context.player.allocator.alloc_script_instance(crate::player::script::ScriptInstance {
                    instance_id: 2,
                    script: second_ref.clone(),
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                });
                let mut cast = crate::player::cast_lib::CastLib::test_external(1, 0);
                cast.name_symbols = std::rc::Rc::from(vec![event.clone(), marker.clone(), stop_event, pass]);
                cast.scripts.insert(1, event_script(first_ref, crate::director::enums::ScriptType::Score, event.clone(), first_handler));
                cast.scripts.insert(2, event_script(second_ref, crate::director::enums::ScriptType::Score, event.clone(), second_handler));
                cast.scripts.insert(3, event_script(static_ref, crate::director::enums::ScriptType::Movie, event.clone(), static_handler));
                context.player.movie.cast_manager.casts.push(cast);
                context.player.movie.score.channels = vec![crate::player::score::SpriteChannel::new(0), crate::player::score::SpriteChannel::new(1)];
                let sprite = &mut context.player.movie.score.channels[1].sprite;
                sprite.entered = true;
                sprite.script_instance_list = vec![first_instance, second_instance];
            })
            .expect("test player exists");
        (event, marker)
    }

    #[test]
    fn global_event_keeps_later_behaviors_and_skips_static_fallback() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        // If static fallback is incorrectly reached, this malformed handler
        // makes the real runtime dispatch fail instead of hiding the bug.
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_invoke_global_event_owned(session.clone(), 1, captured, event, vec![]),
        ));
        assert!(result.is_ok(), "static fallback ran: {:?}", result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| matches!(context.player.get_datum(value), crate::director::lingo::datum::Datum::Int(42)))
            }),
            Some(Some(true)),
            "the later behavior must run before handled propagation suppresses static scripts"
        );
    }

    #[test]
    fn targeted_event_keeps_later_behaviors_and_skips_static_fallback() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        // If targeted dispatch incorrectly falls through after the first
        // handled behavior, this malformed static handler exposes it.
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_dispatch_event_to_sprite_targeted_owned(
                session.clone(),
                1,
                captured,
                event,
                vec![],
                1,
            ),
        ));
        assert!(
            result.as_ref().map(|value| *value).unwrap_or(false),
            "targeted dispatch failed: {:?}",
            result.as_ref().err()
        );
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| {
                    matches!(context.player.get_datum(value), crate::director::lingo::datum::Datum::Int(42))
                })
            }),
            Some(Some(true)),
            "all behaviors must run before handled propagation suppresses static scripts"
        );
    }

    #[test]
    fn queued_targeted_event_uses_captured_owner_for_all_behaviors() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        // Static fallback must not run after a behavior handles the event.
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let instances = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels[1].sprite.script_instance_list.clone()
            })
            .expect("test sprite exists");
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&captured),
                PlayerVMEvent::Targeted(event, vec![], Some(instances)),
            ),
        ));
        assert!(result.is_ok(), "queued targeted dispatch failed: {:?}", result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| {
                    matches!(
                        context.player.get_datum(value),
                        crate::director::lingo::datum::Datum::Int(42)
                    )
                })
            }),
            Some(Some(true)),
            "all queued targeted behaviors must run before static fallback is suppressed"
        );
    }

    #[test]
    fn queued_targeted_event_falls_back_to_static_scripts_for_empty_instances() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let no_behavior = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let static_handler = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let (event, marker) = install_behavior_fixture(
            &session,
            no_behavior.clone(),
            no_behavior,
            static_handler,
        );
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&captured),
                PlayerVMEvent::Targeted(event, vec![], None),
            ),
        ));
        assert!(result.is_ok(), "queued static fallback failed: {:?}", result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| {
                    matches!(
                        context.player.get_datum(value),
                        crate::director::lingo::datum::Datum::Int(42)
                    )
                })
            }),
            Some(Some(true)),
            "an empty targeted behavior list must fall through to static scripts"
        );
    }

    #[test]
    fn queued_targeted_event_reports_static_fallback_error_once() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let no_behavior = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        let (event, _marker) = install_behavior_fixture(
            &session,
            no_behavior.clone(),
            no_behavior,
            static_handler,
        );
        session
            .borrow_mut()
            .with_player(1, |context| context.player.is_playing = true)
            .expect("test player exists");
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&captured),
                PlayerVMEvent::Targeted(event, vec![], None),
            ),
        ));
        assert_eq!(
            result.as_ref().err().map(|error| error.code.clone()),
            Some(crate::player::ScriptErrorCode::Generic),
            "static fallback errors must escape the queued dispatcher"
        );
        assert_eq!(
            session.borrow_mut().with_player(1, |context| context.player.is_playing),
            Some(false),
            "the owner-bound error reporter must stop the failing player"
        );
    }

    #[test]
    fn stop_cleanup_reports_callback_error_once_and_consumes_replay() {
        let session = session_handle();
        let (stop_count, end_count, filmloop_ref) = install_playback_cleanup_fixture(&session);
        let captured = owner(&session);
        let (epoch, _cancel_rx) = session
            .borrow_mut()
            .begin_playback_loop(1, &captured)
            .expect("cleanup loop must be claimable")
            .expect("cleanup loop must be installed");
        assert_eq!(
            session
                .borrow_mut()
                .cancel_playback_loop(1, &captured, true),
            Some(epoch)
        );
        assert!(session
            .borrow_mut()
            .begin_playback_loop(1, &captured)
            .expect("same-owner replay must be queued")
            .is_none());

        crate::player::reset_test_script_error_count();
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            crate::player::stop_movie_sequence_owned(session.clone(), 1, captured.clone()),
        ));
        assert_eq!(
            result.as_ref().err().map(|error| error.code.clone()),
            Some(crate::player::ScriptErrorCode::Generic),
            "live StopMovie callback error must reach cleanup"
        );
        assert_eq!(
            crate::player::test_script_error_count(),
            0,
            "StopMovie cleanup carries the callback failure to its finalizer"
        );
        assert_eq!(read_playback_counter(&session, &stop_count), 1);
        assert_eq!(read_playback_counter(&session, &end_count), 2);
        let exited = session
            .borrow_mut()
            .with_player(1, |context| {
                let stage = context.player.movie.score.channels[1].sprite.exited;
                let filmloop = context
                    .player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&filmloop_ref)
                    .and_then(|member| member.member_type.as_film_loop())
                    .map(|filmloop| filmloop.score.channels[1].sprite.exited);
                (stage, filmloop)
            })
            .expect("cleanup fixture owner must remain live");
        assert_eq!(exited, (true, Some(true)));

        // Exercise the same finalizer used by start_playback_owned. A cleanup
        // error reports once, retires the old loop, and consumes replay without
        // launching a replacement loop.
        crate::player::reset_test_script_error_count();
        crate::player::finish_playback_owned(
            session.clone(),
            1,
            captured.clone(),
            epoch,
            result,
        );
        assert_eq!(read_playback_counter(&session, &stop_count), 1);
        assert_eq!(read_playback_counter(&session, &end_count), 2);
        assert_eq!(
            crate::player::test_script_error_count(),
            1,
            "the real cleanup failure is reported exactly once by the finalizer"
        );

        // Abort is cancellation rather than a script failure. It still
        // consumes a queued replay so a canceled old loop cannot restart, but
        // the live replacement owner must observe no ScriptError callback.
        let (abort_epoch, _abort_cancel_rx) = session
            .borrow_mut()
            .begin_playback_loop(1, &captured)
            .expect("cleanup owner must remain live")
            .expect("failed cleanup must not auto-restart playback");
        assert!(session
            .borrow_mut()
            .begin_playback_loop(1, &captured)
            .expect("abort replay request must be accepted")
            .is_none());
        crate::player::reset_test_script_error_count();
        crate::player::finish_playback_owned(
            session.clone(),
            1,
            captured.clone(),
            abort_epoch,
            Err(crate::player::cancelled_scope_error()),
        );
        assert_eq!(
            crate::player::test_script_error_count(),
            0,
            "Abort cancellation must not report a ScriptError"
        );
        let (_after_abort_epoch, _after_abort_cancel) = session
            .borrow_mut()
            .begin_playback_loop(1, &captured)
            .expect("Abort finalizer must leave the owner usable")
            .expect("Abort finalizer must consume queued replay");
    }

    #[test]
    fn queued_targeted_event_reports_foreign_instance_lookup_error() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;
        use crate::player::script::ScriptInstance;

        let session = session_handle();
        let foreign = session_handle();
        let foreign_instance = foreign
            .borrow_mut()
            .with_player(1, |context| {
                context.player.allocator.alloc_script_instance(ScriptInstance {
                    instance_id: 1,
                    script: CastMemberRef { cast_lib: 99, cast_member: 99 },
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                })
            })
            .expect("foreign instance fixture player exists");
        let first = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let second = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let (event, _marker) = install_behavior_fixture(&session, first, second, static_handler);
        session
            .borrow_mut()
            .with_player(1, |context| context.player.is_playing = true)
            .expect("test player exists");
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&captured),
                PlayerVMEvent::Targeted(event, vec![], Some(vec![foreign_instance])),
            ),
        ));
        assert_eq!(
            result.as_ref().err().map(|error| error.code.clone()),
            Some(crate::player::ScriptErrorCode::Generic),
            "foreign instance lookup must remain an owned error"
        );
        assert_eq!(
            session.borrow_mut().with_player(1, |context| context.player.is_playing),
            Some(false),
            "foreign lookup errors must report against the captured owner"
        );
    }

    #[test]
    fn queued_targeted_event_preserves_stop_event_across_behaviors() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
                Bytecode::new(OpCode::ExtCall, 2, 1),
                Bytecode::new(OpCode::PushArgListNoRet, 0, 2),
                Bytecode::new(OpCode::ExtCall, 3, 3),
                Bytecode::new(OpCode::Ret, 0, 4),
            ],
        );
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        // An erroneous static fallback makes an accidental fallthrough
        // observable instead of allowing the test to pass vacuously.
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let instances = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels[1].sprite.script_instance_list.clone()
            })
            .expect("test sprite exists");
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&captured),
                PlayerVMEvent::Targeted(event, vec![], Some(instances)),
            ),
        ));
        assert!(result.is_ok(), "queued stopEvent dispatch failed: {:?}", result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).is_none()
            }),
            Some(true),
            "stopEvent must prevent later behaviors and static fallback"
        );
    }

    #[test]
    fn queued_targeted_event_rejects_a_retired_owner() {
        let session = session_handle();
        let captured = owner(&session);
        session
            .borrow_mut()
            .reset_player_owned(1, &captured)
            .expect("captured owner can reset its player");
        let result = async_std::task::block_on(dispatch_targeted_vm_event_owned(
            Some(&session),
            1,
            Some(&captured),
            PlayerVMEvent::Targeted(
                Symbol::builtin(BuiltInSymbol::BeginSprite),
                vec![],
                Some(Vec::new()),
            ),
        ));
        assert_eq!(
            result.as_ref().err().map(|error| error.code.clone()),
            Some(crate::player::ScriptErrorCode::Abort),
            "a stale queued targeted event must be cancelled before lookup"
        );
    }

    #[test]
    fn queued_targeted_event_drop_after_suspend_cancels_old_owner() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::enums::ScriptType;
        use crate::director::lingo::opcode::OpCode;
        use crate::player::cast_lib::CastLib;
        use crate::player::score::SpriteChannel;
        use crate::player::script::ScriptInstance;
        use std::{future::Future, task::{Context, Poll}};

        let mut raw_session = RuntimeSession::new(SymbolOwner { session: 913, generation: 1 });
        assert!(raw_session.add_player(1, channel::unbounded().0));
        let session = Rc::new(RefCell::new(raw_session));
        let (event, nothing, replacement_event, replacement_marker) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.symbols.intern("suspendedTargeted"),
                    Symbol::builtin(BuiltInSymbol::Nothing),
                    context.symbols.intern("replacementTargeted"),
                    context.symbols.intern("replacementTargetedMarker"),
                )
            })
            .expect("suspended-target fixture player exists");
        let member_ref = CastMemberRef { cast_lib: 1, cast_member: 1 };
        let handler = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
                Bytecode::new(OpCode::ExtCall, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let mut script = event_script(member_ref.clone(), ScriptType::Movie, event.clone(), handler);
        let replacement_handler = std::rc::Rc::new(crate::director::chunks::handler::HandlerDef {
            name_id: 0,
            bytecode_array: vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 3, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: (1..4).collect(),
            compiled_ir: std::cell::RefCell::new(None),
        });
        std::rc::Rc::get_mut(&mut script)
            .expect("suspended-target fixture script must be uniquely owned")
            .handlers
            .insert(replacement_event.clone(), replacement_handler);
        let instance = session
            .borrow_mut()
            .with_player(1, |context| {
                let mut cast = CastLib::test_external(1, 0);
                cast.name_symbols = Rc::from(vec![
                    event.clone(),
                    nothing,
                    replacement_event.clone(),
                    replacement_marker.clone(),
                ]);
                cast.scripts.insert(1, script);
                context.player.movie.cast_manager.casts.push(cast);
                let instance = context.player.allocator.alloc_script_instance(ScriptInstance {
                    instance_id: 1,
                        script: member_ref.clone(),
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                });
                context.player.movie.score.channels =
                    vec![SpriteChannel::new(0), SpriteChannel::new(1)];
                context.player.movie.score.channels[1].sprite.script_instance_list =
                    vec![instance.clone()];
                instance
            })
            .expect("suspended-target fixture installed");
        let captured = owner(&session);
        let mut operation = Box::pin(dispatch_targeted_vm_event_owned(
            Some(&session),
            1,
            Some(&captured),
            PlayerVMEvent::Targeted(event, vec![], Some(vec![instance])),
        ));
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(matches!(
            operation.as_mut().poll(&mut context),
            Poll::Pending
        ));

        let replacement = session
            .borrow_mut()
            .reset_player_owned(1, &captured)
            .expect("captured owner can reset the suspended target");
        drop(operation);
        assert!(replacement.is_arena_live());
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| {
                    (context.player.event_stopped, context.player.scope_count)
                }),
            Some((false, 0)),
            "dropping a suspended targeted event must not mutate the replacement"
        );

        // Reuse the retained script under the replacement allocator and prove
        // the new owner can complete a fresh callback after the old pending
        // request was cancelled. A stale completion must not satisfy or mutate
        // this replacement callback.
        let replacement_instance = session
            .borrow_mut()
            .with_player(1, |context| {
                let instance = context.player.allocator.alloc_script_instance(ScriptInstance {
                    instance_id: 2,
                    script: member_ref.clone(),
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                });
                context.player.movie.score.channels =
                    vec![SpriteChannel::new(0), SpriteChannel::new(1)];
                context.player.movie.score.channels[1].sprite.entered = true;
                context.player.movie.score.channels[1].sprite.script_instance_list =
                    vec![instance.clone()];
                instance
            })
            .expect("replacement callback fixture must install");
        let replacement_result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            replacement.clone(),
            dispatch_targeted_vm_event_owned(
                Some(&session),
                1,
                Some(&replacement),
                PlayerVMEvent::Targeted(
                    replacement_event,
                    vec![],
                    Some(vec![replacement_instance]),
                ),
            ),
        ));
        assert!(replacement_result.is_ok(), "replacement callback failed: {:?}", replacement_result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&replacement_marker).and_then(|value| {
                    match context.player.get_datum(value) {
                        crate::director::lingo::datum::Datum::Int(value) => Some(*value),
                        _ => None,
                    }
                })
            }),
            Some(Some(42)),
            "replacement callback must complete after old pending callback cancellation",
        );
    }

    #[test]
    fn event_owner_availability_rechecks_pause_after_wait() {
        use std::{future::Future, task::{Context, Poll}, time::Duration};

        let session = session_handle();
        let captured = owner(&session);
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.is_playing = true;
                context.player.in_frame_script = true;
            })
            .expect("busy owner fixture player exists");

        let mut wait = Box::pin(wait_event_owner_available(&session, 1, &captured));
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Pending));

        session
            .borrow_mut()
            .with_player(1, |context| context.player.is_playing = false)
            .expect("pause fixture player exists");
        async_std::task::block_on(async_std::task::sleep(Duration::from_millis(8)));

        assert!(
            matches!(wait.as_mut().poll(&mut context), Poll::Ready(Ok(false))),
            "availability must re-read playing state after handler-gap wait"
        );
    }

    #[test]
    fn receiver_event_loop_retires_on_reset_without_cross_session_state() {
        use std::{cell::Cell, future::Future, rc::Rc, task::{Context, Poll}, time::Duration};

        fn make_session(session_id: u64) -> (RuntimeSessionHandle, OwnerToken) {
            let mut raw = RuntimeSession::new(SymbolOwner {
                session: session_id,
                generation: 1,
            });
            assert!(raw.add_player(1, channel::unbounded().0));
            let session = Rc::new(RefCell::new(raw));
            let captured = owner(&session);
            (session, captured)
        }

        let (session_a, owner_a) = make_session(1001);
        let (session_b, owner_b) = make_session(1002);
        let (tx_a, rx_a) = channel::unbounded::<PlayerVMEvent>();
        let (tx_b, rx_b) = channel::unbounded::<PlayerVMEvent>();

        session_a
            .borrow_mut()
            .with_player(1, |context| {
                context.player.is_playing = true;
                context.player.in_frame_script = true;
            })
            .expect("session A player exists");

        tx_a
            .try_send(PlayerVMEvent::Global(
                Symbol::builtin(BuiltInSymbol::BeginSprite),
                vec![],
            ))
            .expect("session A event queued");

        let mut loop_a = Box::pin(run_event_loop(
            rx_a,
            session_a.clone(),
            1,
            owner_a.clone(),
        ));
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);

        assert!(matches!(
            loop_a.as_mut().poll(&mut context),
            Poll::Pending
        ));

        let first = event_handler(0, vec![crate::director::chunks::handler::Bytecode::new(
            crate::director::lingo::opcode::OpCode::Ret,
            0,
            0,
        )]);
        let second = event_handler(
            0,
            vec![
                crate::director::chunks::handler::Bytecode::new(
                    crate::director::lingo::opcode::OpCode::PushInt8,
                    42,
                    0,
                ),
                crate::director::chunks::handler::Bytecode::new(
                    crate::director::lingo::opcode::OpCode::SetGlobal,
                    1,
                    1,
                ),
                crate::director::chunks::handler::Bytecode::new(
                    crate::director::lingo::opcode::OpCode::Ret,
                    0,
                    2,
                ),
            ],
        );
        let static_handler = event_handler(
            0,
            vec![crate::director::chunks::handler::Bytecode::new(
                crate::director::lingo::opcode::OpCode::Ret,
                0,
                0,
            )],
        );
        let (event_b, marker_b) =
            install_behavior_fixture(&session_b, first, second, static_handler);
        session_b
            .borrow_mut()
            .with_player(1, |context| context.player.is_playing = true)
            .expect("session B player exists");

        // Retire A while its receiver is suspended, then drive B's real receiver
        // and evaluator pump. B must still execute its own handler.
        session_a
            .borrow_mut()
            .reset_player_owned(1, &owner_a)
            .expect("session A owner can be reset");
        tx_b
            .try_send(PlayerVMEvent::Global(event_b, vec![]))
            .expect("session B event queued");

        let marker_seen = Rc::new(Cell::new(false));
        let loop_done = Rc::new(Cell::new(false));
        let operation_done = loop_done.clone();
        let operation_session = session_b.clone();
        let operation_owner = owner_b.clone();
        let operation = Box::pin(async move {
            run_event_loop(rx_b, operation_session, 1, operation_owner).await;
            operation_done.set(true);
        });
        let pump_marker = marker_seen.clone();
        let pump_done = loop_done.clone();
        let pump_session = session_b.clone();
        let pump_owner = owner_b.clone();
        let pump = Box::pin(async move {
            let mut channel_closed = false;
            loop {
                let turns = crate::player::commands::pump_pending_commands(&pump_session, 1);
                for turn in turns {
                    Box::pin(finish_test_command_turn(&pump_session, 1, &pump_owner, turn)).await;
                }
                let _ = Box::pin(crate::player::commands::pump_pending_eval_requests(
                    &pump_session,
                    1,
                ))
                .await;
                let (turns, _) = Box::pin(crate::player::commands::execute_pending_commands(
                    &pump_session,
                    1,
                    &pump_owner,
                ))
                .await;
                for turn in turns {
                    Box::pin(finish_test_command_turn(&pump_session, 1, &pump_owner, turn)).await;
                }

                let marked = pump_session.borrow_mut().with_player(1, |context| {
                    context.player.globals.get(&marker_b).is_some_and(|value| {
                        matches!(
                            context.player.get_datum(value),
                            crate::director::lingo::datum::Datum::Int(42)
                        )
                    })
                });
                if marked == Some(true) {
                    pump_marker.set(true);
                    if !channel_closed {
                        tx_b.close();
                        channel_closed = true;
                    }
                }
                if pump_done.get() {
                    return;
                }
                async_std::task::yield_now().await;
            }
        });
        let run_b = async {
            match futures::future::select(operation, pump).await {
                futures::future::Either::Left(((), mut pump)) => pump.await,
                futures::future::Either::Right(((), mut operation)) => operation.await,
            }
        };
        async_std::task::block_on(async {
            async_std::future::timeout(Duration::from_secs(2), run_b)
                .await
                .expect("session B event loop and production pump must both finish")
        });
        assert!(marker_seen.get(), "session B handler must complete before teardown");

        async_std::task::block_on(async_std::task::sleep(Duration::from_millis(8)));
        assert!(
            matches!(loop_a.as_mut().poll(&mut context), Poll::Ready(())),
            "reset must retire a receiver loop suspended in owner availability"
        );
        assert!(
            event_owner_is_live(&session_b, 1, &owner_b),
            "resetting session A must not retire or replace session B"
        );
    }

    #[test]
    fn targeted_event_preserves_stop_event_and_rejects_missing_sprite() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
                Bytecode::new(OpCode::ExtCall, 2, 1),
                Bytecode::new(OpCode::PushArgListNoRet, 0, 2),
                Bytecode::new(OpCode::ExtCall, 3, 3),
                Bytecode::new(OpCode::Ret, 0, 4),
            ],
        );
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_dispatch_event_to_sprite_targeted_owned(
                session.clone(),
                1,
                captured.clone(),
                event.clone(),
                vec![],
                1,
            ),
        ));
        assert!(
            result.as_ref().map(|value| *value).unwrap_or(false),
            "stopEvent dispatch failed: {:?}",
            result.as_ref().err()
        );
        assert_eq!(
            session.borrow_mut().with_player(1, |context| context.player.globals.get(&marker).is_none()),
            Some(true),
            "stopEvent must prevent later behaviors and static fallback"
        );

        let captured = owner(&session);
        let missing = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_dispatch_event_to_sprite_targeted_owned(
                session,
                1,
                captured,
                event,
                vec![],
                99,
            ),
        ));
        assert_eq!(
            missing.as_ref().ok().copied(),
            Some(false),
            "missing sprite must not fall through: {:?}",
            missing.as_ref().err()
        );
    }

    #[test]
    fn global_event_stop_event_breaks_even_when_handler_passes() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let first = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
                Bytecode::new(OpCode::ExtCall, 2, 1),
                Bytecode::new(OpCode::PushArgListNoRet, 0, 2),
                Bytecode::new(OpCode::ExtCall, 3, 3),
                Bytecode::new(OpCode::Ret, 0, 4),
            ],
        );
        let second = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::PushInt8, 42, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let static_handler = event_handler(0, vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let (event, marker) = install_behavior_fixture(&session, first, second, static_handler);
        let captured = owner(&session);
        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_invoke_global_event_owned(session.clone(), 1, captured, event, vec![]),
        ));
        assert!(result.is_ok(), "stopEvent/pass handler failed: {:?}", result.err());
        assert_eq!(
            session.borrow_mut().with_player(1, |context| context.player.globals.get(&marker).is_none()),
            Some(true),
            "stopEvent must prevent later behaviors even when the handler also calls pass"
        );
    }

    #[test]
    fn global_dispatch_rejects_a_retired_owner_before_handler_lookup() {
        let session = session_handle();
        let captured = owner(&session);
        session
            .borrow_mut()
            .reset_player_owned(1, &captured)
            .expect("captured owner can reset its player");
        let before = session
            .borrow_mut()
            .with_player(1, |context| {
                (context.player.scope_count, context.player.handler_stack_depth)
            })
            .unwrap();

        let result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            player_invoke_global_event_owned(
                session.clone(),
                1,
                captured,
                Symbol::builtin(BuiltInSymbol::Voidp),
                vec![DatumRef::Void],
            ),
        ));
        assert!(result.is_err());
        assert_eq!(
            session
                .borrow_mut()
                .with_player(1, |context| {
                    (context.player.scope_count, context.player.handler_stack_depth)
                }),
            Some(before)
        );
    }

    #[test]
    fn w3d_static_dispatch_keeps_target_args_and_skips_behaviors() {
        use crate::director::chunks::handler::Bytecode;
        use crate::director::lingo::datum::Datum;
        use crate::director::lingo::opcode::OpCode;

        let session = session_handle();
        let behavior_error = event_handler(0, vec![Bytecode::new(OpCode::Invalid, 0, 0)]);
        // Static dispatch has no receiver instance in this fixture, so the
        // optional target is parameter zero. Read it back into the marker to
        // prove the target survived the owned callback boundary.
        let static_handler = event_handler(
            0,
            vec![
                Bytecode::new(OpCode::GetParam, 0, 0),
                Bytecode::new(OpCode::SetGlobal, 1, 1),
                Bytecode::new(OpCode::Ret, 0, 2),
            ],
        );
        let (event, marker) = install_behavior_fixture(
            &session,
            behavior_error.clone(),
            behavior_error,
            static_handler,
        );
        let target = session
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(9)))
            .expect("test player exists");
        let captured = owner(&session);
        let static_result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_w3d_callback_owned(
                session.clone(),
                1,
                captured.clone(),
                W3dCallbackRequest {
                    owner: captured.clone(),
                    receiver: W3dCallbackReceiver::Static { target: Some(target.clone()) },
                    handler_name: event.clone(),
                    args: vec![target],
                },
            ),
        ));
        assert!(
            static_result.is_ok(),
            "static callback must skip behavior scripts: {:?}",
            static_result.err()
        );
        assert_eq!(
            session.borrow_mut().with_player(1, |context| {
                context.player.globals.get(&marker).map(|value| {
                    matches!(
                        context.player.get_datum(value),
                        crate::director::lingo::datum::Datum::Int(9)
                    )
                })
            }),
            Some(Some(true)),
            "static callback must receive its optional target as parameter zero"
        );

        let global_result = async_std::task::block_on(drive_with_production_pump(
            session.clone(),
            1,
            captured.clone(),
            dispatch_w3d_callback_owned(
                session,
                1,
                captured.clone(),
                W3dCallbackRequest {
                    owner: captured,
                    receiver: W3dCallbackReceiver::Global,
                    handler_name: event,
                    args: vec![],
                },
            ),
        ));
        assert!(global_result.is_err(), "global callback must still visit behavior scripts");
    }

}
